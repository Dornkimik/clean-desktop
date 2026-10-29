use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::SystemTime;

use egui::ColorImage;

use crate::lnk;
use crate::scan::{Entry, Kind};

const MAX_TEXT_BYTES: usize = 128 * 1024;
const MAX_IMAGE_SIDE: u32 = 1800;
const MAX_FOLDER_ENTRIES: u64 = 250_000;
const MAX_LISTED_CHILDREN: usize = 300;

pub enum Preview {
    Image {
        image: ColorImage,
        width: u32,
        height: u32,
    },
    Text {
        text: String,
        lines: usize,
        truncated: bool,
    },
    Folder(FolderSummary),
    Shortcut {
        target: Option<PathBuf>,
        target_exists: bool,
        target_is_dir: bool,
        arguments: Option<String>,
        description: Option<String>,
    },
    WebLink {
        url: String,
    },
    /// Nothing to show beyond metadata, with a short reason.
    Unavailable(String),
}

pub struct FolderSummary {
    pub total_size: u64,
    pub files: u64,
    pub folders: u64,
    /// Most recently modified file anywhere inside the folder.
    pub newest: Option<(SystemTime, String)>,
    pub children: Vec<Child>,
    pub child_count: usize,
    /// File kinds by total size, largest first.
    pub kinds: Vec<(Kind, u64, u64)>,
    /// The walk stopped early because the folder is enormous.
    pub incomplete: bool,
}

pub struct Child {
    pub name: String,
    pub is_dir: bool,
    pub kind: Kind,
    pub size: Option<u64>,
}

pub fn load(entry: &Entry) -> Preview {
    let path = &entry.path;
    if entry.is_dir {
        return Preview::Folder(summarize_folder(path));
    }
    match entry.kind {
        Kind::Image => load_image(path),
        Kind::Shortcut => load_shortcut(path),
        Kind::WebLink => load_web_link(path),
        kind if kind.may_be_text() => load_text(path),
        Kind::Video | Kind::Audio => Preview::Unavailable("Media files can't be previewed here — press Enter to open it.".into()),
        _ => Preview::Unavailable("No inline preview for this type — press Enter to open it.".into()),
    }
}

