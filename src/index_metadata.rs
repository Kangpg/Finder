//! Metadata belongs to an index snapshot, shared by its searches and both panes.
//! Compact cells retain successes and failures; the first observation is serialized per record.
use crate::details::Details;
use anyhow::Result;
use std::{
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
const SIZE: u8 = 1;
const MODIFIED: u8 = 2;
const ERROR: u8 = 4;
#[derive(Debug, Clone, Copy)]
struct Fact {
    size: u64,
    modified: SystemTime,
    flags: u8,
}
#[derive(Debug, Default)]
pub(crate) struct Catalog {
    slots: Vec<OnceLock<Fact>>,
    errors: Mutex<Vec<(u32, crate::i18n::Message)>>,
}
// Each generation owns its cells. Retain known facts, then invalidate changed records during replay.
impl Clone for Catalog {
    fn clone(&self) -> Self {
        let slots = self.slots.clone();
        let errors = self
            .errors
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| {
                slots
                    .get(*id as usize)
                    .and_then(OnceLock::get)
                    .is_some_and(|fact| fact.flags & ERROR != 0)
            })
            .cloned()
            .collect();
        Self {
            slots,
            errors: Mutex::new(errors),
        }
    }
}
impl Catalog {
    pub fn ensure_len(&mut self, len: usize) -> Result<()> {
        anyhow::ensure!(
            len <= crate::index::MAX_RECORDS,
            "Metadata exceeds the record budget"
        );
        if len > self.slots.capacity() {
            let capacity = self
                .slots
                .capacity()
                .max(1024)
                .saturating_mul(2)
                .min(crate::index::MAX_RECORDS)
                .max(len);
            self.slots.try_reserve_exact(capacity - self.slots.len())?;
        }
        self.slots.resize_with(len, OnceLock::new);
        Ok(())
    }
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.take();
        }
        self.errors.get_mut().unwrap().clear();
    }
    pub fn clear_failures(&mut self) {
        for slot in &mut self.slots {
            if slot.get().is_some_and(|fact| fact.flags & ERROR != 0) {
                slot.take();
            }
        }
        self.errors.get_mut().unwrap().clear();
    }
    pub fn invalidate(&mut self, id: u32) {
        if let Some(slot) = self.slots.get_mut(id as usize) {
            slot.take();
        }
        self.errors.get_mut().unwrap().retain(|(key, _)| *key != id);
    }
    fn fact(&self, id: u32, details: Details) -> Fact {
        if let Some(error) = &details.error {
            let mut errors = self.errors.lock().unwrap();
            if errors.len() < 32 {
                errors.push((id, error.clone()));
            }
        }
        Fact {
            size: details.size.unwrap_or(0),
            modified: details.modified.unwrap_or(UNIX_EPOCH),
            flags: if details.size.is_some() { SIZE } else { 0 }
                | if details.modified.is_some() {
                    MODIFIED
                } else {
                    0
                }
                | if details.error.is_some() { ERROR } else { 0 },
        }
    }
    fn details(&self, id: u32, fact: &Fact) -> Details {
        Details {
            size: (fact.flags & SIZE != 0).then_some(fact.size),
            modified: (fact.flags & MODIFIED != 0).then_some(fact.modified),
            error: if fact.flags & ERROR != 0 {
                Some(
                    self.errors
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(key, _)| *key == id)
                        .map_or_else(
                            || crate::i18n::Message::new("색인 시 파일 정보 조회 실패 (상세 진단 32개 보관 한도)", "Could not read file metadata while indexing (up to 32 diagnostics retained)"),
                            |(_, error)| error.clone(),
                        ),
                )
            } else {
                None
            },
        }
    }
    pub fn cached(&self, id: u32) -> Option<Details> {
        self.slots
            .get(id as usize)?
            .get()
            .map(|fact| self.details(id, fact))
    }
    pub fn observe(&self, id: u32, read: impl FnOnce() -> Details) -> Details {
        let Some(slot) = self.slots.get(id as usize) else {
            return Details {
                size: None,
                modified: None,
                error: Some(crate::i18n::Message::new(
                    "색인 메타데이터 항목 없음",
                    "No index metadata entry",
                )),
            };
        };
        let fact = slot.get_or_init(|| self.fact(id, read()));
        self.details(id, fact)
    }
    pub fn seed(&mut self, id: u32, details: Details) {
        self.invalidate(id);
        if let Some(slot) = self.slots.get(id as usize) {
            let _ = slot.set(self.fact(id, details));
        }
    }
    pub fn memory_bytes(&self) -> usize {
        self.slots.capacity() * std::mem::size_of::<OnceLock<Fact>>()
            + self
                .errors
                .lock()
                .unwrap()
                .iter()
                .map(|(_, s)| std::mem::size_of::<(u32, crate::i18n::Message)>() + s.heap_bytes())
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_readers_share_one_observation_and_generations_own_invalidation() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let mut catalog = Catalog::default();
        catalog.ensure_len(1).unwrap();
        let catalog = Arc::new(catalog);
        let reads = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let cache = &catalog;
                let reads = &reads;
                scope.spawn(move || {
                    let result = cache.observe(0, || {
                        reads.fetch_add(1, Ordering::Relaxed);
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        Details {
                            size: None,
                            modified: None,
                            error: Some("access denied".into()),
                        }
                    });
                    assert_eq!(result.error.as_deref(), Some("access denied"));
                });
            }
        });
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        let mut next = (*catalog).clone();
        assert_eq!(
            next.cached(0).unwrap().error.as_deref(),
            Some("access denied")
        );
        next.clear_failures();
        assert!(next.cached(0).is_none());
        assert_eq!(
            next.observe(0, || Details {
                size: Some(7),
                modified: None,
                error: None
            })
            .size,
            Some(7)
        );
        assert!(catalog.cached(0).unwrap().size.is_none());
    }
}
