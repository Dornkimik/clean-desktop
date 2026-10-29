use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui_phosphor::regular as icon;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Folder,
    Image,
    Video,
    Audio,
    Document,
    Spreadsheet,
    Presentation,
    Pdf,
    Archive,
    Code,
    Text,
    Program,
    Shortcut,
    WebLink,
    Other,
}

impl Kind {
    pub fn from_extension(ext: &str) -> Kind {
        match ext {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "tif" | "tiff" => Kind::Image,
            "heic" | "heif" | "raw" | "cr2" | "nef" | "psd" | "avif" => Kind::Image,
            "mp4" | "mkv" | "mov" | "avi" | "wmv" | "webm" | "flv" | "m4v" => Kind::Video,
            "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "wma" | "opus" => Kind::Audio,
            "doc" | "docx" | "odt" | "rtf" | "pages" => Kind::Document,
            "xls" | "xlsx" | "ods" | "numbers" => Kind::Spreadsheet,
            "ppt" | "pptx" | "odp" | "key" => Kind::Presentation,
            "pdf" => Kind::Pdf,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "tgz" | "iso" | "cab" => Kind::Archive,
            "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "cs" | "java"
            | "go" | "rb" | "php" | "html" | "htm" | "css" | "scss" | "json" | "toml" | "yaml"
            | "yml" | "xml" | "svg" | "sql" | "sh" | "ps1" | "psm1" | "lua" | "kt" | "swift"
            | "vue" | "ipynb" => Kind::Code,
            "txt" | "md" | "markdown" | "log" | "csv" | "tsv" | "ini" | "cfg" | "conf" | "env"
            | "nfo" | "srt" => Kind::Text,
            "exe" | "msi" | "bat" | "cmd" | "com" | "appx" | "msix" | "jar" => Kind::Program,
            "lnk" => Kind::Shortcut,
            "url" | "webloc" => Kind::WebLink,
            _ => Kind::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::Image => "Image",
            Kind::Video => "Video",
            Kind::Audio => "Audio",
            Kind::Document => "Document",
            Kind::Spreadsheet => "Spreadsheet",
            Kind::Presentation => "Presentation",
            Kind::Pdf => "PDF document",
            Kind::Archive => "Archive",
            Kind::Code => "Source / data file",
            Kind::Text => "Text file",
            Kind::Program => "Program / installer",
            Kind::Shortcut => "Shortcut",
            Kind::WebLink => "Web link",
            Kind::Other => "File",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Kind::Folder => icon::FOLDER_SIMPLE,
            Kind::Image => icon::FILE_IMAGE,
            Kind::Video => icon::FILE_VIDEO,
            Kind::Audio => icon::FILE_AUDIO,
            Kind::Document => icon::FILE_DOC,
            Kind::Spreadsheet => icon::FILE_XLS,
            Kind::Presentation => icon::FILE_PPT,
            Kind::Pdf => icon::FILE_PDF,
            Kind::Archive => icon::FILE_ZIP,
            Kind::Code => icon::FILE_CODE,
            Kind::Text => icon::FILE_TEXT,
            Kind::Program => icon::APP_WINDOW,
            Kind::Shortcut => icon::LINK,
            Kind::WebLink => icon::GLOBE,
            Kind::Other => icon::FILE,
        }
    }

    /// Whether it is worth sniffing the file for readable text.
    pub fn may_be_text(self) -> bool {
        matches!(self, Kind::Code | Kind::Text | Kind::Other)
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub extension: String,
    pub kind: Kind,
    pub is_dir: bool,
    /// File size in bytes. For folders this is filled in once the preview has been computed.
    pub size: Option<u64>,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub read_only: bool,
}

impl Entry {
    /// The most meaningful "last touched" time, used for sorting.
    pub fn last_touched(&self) -> Option<SystemTime> {
        self.modified.or(self.created)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortMode {
    OldestFirst,
    NewestFirst,
    Name,
    Type,
    Largest,
}

impl SortMode {
    pub const ALL: [SortMode; 5] = [
        SortMode::OldestFirst,
        SortMode::NewestFirst,
        SortMode::Largest,
        SortMode::Name,
        SortMode::Type,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortMode::OldestFirst => "Oldest first",
            SortMode::NewestFirst => "Newest first",
            SortMode::Name => "Name",
            SortMode::Type => "Type",
            SortMode::Largest => "Largest first",
        }
    }
}

pub fn sort_entries<T>(items: &mut [T], mode: SortMode, entry: impl Fn(&T) -> &Entry) {
    let name_key = |e: &Entry| e.name.to_lowercase();
    items.sort_by(|a, b| {
        let (a, b) = (entry(a), entry(b));
        match mode {
            SortMode::OldestFirst => a.last_touched().cmp(&b.last_touched()),
            SortMode::NewestFirst => b.last_touched().cmp(&a.last_touched()),
            SortMode::Name => name_key(a).cmp(&name_key(b)),
            SortMode::Type => (b.is_dir, a.kind.label(), &a.extension)
                .cmp(&(a.is_dir, b.kind.label(), &b.extension))
                .then_with(|| name_key(a).cmp(&name_key(b))),
            SortMode::Largest => b.size.unwrap_or(0).cmp(&a.size.unwrap_or(0)),
        }
    });
}

fn is_hidden(meta: &fs::Metadata, name: &str) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        if meta.file_attributes() & (HIDDEN | SYSTEM) != 0 {
            return true;
        }
    }
    let _ = meta;
    name.starts_with('.') || name.eq_ignore_ascii_case("desktop.ini")
}

pub fn scan(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for dir_entry in fs::read_dir(dir)? {
        let Ok(dir_entry) = dir_entry else { continue };
        let path = dir_entry.path();
        let name = dir_entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = fs::metadata(&path).or_else(|_| dir_entry.metadata()) else {
            continue;
        };
        if is_hidden(&meta, &name) {
            continue;
        }
        let is_dir = meta.is_dir();
        let extension = if is_dir {
            String::new()
        } else {
            path.extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        let kind = if is_dir {
            Kind::Folder
        } else {
            Kind::from_extension(&extension)
        };
        entries.push(Entry {
            name,
            extension,
            kind,
            is_dir,
            size: (!is_dir).then_some(meta.len()),
            created: meta.created().ok(),
            modified: meta.modified().ok(),
            accessed: meta.accessed().ok(),
            read_only: meta.permissions().readonly(),
            path,
        });
    }
    Ok(entries)
}

/// The folder to clean: the first command line argument, or the user's desktop.
pub fn default_root() -> PathBuf {
    if let Some(arg) = std::env::args_os().nth(1) {
        return PathBuf::from(arg);
    }
    dirs::desktop_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Desktop")))
        .unwrap_or_else(|| PathBuf::from("."))
}