fn load_image(path: &Path) -> Preview {
    let img = match image::ImageReader::open(path).and_then(|r| r.with_guessed_format()) {
        Ok(reader) => match reader.decode() {
            Ok(img) => img,
            Err(_) => return Preview::Unavailable("This image format can't be previewed.".into()),
        },
        Err(e) => return Preview::Unavailable(format!("Couldn't read the image: {e}")),
    };
    let (width, height) = (img.width(), img.height());
    let img = if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        img.thumbnail(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Preview::Image {
        image: ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        width,
        height,
    }
}

fn read_prefix(path: &Path, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let file = fs::File::open(path)?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::with_capacity(limit.min(len as usize));
    file.take(limit as u64).read_to_end(&mut buf)?;
    Ok((buf, len > limit as u64))
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>().0.iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return Some(String::from_utf16_lossy(&units));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let sample = &bytes[..bytes.len().min(8192)];
    let suspicious = sample
        .iter()
        .filter(|&&b| b == 0 || (b < 0x09) || (0x0E..0x20).contains(&b) && b != 0x1B)
        .count();
    if suspicious * 100 > sample.len().max(1) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn load_text(path: &Path) -> Preview {
    match read_prefix(path, MAX_TEXT_BYTES) {
        Ok((bytes, truncated)) => match decode_text(&bytes) {
            Some(text) => {
                let text = text.replace("\r\n", "\n").replace('\t', "    ");
                if text.trim().is_empty() {
                    return Preview::Unavailable("This file is empty.".into());
                }
                Preview::Text {
                    lines: text.lines().count(),
                    text,
                    truncated,
                }
            }
            None => Preview::Unavailable("Binary file — no readable preview.".into()),
        },
        Err(e) => Preview::Unavailable(format!("Couldn't read the file: {e}")),
    }
}

fn load_shortcut(path: &Path) -> Preview {
    let parsed = fs::read(path).ok().and_then(|data| lnk::parse(&data, path));
    let Some(link) = parsed else {
        return Preview::Unavailable("Couldn't read this shortcut.".into());
    };
    let target = link.target.map(PathBuf::from);
    let target_exists = target.as_ref().is_some_and(|t| t.exists());
    let target_is_dir = target.as_ref().is_some_and(|t| t.is_dir());
    Preview::Shortcut {
        target,
        target_exists,
        target_is_dir,
        arguments: link.arguments,
        description: link.description,
    }
}

fn load_web_link(path: &Path) -> Preview {
    let content = fs::read_to_string(path).unwrap_or_default();
    let url = content
        .lines()
        .find_map(|l| l.trim().strip_prefix("URL=").map(str::to_owned))
        .or_else(|| {
            // .webloc files are plists with a <string> element.
            let start = content.find("<string>")? + "<string>".len();
            let end = content[start..].find("</string>")? + start;
            Some(content[start..end].to_owned())
        });
    match url {
        Some(url) => Preview::WebLink { url },
        None => load_text(path),
    }
}

fn summarize_folder(path: &Path) -> FolderSummary {
    let mut children: Vec<Child> = fs::read_dir(path)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| {
                    let meta = e.metadata().ok();
                    let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
                    let name = e.file_name().to_string_lossy().into_owned();
                    let ext = Path::new(&name)
                        .extension()
                        .map(|x| x.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    Child {
                        kind: if is_dir { Kind::Folder } else { Kind::from_extension(&ext) },
                        size: meta.filter(|m| !m.is_dir()).map(|m| m.len()),
                        is_dir,
                        name,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    children.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let child_count = children.len();
    children.truncate(MAX_LISTED_CHILDREN);

    let mut summary = FolderSummary {
        total_size: 0,
        files: 0,
        folders: 0,
        newest: None,
        children,
        child_count,
        kinds: Vec::new(),
        incomplete: false,
    };
    let mut kinds: HashMap<Kind, (u64, u64)> = HashMap::new();
    let mut seen = 0u64;
    for entry in walkdir::WalkDir::new(path).min_depth(1).into_iter().filter_map(Result::ok) {
        seen += 1;
        if seen > MAX_FOLDER_ENTRIES {
            summary.incomplete = true;
            break;
        }
        if entry.file_type().is_dir() {
            summary.folders += 1;
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        summary.files += 1;
        summary.total_size += meta.len();
        let ext = entry
            .path()
            .extension()
            .map(|x| x.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let slot = kinds.entry(Kind::from_extension(&ext)).or_default();
        slot.0 += 1;
        slot.1 += meta.len();
        if let Ok(modified) = meta.modified()
            && summary.newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                summary.newest = Some((modified, entry.file_name().to_string_lossy().into_owned()));
            }
    }
    let mut kinds: Vec<(Kind, u64, u64)> = kinds.into_iter().map(|(k, (n, b))| (k, n, b)).collect();
    kinds.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
    summary.kinds = kinds;
    summary
}

/// Computes previews on background threads so the UI never stalls.
pub struct Loader {
    tx: Sender<(PathBuf, Preview)>,
    rx: Receiver<(PathBuf, Preview)>,
    pending: HashSet<PathBuf>,
}

impl Loader {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            pending: HashSet::new(),
        }
    }

    pub fn request(&mut self, ctx: &egui::Context, entry: &Entry) {
        if !self.pending.insert(entry.path.clone()) {
            return;
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let entry = entry.clone();
        std::thread::spawn(move || {
            let preview = load(&entry);
            let _ = tx.send((entry.path, preview));
            ctx.request_repaint();
        });
    }

    pub fn is_pending(&self, path: &Path) -> bool {
        self.pending.contains(path)
    }

    pub fn forget(&mut self, path: &Path) {
        self.pending.remove(path);
    }

    pub fn poll(&mut self) -> Vec<(PathBuf, Preview)> {
        let done: Vec<_> = self.rx.try_iter().collect();
        for (path, _) in &done {
            self.pending.remove(path);
        }
        done
    }
}
