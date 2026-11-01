use crate::index::Index;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    #[default]
    All,
    Files,
    Folders,
    Images,
    Documents,
    Audio,
    Videos,
    Archives,
    Programs,
}
impl Filter {
    pub const ALL: [Self; 9] = [
        Self::All,
        Self::Files,
        Self::Folders,
        Self::Images,
        Self::Documents,
        Self::Audio,
        Self::Videos,
        Self::Archives,
        Self::Programs,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::All => crate::i18n::text("전체", "All"),
            Self::Files => crate::i18n::text("파일", "Files"),
            Self::Folders => crate::i18n::text("폴더", "Folders"),
            Self::Images => crate::i18n::text("이미지", "Images"),
            Self::Documents => crate::i18n::text("문서", "Documents"),
            Self::Audio => crate::i18n::text("음악", "Audio"),
            Self::Videos => crate::i18n::text("동영상", "Videos"),
            Self::Archives => crate::i18n::text("압축파일", "Archives"),
            Self::Programs => crate::i18n::text("실행파일", "Programs"),
        }
    }
    pub fn keyword(self) -> Option<&'static str> {
        match self {
            Self::Images => Some("image:"),
            Self::Documents => Some("doc:"),
            Self::Audio => Some("audio:"),
            Self::Videos => Some("video:"),
            Self::Archives => Some("zip:"),
            Self::Programs => Some("exe:"),
            _ => None,
        }
    }
    pub fn matches(self, index: &Index, id: u32) -> bool {
        let directory = index.records[id as usize].is_dir();
        match self {
            Self::All => return true,
            Self::Files => return !directory,
            Self::Folders => return directory,
            _ => {}
        }
        if directory {
            return false;
        }
        let extensions: &[&[u8]] = match self {
            Self::Images => &[
                b"jpg", b"jpeg", b"png", b"gif", b"bmp", b"webp", b"svg", b"ico", b"tif", b"tiff",
                b"heic", b"avif",
            ],
            Self::Documents => &[
                b"txt", b"md", b"pdf", b"doc", b"docx", b"xls", b"xlsx", b"ppt", b"pptx", b"odt",
                b"rtf", b"csv",
            ],
            Self::Audio => &[
                b"mp3", b"wav", b"flac", b"aac", b"ogg", b"m4a", b"wma", b"opus",
            ],
            Self::Videos => &[
                b"mp4", b"mkv", b"avi", b"mov", b"webm", b"wmv", b"mpeg", b"mpg", b"m4v",
            ],
            Self::Archives => &[b"zip", b"7z", b"rar", b"tar", b"gz", b"bz2", b"xz", b"zst"],
            Self::Programs => &[b"exe", b"msi", b"com", b"bat", b"cmd", b"ps1"],
            _ => &[],
        };
        index
            .folded_extension(id)
            .is_some_and(|ext| extensions.contains(&ext))
    }
}
