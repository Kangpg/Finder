//! Snapshot-owned, collision-safe hashed Unicode trigram postings. A posting is
//! only a candidate: the original Query always decides whether an item matches.
use crate::index::Index;
use anyhow::{Result, ensure};
use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const BUILD_BYTES: usize = 128 * 1024 * 1024;
const QUERY_BYTES: usize = 16 * 1024 * 1024;
pub(crate) type Epoch<'a> = Option<(&'a AtomicU64, u64)>;
pub(crate) fn check(epoch: Epoch<'_>) -> Result<()> {
    ensure!(
        !epoch.is_some_and(|(value, id)| value.load(Ordering::Relaxed) != id),
        "Search/index preparation cancelled"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Unavailable {
    Budget,
    Allocation,
    InvalidName,
}
#[derive(Debug, Clone, Default)]
pub(crate) enum Catalog {
    #[default]
    Pending,
    Ready {
        base: Arc<Postings>,
        changed: Vec<u32>,
    },
    Unavailable(Unavailable),
}
#[derive(Debug, serde::Serialize)]
pub(crate) struct Stats {
    pub state: &'static str,
    pub estimated_bytes: usize,
    pub changed_records: usize,
}
impl Catalog {
    pub fn stats(&self) -> Stats {
        let (state, changed_records) = match self {
            Self::Pending => ("pending", 0),
            Self::Ready { changed, .. } => ("ready", changed.len()),
            Self::Unavailable(reason) => (
                match reason {
                    Unavailable::Budget => "memory_budget",
                    Unavailable::Allocation => "allocation_failed",
                    Unavailable::InvalidName => "unsupported_name_encoding",
                },
                0,
            ),
        };
        Stats {
            state,
            estimated_bytes: self.memory_bytes(),
            changed_records,
        }
    }
    pub fn needs_build(&self) -> bool {
        matches!(self, Self::Pending)
    }
    pub fn changed(&mut self, id: u32) {
        if let Self::Ready { base, changed } = self
            && let Err(position) = changed.binary_search(&id)
        {
            let limit = (base.records / 64).clamp(64, 4096);
            if changed.len() == limit {
                *self = Self::Pending;
            } else {
                changed.insert(position, id);
            }
        }
        // Budget failures remain a scan-only generation until a full
        // rebuild/load/compaction, rather than retrying on each USN tick.
    }
    pub fn candidates<'a>(
        &'a self,
        query: &crate::query::Query,
        live: usize,
        epoch: Epoch<'_>,
    ) -> Result<Candidates<'a>> {
        let Self::Ready { base, changed } = self else {
            return Ok(Candidates::All);
        };
        // Dense candidates seldom repay enumeration + result ordering. This is
        // a performance choice, never a completeness or result-count limit.
        let cap = (live / 4).min(QUERY_BYTES / (2 * size_of::<u32>()));
        let mut budget = PlanBudget {
            remaining: QUERY_BYTES,
        };
        let selected = query.candidates(base, cap, &mut budget, epoch)?;
        selected.union(
            Candidates::Ids(Cow::Borrowed(changed)),
            cap,
            &mut budget,
            epoch,
        )
    }
    pub fn memory_bytes(&self) -> usize {
        match self {
            Self::Ready { base, changed } => {
                base.offsets.capacity() * 4
                    + base.ids.capacity() * 4
                    + changed.capacity() * 4
                    + size_of::<Postings>()
                    + 2 * size_of::<usize>()
            }
            _ => 0,
        }
    }
    pub fn build(index: &Index, epoch: Epoch<'_>) -> Result<Self> {
        Self::with_budget(index, epoch, BUILD_BYTES)
    }
    fn with_budget(index: &Index, epoch: Epoch<'_>, budget: usize) -> Result<Self> {
        check(epoch)?;
        let buckets = index.records.len().next_power_of_two().clamp(256, 262144);
        // Include offsets, stamps and filling cursors before allocating any
        // postings. Count representation expansion, not just filename bytes.
        let fixed = (3 * buckets + 1) * size_of::<u32>();
        if fixed > budget {
            return Ok(Self::Unavailable(Unavailable::Budget));
        }
        let allocate = |len: usize| -> Option<Vec<u32>> {
            let mut values = Vec::new();
            values.try_reserve_exact(len).ok()?;
            values.resize(len, 0);
            Some(values)
        };
        let Some(mut offsets) = allocate(buckets + 1) else {
            return Ok(Self::Unavailable(Unavailable::Allocation));
        };
        let Some(mut stamps) = allocate(buckets) else {
            return Ok(Self::Unavailable(Unavailable::Allocation));
        };
        let mask = buckets - 1;
        let mut total = 0usize;
        for (id, record) in index.records.iter().enumerate() {
            if id % 256 == 0 {
                check(epoch)?;
            }
            if !record.alive {
                continue;
            }
            let Ok(name) = std::str::from_utf8(index.folded_name(id as u32)) else {
                return Ok(Self::Unavailable(Unavailable::InvalidName));
            };
            for (position, bucket) in grams(name, mask).enumerate() {
                if position % 1024 == 0 {
                    check(epoch)?;
                }
                if stamps[bucket] == id as u32 + 1 {
                    continue;
                }
                if total >= (budget - fixed) / 4 {
                    return Ok(Self::Unavailable(Unavailable::Budget));
                }
                stamps[bucket] = id as u32 + 1;
                offsets[bucket + 1] += 1;
                total += 1;
            }
        }
        for n in 1..offsets.len() {
            offsets[n] += offsets[n - 1];
        }
        let Some(mut ids) = allocate(total) else {
            return Ok(Self::Unavailable(Unavailable::Allocation));
        };
        let Some(mut cursors) = allocate(buckets) else {
            return Ok(Self::Unavailable(Unavailable::Allocation));
        };
        cursors.copy_from_slice(&offsets[..buckets]);
        stamps.fill(0);
        for (id, record) in index.records.iter().enumerate() {
            if id % 256 == 0 {
                check(epoch)?;
            }
            if !record.alive {
                continue;
            }
            let Ok(name) = std::str::from_utf8(index.folded_name(id as u32)) else {
                return Ok(Self::Unavailable(Unavailable::InvalidName));
            };
            for (position, bucket) in grams(name, mask).enumerate() {
                if position % 1024 == 0 {
                    check(epoch)?;
                }
                if stamps[bucket] != id as u32 + 1 {
                    stamps[bucket] = id as u32 + 1;
                    ids[cursors[bucket] as usize] = id as u32;
                    cursors[bucket] += 1;
                }
            }
        }
        check(epoch)?;
        Ok(Self::Ready {
            base: Arc::new(Postings {
                offsets,
                ids,
                records: index.records.len(),
            }),
            changed: Vec::new(),
        })
    }
}
#[derive(Debug)]
pub(crate) struct Postings {
    offsets: Vec<u32>,
    ids: Vec<u32>,
    records: usize,
}
impl Postings {
    pub fn literal(&self, needle: &[u8], cap: usize) -> Candidates<'_> {
        let Ok(text) = std::str::from_utf8(needle) else {
            return Candidates::All;
        };
        // Choose the rarest required gram. Full evaluation is generally cheaper
        // than intersecting additional postings after this selective first step.
        let selected = grams(text, self.offsets.len() - 2)
            .map(|bucket| {
                &self.ids[self.offsets[bucket] as usize..self.offsets[bucket + 1] as usize]
            })
            .min_by_key(|ids| ids.len());
        match selected {
            Some(ids) if ids.len() <= cap => Candidates::Ids(Cow::Borrowed(ids)),
            _ => Candidates::All,
        }
    }
}

