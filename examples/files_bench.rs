use anyhow::{Result, ensure};
use finder::fileops::{self, Rename};
use std::time::Instant;
fn main() -> Result<()> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Com::*;
        ensure!(
            unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) } >= 0,
            "COM init"
        );
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        let _apartment = Apartment;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("원본");
        std::fs::create_dir(&source)?;
        let contents = vec![42u8; 4096];
        let count = 100;
        for n in 0..count {
            std::fs::write(source.join(format!("파일-{n:03}.bin")), &contents)?;
        }
        let paths: Vec<_> = (0..count)
            .map(|n| source.join(format!("파일-{n:03}.bin")))
            .collect();
        let mut runs = Vec::new();
        for run in 0..2 {
            let destination = temp.path().join(format!("copy-{run}"));
            let moved = temp.path().join(format!("move-{run}"));
            std::fs::create_dir(&destination)?;
            std::fs::create_dir(&moved)?;
            let all = Instant::now();
            let start = Instant::now();
            let report = fileops::transfer(&paths, &destination, false, true)?;
            ensure!(report.complete(), "{report:?}");
            let copy = start.elapsed().as_secs_f64() * 1000.;
            let plan: Vec<_> = (0..count)
                .map(|n| Rename {
                    source: destination.join(format!("파일-{n:03}.bin")),
                    name: format!("바뀐-{n:03}.bin"),
                })
                .collect();
            let start = Instant::now();
            let report = fileops::rename(&plan, true)?;
            ensure!(report.complete(), "{report:?}");
            let rename = start.elapsed().as_secs_f64() * 1000.;
            let renamed: Vec<_> = plan
                .iter()
                .map(|e| e.source.with_file_name(&e.name))
                .collect();
            let start = Instant::now();
            let report = fileops::transfer(&renamed, &moved, true, true)?;
            ensure!(report.complete(), "{report:?}");
            let movement = start.elapsed().as_secs_f64() * 1000.;
            for path in &renamed {
                ensure!(!path.exists(), "Move left original");
                ensure!(
                    std::fs::read(moved.join(path.file_name().unwrap()))? == contents,
                    "Content mismatch"
                );
            }
            runs.push(serde_json::json!({"run":run,"copy_ms":copy,"rename_ms":rename,"move_ms":movement,"completed_workflow_ms":all.elapsed().as_secs_f64()*1000.}));
        }
        println!(
            "{}",
            serde_json::json!({"files":count,"bytes_per_file":contents.len(),"runs":runs,"notes":"Real Windows IFileOperation, silent progress and no undo history for isolated measurement; OS cache not reset; final content verified; UI and external Shell brokers not measured"})
        );
    }
    Ok(())
}
