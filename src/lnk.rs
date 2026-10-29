//! Minimal reader for Windows shell links (`.lnk`), enough to show where a shortcut points.
//! Format reference: [MS-SHLLINK].

use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct ShellLink {
    pub target: Option<String>,
    pub arguments: Option<String>,
    pub description: Option<String>,
}

const HAS_TARGET_ID_LIST: u32 = 0x1;
const HAS_LINK_INFO: u32 = 0x2;
const HAS_NAME: u32 = 0x4;
const HAS_RELATIVE_PATH: u32 = 0x8;
const HAS_WORKING_DIR: u32 = 0x10;
const HAS_ARGUMENTS: u32 = 0x20;
const HAS_ICON_LOCATION: u32 = 0x40;
const IS_UNICODE: u32 = 0x80;
const ENVIRONMENT_BLOCK: u32 = 0xA000_0001;

fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?))
}

fn ansi_z(data: &[u8], pos: usize) -> Option<String> {
    let bytes = data.get(pos..)?;
    let end = bytes.iter().position(|&b| b == 0)?;
    let s: String = bytes[..end].iter().map(|&b| b as char).collect();
    (!s.is_empty()).then_some(s)
}

fn utf16_z(data: &[u8], pos: usize, max_chars: usize) -> Option<String> {
    let units: Vec<u16> = data
        .get(pos..)?
        .as_chunks::<2>().0.iter()
        .take(max_chars)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    let s = String::from_utf16_lossy(&units);
    (!s.is_empty()).then_some(s)
}

fn string_data(data: &[u8], pos: &mut usize, unicode: bool) -> Option<String> {
    let count = u16_at(data, *pos)? as usize;
    *pos += 2;
    let len = if unicode { count * 2 } else { count };
    let bytes = data.get(*pos..*pos + len)?;
    *pos += len;
    let s = if unicode {
        let units: Vec<u16> = bytes
            .as_chunks::<2>().0.iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    };
    Some(s)
}

/// Replaces `%VAR%` occurrences with the value of the environment variable.
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) => out.push_str(&value),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn parse(data: &[u8], link_path: &Path) -> Option<ShellLink> {
    if u32_at(data, 0)? != 0x4C {
        return None;
    }
    let flags = u32_at(data, 0x14)?;
    let unicode = flags & IS_UNICODE != 0;
    let mut pos = 0x4C;

    if flags & HAS_TARGET_ID_LIST != 0 {
        pos += 2 + u16_at(data, pos)? as usize;
    }

    let mut link = ShellLink::default();

    if flags & HAS_LINK_INFO != 0 {
        let info = pos;
        let info_size = u32_at(data, info)? as usize;
        let header_size = u32_at(data, info + 4)?;
        let info_flags = u32_at(data, info + 8)?;
        if info_flags & 0x1 != 0 {
            let unicode_offsets = header_size >= 0x24;
            let base = unicode_offsets
                .then(|| utf16_z(data, info + u32_at(data, info + 0x1C)? as usize, 32_768))
                .flatten()
                .or_else(|| ansi_z(data, info + u32_at(data, info + 0x10)? as usize));
            let suffix = unicode_offsets
                .then(|| utf16_z(data, info + u32_at(data, info + 0x20)? as usize, 32_768))
                .flatten()
                .or_else(|| ansi_z(data, info + u32_at(data, info + 0x18)? as usize));
            link.target = base.map(|b| b + suffix.as_deref().unwrap_or(""));
        }
        pos = info + info_size;
    }

    let mut relative_path = None;
    if flags & HAS_NAME != 0 {
        link.description = string_data(data, &mut pos, unicode).filter(|s| !s.is_empty());
    }
    if flags & HAS_RELATIVE_PATH != 0 {
        relative_path = string_data(data, &mut pos, unicode);
    }
    if flags & HAS_WORKING_DIR != 0 {
        string_data(data, &mut pos, unicode);
    }
    if flags & HAS_ARGUMENTS != 0 {
        link.arguments = string_data(data, &mut pos, unicode).filter(|s| !s.trim().is_empty());
    }
    if flags & HAS_ICON_LOCATION != 0 {
        string_data(data, &mut pos, unicode);
    }

    // Shortcuts made by installers often only carry the target in an environment block.
    if link.target.is_none() {
        while let Some(size) = u32_at(data, pos) {
            let size = size as usize;
            if size < 8 {
                break;
            }
            if u32_at(data, pos + 4)? == ENVIRONMENT_BLOCK {
                link.target = utf16_z(data, pos + 8 + 260, 260).or_else(|| ansi_z(data, pos + 8));
                break;
            }
            pos += size;
        }
    }

    if link.target.is_none()
        && let (Some(rel), Some(dir)) = (relative_path, link_path.parent()) {
            link.target = Some(dir.join(rel).to_string_lossy().into_owned());
        }

    link.target = link.target.map(|t| expand_env(&t));
    Some(link)
}
