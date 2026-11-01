use std::{path::Path, time::SystemTime};
#[derive(Debug, Clone)]
pub struct Details {
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub error: Option<crate::i18n::Message>,
}
impl Details {
    pub fn read(path: &Path) -> Self {
        match std::fs::symlink_metadata(path) {
            Ok(m) => Self::from_metadata(&m),
            Err(e) => Self {
                size: None,
                modified: None,
                error: Some(crate::i18n::Message::error(e)),
            },
        }
    }
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        let modified = metadata.modified();
        Self {
            size: (!metadata.is_dir()).then_some(metadata.len()),
            modified: modified.as_ref().ok().copied(),
            error: modified.err().map(|e| {
                crate::msgf!(
                    "수정일 조회 실패: {e}",
                    "Could not read modification time: {e}"
                )
            }),
        }
    }
    pub fn size_text(&self) -> String {
        self.size
            .map_or_else(|| "—".into(), crate::byte_size::format)
    }
    pub fn date_text(&self) -> String {
        self.modified.map_or_else(
            || "—".into(),
            |t| {
                let date: chrono::DateTime<chrono::Local> = t.into();
                date.format("%Y-%m-%d %H:%M").to_string()
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_metadata_preserves_unknown_and_directory_size() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("한글.txt");
        std::fs::write(&file, b"hello").unwrap();
        let d = Details::read(&file);
        assert_eq!(d.size, Some(5));
        assert!(d.modified.is_some());
        assert_eq!(Details::read(dir.path()).size, None);
        std::fs::remove_file(&file).unwrap();
        let d = Details::read(&file);
        assert_eq!(d.size, None);
        assert_eq!(d.modified, None);
        assert!(d.error.is_some());
    }
}
