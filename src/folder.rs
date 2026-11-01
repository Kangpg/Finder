use crate::{
    index::{Index, Source},
    usn::UsnRecord,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// A portable snapshot; links/reparse directories are recorded, never traversed.
pub fn scan(root: &Path, epoch: Option<(&AtomicU64, u64)>) -> Result<Index> {
    let root = root.canonicalize().with_context(|| {
        crate::i18n::Message::capture(|| format!("Cannot open {}", root.display()))
    })?;
    ensure!(root.is_dir(), "Index root must be a directory");
    let mut index = Index::new(Source::Folder { root: root.clone() }, 1);
    let mut ids = HashMap::<PathBuf, u64>::from([(root.clone(), 1)]);
    let mut next_id = 2;
    let mut entries = walkdir::WalkDir::new(&root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter();
    entries.next();
    while let Some(entry) = entries.next() {
        ensure!(
            !epoch.is_some_and(|(v, n)| v.load(Ordering::Relaxed) != n),
            "Indexing cancelled"
        );
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                index.note(e.to_string());
                continue;
            }
        };
        let metadata = entry.metadata();
        #[cfg(windows)]
        if entry.file_type().is_dir() {
            use std::os::windows::fs::MetadataExt;
            match &metadata {
                Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => {
                    entries.skip_current_dir()
                }
                Err(e) => {
                    entries.skip_current_dir();
                    index.note(e.to_string());
                }
                _ => {}
            }
        }
        let parent = entry.path().parent().and_then(|p| ids.get(p)).copied();
        let Some(parent) = parent else {
            index.note(format!("Unresolved parent: {}", entry.path().display()));
            continue;
        };
        let frn = next_id;
        next_id += 1;
        #[cfg(windows)]
        let name = {
            use std::os::windows::ffi::OsStrExt;
            entry.file_name().encode_wide().collect()
        };
        #[cfg(not(windows))]
        let name = entry.file_name().to_string_lossy().encode_utf16().collect();
        let directory = entry.file_type().is_dir();
        index.upsert(&UsnRecord {
            frn,
            parent,
            usn: 0,
            reason: 0,
            attributes: if directory { 16 } else { 0 },
            name,
        })?;
        let details = match metadata {
            Ok(metadata) => crate::details::Details::from_metadata(&metadata),
            Err(e) => crate::details::Details {
                size: None,
                modified: None,
                error: Some(crate::i18n::Message::error(e)),
            },
        };
        index.seed_details(frn, details);
        if directory {
            ids.insert(entry.path().to_owned(), frn);
        }
    }
    index.rebuild_order(epoch)?;
    Ok(index)
}
