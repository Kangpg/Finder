//! Visible-row enrichment, independent of name searches and index construction.
use crate::{background::Pane, details::Details};
use anyhow::{Result, ensure};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
pub const MAX_VISIBLE: usize = 128;
pub(crate) const MAX_CACHE: usize = 512;
pub const FRESH_FOR: Duration = Duration::from_secs(10);
#[derive(Debug, Clone)]
pub struct Size {
    pub bytes: u64,
    pub complete: bool,
    pub note: Option<crate::i18n::Message>,
}
impl Size {
    pub fn text(&self) -> String {
        let text = Details {
            size: Some(self.bytes),
            modified: None,
            error: None,
        }
        .size_text();
        if self.complete {
            text
        } else {
            format!("≥ {text}")
        }
    }
}
#[derive(Debug, Clone)]
pub struct Decoration {
    pub details: Details,
    pub size: Option<Size>,
    pub size_note: Option<crate::i18n::Message>,
    #[cfg(windows)]
    pub icon: Option<Arc<crate::shell_icon::Icon>>,
    pub icon_note: Option<crate::i18n::Message>,
    pub measured: Instant,
}
impl Decoration {
    fn basic(path: &Path, _native_ready: bool) -> Self {
        let details = Details::read(path);
        #[cfg(windows)]
        let icon = if _native_ready {
            crate::shell_icon::read(path)
        } else {
            Err(anyhow::anyhow!(crate::i18n::Message::new(
                "Windows 아이콘 작업자 초기화 실패",
                "Could not initialize Windows icon worker"
            )))
        };
        Self {
            size: details.size.map(|bytes| Size {
                bytes,
                complete: true,
                note: None,
            }),
            size_note: details.error.clone(),
            details,
            #[cfg(windows)]
            icon: icon.as_ref().ok().cloned(),
            #[cfg(windows)]
            icon_note: icon
                .err()
                .map(|e| crate::i18n::Message::capture(|| format!("{e:#}"))),
            #[cfg(not(windows))]
            icon_note: Some(crate::i18n::Message::new(
                "Windows 파일 아이콘은 Windows에서 사용할 수 있습니다",
                "Windows file icons are available on Windows",
            )),
            measured: Instant::now(),
        }
    }
}
fn reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
fn folder_size(
    path: &Path,
    epoch: &AtomicU64,
    id: u64,
    max_entries: usize,
    max_time: Duration,
) -> Option<(Option<Size>, Option<crate::i18n::Message>)> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => return Some((None, Some(crate::i18n::Message::error(e)))),
    };
    if !metadata.is_dir() {
        return Some((
            None,
            Some(crate::i18n::Message::new(
                "더 이상 폴더가 아닙니다",
                "The item is no longer a folder",
            )),
        ));
    }
    if reparse(&metadata) {
        return Some((
            None,
            Some(crate::i18n::Message::new(
                "링크·junction 폴더는 따라가지 않습니다",
                "Links and junctions are not followed",
            )),
        ));
    }
    let start = Instant::now();
    let mut total = 0u64;
    let mut complete = true;
    let mut note = None;
    let mut walker = walkdir::WalkDir::new(path)
        .follow_links(false)
        .max_open(16)
        .into_iter();
    walker.next();
    let mut examined = 0;
    while let Some(entry) = walker.next() {
        if epoch.load(Ordering::Relaxed) != id {
            return None;
        }
        if examined >= max_entries || start.elapsed() >= max_time {
            complete = false;
            note = Some(crate::i18n::Message::new(
                "계산 한도 도달: 일부 파일 합계입니다 (200,000항목/폴더당 1.5초)",
                "Calculation limit reached: partial total (200,000 items / 1.5 s per folder)",
            ));
            break;
        }
        examined += 1;
        match entry {
            Err(e) => {
                complete = false;
                note = Some(crate::msgf!(
                    "일부 경로 읽기 실패: {e}",
                    "Could not read some paths: {e}"
                ));
            }
            Ok(entry) => match std::fs::symlink_metadata(entry.path()) {
                Err(e) => {
                    complete = false;
                    note = Some(crate::msgf!(
                        "일부 크기 읽기 실패: {e}",
                        "Could not read some sizes: {e}"
                    ));
                    if entry.file_type().is_dir() {
                        walker.skip_current_dir();
                    }
                }
                Ok(m) if reparse(&m) => {
                    if m.is_dir() {
                        walker.skip_current_dir();
                    }
                }
                Ok(m) if m.is_file() => match total.checked_add(m.len()) {
                    Some(n) => total = n,
                    None => {
                        complete = false;
                        note = Some(crate::i18n::Message::new(
                            "용량 합계 범위 초과",
                            "Total size is out of range",
                        ));
                        break;
                    }
                },
                _ => {}
            },
        }
    }
    Some((
        Some(Size {
            bytes: total,
            complete,
            note,
        }),
        None,
    ))
}
struct Request {
    id: u64,
    revision: u64,
    paths: Vec<PathBuf>,
}
pub struct Batch {
    pub id: u64,
    pub values: HashMap<PathBuf, Decoration>,
    pub done: bool,
}
struct Lane {
    epoch: AtomicU64,
    quit: AtomicBool,
    next: Mutex<Option<Request>>,
    wake: Condvar,
    event: Mutex<Option<Batch>>,
}
impl Lane {
    fn stop(&self) {
        // Synchronize the predicate with Condvar::wait to prevent a lost shutdown wake.
        let _next = self.next.lock().unwrap();
        self.quit.store(true, Ordering::Relaxed);
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.wake.notify_one();
    }
    fn finish(&self, id: u64, wake: &dyn Fn()) {
        if self.epoch.load(Ordering::Relaxed) != id {
            return;
        }
        let mut event = self.event.lock().unwrap();
        if event.as_ref().is_none_or(|b| b.id != id) {
            *event = Some(Batch {
                id,
                values: HashMap::new(),
                done: false,
            });
        }
        event.as_mut().unwrap().done = true;
        drop(event);
        wake();
    }
    fn publish(&self, id: u64, path: PathBuf, value: Decoration, wake: &dyn Fn()) {
        if self.epoch.load(Ordering::Relaxed) != id {
            return;
        }
        let mut slot = self.event.lock().unwrap();
        if slot.as_ref().is_none_or(|b| b.id != id) {
            *slot = Some(Batch {
                id,
                values: HashMap::new(),
                done: false,
            });
        }
        slot.as_mut().unwrap().values.insert(path, value);
        drop(slot);
        wake();
    }
}
pub struct Runtime {
    lanes: [Arc<Lane>; 2],
    joins: Vec<thread::JoinHandle<()>>,
}
impl Runtime {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let lanes = std::array::from_fn(|_| {
            Arc::new(Lane {
                epoch: AtomicU64::new(0),
                quit: AtomicBool::new(false),
                next: Mutex::new(None),
                wake: Condvar::new(),
                event: Mutex::new(None),
            })
        });
        let mut joins = Vec::new();
        for pane in Pane::ALL {
            let lane = lanes[pane.slot()].clone();
            let wake = wake.clone();
            match thread::Builder::new()
                .name(format!("visible-metadata-{pane:?}"))
                .spawn(move || {
                    #[cfg(windows)]
                    let _ole = crate::native_drag::Apartment::initialize();
                    #[cfg(windows)]
                    let native_ready = _ole.is_ok();
                    #[cfg(not(windows))]
                    let native_ready = false;
                    let mut cache: HashMap<PathBuf, (u64, Decoration)> = HashMap::new();
                    loop {
                        let request = {
                            let mut next = lane.next.lock().unwrap();
                            while next.is_none() && !lane.quit.load(Ordering::Relaxed) {
                                next = lane.wake.wait(next).unwrap();
                            }
                            if lane.quit.load(Ordering::Relaxed) {
                                break;
                            }
                            next.take().unwrap()
                        };
                        let Request {
                            id,
                            revision,
                            paths,
                        } = request;
                        let mut folders = Vec::new();
                        for path in paths {
                            if lane.epoch.load(Ordering::Relaxed) != id {
                                break;
                            }
                            if let Some((r, value)) = cache.get(&path)
                                && *r == revision
                                && value.measured.elapsed() < FRESH_FOR
                            {
                                lane.publish(id, path, value.clone(), wake.as_ref());
                                continue;
                            }
                            let value = Decoration::basic(&path, native_ready);
                            let directory =
                                std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
                            lane.publish(id, path.clone(), value.clone(), wake.as_ref());
                            if directory {
                                folders.push((path, value));
                            } else {
                                if cache.len() >= MAX_CACHE {
                                    cache.retain(|_, (_, d)| d.measured.elapsed() < FRESH_FOR);
                                    if cache.len() >= MAX_CACHE {
                                        cache.clear();
                                    }
                                }
                                cache.insert(path, (revision, value));
                            }
                        }
                        for (path, mut value) in folders {
                            let Some((size, note)) = folder_size(
                                &path,
                                &lane.epoch,
                                id,
                                200_000,
                                Duration::from_millis(1500),
                            ) else {
                                break;
                            };
                            value.size = size;
                            value.size_note = note;
                            value.measured = Instant::now();
                            lane.publish(id, path.clone(), value.clone(), wake.as_ref());
                            if cache.len() >= MAX_CACHE {
                                cache.clear();
                            }
                            cache.insert(path, (revision, value));
                        }
                        lane.finish(id, wake.as_ref());
                    }
                }) {
                Ok(join) => joins.push(join),
                Err(e) => {
                    for lane in &lanes {
                        lane.stop();
                    }
                    for join in joins {
                        let _ = join.join();
                    }
                    return Err(e.into());
                }
            }
        }
        Ok(Self { lanes, joins })
    }
    pub fn request(&self, pane: Pane, revision: u64, paths: Vec<PathBuf>) -> Result<u64> {
        ensure!(
            paths.len() <= MAX_VISIBLE,
            crate::i18n::Message::new(
                "가시 항목 조회 한도 초과",
                "Visible item lookup limit exceeded"
            )
        );
        let lane = &self.lanes[pane.slot()];
        let mut next = lane.next.lock().unwrap();
        let id = lane.epoch.fetch_add(1, Ordering::Relaxed) + 1;
        *next = Some(Request {
            id,
            revision,
            paths,
        });
        lane.wake.notify_one();
        Ok(id)
    }
    pub fn take(&self, pane: Pane) -> Option<Batch> {
        self.lanes[pane.slot()].event.lock().unwrap().take()
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        for lane in &self.lanes {
            lane.stop();
        }
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_folder_sizes_empty_partial_limits_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("a"), [0; 5]).unwrap();
        std::fs::write(dir.path().join("nested/b"), [0; 8]).unwrap();
        let epoch = AtomicU64::new(1);
        let (size, _) = folder_size(dir.path(), &epoch, 1, 100, Duration::from_secs(5)).unwrap();
        let size = size.unwrap();
        assert_eq!(size.bytes, 13);
        assert!(size.complete);
        let (size, _) = folder_size(dir.path(), &epoch, 1, 1, Duration::from_secs(5)).unwrap();
        assert!(!size.unwrap().complete);
        epoch.store(2, Ordering::Relaxed);
        assert!(folder_size(dir.path(), &epoch, 1, 100, Duration::from_secs(5)).is_none());
        let empty = tempfile::tempdir().unwrap();
        let (s, _) = folder_size(empty.path(), &epoch, 2, 100, Duration::from_secs(5)).unwrap();
        assert_eq!(s.unwrap().bytes, 0);
    }
}
