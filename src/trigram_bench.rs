//! Opt-in completed workloads. Both modes use the same parser, matcher, ordering,
//! corpus and worker count. The control explicitly bypasses candidate selection.
use super::*;
#[test]
#[ignore = "Runs a completed synthetic workload in its own measured process"]
fn completed_workload() {
    let count: usize = std::env::var("REA_TRIGRAM_BENCH_COUNT")
        .unwrap()
        .parse()
        .unwrap();
    assert!([35000, 350000, 1000000].contains(&count));
    let mode = std::env::var("REA_TRIGRAM_BENCH_MODE").unwrap();
    assert!(["scan", "grams"].contains(&mode.as_str()));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    pool.install(|| {
        let build=Instant::now();
        let mut index=Index::new(Source::Folder {root:r"C:\trigram-bench".into()},1);
        let row=|frn,parent,usn,name:&str,attributes| UsnRecord {frn,parent,usn,reason:0,attributes,name:name.encode_utf16().collect()};
        for n in 0..100 {index.upsert(&row(n+2,1,0,&format!("branch_{n:03}"),usn::DIRECTORY)).unwrap();}
        for n in 0..count {
            let name=if n%997==0 {format!("report_{n:07}.txt")}else if n%991==0{format!("한국문서_{n:07}.txt")}else if n%983==0{format!("alpha_beta_{n:07}.cpp")}else{format!("item_{n:07}.dat")};
            index.upsert(&row(n as u64+1000,n as u64%100+2,0,&name,0)).unwrap();
        }
        if mode=="scan" {index.trigrams=crate::trigram::Catalog::Unavailable(crate::trigram::Unavailable::Budget);}
        index.rebuild_order(None).unwrap();
        let build_ms=build.elapsed().as_secs_f64()*1000.;
        let texts=["report_","한국문서","alpha_beta","0000042","not-present-xyz","report_ | 한국문서","item","it","ext:txt","regex:^report_"];
        let queries:Vec<_>=texts.iter().map(|s|Query::parse(s,false).unwrap()).collect();
        let mut digest=blake3::Hasher::new();
        let mut timings=vec![Vec::new();queries.len()];
        let mut cold_ms=0.;
        for round in 0..6 {
            for (q,query) in queries.iter().enumerate() {
                let result=index.search(query,120,None).unwrap();
                if round==0 && q==0 {cold_ms=result.elapsed_ms;}
                if round>0 {timings[q].push(result.elapsed_ms);}
                digest.update(&(result.total as u64).to_le_bytes());
                for id in result.ids {digest.update(&id.to_le_bytes());}
            }
        }
        let update=Instant::now();
        index.upsert(&row(1000,2,1,"renamed_unique_target.txt",0)).unwrap();
        index.rebuild_order(None).unwrap();
        let update_ms=update.elapsed().as_secs_f64()*1000.;
        let result=index.search(&Query::parse("renamed_unique",false).unwrap(),120,None).unwrap();
        assert_eq!(result.total,1);
        digest.update(&(result.total as u64).to_le_bytes());
        for id in result.ids {digest.update(&id.to_le_bytes());}
        let per_query:Vec<_>=timings.iter_mut().enumerate().map(|(n,t)|{t.sort_by(f64::total_cmp);serde_json::json!({"query":texts[n],"median_ms":t[t.len()/2]})}).collect();
        println!("GRAM_BENCH {}",serde_json::json!({"mode":mode,"files":count,"entries":index.len(),"threads":4,"build_ms":build_ms,"first_search_ms":cold_ms,"warm_rounds":5,"per_query":per_query,"rename_and_prepare_ms":update_ms,"estimated_index_bytes":index.memory_bytes(),"filename_index":index.filename_index_stats(),"digest":digest.finalize().to_hex().to_string()}));
    });
}
