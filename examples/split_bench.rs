use anyhow::{Context, Result, ensure};
use finder::background::{IndexCommand, Pane, Runtime, SearchEvent, SearchRequest};
use std::{
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
fn done(r: &Runtime, pane: Pane, id: u64) -> Result<SearchEvent> {
    let start = Instant::now();
    loop {
        if let Some(event) = r.take_search_for(pane)
            && event.id == id
        {
            return Ok(event);
        }
        ensure!(start.elapsed() < Duration::from_secs(30), "search timeout");
        thread::sleep(Duration::from_millis(1));
    }
}
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let snapshot = PathBuf::from(args.next().context("snapshot required")?);
    let mode = args
        .next()
        .context("sequential, split or independent required")?;
    ensure!(
        ["sequential", "split", "independent"].contains(&mode.as_str()),
        "invalid mode"
    );
    let r = Runtime::new(Arc::new(|| {}), 4)?;
    let independent = if mode == "independent" {
        Some(r.independent()?)
    } else {
        None
    };
    let loading = Instant::now();
    for runtime in std::iter::once(&r).chain(independent.as_ref()) {
        let id = runtime.index(IndexCommand::Load(snapshot.clone()));
        loop {
            if let Some(event) = runtime.take_index_event()
                && event.id == id
            {
                event.result?;
                break;
            }
            ensure!(loading.elapsed() < Duration::from_secs(30), "load timeout");
            thread::sleep(Duration::from_millis(1));
        }
    }
    let load_ms = loading.elapsed().as_secs_f64() * 1000.;
    let mut samples = Vec::new();
    let mut expected = None;
    for _ in 0..11 {
        let start = Instant::now();
        let request = |query: &str| SearchRequest {
            query: query.into(),
            limit: 5000,
            ..Default::default()
        };
        let left = r.search_in(Pane::Left, request("ext:rs;toml"));
        let (l, rr) = if mode != "sequential" {
            let right_runtime = independent.as_ref().unwrap_or(&r);
            let right = right_runtime.search_in(Pane::Right, request("folder: !target"));
            (
                done(&r, Pane::Left, left)?,
                done(right_runtime, Pane::Right, right)?,
            )
        } else {
            let l = done(&r, Pane::Left, left)?;
            let right = r.search_in(Pane::Left, request("folder: !target"));
            (l, done(&r, Pane::Left, right)?)
        };
        if mode != "independent" {
            ensure!(l.revision == rr.revision, "shared snapshot mismatch");
        }
        let l = l.result?;
        let rr = rr.result?;
        // Verify equivalent full result identities, rather than timing unmatched work.
        let identity = (
            l.total,
            rr.total,
            l.rows
                .iter()
                .map(|row| row.path.clone())
                .collect::<Vec<_>>(),
            rr.rows
                .iter()
                .map(|row| row.path.clone())
                .collect::<Vec<_>>(),
        );
        if let Some(ref old) = expected {
            ensure!(old == &identity, "result identities changed");
        } else {
            expected = Some(identity);
        }
        samples.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let first = samples.remove(0);
    samples.sort_by(f64::total_cmp);
    let identity = expected.unwrap();
    let digest = blake3::hash(&serde_json::to_vec(&identity)?)
        .to_hex()
        .to_string();
    println!(
        "{}",
        serde_json::json!({"mode":mode,"workers_total":4,"index_copies":if mode=="independent"{2}else{1},"identity_blake3":digest,"load_ms":load_ms,"first_pair_ms":first,"warm_median_pair_ms":samples[5],"warm_max_pair_ms":samples[9],"warm_pairs":10,"matches":[identity.0,identity.1],"returned":[identity.2.len(),identity.3.len()],"cache":"OS cache not reset; first pair is not a cold filesystem run","details":false,"poll_ms":1})
    );
    Ok(())
}
