//! Independent index/search workers. Readers hold immutable snapshots; publication never waits
//! for searches. Only a short pointer exchange is protected by the snapshot mutex.
use crate::{
    details::Details,
    filters::Filter,
    folder,
    index::{Index, SearchOrder, Source},
    query::{Query, QueryOptions},
    storage,
};
use anyhow::{Context, Result, ensure};
use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Instant,
};

pub enum IndexCommand {
    Folder(PathBuf),
    Drive(char),
    Load(PathBuf),
    Refresh,
    Save(PathBuf),
}
pub struct Row {
    pub name: String,
    pub path: Option<PathBuf>,
    pub directory: bool,
    pub details: Option<Details>,
}
pub struct View {
    pub rows: Vec<Row>,
    pub total: usize,
    pub entries: usize,
    pub memory: usize,
    pub elapsed_ms: f64,
    pub source: String,
    pub coverage: usize,
}
pub struct SearchEvent {
    pub id: u64,
    pub revision: u64,
    pub result: Result<View>,
}
pub struct IndexEvent {
    pub id: u64,
    pub result: Result<IndexOutcome>,
    pub elapsed_ms: f64,
}
pub enum IndexOutcome {
    Published { revision: u64, source: Source },
    Unchanged,
    Saved(PathBuf),
}
#[derive(Clone)]
struct Snapshot {
    revision: u64,
    index: Arc<Index>,
}
enum Update {
    Publish(Arc<Index>),
    Unchanged,
    Saved(PathBuf),
}
#[derive(Default)]
pub struct SearchRequest {
    pub query: String,
    pub options: QueryOptions,
    pub filter: Filter,
    pub order: SearchOrder,
    pub limit: usize,
}

