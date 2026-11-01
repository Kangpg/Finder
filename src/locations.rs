//! Snapshot-owned parent paths and stable location groups. No filesystem calls.
use crate::index::Index;
use anyhow::{Result, anyhow, ensure};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
const BUDGET: usize = 64 * 1024 * 1024;
#[derive(Debug)]
struct Parent {
    path: std::result::Result<PathBuf, String>,
    rank: usize,
}
#[derive(Debug)]
pub(crate) struct Locations {
    by_frn: HashMap<u64, usize>,
    parents: Vec<Parent>,
    pub rows: Vec<u32>,
    pub boundaries: Vec<u32>,
    pub unknown: bool,
    estimated_bytes: usize,
}
struct Builder<'a> {
    index: &'a Index,
    data: Locations,
    bytes: usize,
    budget: usize,
}
impl Locations {
    pub fn build(index: &Index, order: &[u32]) -> Option<Self> {
        Self::build_with_budget(index, order, BUDGET).ok()
    }
    fn build_with_budget(index: &Index, order: &[u32], budget: usize) -> Result<Self> {
        let mut builder = Builder {
            index,
            data: Self {
                by_frn: HashMap::new(),
                parents: Vec::new(),
                rows: Vec::new(),
                boundaries: Vec::new(),
                unknown: false,
                estimated_bytes: 0,
            },
            bytes: 0,
            budget,
        };
        builder.insert(index.root_frn, Ok(index.root_path()))?;
        let mut used = HashSet::new();
        for &id in order {
            // Drive-prefixed synthetic components change PathBuf::push semantics. Keep the
            // existing general comparator for these nonstandard names, rather than invent a key.
            #[cfg(windows)]
            {
                let name = index.raw_name(id);
                ensure!(
                    !(name.get(1) == Some(&58)
                        && name
                            .first()
                            .is_some_and(|c| (*c as u8).is_ascii_alphabetic() && *c <= 127)),
                    "Nonstandard path component"
                );
            }
            let parent = builder.resolve(index.records[id as usize].parent)?;
            used.insert(parent);
        }
        let mut keys = Vec::with_capacity(used.len());
        for parent in used {
            let key = builder.data.parents[parent]
                .path
                .as_ref()
                .ok()
                .map(|p| p.to_string_lossy().to_lowercase());
            if let Some(key) = &key {
                builder.charge(key.capacity())?;
            }
            builder.charge(std::mem::size_of::<(usize, Option<String>)>())?;
            keys.push((parent, key));
        }
        keys.sort_unstable_by(|a, b| a.1.is_none().cmp(&b.1.is_none()).then(a.1.cmp(&b.1)));
        let mut counts = Vec::<u32>::new();
        let mut previous: Option<&Option<String>> = None;
        for (parent, key) in &keys {
            if previous != Some(key) {
                counts.push(0);
                previous = Some(key);
            }
            builder.data.parents[*parent].rank = counts.len() - 1;
        }
        builder.data.unknown = keys.last().is_some_and(|(_, key)| key.is_none());
        for &id in order {
            let rank =
                builder.data.parents[builder.data.by_frn[&index.records[id as usize].parent]].rank;
            counts[rank] += 1;
        }
        let mut offset = 0;
        builder
            .data
            .boundaries
            .try_reserve_exact(counts.len() + 1)?;
        for count in &counts {
            builder.data.boundaries.push(offset);
            offset += count;
        }
        builder.data.boundaries.push(offset);
        builder.data.rows.try_reserve_exact(order.len())?;
        builder.data.rows.resize(order.len(), 0);
        let mut cursors = builder.data.boundaries[..counts.len()].to_vec();
        // Global name order makes every equal-location group name-ascending, including ties
        // between case-fold-equivalent physical directories.
        for &id in order {
            let rank =
                builder.data.parents[builder.data.by_frn[&index.records[id as usize].parent]].rank;
            builder.data.rows[cursors[rank] as usize] = id;
            cursors[rank] += 1;
        }
        builder.data.estimated_bytes = builder.data.rows.capacity() * 4
            + builder.data.boundaries.capacity() * 4
            + builder.data.parents.capacity() * std::mem::size_of::<Parent>()
            + builder.data.by_frn.capacity() * (std::mem::size_of::<(u64, usize)>() + 1)
            + builder
                .data
                .parents
                .iter()
                .map(|p| match &p.path {
                    Ok(p) => p.capacity(),
                    Err(e) => e.capacity(),
                })
                .sum::<usize>();
        Ok(builder.data)
    }
    pub fn path(&self, parent: u64) -> Option<Result<PathBuf>> {
        let entry = &self.parents[*self.by_frn.get(&parent)?];
        Some(entry.path.clone().map_err(|e| anyhow!(e)))
    }
    pub fn groups(&self, descending: bool) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
        let count = self.boundaries.len().saturating_sub(1);
        let known = count - usize::from(self.unknown);
        (0..count).map(move |i| {
            let group = if i < known && descending {
                known - 1 - i
            } else {
                i
            };
            self.boundaries[group] as usize..self.boundaries[group + 1] as usize
        })
    }
    pub fn memory_bytes(&self) -> usize {
        self.estimated_bytes
    }
}
impl Builder<'_> {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        ensure!(
            bytes <= self.budget.saturating_sub(self.bytes),
            "Location cache memory budget exceeded"
        );
        self.bytes += bytes;
        Ok(())
    }
    fn insert(&mut self, frn: u64, path: std::result::Result<PathBuf, String>) -> Result<usize> {
        self.charge(
            std::mem::size_of::<Parent>() * 2
                + std::mem::size_of::<(u64, usize)>() * 2
                + match &path {
                    Ok(p) => p.capacity(),
                    Err(e) => e.capacity(),
                },
        )?;
        let slot = self.data.parents.len();
        self.data.parents.try_reserve(1)?;
        self.data.by_frn.try_reserve(1)?;
        self.data.parents.push(Parent { path, rank: 0 });
        self.data.by_frn.insert(frn, slot);
        Ok(slot)
    }
    fn resolve(&mut self, frn: u64) -> Result<usize> {
        if let Some(&id) = self.data.by_frn.get(&frn) {
            return Ok(id);
        }
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut current = frn;
        let mut base = loop {
            ensure!(
                self.bytes.saturating_add(chain.len() * 64) < self.budget,
                "Location ancestry exceeds memory budget"
            );
            if let Some(&id) = self.data.by_frn.get(&current) {
                break self.data.parents[id].path.clone();
            }
            if !seen.insert(current) {
                break Err("Missing or cyclic parent identity".into());
            }
            match self.index.directory_id(current) {
                Ok(id) => {
                    chain.push((current, id));
                    current = self.index.records[id as usize].parent;
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    self.insert(current, Err(error.clone()))?;
                    break Err(error);
                }
            }
        };
        while let Some((parent, id)) = chain.pop() {
            if let Ok(path) = &mut base {
                self.index.push_name(path, id);
            }
            self.insert(parent, base.clone())?;
        }
        self.data
            .by_frn
            .get(&frn)
            .copied()
            .ok_or_else(|| anyhow!("Unresolved location cache entry"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_cache_budget_does_not_mutate_the_name_index() {
        let mut index = Index::new(
            crate::index::Source::Folder {
                root: PathBuf::from("C:/fixture"),
            },
            1,
        );
        index
            .upsert(&crate::usn::UsnRecord {
                frn: 2,
                parent: 1,
                usn: 0,
                reason: 0,
                attributes: 0,
                name: "file.txt".encode_utf16().collect(),
            })
            .unwrap();
        index.rebuild_order(None).unwrap();
        let ids = index
            .search(&crate::query::Query::parse("", false).unwrap(), 10, None)
            .unwrap()
            .ids;
        assert!(Locations::build_with_budget(&index, &ids, 1).is_err());
        assert_eq!(
            index
                .search(&crate::query::Query::parse("", false).unwrap(), 10, None)
                .unwrap()
                .ids,
            ids
        );
    }
}
