use crate::{filters::Filter, index::SearchOrder, query::QueryOptions};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
};

const MAX_BYTES: u64 = 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub query: String,
    pub options: QueryOptions,
    pub filter: Filter,
    pub order: SearchOrder,
}
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    pub options: QueryOptions,
    pub filter: Filter,
    pub order: SearchOrder,
    pub details: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub version: u32,
    pub language: crate::i18n::Language,
    pub refresh_interval_secs: u32,
    pub close_to_tray: bool,
    pub hotkey: Option<crate::hotkey::Hotkey>,
    #[serde(default, with = "saved_path")]
    pub last_folder: Option<PathBuf>,
    #[serde(default, with = "saved_path")]
    pub right_folder: Option<PathBuf>,
    pub sync_indexes: bool,
    pub options: QueryOptions,
    pub filter: Filter,
    pub order: SearchOrder,
    pub details: bool,
    pub split: bool,
    pub secondary: SearchSettings,
    pub history: Vec<String>,
    pub bookmarks: Vec<Bookmark>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            language: Default::default(),
            refresh_interval_secs: 3,
            close_to_tray: true,
            hotkey: Some(Default::default()),
            last_folder: None,
            right_folder: None,
            sync_indexes: true,
            options: Default::default(),
            filter: Default::default(),
            order: Default::default(),
            details: false,
            split: false,
            secondary: SearchSettings::default(),
            history: Vec::new(),
            bookmarks: Vec::new(),
            extra: Default::default(),
        }
    }
}
impl Preferences {
    pub fn primary_settings(&self) -> SearchSettings {
        SearchSettings {
            options: self.options,
            filter: self.filter,
            order: self.order,
            details: self.details,
        }
    }
    pub fn set_primary(&mut self, s: SearchSettings) {
        self.options = s.options;
        self.filter = s.filter;
        self.order = s.order;
        self.details = s.details;
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1,
            crate::i18n::Message::new("지원하지 않는 설정 버전", "Unsupported settings version")
        );
        if let Some(hotkey) = self.hotkey {
            hotkey.validate()?;
        }
        ensure!(
            self.refresh_interval_secs > 0,
            crate::i18n::Message::new(
                "자동 갱신 간격은 1초 이상이어야 합니다",
                "The refresh interval must be at least 1 second"
            )
        );
        for path in [&self.last_folder, &self.right_folder]
            .into_iter()
            .flatten()
        {
            ensure!(
                path.is_absolute(),
                crate::i18n::Message::new(
                    "저장된 폴더가 절대 경로가 아닙니다",
                    "The saved folder path is not absolute"
                )
            );
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                let units: Vec<_> = path.as_os_str().encode_wide().collect();
                ensure!(
                    units.len() <= 32767 && !units.contains(&0),
                    crate::i18n::Message::new("저장된 폴더 경로 오류", "Invalid saved folder path")
                );
            }
        }
        ensure!(
            self.history.len() <= 32 && self.bookmarks.len() <= 32,
            crate::i18n::Message::new(
                "최근 검색/즐겨찾기 한도 초과",
                "Recent search or bookmark limit exceeded"
            )
        );
        ensure!(
            self.history.iter().all(|q| q.len() <= 8192)
                && self
                    .bookmarks
                    .iter()
                    .all(|b| b.query.len() <= 8192 && b.name.len() <= 200),
            crate::i18n::Message::new(
                "검색어/이름 한도 초과",
                "Query or name length limit exceeded"
            )
        );
        Ok(())
    }
    pub fn remember(&mut self, query: &str) {
        if query.trim().is_empty() || query.len() > 8192 {
            return;
        }
        self.history.retain(|q| q != query);
        self.history.insert(0, query.into());
        self.history.truncate(32);
    }
}

