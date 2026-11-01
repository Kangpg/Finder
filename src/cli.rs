use crate::{
    filters::Filter,
    folder,
    index::{Index, SearchOrder, Source},
    query::{Query, QueryOptions},
    storage,
};
use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};
use std::{path::PathBuf, time::Instant};

#[derive(Parser)]
#[command(
    version,
    about = "Local Windows x64 filename search. No arguments opens the desktop app."
)]
struct Cli {
    /// Number of search/sort workers (1..32).
    #[arg(long, global = true, default_value_t = 4)]
    threads: usize,
    /// Desktop output backend; use opengl for compatibility/comparison.
    #[cfg(feature = "gui")]
    #[arg(long, global = true, value_enum, default_value_t = crate::graphics::RendererChoice::default())]
    renderer: crate::graphics::RendererChoice,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Args)]
#[group(required = true, multiple = false)]
struct Location {
    /// Scan one directory tree without administrator privileges.
    #[arg(long)]
    folder: Option<PathBuf>,
    /// Read the NTFS MFT and USN journal, e.g. D: (requires volume access).
    #[arg(long)]
    drive: Option<String>,
}
#[derive(Subcommand)]
enum Command {
    /// Query current NTFS USN journal limits without changing the volume.
    Journal {
        #[arg(long)]
        drive: String,
    },
    /// Build and atomically save a filename index.
    Index {
        #[command(flatten)]
        source: Location,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Search a saved snapshot. NTFS defaults to journal replay before searching.
    Search {
        #[arg(short, long)]
        index: PathBuf,
        #[arg(default_value = "")]
        query: String,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        case_sensitive: bool,
        #[arg(long)]
        match_path: bool,
        #[arg(long)]
        whole_word: bool,
        #[arg(long)]
        regex: bool,
        #[arg(long,value_enum,default_value_t=Filter::All)]
        filter: Filter,
        #[arg(long)]
        descending: bool,
        #[arg(long)]
        folders_first: bool,
        #[arg(long)]
        json: bool,
        /// Explicitly search the saved NTFS state without checking disk freshness.
        #[arg(long)]
        snapshot: bool,
    },
    /// Display index size, source and coverage.
    Stats {
        #[arg(short, long)]
        index: PathBuf,
    },
    /// Apply the NTFS journal, or rebuild a folder snapshot; save only on success.
    Refresh {
        #[arg(short, long)]
        index: PathBuf,
    },
    /// Refresh repeatedly (Ctrl+C exits). Folder mode rescans the selected tree.
    Watch {
        #[arg(short, long)]
        index: PathBuf,
        #[arg(long, default_value_t = 3)]
        interval: u64,
    },
    /// Open the desktop application with a directory, drive or saved snapshot.
    Gui {
        #[arg(long,conflicts_with_all=["drive","index"])]
        folder: Option<PathBuf>,
        #[arg(long, conflicts_with = "index")]
        drive: Option<String>,
        #[arg(long)]
        index: Option<PathBuf>,
    },
    /// Measure representative queries on an existing snapshot; no disk freshness claim.
    Bench {
        #[arg(short, long)]
        index: PathBuf,
        #[arg(long, default_value_t = 10)]
        runs: usize,
    },
}
fn drive_letter(s: &str) -> Result<char> {
    let bytes = s.as_bytes();
    ensure!(
        (bytes.len() == 1 || bytes.len() == 2 && bytes[1] == b':')
            && bytes[0].is_ascii_alphabetic(),
        "Use a drive letter, e.g. D:"
    );
    Ok((bytes[0] as char).to_ascii_uppercase())
}
fn build(source: Location) -> Result<Index> {
    if let Some(root) = source.folder {
        return folder::scan(&root, None);
    }
    let drive = drive_letter(&source.drive.context("Missing index source")?)?;
    #[cfg(windows)]
    {
        crate::ntfs::scan(drive, None)
    }
    #[cfg(not(windows))]
    {
        let _ = drive;
        anyhow::bail!("NTFS drive indexing is Windows-only")
    }
}
fn refresh(index: &mut Index) -> Result<()> {
    match &index.source {
        Source::Folder { root } => {
            *index = folder::scan(root, None)?;
            Ok(())
        }
        Source::Ntfs { .. } => {
            #[cfg(windows)]
            {
                crate::ntfs::refresh(index, None)?;
                Ok(())
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("NTFS refresh is Windows-only")
            }
        }
    }
}
fn stats(index: &Index) -> serde_json::Value {
    serde_json::json!({"entries":index.len(),"estimated_index_bytes":index.memory_bytes(),"filename_index":index.filename_index_stats(),"source":index.source,"scanned_unix":index.scanned_unix,"generation":index.generation,"coverage_errors":index.coverage_errors,"coverage_notes":index.coverage_notes})
}
pub fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    ensure!((1..=32).contains(&cli.threads), "--threads must be 1..32");
    if cli
        .command
        .as_ref()
        .is_some_and(|command| !matches!(command, Command::Gui { .. } | Command::Journal { .. }))
    {
        rayon::ThreadPoolBuilder::new()
            .num_threads(cli.threads)
            .build_global()?;
    }
    match cli.command {
        Some(Command::Journal { drive }) => {
            let snapshot = crate::journal::query(drive_letter(&drive)?)?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        Some(Command::Index { source, output }) => {
            let start = Instant::now();
            let index = build(source)?;
            storage::save(&index, &output)?;
            println!(
                "{}",
                serde_json::json!({"index":output,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"stats":stats(&index)})
            );
        }
        Some(Command::Search {
            index: path,
            query,
            limit,
            case_sensitive,
            match_path,
            whole_word,
            regex,
            filter,
            descending,
            folders_first,
            json,
            snapshot,
        }) => {
            let query = Query::parse_with(
                &query,
                QueryOptions {
                    case_sensitive,
                    path_match: match_path,
                    whole_word,
                    regex,
                },
                filter,
            )?;
            let mut index = storage::load(&path, None)?;
            let freshness = if matches!(index.source, Source::Ntfs { .. }) && !snapshot {
                refresh(&mut index)?;
                "journal_replayed"
            } else {
                "saved_snapshot"
            };
            if freshness == "saved_snapshot" && !json {
                eprintln!("Searching a saved snapshot; use refresh for current data.");
            }
            let r = index.search_ordered(
                &query,
                limit,
                None,
                SearchOrder {
                    column: Default::default(),
                    descending,
                    folders_first,
                },
            )?;
            let rows:Vec<_>=r.ids.iter().map(|&id|{let path=index.path(id).ok();let known=path.is_some();serde_json::json!({"frn":index.records[id as usize].frn,"name":index.name(id),"directory":index.records[id as usize].is_dir(),"path":path,"path_known":known})}).collect();
            if json {
                println!(
                    "{}",
                    serde_json::json!({"total":r.total,"returned":rows.len(),"elapsed_ms":r.elapsed_ms,"freshness":freshness,"coverage_errors":index.coverage_errors,"results":rows})
                );
            } else {
                for row in rows {
                    println!("{}", row["path"].as_str().unwrap_or("<unresolved parent>"));
                }
                eprintln!(
                    "{} matches; {} shown; {:.3} ms",
                    r.total,
                    r.ids.len(),
                    r.elapsed_ms
                );
            }
        }
        Some(Command::Stats { index }) => println!(
            "{}",
            serde_json::to_string_pretty(&stats(&storage::load(&index, None)?))?
        ),
        Some(Command::Refresh { index: path }) => {
            let mut i = storage::load(&path, None)?;
            refresh(&mut i)?;
            storage::save(&i, &path)?;
            println!("{}", stats(&i));
        }
        Some(Command::Watch {
            index: path,
            interval,
        }) => {
            ensure!(
                (1..=3600).contains(&interval),
                "Watch interval must be 1..3600 seconds"
            );
            let mut index = storage::load(&path, None)?;
            loop {
                refresh(&mut index)?;
                storage::save(&index, &path)?;
                println!("{}", stats(&index));
                std::thread::sleep(std::time::Duration::from_secs(interval));
            }
        }
        Some(Command::Bench { index, runs }) => {
            ensure!((1..=1000).contains(&runs), "Runs must be 1..1000");
            let start = Instant::now();
            let i = storage::load(&index, None)?;
            let load_ms = start.elapsed().as_secs_f64() * 1000.;
            let mut results = Vec::new();
            for text in ["src", "ext:rs;txt", "folder:", "*.exe", "!folder: a"] {
                let q = Query::parse(text, false)?;
                let cold = i.search(&q, 100, None)?;
                let mut warm = Vec::new();
                for _ in 0..runs {
                    warm.push(i.search(&q, 100, None)?.elapsed_ms);
                }
                warm.sort_by(f64::total_cmp);
                results.push(serde_json::json!({"query":text,"matches":cold.total,"first_ms":cold.elapsed_ms,"warm_median_ms":warm[warm.len()/2],"warm_max_ms":warm[warm.len()-1]}));
            }
            println!(
                "{}",
                serde_json::json!({"entries":i.len(),"load_ms":load_ms,"estimated_index_bytes":i.memory_bytes(),"threads":cli.threads,"warm_runs":runs,"queries":results})
            );
        }
        command => {
            #[cfg(feature = "gui")]
            {
                let (folder, drive, index) = if let Some(Command::Gui {
                    folder,
                    drive,
                    index,
                }) = command
                {
                    (folder, drive.map(|s| drive_letter(&s)).transpose()?, index)
                } else {
                    (None, None, None)
                };
                crate::gui::launch(folder, drive, index, cli.threads, cli.renderer)?;
            }
            #[cfg(not(feature = "gui"))]
            {
                let _ = command;
                anyhow::bail!("Rebuild with the gui feature to open the desktop app");
            }
        }
    }
    Ok(())
}
