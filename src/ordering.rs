//! Bounded full-match sorting. Names retain the precomputed fast path in Index.
use crate::{
    index::{Index, SearchOrder, SearchResult, SortColumn},
    query::Query,
};
use anyhow::{Result, ensure};
use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
    time::{Instant, SystemTime},
};
#[derive(Eq, PartialEq, Ord, PartialOrd)]
enum Key {
    Text(String),
    Size(u64),
    Date(SystemTime),
}
struct Candidate {
    id: u32,
    folder_rank: bool,
    key: Option<Key>,
    name: Vec<u8>,
    descending: bool,
}
impl Candidate {
    fn key_bytes(&self) -> usize {
        self.name.capacity()
            + match &self.key {
                Some(Key::Text(text)) => text.capacity(),
                _ => 0,
            }
    }
}
impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.folder_rank
            .cmp(&other.folder_rank)
            .then_with(|| self.key.is_none().cmp(&other.key.is_none()))
            .then_with(|| {
                let c = self.key.cmp(&other.key);
                if self.descending { c.reverse() } else { c }
            })
            .then_with(|| self.name.cmp(&other.name))
            .then(self.id.cmp(&other.id))
    }
}
pub(crate) fn search(
    index: &Index,
    query: &Query,
    limit: usize,
    epoch: Option<(&AtomicU64, u64)>,
    order: SearchOrder,
    ids: impl Iterator<Item = u32>,
    start: Instant,
) -> Result<SearchResult> {
    let mut total = 0;
    let mut heap = BinaryHeap::with_capacity(limit);
    let mut retained_bytes = 0usize;
    const MAX_SORT_KEY_BYTES: usize = 64 * 1024 * 1024;
    for id in ids {
        let record = &index.records[id as usize];
        ensure!(
            !epoch.is_some_and(|(e, n)| e.load(AtomicOrdering::Relaxed) != n),
            "Search cancelled"
        );
        if !record.alive || !query.matches(index, id) {
            continue;
        }
        total += 1;
        let key = match order.column {
            SortColumn::Name => Some(Key::Text(
                String::from_utf8_lossy(index.folded_name(id)).into_owned(),
            )),
            SortColumn::Path => index.path(id).ok().and_then(|p| {
                p.parent()
                    .map(|p| Key::Text(p.to_string_lossy().to_lowercase()))
            }),
            SortColumn::Type => Some(Key::Text(if record.is_dir() {
                crate::i18n::text("폴더", "Folders").into()
            } else {
                String::from_utf8_lossy(index.folded_extension(id).unwrap_or_default()).into_owned()
            })),
            SortColumn::Size | SortColumn::Modified => {
                let d = index.details(id);
                if order.column == SortColumn::Size {
                    d.size.map(Key::Size)
                } else {
                    d.modified.map(Key::Date)
                }
            }
        };
        let candidate = Candidate {
            id,
            folder_rank: order.folders_first && !record.is_dir(),
            key,
            name: index.folded_name(id).to_vec(),
            descending: order.descending,
        };
        if heap.len() < limit || heap.peek().is_some_and(|worst| candidate < *worst) {
            let removed = if heap.len() == limit {
                heap.peek().unwrap().key_bytes()
            } else {
                0
            };
            let next = retained_bytes - removed;
            ensure!(
                candidate.key_bytes() <= MAX_SORT_KEY_BYTES - next,
                "Sort keys exceed the 64 MB memory budget; narrow the search or choose name sorting"
            );
            retained_bytes = next + candidate.key_bytes();
            if heap.len() < limit {
                heap.push(candidate);
            } else {
                *heap.peek_mut().unwrap() = candidate;
            }
        }
    }
    ensure!(
        !epoch.is_some_and(|(e, n)| e.load(AtomicOrdering::Relaxed) != n),
        "Search cancelled"
    );
    Ok(SearchResult {
        ids: heap.into_sorted_vec().into_iter().map(|c| c.id).collect(),
        total,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.,
        generation: index.generation,
    })
}
