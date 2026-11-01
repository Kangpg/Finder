use crate::{
    query::Query,
    usn::{self, UsnRecord},
};
use anyhow::{Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

pub const MAX_RECORDS: usize = 10_000_000;
const MAX_POOL_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchOrder {
    pub column: SortColumn,
    pub descending: bool,
    pub folders_first: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortColumn {
    #[default]
    Name,
    Path,
    Size,
    Type,
    Modified,
}
impl SortColumn {
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::Path,
        Self::Size,
        Self::Type,
        Self::Modified,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => crate::i18n::text("이름", "Name"),
            Self::Path => crate::i18n::text("위치", "Location"),
            Self::Size => crate::i18n::text("크기", "Size"),
            Self::Type => crate::i18n::text("종류", "Type"),
            Self::Modified => crate::i18n::text("수정한 날짜", "Date modified"),
        }
    }
}
impl SearchOrder {
    pub fn select(&mut self, column: SortColumn) {
        if self.column == column {
            self.descending = !self.descending;
        } else {
            self.column = column;
            self.descending = false;
        }
    }
    pub fn label(self) -> String {
        format!(
            "{} {}",
            self.column.label(),
            if self.descending { "↓" } else { "↑" }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Source {
    Folder {
        root: PathBuf,
    },
    Ntfs {
        drive: char,
        serial: u32,
        journal_id: u64,
        next_usn: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub frn: u64,
    pub parent: u64,
    pub usn: i64,
    pub attributes: u32,
    name_start: u32,
    name_length: u32,
    fold_start: u32,
    fold_length: u32,
    pub alive: bool,
}
impl Record {
    pub fn is_dir(&self) -> bool {
        self.attributes & usn::DIRECTORY != 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub source: Source,
    pub root_frn: u64,
    pub scanned_unix: u64,
    pub generation: u64,
    pub coverage_errors: usize,
    pub coverage_notes: Vec<String>,
    pub records: Vec<Record>,
    names: Vec<u16>,
    folded: Vec<u8>,
    #[serde(skip)]
    by_frn: HashMap<u64, u32>,
    #[serde(skip)]
    order: Vec<u32>,
    #[serde(skip)]
    metadata: crate::index_metadata::Catalog,
    #[serde(skip)]
    locations: Option<Arc<crate::locations::Locations>>,
    #[serde(skip)]
    trigrams: crate::trigram::Catalog,
}

#[derive(Debug)]
pub struct SearchResult {
    pub ids: Vec<u32>,
    pub total: usize,
    pub elapsed_ms: f64,
    pub generation: u64,
}

impl Index {
    pub fn new(source: Source, root_frn: u64) -> Self {
        Self {
            source,
            root_frn,
            scanned_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            generation: 0,
            coverage_errors: 0,
            coverage_notes: Vec::new(),
            records: Vec::new(),
            names: Vec::new(),
            folded: Vec::new(),
            by_frn: HashMap::new(),
            order: Vec::new(),
            metadata: Default::default(),
            locations: None,
            trigrams: Default::default(),
        }
    }
    pub fn root_path(&self) -> PathBuf {
        match &self.source {
            Source::Folder { root } => root.clone(),
            Source::Ntfs { drive, .. } => PathBuf::from(format!("{drive}:\\")),
        }
    }
    pub fn cached_details(&self, id: u32) -> Option<crate::details::Details> {
        self.metadata.cached(id)
    }
    pub fn details(&self, id: u32) -> crate::details::Details {
        self.metadata.observe(id, || match self.path(id) {
            Ok(path) => crate::details::Details::read(&path),
            Err(e) => crate::details::Details {
                size: None,
                modified: None,
                error: Some(crate::i18n::Message::error(e)),
            },
        })
    }
    pub(crate) fn seed_details(&mut self, frn: u64, details: crate::details::Details) {
        if let Some(&id) = self.by_frn.get(&frn) {
            self.metadata.seed(id, details);
        }
    }
    pub(crate) fn invalidate_failed_metadata(&mut self) {
        self.metadata.clear_failures();
    }
    pub fn raw_name(&self, id: u32) -> &[u16] {
        let r = &self.records[id as usize];
        &self.names[r.name_start as usize..(r.name_start + r.name_length) as usize]
    }
    pub fn name(&self, id: u32) -> String {
        String::from_utf16_lossy(self.raw_name(id))
    }
    pub fn folded_name(&self, id: u32) -> &[u8] {
        let r = &self.records[id as usize];
        &self.folded[r.fold_start as usize..(r.fold_start + r.fold_length) as usize]
    }
    pub fn folded_extension(&self, id: u32) -> Option<&[u8]> {
        let name = self.folded_name(id);
        let dot = name.iter().rposition(|&b| b == b'.')?;
        Some(&name[dot + 1..])
    }
    fn store_name(&mut self, name: &[u16]) -> Result<(u32, u32, u32, u32)> {
        let fold = String::from_utf16_lossy(name).to_lowercase();
        ensure!(
            self.names
                .len()
                .checked_add(name.len())
                .is_some_and(|n| n <= MAX_POOL_BYTES / 2)
                && self
                    .folded
                    .len()
                    .checked_add(fold.len())
                    .is_some_and(|n| n <= MAX_POOL_BYTES),
            "Name pool exceeds memory budget"
        );
        self.names.try_reserve(name.len())?;
        self.folded.try_reserve(fold.len())?;
        let result = (
            self.names.len() as u32,
            name.len() as u32,
            self.folded.len() as u32,
            fold.len() as u32,
        );
        self.names.extend_from_slice(name);
        self.folded.extend_from_slice(fold.as_bytes());
        Ok(result)
    }
    pub fn upsert(&mut self, row: &UsnRecord) -> Result<bool> {
        if row.frn == self.root_frn {
            return Ok(false);
        }
        ensure!(
            !row.name.is_empty()
                && !row.name.iter().any(|v| [0, 47, 92].contains(v))
                && row.name != [46]
                && row.name != [46, 46],
            "Invalid filename component"
        );
        if let Some(&id) = self.by_frn.get(&row.frn) {
            let r = &self.records[id as usize];
            if row.usn < r.usn {
                return Ok(false);
            }
            self.metadata.invalidate(id);
            if row.parent == r.parent
                && row.attributes == r.attributes
                && self.raw_name(id) == row.name
            {
                self.records[id as usize].usn = row.usn;
                return Ok(false);
            }
        }
        if !self.by_frn.contains_key(&row.frn) {
            ensure!(
                self.records.len() < MAX_RECORDS,
                "Index exceeds {MAX_RECORDS} records"
            );
            self.records.try_reserve(1)?;
            self.by_frn.try_reserve(1)?;
            self.metadata.ensure_len(self.records.len() + 1)?;
        }
        self.locations = None;
        let (name_start, name_length, fold_start, fold_length) = self.store_name(&row.name)?;
        let record = Record {
            frn: row.frn,
            parent: row.parent,
            usn: row.usn,
            attributes: row.attributes,
            name_start,
            name_length,
            fold_start,
            fold_length,
            alive: true,
        };
        if let Some(&id) = self.by_frn.get(&row.frn) {
            if self.raw_name(id) != row.name {
                self.trigrams.changed(id);
            }
            self.records[id as usize] = record;
        } else {
            let id = self.records.len() as u32;
            self.by_frn.insert(row.frn, id);
            self.records.push(record);
            self.trigrams.changed(id);
        }
        Ok(true)
    }
    pub fn apply(&mut self, row: &UsnRecord) -> Result<bool> {
        if row.reason & usn::DELETE != 0 {
            if let Some(&id) = self.by_frn.get(&row.frn) {
                if row.usn < self.records[id as usize].usn {
                    return Ok(false);
                }
                self.metadata.invalidate(id);
                self.locations = None;
                self.by_frn.remove(&row.frn);
                self.records[id as usize].alive = false;
                return Ok(true);
            }
            return Ok(false);
        }
        // OLD_NAME is paired with NEW_NAME; retain the current entry until the new identity arrives.
        if row.reason & usn::OLD_NAME != 0 && row.reason & usn::NEW_NAME == 0 {
            return Ok(false);
        }
        self.upsert(row)
    }
    fn compare_names(&self, a: u32, b: u32) -> std::cmp::Ordering {
        self.folded_name(a).cmp(self.folded_name(b)).then(a.cmp(&b))
    }
    pub fn rebuild_order(&mut self, epoch: Option<(&AtomicU64, u64)>) -> Result<()> {
        crate::trigram::check(epoch)?;
        let mut order: Vec<u32> = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.alive)
            .map(|(i, _)| i as u32)
            .collect();
        order.par_sort_unstable_by(|&a, &b| self.compare_names(a, b));
        self.order = order;
        crate::trigram::check(epoch)?;
        self.locations = None;
        self.locations = crate::locations::Locations::build(self, &self.order).map(Arc::new);
        if self.trigrams.needs_build() {
            self.trigrams = crate::trigram::Catalog::build(self, epoch)?;
        }
        crate::trigram::check(epoch)?;
        self.generation = self.generation.wrapping_add(1);
        Ok(())
    }
    pub fn validate(&mut self, epoch: Option<(&AtomicU64, u64)>) -> Result<()> {
        crate::trigram::check(epoch)?;
        match &self.source {
            Source::Folder { root } => {
                ensure!(root.is_absolute(), "Snapshot root must be absolute")
            }
            Source::Ntfs {
                drive, next_usn, ..
            } => ensure!(
                drive.is_ascii_uppercase() && *next_usn >= 0,
                "Invalid NTFS snapshot identity"
            ),
        }
        ensure!(
            self.coverage_notes.len() <= 32 && self.coverage_notes.iter().all(|s| s.len() <= 4096),
            "Invalid coverage notes"
        );
        ensure!(
            self.records.len() <= MAX_RECORDS
                && self.names.len() <= MAX_POOL_BYTES / 2
                && self.folded.len() <= MAX_POOL_BYTES,
            "Snapshot exceeds index resource budgets"
        );
        self.by_frn.clear();
        for (id, r) in self.records.iter().enumerate() {
            if id % 256 == 0 {
                crate::trigram::check(epoch)?;
            }
            ensure!(
                r.name_start
                    .checked_add(r.name_length)
                    .is_some_and(|n| n as usize <= self.names.len())
                    && r.fold_start
                        .checked_add(r.fold_length)
                        .is_some_and(|n| n as usize <= self.folded.len()),
                "Invalid snapshot name range"
            );
            if r.alive {
                ensure!(
                    self.by_frn.insert(r.frn, id as u32).is_none(),
                    "Duplicate NTFS file identity in snapshot"
                );
            }
        }
        for (id, r) in self.records.iter().enumerate().filter(|(_, r)| r.alive) {
            if id % 256 == 0 {
                crate::trigram::check(epoch)?;
            }
            let name = self.raw_name(id as u32);
            ensure!(
                r.frn != self.root_frn
                    && !name.is_empty()
                    && !name.iter().any(|v| [0, 47, 92].contains(v))
                    && name != [46]
                    && name != [46, 46],
                "Invalid snapshot filename component"
            );
            ensure!(
                String::from_utf16_lossy(self.raw_name(id as u32))
                    .to_lowercase()
                    .as_bytes()
                    == self.folded_name(id as u32),
                "Snapshot folded name mismatch for {}",
                r.frn
            );
        }
        self.metadata.clear();
        self.metadata.ensure_len(self.records.len())?;
        self.trigrams = Default::default();
        self.rebuild_order(epoch)?;
        Ok(())
    }
    pub(crate) fn directory_id(&self, frn: u64) -> Result<u32> {
        let id = *self
            .by_frn
            .get(&frn)
            .ok_or_else(|| anyhow::anyhow!("Parent FRN {frn:016x} unavailable"))?;
        ensure!(
            self.records[id as usize].alive && self.records[id as usize].is_dir(),
            "Parent is not a directory"
        );
        Ok(id)
    }
    pub(crate) fn push_name(&self, path: &mut PathBuf, id: u32) {
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            path.push(OsString::from_wide(self.raw_name(id)));
        }
        #[cfg(not(windows))]
        {
            path.push(OsString::from(String::from_utf16_lossy(self.raw_name(id))));
        }
    }
    pub fn path(&self, id: u32) -> Result<PathBuf> {
        ensure!(
            (id as usize) < self.records.len(),
            "Unknown result record ID"
        );
        ensure!(
            self.records[id as usize].alive,
            "Missing or cyclic parent identity"
        );
        if let Some(cached) = self
            .locations
            .as_ref()
            .and_then(|cache| cache.path(self.records[id as usize].parent))
        {
            let mut path = cached?;
            self.push_name(&mut path, id);
            return Ok(path);
        }
        let mut names = Vec::new();
        let mut seen = HashSet::new();
        let mut current = id;
        loop {
            let r = &self.records[current as usize];
            ensure!(
                r.alive && seen.insert(r.frn),
                "Missing or cyclic parent identity"
            );
            names.push(current);
            if r.parent == self.root_frn {
                break;
            }
            current = self.directory_id(r.parent)?;
        }
        let mut path = self.root_path();
        for &id in names.iter().rev() {
            self.push_name(&mut path, id);
        }
        Ok(path)
    }
    pub fn search(
        &self,
        query: &Query,
        limit: usize,
        epoch: Option<(&AtomicU64, u64)>,
    ) -> Result<SearchResult> {
        self.search_ordered(query, limit, epoch, SearchOrder::default())
    }
    pub fn search_ordered(
        &self,
        query: &Query,
        limit: usize,
        epoch: Option<(&AtomicU64, u64)>,
        order: SearchOrder,
    ) -> Result<SearchResult> {
        let started = Instant::now();
        ensure!(
            (1..=100_000).contains(&limit),
            "Result limit must be 1..100000"
        );
        let candidates = self.trigrams.candidates(query, self.order.len(), epoch)?;
        if let crate::trigram::Candidates::Ids(ids) = &candidates {
            if order.column != SortColumn::Name {
                return crate::ordering::search(
                    self,
                    query,
                    limit,
                    epoch,
                    order,
                    ids.iter().copied(),
                    started,
                );
            }
            let mut matches: Vec<u32> = ids
                .par_iter()
                .filter_map(|&id| {
                    if epoch.is_some_and(|(e, n)| e.load(Ordering::Relaxed) != n) {
                        return None;
                    }
                    (self.records[id as usize].alive && query.matches(self, id)).then_some(id)
                })
                .collect();
            crate::trigram::check(epoch)?;
            let total = matches.len();
            let compare = |&a: &u32, &b: &u32| {
                let folder = (order.folders_first && !self.records[a as usize].is_dir())
                    .cmp(&(order.folders_first && !self.records[b as usize].is_dir()));
                folder.then_with(|| {
                    let names = self.compare_names(a, b);
                    if order.descending {
                        names.reverse()
                    } else {
                        names
                    }
                })
            };
            if matches.len() > limit {
                matches.select_nth_unstable_by(limit, compare);
                matches.truncate(limit);
            }
            matches.par_sort_unstable_by(compare);
            crate::trigram::check(epoch)?;
            return Ok(SearchResult {
                ids: matches,
                total,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.,
                generation: self.generation,
            });
        }
        let locations = if order.column == SortColumn::Path {
            self.locations.as_deref()
        } else {
            None
        };
        if order.column != SortColumn::Name && locations.is_none() {
            return crate::ordering::search(
                self,
                query,
                limit,
                epoch,
                order,
                0..self.records.len() as u32,
                started,
            );
        }
        let ordered = locations.map_or(self.order.as_slice(), |cache| cache.rows.as_slice());
        let counts: Vec<usize> = ordered
            .par_chunks(4096)
            .map(|ids| {
                if epoch.is_some_and(|(v, n)| v.load(Ordering::Relaxed) != n) {
                    return 0;
                }
                let mut total = 0;
                for (offset, &id) in ids.iter().enumerate() {
                    if offset % 256 == 0
                        && epoch.is_some_and(|(v, n)| v.load(Ordering::Relaxed) != n)
                    {
                        break;
                    }
                    if query.matches(self, id) {
                        total += 1;
                    }
                }
                total
            })
            .collect();
        ensure!(
            !epoch.is_some_and(|(v, n)| v.load(Ordering::Relaxed) != n),
            "Search cancelled"
        );
        let total = counts.iter().sum();
        let mut ids = Vec::with_capacity(limit.min(total));
        let phases: &[Option<bool>] = if order.folders_first {
            &[Some(true), Some(false)]
        } else {
            &[None]
        };
        'collect: for &directory in phases {
            if let Some(locations) = locations {
                for group in locations.groups(order.descending) {
                    let mut pos = group.start;
                    while pos < group.end {
                        ensure!(
                            !epoch.is_some_and(|(value, n)| value.load(Ordering::Relaxed) != n),
                            "Search cancelled"
                        );
                        if counts[pos / 4096] == 0 {
                            pos = ((pos / 4096 + 1) * 4096).min(group.end);
                            continue;
                        }
                        let id = ordered[pos];
                        pos += 1;
                        if directory.is_some_and(|d| self.records[id as usize].is_dir() != d) {
                            continue;
                        }
                        if query.matches(self, id) {
                            ids.push(id);
                            if ids.len() == limit {
                                break 'collect;
                            }
                        }
                    }
                }
                continue;
            }
            for block in 0..counts.len() {
                let block = if order.descending {
                    counts.len() - 1 - block
                } else {
                    block
                };
                if counts[block] == 0 {
                    continue;
                }
                ensure!(
                    !epoch.is_some_and(|(v, n)| v.load(Ordering::Relaxed) != n),
                    "Search cancelled"
                );
                let start = block * 4096;
                let end = (start + 4096).min(ordered.len());
                for offset in 0..end - start {
                    let pos = if order.descending {
                        end - 1 - offset
                    } else {
                        start + offset
                    };
                    let id = ordered[pos];
                    if directory.is_some_and(|d| self.records[id as usize].is_dir() != d) {
                        continue;
                    }
                    if query.matches(self, id) {
                        ids.push(id);
                        if ids.len() == limit {
                            break 'collect;
                        }
                    }
                }
            }
        }
        Ok(SearchResult {
            ids,
            total,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            generation: self.generation,
        })
    }
    pub fn len(&self) -> usize {
        self.by_frn.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_frn.is_empty()
    }
    pub(crate) fn filename_index_stats(&self) -> crate::trigram::Stats {
        self.trigrams.stats()
    }
    pub fn memory_bytes(&self) -> usize {
        self.records.capacity() * std::mem::size_of::<Record>()
            + self.names.capacity() * 2
            + self.folded.capacity()
            + self.order.capacity() * 4
            + self.metadata.memory_bytes()
            + self.trigrams.memory_bytes()
            + self
                .locations
                .as_ref()
                .map_or(0, |cache| cache.memory_bytes())
            + self.by_frn.capacity() * (std::mem::size_of::<(u64, u32)>() + 1)
    }
    pub fn note(&mut self, mut message: String) {
        self.coverage_errors += 1;
        if self.coverage_notes.len() < 32 {
            if message.len() > 4096 {
                let mut end = 4096;
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                message.truncate(end);
            }
            self.coverage_notes.push(message);
        }
    }
    pub fn compact_if_needed(&mut self) -> Result<()> {
        let live_names: usize = self
            .records
            .iter()
            .filter(|r| r.alive)
            .map(|r| r.name_length as usize)
            .sum();
        if self.records.len().saturating_sub(self.len()) <= self.len() / 4
            && (self.names.len() < 1024 * 1024 || self.names.len() < live_names.saturating_mul(2))
        {
            return Ok(());
        }
        let mut rebuilt = Index::new(self.source.clone(), self.root_frn);
        for (id, r) in self.records.iter().enumerate().filter(|(_, r)| r.alive) {
            rebuilt.upsert(&UsnRecord {
                frn: r.frn,
                parent: r.parent,
                usn: r.usn,
                reason: 0,
                attributes: r.attributes,
                name: self.raw_name(id as u32).to_vec(),
            })?;
            if let Some(details) = self
                .cached_details(id as u32)
                .filter(|details| details.error.is_none())
            {
                rebuilt.seed_details(r.frn, details);
            }
        }
        self.locations = None;
        self.records = rebuilt.records;
        self.names = rebuilt.names;
        self.folded = rebuilt.folded;
        self.by_frn = rebuilt.by_frn;
        self.metadata = rebuilt.metadata;
        self.trigrams = Default::default();
        Ok(())
    }
}

#[cfg(test)]
#[path = "trigram_bench.rs"]
mod trigram_bench;

#[cfg(test)]
mod tests {
    fn gram_fixture(count: usize) -> super::Index {
        use super::*;
        let mut index = Index::new(
            Source::Folder {
                root: r"C:\gram-fixture".into(),
            },
            1,
        );
        for n in 0..8u64 {
            index
                .upsert(&row(n + 2, 1, 0, 0, &format!("parent_{n}"), usn::DIRECTORY))
                .unwrap();
        }
        for n in 0..count {
            let name = match n % 113 {
                0 => format!("report_{n:07}.txt"),
                1 => format!("한국어문서_{n:07}.txt"),
                2 => format!("REPORT_{n:07}.TXT"),
                3 => format!("🙂alpha_beta_{n:07}.cpp"),
                4 => format!("aaaaa_{n:07}.cpp"),
                _ => format!("item_{n:07}.dat"),
            };
            let frn = n as u64 + 100;
            index
                .upsert(&row(
                    frn,
                    n as u64 % 8 + 2,
                    0,
                    0,
                    &name,
                    if n % 17 == 0 { usn::DIRECTORY } else { 0 },
                ))
                .unwrap();
            index.seed_details(
                frn,
                crate::details::Details {
                    size: (n % 17 != 0).then_some(n as u64 * 19),
                    modified: Some(
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(n as u64),
                    ),
                    error: None,
                },
            );
        }
        index.rebuild_order(None).unwrap();
        index
    }
    fn same_as_scan(
        index: &mut super::Index,
        query: &super::Query,
        order: super::SearchOrder,
        limit: usize,
    ) {
        let actual = index.search_ordered(query, limit, None, order).unwrap();
        let acceleration = std::mem::replace(
            &mut index.trigrams,
            crate::trigram::Catalog::Unavailable(crate::trigram::Unavailable::Budget),
        );
        let expected = index.search_ordered(query, limit, None, order).unwrap();
        index.trigrams = acceleration;
        assert_eq!(
            (actual.total, actual.ids),
            (expected.total, expected.ids),
            "query={query:?},order={order:?},limit={limit}"
        );
    }
    #[test]
    fn trigram_queries_match_complete_scan_counts_limits_and_every_sort() {
        use super::*;
        use crate::{filters::Filter, query::QueryOptions, trigram::Candidates};
        let mut index = gram_fixture(2300);
        assert_eq!(index.filename_index_stats().state, "ready");
        let selective = Query::parse("report_", false).unwrap();
        assert!(matches!(
            index
                .trigrams
                .candidates(&selective, index.len(), None)
                .unwrap(),
            Candidates::Ids(_)
        ));
        let cases = [
            "report_",
            "REPORT_",
            "한국어문서",
            "🙂alpha",
            "aaaa",
            "missing_xyz",
            "re",
            "",
            "report_ 한국어문서",
            "report_ !ext:txt",
            "report_ | 한국어문서",
            "report_ | it",
            "!report_",
            "!(report_ | 한국어문서)",
            "(report_ | 한국어문서) !0000001",
            "report_ path:parent_0",
            "report_ | path:parent_1",
            "report_ regex:[0-9]+",
            "report*",
            "regex:^report_",
            "ext:txt",
            r"\parent_1",
        ];
        for text in cases {
            for options in [QueryOptions::default()] {
                let query = Query::parse_with(text, options, Filter::All).unwrap();
                for column in SortColumn::ALL {
                    for descending in [false, true] {
                        for folders_first in [false, true] {
                            for limit in [1, 19, 5000] {
                                same_as_scan(
                                    &mut index,
                                    &query,
                                    SearchOrder {
                                        column,
                                        descending,
                                        folders_first,
                                    },
                                    limit,
                                );
                            }
                        }
                    }
                }
            }
        }
        for options in [
            QueryOptions {
                case_sensitive: true,
                ..Default::default()
            },
            QueryOptions {
                whole_word: true,
                ..Default::default()
            },
            QueryOptions {
                regex: true,
                ..Default::default()
            },
            QueryOptions {
                path_match: true,
                ..Default::default()
            },
        ] {
            let query = Query::parse_with("report_", options, Filter::All).unwrap();
            assert!(matches!(
                index
                    .trigrams
                    .candidates(&query, index.len(), None)
                    .unwrap(),
                Candidates::All
            ));
            same_as_scan(&mut index, &query, SearchOrder::default(), 5000);
        }
        for filter in Filter::ALL {
            same_as_scan(
                &mut index,
                &Query::parse_with("report_", QueryOptions::default(), filter).unwrap(),
                SearchOrder::default(),
                5000,
            );
        }
        let epoch = AtomicU64::new(2);
        assert!(index.search(&selective, 10, Some((&epoch, 1))).is_err());
    }
    #[test]
    fn trigram_generations_cover_rename_create_delete_reuse_and_compaction() {
        use super::*;
        use crate::trigram::Catalog;
        let mut index = gram_fixture(1000);
        let original = index.clone();
        let Catalog::Ready {
            base: original_base,
            ..
        } = &original.trigrams
        else {
            panic!()
        };
        index
            .upsert(&row(100, 2, 1, 0, "new_한국어_target.txt", 0))
            .unwrap();
        index
            .upsert(&row(9000, 3, 1, 0, "new_한국어_created.txt", 0))
            .unwrap();
        index
            .apply(&row(101, 3, 1, usn::DELETE, "한국어문서_0000001.txt", 0))
            .unwrap();
        index
            .upsert(&row(101, 3, 2, 0, "new_한국어_reused.txt", 0))
            .unwrap();
        index.rebuild_order(None).unwrap();
        let Catalog::Ready { base, changed } = &index.trigrams else {
            panic!()
        };
        assert!(Arc::ptr_eq(base, original_base));
        assert_eq!(changed.len(), 3);
        assert_eq!(
            original
                .search(&Query::parse("new_한국어", false).unwrap(), 5000, None)
                .unwrap()
                .total,
            0
        );
        for text in [
            "new_한국어",
            "report_",
            "한국어문서",
            "report_ | new_한국어",
            "new_한국어 !created",
        ] {
            same_as_scan(
                &mut index,
                &Query::parse(text, false).unwrap(),
                SearchOrder::default(),
                5000,
            );
        }
        // Metadata and moves retain the filename posting base and the delta.
        let before = index.filename_index_stats().changed_records;
        index
            .upsert(&row(100, 4, 3, 0, "new_한국어_target.txt", usn::DIRECTORY))
            .unwrap();
        index.rebuild_order(None).unwrap();
        assert_eq!(index.filename_index_stats().changed_records, before);
        // Cross the bounded cumulative delta and rebuild, without changing the
        // snapshot still held by an already running reader.
        for n in 200..280 {
            index
                .upsert(&row(n, 2, 4, 0, &format!("renamed_{n}.txt"), 0))
                .unwrap();
        }
        index.rebuild_order(None).unwrap();
        let Catalog::Ready { base, changed } = &index.trigrams else {
            panic!()
        };
        assert!(!Arc::ptr_eq(base, original_base));
        assert!(changed.is_empty());
        for n in 300..700 {
            index
                .apply(&row(n, 2, 5, usn::DELETE, "removed", 0))
                .unwrap();
        }
        index.compact_if_needed().unwrap();
        index.rebuild_order(None).unwrap();
        for text in ["renamed_", "new_한국어", "item_"] {
            same_as_scan(
                &mut index,
                &Query::parse(text, false).unwrap(),
                SearchOrder::default(),
                71,
            );
        }
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("grams.reaidx");
        crate::storage::save(&index, &path).unwrap();
        let mut loaded = crate::storage::load(&path, None).unwrap();
        assert_eq!(loaded.filename_index_stats().state, "ready");
        same_as_scan(
            &mut loaded,
            &Query::parse("new_한국어", false).unwrap(),
            SearchOrder::default(),
            5000,
        );
    }
    use super::*;
    fn row(frn: u64, parent: u64, usn: i64, reason: u32, name: &str, attributes: u32) -> UsnRecord {
        UsnRecord {
            frn,
            parent,
            usn,
            reason,
            name: name.encode_utf16().collect(),
            attributes,
        }
    }
    #[test]
    fn rename_delete_and_reused_record_identity() {
        let mut i = Index::new(
            Source::Folder {
                root: "C:\\fixture".into(),
            },
            1,
        );
        i.upsert(&row(2, 1, 1, 0, "folder", usn::DIRECTORY))
            .unwrap();
        i.upsert(&row(0x1000000000003, 2, 2, 0, "old.txt", 0))
            .unwrap();
        assert!(
            !i.apply(&row(0x1000000000003, 2, 3, usn::OLD_NAME, "old.txt", 0))
                .unwrap()
        );
        i.apply(&row(0x1000000000003, 1, 4, usn::NEW_NAME, "new.txt", 0))
            .unwrap();
        let id = i.by_frn[&0x1000000000003];
        assert_eq!(
            i.path(id).unwrap(),
            PathBuf::from("C:\\fixture").join("new.txt")
        );
        assert!(
            !i.apply(&row(0x1000000000003, 2, 2, usn::DELETE, "old.txt", 0))
                .unwrap()
        );
        i.apply(&row(0x1000000000003, 1, 5, usn::DELETE, "new.txt", 0))
            .unwrap();
        i.apply(&row(0x2000000000003, 1, 6, usn::CREATE, "reused.txt", 0))
            .unwrap();
        i.rebuild_order(None).unwrap();
        assert_eq!(
            i.search(&Query::parse("txt", false).unwrap(), 10, None)
                .unwrap()
                .total,
            1
        );
        assert_eq!(i.name(i.by_frn[&0x2000000000003]), "reused.txt");
    }

    #[test]
    fn journal_record_changes_and_compaction_preserve_unaffected_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"bb").unwrap();
        let original = crate::folder::scan(dir.path(), None).unwrap();
        let query = |q: &str| Query::parse(q, false).unwrap();
        let aid = original.search(&query("a.txt"), 1, None).unwrap().ids[0];
        let bid = original.search(&query("b.txt"), 1, None).unwrap().ids[0];
        let ar = original.records[aid as usize].clone();
        let br = original.records[bid as usize].clone();
        let mut next = original.clone();
        assert_eq!(next.cached_details(aid).unwrap().size, Some(1));
        std::fs::write(&a, b"changed").unwrap();
        std::fs::write(&b, b"later unobserved data").unwrap();
        next.apply(&row(ar.frn, ar.parent, 1, 0, "a.txt", ar.attributes))
            .unwrap();
        assert!(next.cached_details(aid).is_none());
        assert_eq!(next.details(aid).size, Some(7));
        assert_eq!(next.details(bid).size, Some(2));
        assert_eq!(original.details(aid).size, Some(1));
        next.apply(&row(
            ar.frn,
            ar.parent,
            2,
            usn::DELETE,
            "a.txt",
            ar.attributes,
        ))
        .unwrap();
        next.compact_if_needed().unwrap();
        next.rebuild_order(None).unwrap();
        let new_bid = next.by_frn[&br.frn];
        assert_eq!(next.cached_details(new_bid).unwrap().size, Some(2));
        assert_eq!(original.details(bid).size, Some(2));
    }

