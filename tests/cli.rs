use std::{
    path::Path,
    process::{Command, Output},
};
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_finder-cli"))
        .args(args)
        .output()
        .unwrap()
}
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}
#[test]
fn real_cli_filename_index_preserves_unicode_candidates_after_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    std::fs::create_dir(&root).unwrap();
    for n in 0..127 {
        std::fs::write(root.join(format!("noise_{n:03}.dat")), "data").unwrap();
    }
    std::fs::write(root.join("Report_한국어.txt"), "report").unwrap();
    std::fs::write(root.join("alpha.cpp"), "alpha").unwrap();
    let cache = temp.path().join("files.reaidx");
    let built = run(&["index", "--folder", text(&root), "--output", text(&cache)]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let stats = run(&["stats", "--index", text(&cache)]);
    assert!(stats.status.success());
    let stats: serde_json::Value = serde_json::from_slice(&stats.stdout).unwrap();
    assert_eq!(stats["filename_index"]["state"], "ready");
    assert!(stats["filename_index"]["estimated_bytes"].as_u64().unwrap() > 0);
    let result = run(&["search", "--index", text(&cache), "report_", "--json"]);
    assert!(result.status.success());
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["total"], 1);
    assert_eq!(result["results"][0]["name"], "Report_한국어.txt");
    std::fs::rename(root.join("Report_한국어.txt"), root.join("new_한국어.txt")).unwrap();
    std::fs::write(root.join("new_한국어_2.txt"), "new").unwrap();
    std::fs::remove_file(root.join("alpha.cpp")).unwrap();
    assert!(run(&["refresh", "--index", text(&cache)]).status.success());
    for (query, count) in [
        ("new_한국어", 2),
        ("report_", 0),
        ("new_한국어 | noise_126", 3),
    ] {
        let result = run(&["search", "--index", text(&cache), query, "--json"]);
        assert!(result.status.success());
        let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(result["total"], count);
    }
}
#[test]
fn real_cli_snapshot_search_refresh_and_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("한국 자료");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("Hello Report.TXT"), "one").unwrap();
    std::fs::write(root.join("second.rs"), "two").unwrap();
    let cache = temp.path().join("test.reaidx");
    let built = run(&["index", "--folder", text(&root), "--output", text(&cache)]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let result = run(&[
        "search",
        "--index",
        text(&cache),
        "ext:txt;rs",
        "--limit",
        "1",
        "--json",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let data: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(data["total"], 2);
    assert_eq!(data["returned"], 1);
    assert_eq!(data["freshness"], "saved_snapshot");
    assert_eq!(data["results"][0]["name"], "Hello Report.TXT");
    assert_eq!(data["results"][0]["path_known"], true);
    std::fs::rename(root.join("second.rs"), root.join("changed.cpp")).unwrap();
    std::fs::remove_file(root.join("Hello Report.TXT")).unwrap();
    assert!(run(&["refresh", "--index", text(&cache)]).status.success());
    let result = run(&["search", "--index", text(&cache), "ext:cpp", "--json"]);
    assert!(result.status.success());
    let data: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(data["total"], 1);
    assert!(
        !run(&["search", "--index", text(&cache), "regex:["])
            .status
            .success()
    );
    assert!(
        !run(&["search", "--index", text(&cache), "x", "--limit", "0"])
            .status
            .success()
    );
    std::fs::write(&cache, b"broken cache").unwrap();
    assert!(!run(&["stats", "--index", text(&cache)]).status.success());
}

#[test]
fn real_cli_filter_options_and_global_sort() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("사진 모음");
    std::fs::create_dir(&root).unwrap();
    for name in ["a.JPG", "z.png", "album.png.txt"] {
        std::fs::write(root.join(name), b"data").unwrap();
    }
    std::fs::create_dir(root.join("zz-folder")).unwrap();
    let cache = dir.path().join("index.reaidx");
    assert!(
        run(&["index", "--folder", text(&root), "-o", text(&cache)])
            .status
            .success()
    );
    let r = run(&[
        "search",
        "-i",
        text(&cache),
        "--filter",
        "images",
        "--descending",
        "--limit",
        "1",
        "--json",
    ]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let j: serde_json::Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(j["total"], 2);
    assert_eq!(j["results"][0]["name"], "z.png");
    let r = run(&[
        "search",
        "-i",
        text(&cache),
        "사진 모음",
        "--match-path",
        "--json",
    ]);
    assert!(r.status.success());
    let j: serde_json::Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(j["total"], 4);
    let r = run(&[
        "search",
        "-i",
        text(&cache),
        r"^(a|z)\.",
        "--regex",
        "--json",
    ]);
    assert!(r.status.success());
    let j: serde_json::Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(j["total"], 2);
    let r = run(&[
        "search",
        "-i",
        text(&cache),
        "",
        "--folders-first",
        "--limit",
        "1",
        "--json",
    ]);
    assert!(r.status.success());
    let j: serde_json::Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(j["results"][0]["name"], "zz-folder");
}

#[test]
fn journal_query_is_read_only_and_rejects_invalid_drive_inputs() {
    for drive in ["?", "D:/folder", "\\\\server\\share", "DD", ""] {
        let result = run(&["journal", "--drive", drive]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Use a drive letter"));
    }
    let help = run(&["journal", "--help"]);
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("--apply"));
}
