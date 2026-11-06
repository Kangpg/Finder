//! Bounded background lane for shell actions and portable settings writes.
use crate::preferences::{Preferences, Store};
use anyhow::{Result, ensure};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};
pub enum Job {
    Open {
        path: PathBuf,
        directory: bool,
    },
    Edit(PathBuf),
    ReadPaste(PathBuf),
    #[cfg(windows)]
    PrepareDrag {
        paths: Arc<[PathBuf]>,
        epoch: Arc<std::sync::atomic::AtomicU64>,
        id: u64,
    },
    CopyFiles {
        paths: Vec<PathBuf>,
        cut: bool,
        expected_sequence: u32,
    },
    Transfer {
        paths: Vec<PathBuf>,
        destination: PathBuf,
        cut: bool,
        clipboard_sequence: Option<u32>,
    },
    Rename(Vec<crate::fileops::Rename>),
    Reveal(PathBuf),
    #[cfg(windows)]
    PrepareMenu {
        paths: Arc<[PathBuf]>,
        epoch: Arc<std::sync::atomic::AtomicU64>,
        id: u64,
    },
    JournalQuery(char),
    JournalApply {
        expected: crate::journal::Snapshot,
        limits: crate::journal::Limits,
    },
    Save(Preferences),
}
pub enum Outcome {
    JournalQuery(crate::journal::Result<crate::journal::Snapshot>),
    JournalApplied(crate::journal::Result<crate::journal::Report>),
    #[cfg(windows)]
    DragPrepared {
        id: u64,
        result: Result<crate::shell_path::Selection>,
    },
    Done,
    Copied {
        count: usize,
        cut: bool,
    },
    Files(crate::fileops::Report),
    PasteReady {
        clipboard: crate::fileops::Clipboard,
        destination: PathBuf,
    },
    Saved,
    #[cfg(windows)]
    MenuPrepared {
        id: u64,
        result: Result<crate::shell_path::Selection>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Other,
    Settings,
    Mutation,
    PreparePaste,
    Journal,
}
impl Kind {
    pub fn file_work(self) -> bool {
        matches!(self, Self::Mutation | Self::PreparePaste)
    }
}
impl Job {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Save(_) => Kind::Settings,
            Self::JournalQuery(_) | Self::JournalApply { .. } => Kind::Journal,
            Self::ReadPaste(_) => Kind::PreparePaste,
            Self::Transfer { .. } | Self::Rename(_) => Kind::Mutation,
            _ => Kind::Other,
        }
    }
}
pub struct Pending {
    pub kind: Kind,
    pub receiver: Receiver<Result<Outcome>>,
}
struct Request {
    job: Job,
    reply: SyncSender<Result<Outcome>>,
}
struct Worker {
    tx: Option<SyncSender<Request>>,
    join: Option<thread::JoinHandle<()>>,
}
impl Worker {
    fn new(
        name: &str,
        mut store: Option<Store>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<Request>(16);
        let join = thread::Builder::new().name(name.into()).spawn(move || {
            #[cfg(windows)]
            let com = Com::new();
            #[cfg(windows)]
            let mut clipboard_owner = None;
            loop {
                #[cfg(windows)]
                crate::file_clipboard::pump_messages();
                let request = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(r) => r,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let result = (|| match request.job {
                    Job::JournalQuery(drive) => {
                        Ok(Outcome::JournalQuery(crate::journal::query(drive)))
                    }
                    Job::JournalApply { expected, limits } => {
                        let result = (|| {
                            let root = crate::app_paths::data_dir().map_err(|e| {
                                crate::journal::Error::Backup(crate::i18n::Message::capture(|| {
                                    format!("{e:#}")
                                }))
                            })?;
                            crate::journal::apply(&expected, limits, &root.join("usn-backups"))
                        })();
                        Ok(Outcome::JournalApplied(result))
                    }
                    Job::Save(p) => {
                        store
                            .as_mut()
                            .ok_or_else(|| {
                                anyhow::anyhow!(crate::i18n::Message::new(
                                    "설정 저장 비활성화: 기존 파일을 확인하세요",
                                    "Settings saving disabled: check the existing file"
                                ))
                            })?
                            .save(&p)?;
                        Ok(Outcome::Saved)
                    }
                    #[cfg(windows)]
                    Job::PrepareDrag { paths, epoch, id } => {
                        let result = if com.0 >= 0 {
                            crate::shell_path::prepare(&paths, &epoch, id)
                        } else {
                            Err(anyhow::anyhow!(crate::i18n::Message::new(
                                "Windows COM 초기화 실패",
                                "Could not initialize Windows COM"
                            )))
                        };
                        Ok(Outcome::DragPrepared { id, result })
                    }
                    Job::ReadPaste(destination) => {
                        #[cfg(windows)]
                        {
                            Ok(Outcome::PasteReady {
                                clipboard: crate::file_clipboard::read()?,
                                destination,
                            })
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = destination;
                            anyhow::bail!("Windows required");
                        }
                    }
                    Job::CopyFiles {
                        paths,
                        cut,
                        expected_sequence,
                    } => {
                        #[cfg(windows)]
                        {
                            if clipboard_owner.is_none() {
                                clipboard_owner = Some(crate::file_clipboard::Owner::new()?);
                            }
                            clipboard_owner.as_mut().unwrap().set(
                                &paths,
                                cut,
                                expected_sequence,
                            )?;
                            Ok(Outcome::Copied {
                                count: paths.len(),
                                cut,
                            })
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = (paths, cut, expected_sequence);
                            anyhow::bail!("Windows required");
                        }
                    }
                    Job::Transfer {
                        paths,
                        destination,
                        cut,
                        clipboard_sequence,
                    } => {
                        #[cfg(windows)]
                        {
                            ensure!(
                                com.0 >= 0,
                                crate::i18n::Message::new(
                                    "Windows COM 초기화 실패",
                                    "Could not initialize Windows COM"
                                )
                            );
                            let mut report =
                                crate::fileops::transfer(&paths, &destination, cut, false)?;
                            if cut
                                && report.complete()
                                && let Some(sequence) = clipboard_sequence
                                && let Err(e) = crate::file_clipboard::clear_if_current(sequence)
                            {
                                report.warning = Some(crate::msgf!(
                                    "파일 이동 완료, 클립보드 정리 실패: {e:#}",
                                    "Files moved, but clipboard cleanup failed: {e:#}"
                                ));
                            }
                            Ok(Outcome::Files(report))
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = (paths, destination, cut, clipboard_sequence);
                            anyhow::bail!("Windows required");
                        }
                    }
                    Job::Rename(plan) => {
                        #[cfg(windows)]
                        {
                            ensure!(
                                com.0 >= 0,
                                crate::i18n::Message::new(
                                    "Windows COM 초기화 실패",
                                    "Could not initialize Windows COM"
                                )
                            );
                            Ok(Outcome::Files(crate::fileops::rename(&plan, false)?))
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = plan;
                            anyhow::bail!("Windows required");
                        }
                    }
                    Job::Edit(path) => {
                        #[cfg(windows)]
                        {
                            ensure!(
                                com.0 >= 0,
                                crate::i18n::Message::new(
                                    "Windows COM 초기화 실패",
                                    "Could not initialize Windows COM"
                                )
                            );
                            PreparedOpen::new(&path, false, true)?.execute()?;
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = path;
                            anyhow::bail!("Windows required");
                        }
                        Ok(Outcome::Done)
                    }
                    #[cfg(windows)]
                    Job::PrepareMenu { paths, epoch, id } => {
                        let result = if com.0 >= 0 {
                            crate::shell_path::prepare(&paths, &epoch, id)
                        } else {
                            Err(anyhow::anyhow!(crate::i18n::Message::new(
                                "Windows COM 초기화 실패",
                                "Could not initialize Windows COM"
                            )))
                        };
                        Ok(Outcome::MenuPrepared { id, result })
                    }
                    Job::Open { path, directory } => {
                        #[cfg(windows)]
                        {
                            ensure!(
                                com.0 >= 0,
                                crate::msgf!(
                                    "Windows COM 초기화 실패: {}",
                                    "Could not initialize Windows COM: {}",
                                    com.0
                                )
                            );
                            PreparedOpen::new(&path, directory, false)?.execute()?;
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = (path, directory);
                            anyhow::bail!("Windows required");
                        }
                        Ok(Outcome::Done)
                    }
                    Job::Reveal(path) => {
                        std::process::Command::new("explorer.exe")
                            .arg("/select,")
                            .arg(dunce::simplified(&path))
                            .spawn()?;
                        Ok(Outcome::Done)
                    }
                })();
                let _ = request.reply.send(result);
                wake();
            }
        })?;
        Ok(Self {
            tx: Some(tx),
            join: Some(join),
        })
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
pub struct Services {
    desktop: Worker,
    preparation: Worker,
}
impl Services {
    pub fn new(store: Option<Store>, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        Ok(Self {
            desktop: Worker::new("desktop-services", store, wake.clone())?,
            preparation: Worker::new("shell-preparation", None, wake)?,
        })
    }
    /// Closing already joins the worker; enqueue the last settings snapshot before draining it.
    pub fn save_on_exit(&self, prefs: Preferences) -> Result<()> {
        let (reply, _receiver) = mpsc::sync_channel(1);
        self.desktop
            .tx
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!(crate::i18n::Message::new(
                    "작업 스레드 종료",
                    "Worker stopped"
                ))
            })?
            .send(Request {
                job: Job::Save(prefs),
                reply,
            })
            .map_err(|_| {
                anyhow::anyhow!(crate::i18n::Message::new(
                    "설정 저장 스레드 종료",
                    "Settings worker stopped"
                ))
            })?;
        Ok(())
    }
    pub fn submit(&self, job: Job) -> Result<Pending> {
        let kind = job.kind();
        let (reply, receiver) = mpsc::sync_channel(1);
        // Keep interactive Shell preparation independent of long file transfers.
        #[cfg(windows)]
        let prepare = matches!(
            &job,
            Job::PrepareMenu { .. }
                | Job::PrepareDrag { .. }
                | Job::JournalQuery(_)
                | Job::JournalApply { .. }
        );
        #[cfg(not(windows))]
        let prepare = matches!(&job, Job::JournalQuery(_) | Job::JournalApply { .. });
        let worker = if prepare {
            &self.preparation
        } else {
            &self.desktop
        };
        worker
            .tx
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!(crate::i18n::Message::new(
                    "작업 스레드 종료",
                    "Worker stopped"
                ))
            })?
            .try_send(Request { job, reply })
            .map_err(|_| {
                anyhow::anyhow!(crate::i18n::Message::new(
                    "작업이 많습니다. 잠시 후 다시 시도하세요",
                    "Too many pending tasks. Try again shortly"
                ))
            })?;
        Ok(Pending { receiver, kind })
    }
}
#[cfg(windows)]
struct Com(i32);
#[cfg(windows)]
impl Com {
    fn new() -> Self {
        use windows_sys::Win32::System::Com::*;
        Self(unsafe {
            CoInitializeEx(
                std::ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        })
    }
}
#[cfg(windows)]
impl Drop for Com {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
        }
    }
}
/// Bind the exact filesystem item before invoking a verb. A bare ShellExecute path
/// can resolve an extensionless folder name to an adjacent .bat/.exe instead.
#[cfg(windows)]
struct PreparedOpen {
    pidl: crate::shell_path::Pidl,
    verb: windows::core::PCWSTR,
    working_directory: Vec<u16>,
}
#[cfg(windows)]
impl PreparedOpen {
    fn new(path: &std::path::Path, directory: bool, edit: bool) -> Result<Self> {
        use anyhow::Context;
        // Check the published row's intent against the current filesystem. Never
        // turn a stale folder selection into execution of its replacement file.
        let metadata = std::fs::metadata(path).context(crate::i18n::Message::new(
            "선택 항목을 확인할 수 없습니다",
            "Could not verify selected item",
        ))?;
        ensure!(
            metadata.is_dir() == directory,
            crate::i18n::Message::new(
                "파일 종류가 변경되었습니다. 목록을 갱신하세요",
                "The item type changed. Refresh the results"
            )
        );
        ensure!(
            !edit || !directory,
            crate::i18n::Message::new("폴더는 편집할 수 없습니다", "Folders cannot be edited")
        );
        let pidl = crate::shell_path::Pidl::parse(path)?;
        let verb = if directory {
            windows::core::w!("explore")
        } else if edit {
            windows::core::w!("edit")
        } else {
            windows::core::PCWSTR::null()
        };
        let directory_path = if directory {
            path
        } else {
            path.parent().context(crate::i18n::Message::new(
                "파일의 상위 폴더가 없습니다",
                "The file has no parent folder",
            ))?
        };
        let working_directory = crate::shell_path::wide(directory_path)?;
        Ok(Self {
            pidl,
            verb,
            working_directory,
        })
    }
    fn info(&self) -> windows::Win32::UI::Shell::SHELLEXECUTEINFOW {
        use windows::Win32::UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL};
        SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_IDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: self.verb,
            lpIDList: self.pidl.0.cast(),
            lpDirectory: windows::core::PCWSTR(self.working_directory.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        }
    }
    fn execute(&self) -> Result<()> {
        use anyhow::Context;
        let mut info = self.info();
        unsafe { windows::Win32::UI::Shell::ShellExecuteExW(&mut info) }.context(
            crate::i18n::Message::new("Windows 파일 열기 실패", "Windows could not open the item"),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn file_launches_own_the_selected_items_working_directory() {
        let com = Com::new();
        assert!(com.0 >= 0);
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("한글 배치 & config");
        std::fs::create_dir(&folder).unwrap();
        let original = std::env::current_dir().unwrap();
        for name in ["start.bat", "start.cmd", "tool.exe"] {
            let path = folder.join(name);
            std::fs::write(&path, b"not executed").unwrap();
            for edit in [false, true] {
                let prepared = PreparedOpen::new(&path, false, edit).unwrap();
                let working = unsafe { prepared.info().lpDirectory.to_string().unwrap() };
                assert_eq!(
                    dunce::canonicalize(working).unwrap(),
                    dunce::canonicalize(&folder).unwrap()
                );
                assert!(prepared.info().lpFile.is_null());
            }
        }
        let prepared = PreparedOpen::new(&folder, true, false).unwrap();
        let working = unsafe { prepared.info().lpDirectory.to_string().unwrap() };
        assert_eq!(
            dunce::canonicalize(working).unwrap(),
            dunce::canonicalize(&folder).unwrap()
        );
        assert_eq!(std::env::current_dir().unwrap(), original);
    }
    #[cfg(windows)]
    #[test]
    fn opening_folder_binds_exact_item_despite_adjacent_batch_file() {
        use std::os::windows::ffi::OsStringExt;
        use windows::Win32::UI::Shell::*;
        let com = Com::new();
        assert!(com.0 >= 0);
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("한글 deploy");
        std::fs::create_dir(&folder).unwrap();
        let batch = temp.path().join("한글 deploy.bat");
        std::fs::write(&batch, "@echo must never execute").unwrap();
        // Inspect the exact production request; never invoke Explorer or a script.
        let prepared = PreparedOpen::new(&folder, true, false).unwrap();
        let info = prepared.info();
        assert!(info.lpFile.is_null());
        assert_eq!(info.fMask & SEE_MASK_IDLIST, SEE_MASK_IDLIST);
        assert_eq!(unsafe { info.lpVerb.to_string().unwrap() }, "explore");
        let mut resolved = vec![0u16; 32768];
        assert!(
            unsafe { SHGetPathFromIDListEx(prepared.pidl.0, &mut resolved, GPFIDL_DEFAULT) }
                .as_bool()
        );
        let length = resolved.iter().position(|&c| c == 0).unwrap();
        let resolved = PathBuf::from(std::ffi::OsString::from_wide(&resolved[..length]));
        assert_eq!(
            dunce::canonicalize(resolved).unwrap(),
            dunce::canonicalize(&folder).unwrap()
        );
        assert!(PreparedOpen::new(&batch, true, false).is_err());
        assert!(PreparedOpen::new(&folder, false, false).is_err());
        assert!(PreparedOpen::new(&folder, true, true).is_err());
        let file = PreparedOpen::new(&batch, false, false).unwrap();
        assert!(file.info().lpVerb.is_null());
        let edit = PreparedOpen::new(&batch, false, true).unwrap();
        assert_eq!(unsafe { edit.info().lpVerb.to_string().unwrap() }, "edit");
        assert!(PreparedOpen::new(&temp.path().join("missing"), true, false).is_err());
        assert_eq!(
            std::fs::read_to_string(batch).unwrap(),
            "@echo must never execute"
        );
    }
    #[cfg(windows)]
    #[test]
    fn shell_preparation_completes_even_when_desktop_queue_is_full() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("menu.txt");
        std::fs::write(&path, "unchanged").unwrap();
        // Hold the desktop lane at capacity, modeling a busy file-transfer worker.
        let (tx, _blocked) = mpsc::sync_channel(1);
        let svc = Services {
            desktop: Worker {
                tx: Some(tx),
                join: None,
            },
            preparation: Worker::new("test-shell-preparation", None, Arc::new(|| {})).unwrap(),
        };
        let _pending = svc.submit(Job::Save(Preferences::default())).unwrap();
        assert!(svc.submit(Job::Save(Preferences::default())).is_err());
        let epoch = Arc::new(std::sync::atomic::AtomicU64::new(1));
        let pending = svc
            .submit(Job::PrepareMenu {
                paths: vec![path.clone()].into(),
                epoch: epoch.clone(),
                id: 1,
            })
            .unwrap();
        match pending
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap()
        {
            Outcome::MenuPrepared { id: 1, result } => {
                assert_eq!(result.unwrap().pointers().len(), 1);
            }
            _ => panic!("wrong completion"),
        }
        epoch.store(2, std::sync::atomic::Ordering::Relaxed);
        let pending = svc
            .submit(Job::PrepareMenu {
                paths: vec![path.clone()].into(),
                epoch,
                id: 1,
            })
            .unwrap();
        match pending
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap()
        {
            Outcome::MenuPrepared { result, .. } => assert!(result.is_err()),
            _ => panic!("wrong completion"),
        }
        assert_eq!(std::fs::read_to_string(path).unwrap(), "unchanged");
    }
    #[test]
    fn shutdown_save_completes_without_shell_execution() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("data/preferences.json");
        let (store, mut prefs) = Store::load(config.clone()).unwrap();
        let svc = Services::new(Some(store), Arc::new(|| {})).unwrap();
        prefs.remember("한글");
        svc.save_on_exit(prefs).unwrap();
        drop(svc);
        let (_, saved) = Store::load(config).unwrap();
        assert_eq!(saved.history, ["한글"]);
    }
}