/// One pending request, one running request, and one retained completion per worker.
struct Queue<T> {
    next: Mutex<Option<(u64, T)>>,
    wake: Condvar,
    epoch: AtomicU64,
    quit: AtomicBool,
}
impl<T> Queue<T> {
    fn new() -> Self {
        Self {
            next: Mutex::new(None),
            wake: Condvar::new(),
            epoch: AtomicU64::new(0),
            quit: AtomicBool::new(false),
        }
    }
    fn submit(&self, job: T) -> u64 {
        let mut next = self.next.lock().unwrap();
        let id = self.epoch.fetch_add(1, Ordering::Relaxed) + 1;
        *next = Some((id, job));
        self.wake.notify_one();
        id
    }
    fn take(&self) -> Option<(u64, T)> {
        let mut next = self.next.lock().unwrap();
        while next.is_none() && !self.quit.load(Ordering::Acquire) {
            next = self.wake.wait(next).unwrap();
        }
        if self.quit.load(Ordering::Acquire) {
            None
        } else {
            next.take()
        }
    }
    fn stop(&self) {
        let mut next = self.next.lock().unwrap();
        self.quit.store(true, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::Relaxed);
        *next = None;
        self.wake.notify_one();
    }
}
type SharedSnapshot = Arc<Mutex<Option<Snapshot>>>;
type Wake = Arc<dyn Fn() + Send + Sync>;
type Builder =
    Arc<dyn Fn(&IndexCommand, Option<&Snapshot>, &AtomicU64, u64) -> Result<Update> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    Left,
    Right,
}
impl Pane {
    pub const ALL: [Self; 2] = [Self::Left, Self::Right];
    pub fn slot(self) -> usize {
        match self {
            Self::Left => 0,
            Self::Right => 1,
        }
    }
}
struct SearchLane {
    queue: Arc<Queue<SearchRequest>>,
    event: Arc<Mutex<Option<SearchEvent>>>,
}
#[derive(Clone)]
struct Pools {
    index: Arc<rayon::ThreadPool>,
    search: Arc<rayon::ThreadPool>,
}
pub struct Runtime {
    pools: Pools,
    wake: Wake,
    snapshot: SharedSnapshot,
    index_queue: Arc<Queue<IndexCommand>>,
    lanes: [SearchLane; 2],
    index_event: Arc<Mutex<Option<IndexEvent>>>,
    joins: Vec<thread::JoinHandle<()>>,
}
impl Runtime {
    pub fn new(wake: Wake, workers: usize) -> Result<Self> {
        Self::with_builder(wake, workers, Arc::new(build))
    }
    fn with_builder(wake: Wake, workers: usize, builder: Builder) -> Result<Self> {
        ensure!((1..=32).contains(&workers), "Worker count must be 1..32");
        let index_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(workers.div_ceil(2).min(4))
                .thread_name(|n| format!("index-cpu-{n}"))
                .build()?,
        );
        let search_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(workers)
                .thread_name(|n| format!("search-cpu-{n}"))
                .build()?,
        );
        Self::spawn(
            wake,
            Pools {
                index: index_pool,
                search: search_pool,
            },
            builder,
        )
    }
    /// Separate snapshots and queues, with the same bounded CPU budgets.
    pub fn independent(&self) -> Result<Self> {
        Self::spawn(self.wake.clone(), self.pools.clone(), Arc::new(build))
    }
    fn spawn(wake: Wake, pools: Pools, builder: Builder) -> Result<Self> {
        let index_pool = pools.index.clone();
        let search_pool = pools.search.clone();
        let snapshot: SharedSnapshot = Arc::new(Mutex::new(None));
        let index_queue = Arc::new(Queue::new());
        let lanes: [SearchLane; 2] = std::array::from_fn(|_| SearchLane {
            queue: Arc::new(Queue::new()),
            event: Arc::new(Mutex::new(None)),
        });
        let index_event = Arc::new(Mutex::new(None));
        let (queue, slot, completion, notify) = (
            index_queue.clone(),
            snapshot.clone(),
            index_event.clone(),
            wake.clone(),
        );
        let index_join = thread::Builder::new()
            .name("index-coordinator".into())
            .spawn(move || {
                let mut revision = 0;
                while let Some((id, command)) = queue.take() {
                    let start = Instant::now();
                    let previous = slot.lock().unwrap().clone();
                    let result = index_pool
                        .install(|| builder(&command, previous.as_ref(), &queue.epoch, id));
                    // Serialize request acceptance and publication, so a superseded build cannot
                    // pass its epoch check and then publish after a newer request was accepted.
                    let commit = queue.next.lock().unwrap();
                    if queue.epoch.load(Ordering::Relaxed) != id {
                        continue;
                    }
                    let mut retired = None;
                    let result = result.map(|update| match update {
                        Update::Publish(index) => {
                            revision += 1;
                            let source = index.source.clone();
                            let next = Snapshot { revision, index };
                            retired = slot.lock().unwrap().replace(next);
                            IndexOutcome::Published { revision, source }
                        }
                        Update::Unchanged => IndexOutcome::Unchanged,
                        Update::Saved(path) => IndexOutcome::Saved(path),
                    });
                    *completion.lock().unwrap() = Some(IndexEvent {
                        id,
                        result,
                        elapsed_ms: start.elapsed().as_secs_f64() * 1000.,
                    });
                    drop(commit);
                    // Large index destruction must not hold either the request or snapshot lock.
                    drop(retired);
                    notify();
                }
            })?;
        let mut joins = vec![index_join];
        for pane in Pane::ALL {
            let lane = &lanes[pane.slot()];
            let (queue, slot, completion, notify, pool) = (
                lane.queue.clone(),
                snapshot.clone(),
                lane.event.clone(),
                wake.clone(),
                search_pool.clone(),
            );
            match thread::Builder::new()
                .name(format!("search-{pane:?}"))
                .spawn(move || {
                    while let Some((id, command)) = queue.take() {
                        let snapshot = slot.lock().unwrap().clone();
                        let revision = snapshot.as_ref().map_or(0, |s| s.revision);
                        let result = pool.install(|| {
                            let snapshot = snapshot.as_ref().context(crate::i18n::Message::new(
                                "먼저 색인을 만드세요",
                                "Create an index first",
                            ))?;
                            view(&snapshot.index, &command, &queue.epoch, id)
                        });
                        if queue.epoch.load(Ordering::Relaxed) != id {
                            continue;
                        }
                        let retired = completion.lock().unwrap().replace(SearchEvent {
                            id,
                            revision,
                            result,
                        });
                        drop(retired);
                        notify();
                    }
                }) {
                Ok(join) => joins.push(join),
                Err(e) => {
                    index_queue.stop();
                    for lane in &lanes {
                        lane.queue.stop();
                    }
                    for join in joins {
                        let _ = join.join();
                    }
                    return Err(e.into());
                }
            }
        }
        Ok(Self {
            pools,
            wake,
            snapshot,
            index_queue,
            lanes,
            index_event,
            joins,
        })
    }
    pub fn index(&self, command: IndexCommand) -> u64 {
        self.index_queue.submit(command)
    }
    pub fn search(&self, query: String, case_sensitive: bool, limit: usize) -> u64 {
        self.search_with(SearchRequest {
            query,
            options: QueryOptions {
                case_sensitive,
                ..Default::default()
            },
            limit,
            ..Default::default()
        })
    }
    pub fn search_with(&self, request: SearchRequest) -> u64 {
        self.search_in(Pane::Left, request)
    }
    pub fn search_in(&self, pane: Pane, request: SearchRequest) -> u64 {
        self.lanes[pane.slot()].queue.submit(request)
    }
    pub fn revision(&self) -> u64 {
        self.snapshot
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |s| s.revision)
    }
    pub fn take_index_event(&self) -> Option<IndexEvent> {
        self.index_event.lock().unwrap().take()
    }
    pub fn take_search_event(&self) -> Option<SearchEvent> {
        self.take_search_for(Pane::Left)
    }
}
impl Runtime {
    pub fn take_search_for(&self, pane: Pane) -> Option<SearchEvent> {
        self.lanes[pane.slot()].event.lock().unwrap().take()
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.index_queue.stop();
        for lane in &self.lanes {
            lane.queue.stop();
        }
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
    }
}
fn build(
    command: &IndexCommand,
    previous: Option<&Snapshot>,
    epoch: &AtomicU64,
    id: u64,
) -> Result<Update> {
    let cancel = Some((epoch, id));
    match command {
        IndexCommand::Folder(root) => Ok(Update::Publish(Arc::new(folder::scan(root, cancel)?))),
        IndexCommand::Drive(drive) => {
            #[cfg(windows)]
            {
                Ok(Update::Publish(Arc::new(crate::ntfs::scan(
                    *drive, cancel,
                )?)))
            }
            #[cfg(not(windows))]
            {
                let _ = drive;
                anyhow::bail!("NTFS indexing requires Windows")
            }
        }
        IndexCommand::Load(path) => {
            let mut index = storage::load(path, cancel)?;
            if matches!(index.source, Source::Ntfs { .. }) {
                #[cfg(windows)]
                {
                    crate::ntfs::refresh(&mut index, cancel)?;
                }
            }
            Ok(Update::Publish(Arc::new(index)))
        }
        IndexCommand::Refresh => {
            let current = &previous
                .context(crate::i18n::Message::new(
                    "먼저 색인을 만드세요",
                    "Create an index first",
                ))?
                .index;
            match &current.source {
                Source::Folder { root } => {
                    Ok(Update::Publish(Arc::new(folder::scan(root, cancel)?)))
                }
                Source::Ntfs { .. } => {
                    #[cfg(windows)]
                    {
                        // Avoid copying the index when the journal has no changes.
                        if !crate::ntfs::has_changes(current)? {
                            return Ok(Update::Unchanged);
                        }
                        let mut next = (**current).clone();
                        crate::ntfs::refresh(&mut next, cancel)?;
                        Ok(Update::Publish(Arc::new(next)))
                    }
                    #[cfg(not(windows))]
                    {
                        anyhow::bail!("NTFS refresh requires Windows")
                    }
                }
            }
        }
        IndexCommand::Save(path) => {
            storage::save(
                &previous
                    .context(crate::i18n::Message::new(
                        "먼저 색인을 만드세요",
                        "Create an index first",
                    ))?
                    .index,
                path,
            )?;
            Ok(Update::Saved(path.clone()))
        }
    }
}
fn view(index: &Index, command: &SearchRequest, epoch: &AtomicU64, id: u64) -> Result<View> {
    let start = Instant::now();
    let query = Query::parse_with(&command.query, command.options, command.filter)?;
    let r = index.search_ordered(&query, command.limit, Some((epoch, id)), command.order)?;
    let mut rows = Vec::with_capacity(r.ids.len());
    for &record in &r.ids {
        ensure!(epoch.load(Ordering::Relaxed) == id, "Search cancelled");
        rows.push(Row {
            name: index.name(record),
            path: index.path(record).ok(),
            directory: index.records[record as usize].is_dir(),
            details: index.cached_details(record),
        });
    }
    let source = match &index.source {
        Source::Folder { root } => {
            crate::trf!("폴더 스냅샷 · {}", "Folder snapshot · {}", root.display())
        }
        Source::Ntfs { drive, .. } => format!("NTFS / USN · {drive}:\\"),
    };
    Ok(View {
        rows,
        total: r.total,
        entries: index.len(),
        memory: index.memory_bytes(),
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.,
        source,
        coverage: index.coverage_errors,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn cold_name_search_publishes_rows_without_reading_display_metadata() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        for n in 0..40 {
            std::fs::write(dir.path().join(format!("entry_{n:03}.txt")), "data").unwrap();
        }
        let index = folder::scan(dir.path(), None).unwrap();
        let cache = dir.path().join("index.reaidx");
        storage::save(&index, &cache).unwrap();
        let index = storage::load(&cache, None).unwrap();
        let epoch = AtomicU64::new(1);
        let result = view(
            &index,
            &SearchRequest {
                query: "entry_".into(),
                limit: 5000,
                ..Default::default()
            },
            &epoch,
            1,
        )
        .unwrap();
        assert_eq!(result.rows.len(), 40);
        assert!(result.rows.iter().all(|r| r.details.is_none()));
        assert!(
            index
                .records
                .iter()
                .enumerate()
                .all(|(id, _)| index.cached_details(id as u32).is_none())
        );
        // Date/size ordering still observes the facts needed to order all matches.
        let result = view(
            &index,
            &SearchRequest {
                query: "entry_".into(),
                limit: 5000,
                order: SearchOrder {
                    column: crate::index::SortColumn::Modified,
                    ..Default::default()
                },
                ..Default::default()
            },
            &epoch,
            1,
        )
        .unwrap();
        assert!(
            result
                .rows
                .iter()
                .all(|r| r.details.as_ref().is_some_and(|d| d.modified.is_some()))
        );
    }
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    fn index_done(r: &Runtime, id: u64) -> IndexEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(e) = r.take_index_event()
                && e.id == id
            {
                return e;
            }
            assert!(Instant::now() < deadline, "index completion timeout");
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn search_done(r: &Runtime, id: u64) -> SearchEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(e) = r.take_search_event()
                && e.id == id
            {
                return e;
            }
            assert!(
                Instant::now() < deadline,
                "search waited for the blocked index worker"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn production_search_orders_all_matches_before_limiting_each_column() {
        use crate::index::SortColumn;
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("alpha");
        let z = dir.path().join("omega");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&z).unwrap();
        for (path, size, seconds) in [
            (a.join("zeta.log"), 8, 300),
            (z.join("alpha.txt"), 100, 100),
            (z.join("middle.bin"), 2, 200),
        ] {
            std::fs::write(&path, vec![0; size]).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds))
                .unwrap();
        }
        let runtime = Runtime::new(Arc::new(|| {}), 2).unwrap();
        let id = runtime.index(IndexCommand::Folder(dir.path().into()));
        index_done(&runtime, id).result.unwrap();
        for (column, ascending, descending) in [
            (SortColumn::Name, "alpha.txt", "zeta.log"),
            (SortColumn::Path, "zeta.log", "alpha.txt"),
            (SortColumn::Size, "middle.bin", "alpha.txt"),
            (SortColumn::Type, "middle.bin", "alpha.txt"),
            (SortColumn::Modified, "alpha.txt", "zeta.log"),
        ] {
            for (reverse, expected) in [(false, ascending), (true, descending)] {
                let id = runtime.search_with(SearchRequest {
                    order: SearchOrder {
                        column,
                        descending: reverse,
                        folders_first: false,
                    },
                    limit: 1,
                    filter: Filter::Files,
                    ..Default::default()
                });
                let view = search_done(&runtime, id).result.unwrap();
                assert_eq!(view.total, 3);
                assert_eq!(view.rows.len(), 1);
                assert_eq!(view.rows[0].name, expected, "{column:?} reverse={reverse}");
            }
        }
        let id = runtime.search_with(SearchRequest {
            order: SearchOrder {
                column: SortColumn::Size,
                descending: true,
                folders_first: false,
            },
            limit: 10,
            ..Default::default()
        });
        let view = search_done(&runtime, id).result.unwrap();
        assert!(view.rows[..3].iter().all(|r| !r.directory));
        assert!(view.rows[3..].iter().all(|r| r.directory));
        let id = runtime.search_with(SearchRequest {
            order: SearchOrder {
                column: SortColumn::Size,
                descending: true,
                folders_first: true,
            },
            limit: 1,
            ..Default::default()
        });
        assert!(search_done(&runtime, id).result.unwrap().rows[0].directory);
    }

    #[test]
    fn searches_finish_during_refresh_and_only_complete_snapshots_publish() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), "old").unwrap();
        let gate = Arc::new(AtomicBool::new(false));
        let gate_worker = gate.clone();
        let (started, signal) = mpsc::channel();
        let r = Runtime::with_builder(
            Arc::new(|| {}),
            2,
            Arc::new(move |command, previous, epoch, id| {
                if matches!(command, IndexCommand::Refresh) {
                    started.send(()).unwrap();
                    while !gate_worker.load(Ordering::Acquire) {
                        ensure!(
                            epoch.load(Ordering::Relaxed) == id,
                            "Cancelled blocked refresh"
                        );
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                build(command, previous, epoch, id)
            }),
        )
        .unwrap();
        let id = r.index(IndexCommand::Folder(dir.path().into()));
        index_done(&r, id).result.unwrap();
        let old = r.snapshot.lock().unwrap().clone().unwrap();
        std::fs::remove_file(dir.path().join("old.txt")).unwrap();
        std::fs::write(dir.path().join("new.txt"), "new").unwrap();
        let update = r.index(IndexCommand::Refresh);
        signal.recv_timeout(Duration::from_secs(5)).unwrap();
        let id = r.search("old".into(), false, 100);
        let found = search_done(&r, id);
        assert_eq!(found.revision, old.revision);
        assert_eq!(found.result.unwrap().total, 1);
        let id = r.search("new".into(), false, 100);
        assert_eq!(search_done(&r, id).result.unwrap().total, 0);
        let id = r.search("regex:[".into(), false, 100);
        assert!(search_done(&r, id).result.is_err());
        let id = r.search("old".into(), false, 100);
        assert_eq!(search_done(&r, id).result.unwrap().total, 1);
        assert!(r.take_index_event().is_none());
        gate.store(true, Ordering::Release);
        index_done(&r, update).result.unwrap();
        let id = r.search("new".into(), false, 100);
        let found = search_done(&r, id);
        assert!(found.revision > old.revision);
        assert_eq!(found.result.unwrap().total, 1);
        assert_eq!(
            old.index
                .search(&Query::parse("old", false).unwrap(), 100, None)
                .unwrap()
                .total,
            1
        );
        let id = r.index(IndexCommand::Folder(dir.path().join("missing")));
        assert!(index_done(&r, id).result.is_err());
        let id = r.search("new".into(), false, 100);
        assert_eq!(search_done(&r, id).result.unwrap().total, 1);
        gate.store(false, Ordering::Release);
        r.index(IndexCommand::Refresh);
        signal.recv_timeout(Duration::from_secs(5)).unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("other.txt"), "other").unwrap();
        let before = r.revision();
        let id = r.index(IndexCommand::Folder(other.path().into()));
        index_done(&r, id).result.unwrap();
        assert_eq!(
            r.revision(),
            before + 1,
            "Superseded refresh must not publish"
        );
        let id = r.search("other".into(), false, 100);
        assert_eq!(search_done(&r, id).result.unwrap().total, 1);
        r.index(IndexCommand::Refresh);
        signal.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(r);
    }
    #[test]
    fn metadata_is_reused_by_sorting_both_panes_and_display_until_refresh() {
        use crate::index::SortColumn;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actual.txt");
        std::fs::write(&path, b"one").unwrap();
        let r = Runtime::new(Arc::new(|| {}), 2).unwrap();
        let id = r.index(IndexCommand::Folder(dir.path().into()));
        index_done(&r, id).result.unwrap();
        let request = |column, descending| SearchRequest {
            query: "actual".into(),
            limit: 10,
            order: SearchOrder {
                column,
                descending,
                folders_first: false,
            },
            ..Default::default()
        };
        std::fs::write(&path, b"changed without refresh").unwrap();
        for (column, descending) in [
            (SortColumn::Size, false),
            (SortColumn::Size, true),
            (SortColumn::Modified, false),
            (SortColumn::Name, false),
        ] {
            let id = r.search_with(request(column, descending));
            let view = search_done(&r, id).result.unwrap();
            assert_eq!(
                view.rows[0].details.as_ref().unwrap().size,
                Some(3),
                "reuse the folder enumeration observation, not another disk read"
            );
        }
        let id = r.search_in(Pane::Right, request(SortColumn::Size, true));
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(event) = r.take_search_for(Pane::Right)
                && event.id == id
            {
                assert_eq!(
                    event.result.unwrap().rows[0].details.as_ref().unwrap().size,
                    Some(3)
                );
                break;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        let id = r.index(IndexCommand::Refresh);
        index_done(&r, id).result.unwrap();
        let id = r.search_with(request(SortColumn::Size, false));
        assert_eq!(
            search_done(&r, id).result.unwrap().rows[0]
                .details
                .as_ref()
                .unwrap()
                .size,
            Some(b"changed without refresh".len() as u64)
        );
        std::fs::remove_file(&path).unwrap();
        let id = r.search_with(request(SortColumn::Size, true));
        assert_eq!(search_done(&r, id).result.unwrap().total, 1);
        let id = r.index(IndexCommand::Refresh);
        index_done(&r, id).result.unwrap();
        let id = r.search_with(request(SortColumn::Size, true));
        assert_eq!(search_done(&r, id).result.unwrap().total, 0);
    }
    #[test]
    fn legacy_snapshot_metadata_loads_once_and_refresh_clears_failed_observations() {
        use crate::index::SortColumn;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actual.txt");
        let cache = dir.path().join("index.reaidx");
        std::fs::write(&path, b"one").unwrap();
        let index = folder::scan(dir.path(), None).unwrap();
        storage::save(&index, &cache).unwrap();
        let r = Runtime::new(Arc::new(|| {}), 2).unwrap();
        let id = r.index(IndexCommand::Load(cache));
        index_done(&r, id).result.unwrap();
        let request = || SearchRequest {
            query: "actual".into(),
            limit: 10,
            order: SearchOrder {
                column: SortColumn::Size,
                ..Default::default()
            },
            ..Default::default()
        };
        let id = r.search_with(request());
        assert_eq!(
            search_done(&r, id).result.unwrap().rows[0]
                .details
                .as_ref()
                .unwrap()
                .size,
            Some(3)
        );
        std::fs::remove_file(&path).unwrap();
        let id = r.search_with(request());
        assert_eq!(
            search_done(&r, id).result.unwrap().rows[0]
                .details
                .as_ref()
                .unwrap()
                .size,
            Some(3)
        );
        // A separately loaded snapshot observes an error and retains that unknown until refresh.
        let r2 = Runtime::new(Arc::new(|| {}), 2).unwrap();
        let id = r2.index(IndexCommand::Load(dir.path().join("index.reaidx")));
        index_done(&r2, id).result.unwrap();
        let id = r2.search_with(request());
        let view = search_done(&r2, id).result.unwrap();
        assert!(view.rows[0].details.as_ref().unwrap().error.is_some());
        std::fs::write(&path, b"restored").unwrap();
        let id = r2.search_with(request());
        assert!(
            search_done(&r2, id).result.unwrap().rows[0]
                .details
                .as_ref()
                .unwrap()
                .error
                .is_some()
        );
        let id = r2.index(IndexCommand::Refresh);
        index_done(&r2, id).result.unwrap();
        let id = r2.search_with(request());
        assert_eq!(
            search_done(&r2, id).result.unwrap().rows[0]
                .details
                .as_ref()
                .unwrap()
                .size,
            Some(8)
        );
    }
    #[test]
    fn pane_searches_have_independent_cancellation_and_share_snapshot_revisions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("left.txt"), "left").unwrap();
        std::fs::write(dir.path().join("right.txt"), "right").unwrap();
        let r = Runtime::new(Arc::new(|| {}), 2).unwrap();
        let id = r.index(IndexCommand::Folder(dir.path().into()));
        index_done(&r, id).result.unwrap();
        let request = |q: &str| SearchRequest {
            query: q.into(),
            limit: 100,
            ..Default::default()
        };
        let done = |pane, id| {
            let start = Instant::now();
            loop {
                if let Some(e) = r.take_search_for(pane)
                    && e.id == id
                {
                    break e;
                }
                assert!(start.elapsed() < Duration::from_secs(5));
                thread::sleep(Duration::from_millis(1));
            }
        };
        let left = r.search_in(Pane::Left, request("left"));
        for _ in 0..20 {
            r.search_in(Pane::Right, request("superseded"));
        }
        let right = r.search_in(Pane::Right, request("right"));
        let l = done(Pane::Left, left);
        let rr = done(Pane::Right, right);
        assert_eq!(l.revision, rr.revision);
        assert_eq!(l.result.unwrap().rows[0].name, "left.txt");
        assert_eq!(rr.result.unwrap().rows[0].name, "right.txt");
        let invalid = r.search_in(Pane::Right, request("regex:["));
        let left = r.search_in(Pane::Left, request("left"));
        assert!(done(Pane::Right, invalid).result.is_err());
        assert_eq!(done(Pane::Left, left).result.unwrap().total, 1);
        std::fs::remove_file(dir.path().join("left.txt")).unwrap();
        std::fs::write(dir.path().join("left-new.txt"), "new").unwrap();
        let old_revision = r.revision();
        let id = r.index(IndexCommand::Refresh);
        index_done(&r, id).result.unwrap();
        for (pane, query) in [(Pane::Left, "left-new"), (Pane::Right, "right")] {
            let id = r.search_in(pane, request(query));
            let event = done(pane, id);
            assert_eq!(event.revision, old_revision + 1);
            assert_eq!(event.result.unwrap().total, 1);
        }
    }
}
