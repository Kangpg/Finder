use anyhow::{Context, Result, ensure};
use finder::{
    background::{IndexCommand, Runtime, SearchRequest},
    filters::Filter,
    index::{SearchOrder, SortColumn},
    query::QueryOptions,
};
use std::{
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
fn completed(r: &Runtime, request: SearchRequest) -> Result<(f64, f64, usize, usize, usize)> {
    let start = Instant::now();
    let id = r.search_with(request);
    loop {
        if let Some(e) = r.take_search_event()
            && e.id == id
        {
            let v = e.result?;
            let errors = v
                .rows
                .iter()
                .filter(|r| r.details.as_ref().is_some_and(|d| d.error.is_some()))
                .count();
            return Ok((
                start.elapsed().as_secs_f64() * 1000.,
                v.elapsed_ms,
                v.total,
                v.rows.len(),
                errors,
            ));
        }
        ensure!(start.elapsed() < Duration::from_secs(30), "Search timeout");
        thread::sleep(Duration::from_millis(1));
    }
}
fn main() -> Result<()> {
    let cache = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("Usage: features_bench SNAPSHOT")?,
    );
    let r = Runtime::new(Arc::new(|| {}), 4)?;
    let id = r.index(IndexCommand::Load(cache));
    let start = Instant::now();
    loop {
        if let Some(e) = r.take_index_event()
            && e.id == id
        {
            e.result?;
            break;
        }
        ensure!(start.elapsed() < Duration::from_secs(30), "Load timeout");
        thread::sleep(Duration::from_millis(1));
    }
    let names_only = std::env::args().nth(2).as_deref() == Some("names");
    let mut results = Vec::new();
    for (column, descending, filter) in [
        (SortColumn::Name, false, Filter::All),
        (SortColumn::Name, true, Filter::All),
        (SortColumn::Name, false, Filter::Images),
        (SortColumn::Path, false, Filter::All),
        (SortColumn::Type, false, Filter::All),
        (SortColumn::Size, false, Filter::All),
        (SortColumn::Modified, false, Filter::All),
    ] {
        if names_only && column != SortColumn::Name {
            continue;
        }
        let request = || SearchRequest {
            query: String::new(),
            options: QueryOptions::default(),
            filter,
            order: SearchOrder {
                column,
                descending,
                folders_first: false,
            },
            limit: 5000,
        };
        let first = completed(&r, request())?;
        let mut requests = Vec::new();
        let mut work = Vec::new();
        let mut errors = first.4;
        for _ in 0..10 {
            let t = completed(&r, request())?;
            ensure!(t.2 == first.2 && t.3 == first.3, "Result count changed");
            requests.push(t.0);
            work.push(t.1);
            errors += t.4;
        }
        requests.sort_by(f64::total_cmp);
        work.sort_by(f64::total_cmp);
        results.push(serde_json::json!({"column":format!("{column:?}"),"descending":descending,"filter":format!("{filter:?}"),"matches":first.2,"returned":first.3,"first_request_ms":first.0,"first_work_ms":first.1,"warm_median_request_ms":requests[5],"warm_median_work_ms":work[5],"warm_max_request_ms":requests[9],"metadata_errors_all_runs":errors}));
    }
    println!(
        "{}",
        serde_json::json!({"runs":results,"workers":4,"warm_runs":10,"poll_ms":1,"cache":"index-scoped metadata reused until refresh; metadata sorting may read disk; display columns do not request disk metadata; OS filesystem cache not reset"})
    );
    Ok(())
}