    #[test]
    fn prepared_locations_match_general_sort_in_both_directions_and_keep_unknown_last() {
        let mut index = Index::new(
            Source::Folder {
                root: PathBuf::from(r"C:\fixture"),
            },
            1,
        );
        for (frn, parent, name, dir) in [
            (2, 1, "a", true),
            (3, 1, "a-b", true),
            (4, 1, "a0", true),
            (5, 1, "A", true),
            (6, 2, "b", true),
            (7, 1, "한글 ΟΣ", true),
            (8, 9, "cycle-a", true),
            (9, 8, "cycle-b", true),
            (10, 1, "not-dir", false),
        ] {
            index
                .upsert(&row(
                    frn,
                    parent,
                    0,
                    0,
                    name,
                    if dir { usn::DIRECTORY } else { 0 },
                ))
                .unwrap();
        }
        for n in 0..5007 {
            let parent = [1, 2, 3, 4, 5, 6, 7, 999, 8, 10][n % 10];
            index
                .upsert(&row(
                    100 + n as u64,
                    parent,
                    0,
                    0,
                    &format!("file-{n:05}.txt"),
                    0,
                ))
                .unwrap();
        }
        index.upsert(&row(50, 2, 0, 0, "same.TXT", 0)).unwrap();
        index.upsert(&row(51, 5, 0, 0, "SAME.txt", 0)).unwrap();
        #[cfg(windows)]
        {
            index
                .upsert(&UsnRecord {
                    frn: 60,
                    parent: 1,
                    usn: 0,
                    reason: 0,
                    attributes: usn::DIRECTORY,
                    name: vec![0xd800],
                })
                .unwrap();
            index.upsert(&row(61, 60, 0, 0, "child.txt", 0)).unwrap();
        }
        index.rebuild_order(None).unwrap();
        assert!(index.locations.is_some());
        let mut general = index.clone();
        general.locations = None;
        for &id in &index.order {
            assert_eq!(index.path(id).ok(), general.path(id).ok());
        }
        for query in ["", "file:", "folder:", "ext:txt", "path:a"] {
            let query = Query::parse(query, false).unwrap();
            for descending in [false, true] {
                for folders_first in [false, true] {
                    for limit in [1, 7, 5000] {
                        let order = SearchOrder {
                            column: SortColumn::Path,
                            descending,
                            folders_first,
                        };
                        let actual = index.search_ordered(&query, limit, None, order).unwrap();
                        let expected = crate::ordering::search(
                            &general,
                            &query,
                            limit,
                            None,
                            order,
                            0..general.records.len() as u32,
                            Instant::now(),
                        )
                        .unwrap();
                        assert_eq!(actual.total, expected.total);
                        assert_eq!(
                            actual.ids, expected.ids,
                            "descending={descending}, folders_first={folders_first}, limit={limit}"
                        );
                    }
                }
            }
        }
        let epoch = AtomicU64::new(2);
        assert!(
            index
                .search_ordered(
                    &Query::parse("", false).unwrap(),
                    10,
                    Some((&epoch, 1)),
                    SearchOrder {
                        column: SortColumn::Path,
                        ..Default::default()
                    }
                )
                .is_err()
        );
        let empty = Index::new(
            Source::Folder {
                root: PathBuf::from(r"C:\fixture"),
            },
            1,
        );
        assert!(
            empty
                .search_ordered(
                    &Query::parse("", false).unwrap(),
                    10,
                    None,
                    SearchOrder {
                        column: SortColumn::Path,
                        ..Default::default()
                    }
                )
                .unwrap()
                .ids
                .is_empty()
        );
    }
    #[test]
    fn location_generations_reuse_unchanged_paths_and_rebuild_after_directory_rename() {
        let mut index = Index::new(
            Source::Folder {
                root: PathBuf::from(r"C:\fixture"),
            },
            1,
        );
        index
            .upsert(&row(2, 1, 0, 0, "before", usn::DIRECTORY))
            .unwrap();
        index.upsert(&row(3, 2, 0, 0, "child.txt", 0)).unwrap();
        index.rebuild_order(None).unwrap();
        let old = index.clone();
        let cache = old.locations.as_ref().unwrap();
        assert!(Arc::ptr_eq(cache, index.locations.as_ref().unwrap()));
        index.apply(&row(3, 2, 1, 0, "child.txt", 0)).unwrap();
        assert!(Arc::ptr_eq(cache, index.locations.as_ref().unwrap()));
        index
            .apply(&row(2, 1, 2, usn::NEW_NAME, "after", usn::DIRECTORY))
            .unwrap();
        assert!(index.locations.is_none());
        index.rebuild_order(None).unwrap();
        assert!(!Arc::ptr_eq(cache, index.locations.as_ref().unwrap()));
        let id = index.by_frn[&3];
        assert!(index.path(id).unwrap().ends_with(r"after\child.txt"));
        assert!(
            old.path(old.by_frn[&3])
                .unwrap()
                .ends_with(r"before\child.txt")
        );
        index
            .apply(&row(2, 1, 3, usn::DELETE, "after", usn::DIRECTORY))
            .unwrap();
        index.compact_if_needed().unwrap();
        index.rebuild_order(None).unwrap();
        assert!(index.path(index.by_frn[&3]).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn nonstandard_drive_component_keeps_the_general_path_semantics() {
        let mut index = Index::new(
            Source::Folder {
                root: PathBuf::from(r"C:\fixture"),
            },
            1,
        );
        index.upsert(&row(2, 1, 0, 0, "C:entry", 0)).unwrap();
        index.rebuild_order(None).unwrap();
        assert!(index.locations.is_none());
        let query = Query::parse("", false).unwrap();
        let actual = index
            .search_ordered(
                &query,
                10,
                None,
                SearchOrder {
                    column: SortColumn::Path,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(actual.total, 1);
    }

    #[test]
    fn unresolved_and_cyclic_parents_do_not_fabricate_paths() {
        let mut i = Index::new(
            Source::Folder {
                root: "C:\\fixture".into(),
            },
            1,
        );
        i.upsert(&row(2, 99, 1, 0, "lost.txt", 0)).unwrap();
        assert!(i.path(0).is_err());
        i.upsert(&row(99, 2, 1, 0, "cycle", usn::DIRECTORY))
            .unwrap();
        assert!(i.path(0).is_err());
    }
}