// Three Unicode scalar values form an exact 63-bit key before hashing. Hash
// collisions add candidates only; they can never remove a genuine match.
fn grams(text: &str, mask: usize) -> impl Iterator<Item = usize> + '_ {
    let mut chars = text.chars();
    let mut a = chars.next();
    let mut b = chars.next();
    std::iter::from_fn(move || {
        let c = chars.next()?;
        let mut key =
            (u64::from(a? as u32) << 42) | (u64::from(b? as u32) << 21) | u64::from(c as u32);
        a = b;
        b = Some(c);
        key ^= key >> 33;
        key = key.wrapping_mul(0xff51afd7ed558ccd);
        key ^= key >> 33;
        key = key.wrapping_mul(0xc4ceb9fe1a85ec53);
        key ^= key >> 33;
        Some(key as usize & mask)
    })
}

#[derive(Debug)]
pub(crate) struct PlanBudget {
    remaining: usize,
}

pub(crate) enum Candidates<'a> {
    All,
    Ids(Cow<'a, [u32]>),
}
impl<'a> Candidates<'a> {
    pub fn len(&self) -> usize {
        match self {
            Self::All => usize::MAX,
            Self::Ids(ids) => ids.len(),
        }
    }
    pub fn union(
        self,
        other: Self,
        cap: usize,
        budget: &mut PlanBudget,
        epoch: Epoch<'_>,
    ) -> Result<Self> {
        let (Self::Ids(left), Self::Ids(right)) = (self, other) else {
            return Ok(Self::All);
        };
        if left.len() > cap || right.len() > cap {
            return Ok(Self::All);
        }
        if left.is_empty() {
            return Ok(Self::Ids(right));
        }
        if right.is_empty() {
            return Ok(Self::Ids(left));
        }
        let mut merged = Vec::new();
        // All owned allocations across the complete recursive plan share one
        // monotone budget. Borrowed base postings consume no scratch.
        let capacity = (left.len() + right.len()).min(cap);
        if capacity > budget.remaining / 4 || merged.try_reserve_exact(capacity).is_err() {
            return Ok(Self::All);
        }
        let bytes = merged.capacity() * 4;
        if bytes > budget.remaining {
            return Ok(Self::All);
        }
        budget.remaining -= bytes;
        let (mut a, mut b) = (0, 0);
        while a < left.len() || b < right.len() {
            if merged.len() % 1024 == 0 {
                check(epoch)?;
            }
            let next = match (left.get(a), right.get(b)) {
                (Some(&l), Some(&r)) if l == r => {
                    a += 1;
                    b += 1;
                    l
                }
                (Some(&l), Some(&r)) if l < r => {
                    a += 1;
                    l
                }
                (Some(_), Some(&r)) => {
                    b += 1;
                    r
                }
                (Some(&l), None) => {
                    a += 1;
                    l
                }
                (None, Some(&r)) => {
                    b += 1;
                    r
                }
                _ => unreachable!(),
            };
            if merged.len() == cap {
                return Ok(Self::All);
            }
            merged.push(next);
        }
        Ok(Self::Ids(Cow::Owned(merged)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{index::Source, query::Query, usn::UsnRecord};
    #[test]
    fn collisions_unicode_budget_and_cancellation_preserve_complete_search() {
        let mut seen = std::collections::HashMap::new();
        let mut collision = None;
        'find: for a in b'a'..=b'z' {
            for b in b'a'..=b'z' {
                for c in b'a'..=b'z' {
                    let text = String::from_utf8(vec![a, b, c]).unwrap();
                    let bucket = grams(&text, 255).next().unwrap();
                    if let Some(other) = seen.insert(bucket, text.clone()) {
                        collision = Some((other, text));
                        break 'find;
                    }
                }
            }
        }
        let (first, second) = collision.unwrap();
        let mut index = Index::new(
            Source::Folder {
                root: r"C:\trigram-boundary".into(),
            },
            1,
        );
        let mut add = |frn, name: Vec<u16>| {
            index
                .upsert(&UsnRecord {
                    frn,
                    parent: 1,
                    usn: 0,
                    reason: 0,
                    attributes: 0,
                    name,
                })
                .unwrap();
        };
        add(2, first.encode_utf16().collect());
        add(3, second.encode_utf16().collect());
        add(4, "한국어🙂abc".encode_utf16().collect());
        add(5, vec![0xd800, 97, 98, 99]);
        for n in 0..100 {
            add(n + 10, format!("noise_{n:03}").encode_utf16().collect());
        }
        index.rebuild_order(None).unwrap();
        let Catalog::Ready { base, .. } = Catalog::build(&index, None).unwrap() else {
            panic!()
        };
        let Candidates::Ids(ids) = base.literal(first.as_bytes(), 1000) else {
            panic!()
        };
        assert!(ids.contains(&0) && ids.contains(&1));
        assert_eq!(
            index
                .search(&Query::parse(&first, false).unwrap(), 5000, None)
                .unwrap()
                .total,
            1
        );
        assert!(matches!(
            base.literal("한국".as_bytes(), 1000),
            Candidates::All
        ));
        for text in ["한국어", "🙂abc", "�ab"] {
            assert_eq!(
                index
                    .search(&Query::parse(text, false).unwrap(), 5000, None)
                    .unwrap()
                    .total,
                1
            );
        }
        assert_eq!(
            Catalog::with_budget(&index, None, 0).unwrap().stats().state,
            "memory_budget"
        );
        assert_eq!(
            Catalog::with_budget(&index, None, (3 * 256 + 1) * 4 + 4)
                .unwrap()
                .stats()
                .state,
            "memory_budget"
        );
        let ready = Catalog::with_budget(&index, None, 32 * 1024).unwrap();
        assert_eq!(ready.stats().state, "ready");
        assert!(ready.memory_bytes() <= 32 * 1024);
        let epoch = AtomicU64::new(2);
        assert!(Catalog::build(&index, Some((&epoch, 1))).is_err());
        assert!(
            ready
                .candidates(
                    &Query::parse(&first, false).unwrap(),
                    index.len(),
                    Some((&epoch, 1))
                )
                .is_err()
        );
        let mut budget = PlanBudget { remaining: 0 };
        assert!(matches!(
            Candidates::Ids(Cow::Borrowed(&[1, 3]))
                .union(
                    Candidates::Ids(Cow::Borrowed(&[2, 3])),
                    10,
                    &mut budget,
                    None
                )
                .unwrap(),
            Candidates::All
        ));
    }
}