mod saved_path {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::path::PathBuf;
    #[derive(Serialize, Deserialize)]
    #[serde(untagged)]
    enum Wire {
        Text(String),
        Windows { utf16: Vec<u16> },
    }
    pub fn serialize<S: Serializer>(
        path: &Option<PathBuf>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let wire = path
            .as_ref()
            .map(|path| {
                if let Some(text) = path.to_str() {
                    return Ok(Wire::Text(text.into()));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStrExt;
                    Ok(Wire::Windows {
                        utf16: path.as_os_str().encode_wide().collect(),
                    })
                }
                #[cfg(not(windows))]
                {
                    Err(serde::ser::Error::custom(crate::i18n::text(
                        "폴더 경로를 저장할 수 없습니다",
                        "Could not save folder path",
                    )))
                }
            })
            .transpose()?;
        wire.serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<PathBuf>, D::Error> {
        Option::<Wire>::deserialize(deserializer)?
            .map(|wire| match wire {
                Wire::Text(text) => Ok(PathBuf::from(text)),
                Wire::Windows { utf16 } => {
                    #[cfg(windows)]
                    {
                        use std::os::windows::ffi::OsStringExt;
                        Ok(PathBuf::from(std::ffi::OsString::from_wide(&utf16)))
                    }
                    #[cfg(not(windows))]
                    {
                        let _ = utf16;
                        Err(serde::de::Error::custom(crate::i18n::text(
                            "Windows 폴더 경로는 이 플랫폼에서 사용할 수 없습니다",
                            "Windows folder paths are unavailable on this platform",
                        )))
                    }
                }
            })
            .transpose()
    }
}
fn read(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        crate::i18n::Message::new("설정 파일 크기 초과", "Settings file size limit exceeded")
    );
    Ok(Some(bytes))
}
pub struct Store {
    path: PathBuf,
    expected: Option<blake3::Hash>,
}
impl Store {
    pub fn load(path: PathBuf) -> Result<(Self, Preferences)> {
        let bytes = read(&path)?;
        let prefs = match &bytes {
            Some(b) => serde_json::from_slice::<Preferences>(b).context(
                crate::i18n::Message::new("설정 파일 오류", "Invalid settings file"),
            )?,
            None => Preferences::default(),
        };
        prefs.validate()?;
        Ok((
            Self {
                path,
                expected: bytes.as_ref().map(|b| blake3::hash(b)),
            },
            prefs,
        ))
    }
    pub fn save(&mut self, prefs: &Preferences) -> Result<()> {
        prefs.validate()?;
        let parent = self.path.parent().context(crate::i18n::Message::new(
            "설정 경로 오류",
            "Invalid settings path",
        ))?;
        std::fs::create_dir_all(parent)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.with_extension("lock"))?;
        lock.try_lock().context(crate::i18n::Message::new(
            "다른 창에서 설정 저장 중",
            "Another window is saving settings",
        ))?;
        let current = read(&self.path)?;
        ensure!(
            current.as_ref().map(|b| blake3::hash(b)) == self.expected,
            crate::i18n::Message::new(
                "외부에서 설정이 변경되어 저장하지 않았습니다",
                "Settings changed outside the app; changes were not saved"
            )
        );
        let bytes = serde_json::to_vec_pretty(prefs)?;
        ensure!(
            bytes.len() as u64 <= MAX_BYTES,
            crate::i18n::Message::new("설정 파일 크기 초과", "Settings file size limit exceeded")
        );
        if current.as_ref() == Some(&bytes) {
            return Ok(());
        }
        if let Some(old) = current {
            let backup = self.path.with_extension("json.bak");
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            temp.write_all(&old)?;
            temp.as_file().sync_all()?;
            temp.persist(backup)?;
        }
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)?;
        self.expected = Some(blake3::hash(&bytes));
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn language_defaults_roundtrips_and_preserves_unsupported_preferences() {
        use crate::i18n::Language;
        let legacy: super::Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.language, Language::Korean);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("preferences.json");
        let (mut store, mut prefs) = super::Store::load(path.clone()).unwrap();
        store.save(&prefs).unwrap();
        let original = std::fs::read(&path).unwrap();
        prefs.language = Language::English;
        prefs.remember("한글 query");
        store.save(&prefs).unwrap();
        assert_eq!(
            std::fs::read(path.with_extension("json.bak")).unwrap(),
            original
        );
        let (_, saved) = super::Store::load(path.clone()).unwrap();
        assert_eq!(saved.language, Language::English);
        assert_eq!(saved.history, ["한글 query"]);
        let unsupported = br#"{"language":"future-language","future":true}"#;
        std::fs::write(&path, unsupported).unwrap();
        assert!(super::Store::load(path.clone()).is_err());
        assert!(store.save(&prefs).is_err());
        assert_eq!(std::fs::read(path).unwrap(), unsupported);
    }
    #[test]
    fn resident_preferences_load_legacy_and_roundtrip_validated_hotkeys() {
        let legacy: super::Preferences = serde_json::from_str("{}").unwrap();
        assert!(legacy.close_to_tray);
        assert_eq!(legacy.hotkey, Some(Default::default()));
        assert_eq!(legacy.order.column, crate::index::SortColumn::Name);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let (mut store, mut prefs) = super::Store::load(path.clone()).unwrap();
        prefs.close_to_tray = false;
        prefs.hotkey = Some(crate::hotkey::Hotkey {
            ctrl: true,
            alt: false,
            shift: true,
            win: false,
            key: 70,
        });
        prefs.order.column = crate::index::SortColumn::Modified;
        prefs.secondary.order.column = crate::index::SortColumn::Size;
        store.save(&prefs).unwrap();
        let (_, loaded) = super::Store::load(path).unwrap();
        assert!(!loaded.close_to_tray);
        assert_eq!(loaded.hotkey, prefs.hotkey);
        assert_eq!(loaded.order, prefs.order);
        assert_eq!(loaded.secondary.order, prefs.secondary.order);
        prefs.hotkey = Some(crate::hotkey::Hotkey {
            key: 123,
            ..Default::default()
        });
        assert!(store.save(&prefs).is_err());
        prefs.hotkey = None;
        store.save(&prefs).unwrap();
    }
    use super::*;
    #[test]
    fn persistence_preserves_unknown_fields_backup_and_foreign_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let (mut store, mut p) = Store::load(path.clone()).unwrap();
        p.remember("한글 *.txt");
        p.extra.insert("future".into(), serde_json::json!({"a":1}));
        store.save(&p).unwrap();
        let first = std::fs::read(&path).unwrap();
        store.save(&p).unwrap();
        assert!(!path.with_extension("json.bak").exists());
        p.remember("image:");
        store.save(&p).unwrap();
        assert_eq!(
            std::fs::read(path.with_extension("json.bak")).unwrap(),
            first
        );
        let (_, loaded) = Store::load(path.clone()).unwrap();
        assert_eq!(loaded.history[1], "한글 *.txt");
        assert_eq!(loaded.extra["future"]["a"], 1);
        std::fs::write(&path, b"{}").unwrap();
        assert!(store.save(&p).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        std::fs::write(&path, b"{\"version\":2}").unwrap();
        assert!(Store::load(path).is_err());
    }
    #[test]
    fn old_settings_upgrade_to_independent_panes_without_losing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let old = br#"{"version":1,"details":true,"history":["old"],"future":{"value":7}}"#;
        std::fs::write(&path, old).unwrap();
        let (mut store, mut prefs) = Store::load(path.clone()).unwrap();
        assert!(prefs.primary_settings().details);
        assert!(prefs.last_folder.is_none());
        assert!(prefs.right_folder.is_none() && prefs.sync_indexes);
        prefs.last_folder = Some(dir.path().join("한국 폴더"));
        assert!(!prefs.split && !prefs.secondary.details);
        prefs.split = true;
        prefs.secondary.options.path_match = true;
        prefs.secondary.order.descending = true;
        store.save(&prefs).unwrap();
        let (_, loaded) = Store::load(path.clone()).unwrap();
        assert!(loaded.split && loaded.details);
        assert_eq!(loaded.last_folder, prefs.last_folder);
        assert!(!loaded.options.path_match && loaded.secondary.options.path_match);
        assert!(!loaded.order.descending && loaded.secondary.order.descending);
        assert_eq!(loaded.extra["future"]["value"], 7);
        assert_eq!(std::fs::read(path.with_extension("json.bak")).unwrap(), old);
    }
    #[cfg(windows)]
    #[test]
    fn saved_folder_roundtrip_preserves_raw_utf16_and_rejects_invalid_paths() {
        use std::os::windows::ffi::OsStringExt;
        let mut units: Vec<u16> = r"C:\raw\".encode_utf16().collect();
        units.push(0xd800);
        let raw = PathBuf::from(std::ffi::OsString::from_wide(&units));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let (mut store, mut p) = Store::load(path.clone()).unwrap();
        p.last_folder = Some(raw.clone());
        store.save(&p).unwrap();
        let (_, loaded) = Store::load(path.clone()).unwrap();
        assert_eq!(loaded.last_folder, Some(raw));
        p.last_folder = Some("relative".into());
        assert!(store.save(&p).is_err());
        p.last_folder = Some("C:\\bad\0name".into());
        assert!(store.save(&p).is_err());
        let (_, preserved) = Store::load(path).unwrap();
        assert_eq!(preserved.last_folder, loaded.last_folder);
    }
    #[test]
    fn refresh_interval_loads_legacy_default_and_persists_user_seconds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        std::fs::write(&path, b"{}").unwrap();
        let (mut store, mut prefs) = Store::load(path.clone()).unwrap();
        assert_eq!(prefs.refresh_interval_secs, 3);
        prefs.refresh_interval_secs = 17;
        store.save(&prefs).unwrap();
        assert_eq!(
            Store::load(path.clone()).unwrap().1.refresh_interval_secs,
            17
        );
        prefs.refresh_interval_secs = 0;
        assert!(store.save(&prefs).is_err());
        assert_eq!(
            Store::load(path.clone()).unwrap().1.refresh_interval_secs,
            17
        );
        std::fs::write(&path, br#"{"refresh_interval_secs":0}"#).unwrap();
        assert!(Store::load(path).is_err());
    }
}
