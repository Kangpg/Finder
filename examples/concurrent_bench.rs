//! Measures completed refresh + search tasks without automating a desktop window.
use anyhow::{Context, Result, ensure};
use finder::{
    background::{IndexCommand, Runtime},
    folder,
    index::Source,
    query::Query,
    storage,
};
use std::{
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let cache = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("Usage: concurrent_bench SNAPSHOT.reaidx")?,
    );
    let saved = storage::load(&cache, None)?;
    let Source::Folder { root } = &saved.source else {
        anyhow::bail!("Benchmark requires a folder snapshot")
    };
    let root = root.clone();
    let query = "ext:rs;toml";
    // Reproduce the former single-worker ordering: refresh runs before the queued search.
    // This is a serial scheduling reference, not a run of the old GUI binary.
    let reference_pool = rayon::ThreadPoolBuilder::new().num_threads(4).build()?;
    let (started_tx, started_rx) = mpsc::channel();
    let serial_root = root.clone();
    let serial = thread::spawn(move || {
        reference_pool.install(|| -> Result<(f64, usize)> {
            let start = Instant::now();
            started_tx.send(()).unwrap();
            let index = folder::scan(&serial_root, None)?;
            let found = index.search(&Query::parse(query, false)?, 100, None)?;
            for id in found.ids {
                let _ = index.name(id);
                let _ = index.path(id);
            }
            Ok((start.elapsed().as_secs_f64() * 1000., found.total))
        })
    });
    started_rx.recv()?;
    let serial_result = serial.join().unwrap()?;
    let runtime = Runtime::new(Arc::new(|| {}), 4)?;
    let loaded = runtime.index(IndexCommand::Load(cache));
    loop {
        if let Some(event) = runtime.take_index_event()
            && event.id == loaded
        {
            event.result?;
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let revision = runtime.revision();
    let refresh = runtime.index(IndexCommand::Refresh);
    let refresh_start = Instant::now();
    let mut latencies = Vec::new();
    let mut query_ms = Vec::new();
    let mut previous_revision_results = 0;
    let updated_ms = loop {
        if let Some(event) = runtime.take_index_event()
            && event.id == refresh
        {
            event.result?;
            break event.elapsed_ms;
        }
        ensure!(
            refresh_start.elapsed() < Duration::from_secs(30),
            "Refresh timeout"
        );
        let start = Instant::now();
        let id = runtime.search(query.into(), false, 100);
        loop {
            if let Some(event) = runtime.take_search_event()
                && event.id == id
            {
                let view = event.result?;
                if event.revision == revision {
                    previous_revision_results += 1;
                }
                query_ms.push(view.elapsed_ms);
                latencies.push(start.elapsed().as_secs_f64() * 1000.);
                break;
            }
            ensure!(
                start.elapsed() < Duration::from_secs(5),
                "Search stalled during refresh"
            );
            thread::sleep(Duration::from_millis(1));
        }
    };
    ensure!(
        previous_revision_results > 0,
        "No search completed while the old snapshot was published"
    );
    ensure!(
        runtime.revision() > revision,
        "Completed refresh was not published"
    );
    let id = runtime.search(query.into(), false, 100);
    let deadline = Instant::now() + Duration::from_secs(5);
    let final_matches = loop {
        if let Some(event) = runtime.take_search_event()
            && event.id == id
        {
            ensure!(
                event.revision > revision,
                "Final search used the old snapshot"
            );
            break event.result?.total;
        }
        ensure!(Instant::now() < deadline, "Final search timeout");
        thread::sleep(Duration::from_millis(1));
    };
    latencies.sort_by(f64::total_cmp);
    query_ms.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({"entries_before":saved.len(),"serial_refresh_then_search_ms":serial_result.0,"serial_final_matches":serial_result.1,"parallel_refresh_ms":updated_ms,"searches_during_refresh":latencies.len(),"old_snapshot_searches":previous_revision_results,"parallel_request_median_ms":latencies[latencies.len()/2],"parallel_request_max_ms":latencies[latencies.len()-1],"search_work_median_ms":query_ms[query_ms.len()/2],"final_matches":final_matches,"poll_interval_ms":1,"query":query,"comparison":"serial scheduling reference vs production concurrent Runtime; filesystem cache not reset"})
    );
    Ok(())
}
