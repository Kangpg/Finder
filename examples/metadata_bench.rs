use anyhow::{Result, ensure};
use finder::{
    background::{IndexCommand, Pane, Runtime, SearchRequest},
    row_metadata,
};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
fn main() -> Result<()> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "enriched".into());
    ensure!(
        ["names", "enriched"].contains(&mode.as_str()),
        "invalid mode"
    );
    let temp = tempfile::tempdir()?;
    let mut visible = Vec::new();
    for n in 0..10 {
        let dir = temp.path().join(format!("folder-{n:02}"));
        std::fs::create_dir(&dir)?;
        visible.push(dir.clone());
        for f in 0..100 {
            std::fs::write(dir.join(format!("file-{f:03}.bin")), [0u8; 4096])?;
        }
    }
    for n in 0..20 {
        let file = temp.path().join(format!("root-{n:02}.txt"));
        std::fs::write(&file, [0u8; 100])?;
        visible.push(file);
    }
    let search = Runtime::new(Arc::new(|| {}), 4)?;
    let metadata = row_metadata::Runtime::new(Arc::new(|| {}))?;
    let index = search.index(IndexCommand::Folder(temp.path().into()));
    let start = Instant::now();
    loop {
        if let Some(e) = search.take_index_event()
            && e.id == index
        {
            e.result?;
            break;
        }
        ensure!(start.elapsed() < Duration::from_secs(30));
        thread::sleep(Duration::from_millis(1));
    }
    let mut times = Vec::new();
    for _ in 0..11 {
        let start = Instant::now();
        let id = search.search_with(SearchRequest {
            query: String::new(),
            limit: 5000,
            ..Default::default()
        });
        loop {
            if let Some(e) = search.take_search_event()
                && e.id == id
            {
                let v = e.result?;
                ensure!(v.total >= 1030);
                break;
            }
            ensure!(start.elapsed() < Duration::from_secs(30));
            thread::sleep(Duration::from_millis(1));
        }
        if mode == "enriched" {
            let id = metadata.request(Pane::Left, search.revision(), visible.clone())?;
            let mut values = std::collections::HashMap::new();
            loop {
                if let Some(batch) = metadata.take(Pane::Left)
                    && batch.id == id
                {
                    values.extend(batch.values);
                }
                if values.len() == visible.len()
                    && values
                        .values()
                        .all(|d| d.size.as_ref().is_some_and(|s| s.complete))
                {
                    break;
                }
                ensure!(start.elapsed() < Duration::from_secs(30));
                thread::sleep(Duration::from_millis(1));
            }
            for path in &visible {
                let expected = if path.is_dir() { 409600 } else { 100 };
                ensure!(values[path].size.as_ref().unwrap().bytes == expected);
            }
        }
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let first = times.remove(0);
    times.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({"mode":mode,"first_ms":first,"warm_median_ms":times[5],"warm_max_ms":times[9],"warm_runs":10,"indexed_files":1020,"visible_rows":30,"cached_metadata_lifetime_s":10,"cache":"OS cache not reset; warm enrichment reuses validated TTL entries; first enrichment scans folders"})
    );
    Ok(())
}
