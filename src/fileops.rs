//! Plans own the exact selected paths. Filesystem changes run only on the STA worker.
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
pub const MAX_ITEMS: usize = 5000;
#[derive(Debug, Clone)]
pub struct Clipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
    pub sequence: u32,
}
#[derive(Debug, Clone)]
pub struct Rename {
    pub source: PathBuf,
    pub name: String,
}
#[derive(Clone)]
pub struct RenameSource {
    pub path: PathBuf,
    pub directory: bool,
}
pub enum RenameRule<'a> {
    Single(&'a str),
    Numbered {
        base: &'a str,
        start: u32,
        keep_extension: bool,
    },
    Replace {
        from: &'a str,
        to: &'a str,
    },
}
pub fn valid_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty() && name.encode_utf16().count() <= 255,
        crate::i18n::Message::new(
            "이름은 1~255 UTF-16 문자여야 합니다",
            "Names must contain 1–255 UTF-16 code units"
        )
    );
    ensure!(
        !name.ends_with(['.', ' '])
            && !name
                .chars()
                .any(|c| c < ' ' || ['<', '>', ':', '"', '/', '\\', '|', '?', '*'].contains(&c)),
        crate::i18n::Message::new(
            "이름에 사용할 수 없는 문자가 있습니다",
            "The name contains invalid characters"
        )
    );
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$"]
        .contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p).is_some_and(|s| {
                ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&s)
            })
        });
    ensure!(
        !reserved,
        crate::i18n::Message::new(
            "Windows 예약 이름입니다",
            "This name is reserved by Windows"
        )
    );
    Ok(())
}
pub fn rename_plan(items: &[RenameSource], rule: RenameRule<'_>) -> Result<Vec<Rename>> {
    ensure!(
        !items.is_empty() && items.len() <= MAX_ITEMS,
        crate::i18n::Message::new(
            "선택 항목은 1~5000개여야 합니다",
            "Select between 1 and 5,000 items"
        )
    );
    if matches!(rule, RenameRule::Single(_)) {
        ensure!(
            items.len() == 1,
            crate::i18n::Message::new(
                "단일 이름은 한 항목에만 적용할 수 있습니다",
                "A single name can only be applied to one item"
            )
        );
    }
    if let RenameRule::Replace { from, .. } = rule {
        ensure!(
            !from.is_empty(),
            crate::i18n::Message::new("찾을 문자열을 입력하세요", "Enter the text to find")
        );
    }
    let mut result = Vec::new();
    let mut targets = HashSet::new();
    for (n, item) in items.iter().enumerate() {
        ensure!(
            item.path.is_absolute(),
            crate::i18n::Message::new("절대 경로가 필요합니다", "An absolute path is required")
        );
        let old =
            item.path
                .file_name()
                .and_then(|s| s.to_str())
                .context(crate::i18n::Message::new(
                    "표시할 수 없는 파일 이름입니다",
                    "The file name cannot be displayed",
                ))?;
        let name = match rule {
            RenameRule::Single(name) => name.into(),
            RenameRule::Replace { from, to } => old.replace(from, to),
            RenameRule::Numbered {
                base,
                start,
                keep_extension,
            } => {
                ensure!(
                    !base.trim().is_empty(),
                    crate::i18n::Message::new("새 기본 이름을 입력하세요", "Enter a new base name")
                );
                let number = start
                    .checked_add(n as u32)
                    .context(crate::i18n::Message::new(
                        "번호 범위 초과",
                        "Number is out of range",
                    ))?;
                let ext = if keep_extension && !item.directory {
                    item.path.extension().and_then(|s| s.to_str())
                } else {
                    None
                };
                format!(
                    "{base} ({number}){}",
                    ext.map_or(String::new(), |s| format!(".{s}"))
                )
            }
        };
        valid_name(&name)?;
        let target = item.path.with_file_name(&name);
        // Conservative duplicate check; the Shell performs final filesystem collision handling.
        ensure!(
            targets.insert(target.to_string_lossy().to_lowercase()),
            crate::i18n::Message::new(
                "같은 폴더에 중복될 새 이름이 있습니다",
                "The new names would conflict in the same folder"
            )
        );
        if old != name {
            result.push(Rename {
                source: item.path.clone(),
                name,
            });
        }
    }
    ensure!(
        !result.is_empty(),
        crate::i18n::Message::new("변경할 이름이 없습니다", "No names would change")
    );
    Ok(result)
}
#[derive(Debug)]
pub struct Report {
    pub aborted: bool,
    pub error: Option<crate::i18n::Message>,
    pub warning: Option<crate::i18n::Message>,
}
impl Report {
    pub fn complete(&self) -> bool {
        !self.aborted && self.error.is_none()
    }
}
#[cfg(windows)]
mod native {
    use super::*;
    use windows::{
        Win32::{
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            UI::Shell::*,
        },
        core::PCWSTR,
    };
    fn item(path: &Path) -> Result<IShellItem> {
        let w = crate::shell_path::wide(path)?;
        Ok(unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None) }?)
    }
    fn operation(silent: bool, copy: bool) -> Result<IFileOperation> {
        let op: IFileOperation =
            unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER) }?;
        let mut flags = FOF_NOCONFIRMMKDIR;
        if !silent {
            flags |= FOF_ALLOWUNDO | FOFX_ADDUNDORECORD;
        }
        if copy {
            flags |= FOF_RENAMEONCOLLISION;
        }
        if silent {
            flags |= FOF_SILENT | FOF_NOERRORUI | FOFX_EARLYFAILURE;
        }
        unsafe { op.SetOperationFlags(flags) }?;
        Ok(op)
    }
    fn perform(op: &IFileOperation) -> Report {
        let result = unsafe { op.PerformOperations() };
        let aborted = unsafe { op.GetAnyOperationsAborted() };
        Report {
            warning: None,
            aborted: aborted.as_ref().map_or(true, |v| v.as_bool()),
            error: result
                .err()
                .map(crate::i18n::Message::error)
                .or_else(|| aborted.err().map(crate::i18n::Message::error)),
        }
    }
    pub fn transfer(
        paths: &[PathBuf],
        destination: &Path,
        cut: bool,
        silent: bool,
    ) -> Result<Report> {
        ensure!(
            !paths.is_empty() && paths.len() <= MAX_ITEMS,
            crate::i18n::Message::new(
                "선택 항목은 1~5000개여야 합니다",
                "Select between 1 and 5,000 items"
            )
        );
        ensure!(
            destination.is_dir(),
            crate::i18n::Message::new(
                "붙여넣을 폴더가 없습니다",
                "The destination folder does not exist"
            )
        );
        let destination_real = destination.canonicalize()?;
        for source in paths {
            let actual = source.canonicalize().with_context(|| {
                crate::msgf!(
                    "원본 확인 실패: {}",
                    "Could not verify source: {}",
                    source.display()
                )
            })?;
            if source.is_dir() {
                ensure!(
                    !destination_real.starts_with(&actual),
                    crate::i18n::Message::new(
                        "폴더 자신 또는 하위 폴더로 복사/이동할 수 없습니다",
                        "A folder cannot be copied or moved into itself or a descendant"
                    )
                );
            }
            if cut {
                ensure!(
                    actual.parent() != Some(destination_real.as_path()),
                    crate::i18n::Message::new(
                        "같은 폴더로 이동할 수 없습니다",
                        "An item cannot be moved into its current folder"
                    )
                );
            }
        }
        let dest = item(destination)?;
        let op = operation(silent, !cut)?;
        for source in paths {
            let source = item(source)?;
            unsafe {
                if cut {
                    op.MoveItem(&source, &dest, PCWSTR::null(), None)
                } else {
                    op.CopyItem(&source, &dest, PCWSTR::null(), None)
                }
            }?;
        }
        Ok(perform(&op))
    }
    pub fn rename(plan: &[Rename], silent: bool) -> Result<Report> {
        ensure!(
            !plan.is_empty() && plan.len() <= MAX_ITEMS,
            crate::i18n::Message::new(
                "이름 변경 항목 수가 잘못되었습니다",
                "Invalid number of items to rename"
            )
        );
        for entry in plan {
            valid_name(&entry.name)?;
        }
        // Renaming a parent invalidates paths of its selected descendants. Reject before queueing.
        for (n, entry) in plan.iter().enumerate() {
            if entry.source.is_dir() {
                for (m, other) in plan.iter().enumerate() {
                    if m != n {
                        ensure!(
                            !other.source.starts_with(&entry.source),
                            crate::i18n::Message::new(
                                "상위 폴더와 내부 항목을 동시에 이름 변경할 수 없습니다",
                                "A folder and its descendants cannot be renamed together"
                            )
                        );
                    }
                }
            }
        }
        let op = operation(silent, false)?;
        for entry in plan {
            let source = item(&entry.source)?;
            let name: Vec<u16> = entry.name.encode_utf16().chain(Some(0)).collect();
            unsafe { op.RenameItem(&source, PCWSTR(name.as_ptr()), None) }?;
        }
        Ok(perform(&op))
    }
}
#[cfg(windows)]
pub use native::{rename, transfer};
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rename_preview_is_exact_and_rejects_invalid_or_duplicate_names() {
        let src = |name: &str| RenameSource {
            path: PathBuf::from(r"C:\sample").join(name),
            directory: false,
        };
        let items = [src("a.txt"), src("b.png")];
        let plan = rename_plan(
            &items,
            RenameRule::Numbered {
                base: "사진",
                start: 3,
                keep_extension: true,
            },
        )
        .unwrap();
        assert_eq!(plan[0].name, "사진 (3).txt");
        assert_eq!(plan[1].name, "사진 (4).png");
        let plan = rename_plan(
            &[src("report.txt")],
            RenameRule::Replace {
                from: "report",
                to: "문서",
            },
        )
        .unwrap();
        assert_eq!(plan[0].name, "문서.txt");
        for name in [
            "", "..", "NUL.txt", "com1", "a/b", "a:b", "a.", "a ", "x?", "a\\b",
        ] {
            assert!(valid_name(name).is_err(), "{name}");
        }
        assert!(
            rename_plan(
                &[src("a.txt"), src("b.txt")],
                RenameRule::Replace { from: "a", to: "b" }
            )
            .is_err()
        );
        assert!(rename_plan(&items, RenameRule::Replace { from: "", to: "x" }).is_err());
    }
}

#[cfg(all(test, windows))]
mod native_tests {
    use super::*;
    struct Apartment;
    impl Apartment {
        fn new() -> Self {
            use windows_sys::Win32::System::Com::*;
            assert!(
                unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) } >= 0
            );
            Self
        }
    }
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { windows_sys::Win32::System::Com::CoUninitialize() };
        }
    }
    #[test]
    fn real_shell_copy_directory_duplicate_move_and_rename_in_temp_tree() {
        let _apartment = Apartment::new();
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("원본");
        let dest = tmp.path().join("복사 대상");
        let moved = tmp.path().join("이동 대상");
        for p in [&source, &dest, &moved] {
            std::fs::create_dir(p).unwrap();
        }
        std::fs::write(source.join("a.txt"), b"first").unwrap();
        std::fs::create_dir(source.join("folder")).unwrap();
        std::fs::write(source.join("folder/child.txt"), b"nested").unwrap();
        let paths = vec![source.join("a.txt"), source.join("folder")];
        let r = transfer(&paths, &dest, false, true).unwrap();
        assert!(r.complete(), "{r:?}");
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"first");
        assert_eq!(
            std::fs::read(dest.join("folder/child.txt")).unwrap(),
            b"nested"
        );
        // Collision creates a second copy rather than overwriting either existing file.
        std::fs::write(source.join("a.txt"), b"second").unwrap();
        let r = transfer(&[source.join("a.txt")], &dest, false, true).unwrap();
        assert!(r.complete(), "{r:?}");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 3);
        assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"first");
        assert!(
            std::fs::read_dir(&dest)
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e.path().is_file() && std::fs::read(e.path()).unwrap() == b"second")
        );
        let plan = vec![Rename {
            source: dest.join("a.txt"),
            name: "새 이름.txt".into(),
        }];
        let r = rename(&plan, true).unwrap();
        assert!(r.complete(), "{r:?}");
        assert!(!dest.join("a.txt").exists());
        assert_eq!(std::fs::read(dest.join("새 이름.txt")).unwrap(), b"first");
        let r = transfer(&[dest.join("새 이름.txt")], &moved, true, true).unwrap();
        assert!(r.complete(), "{r:?}");
        assert!(!dest.join("새 이름.txt").exists());
        assert_eq!(std::fs::read(moved.join("새 이름.txt")).unwrap(), b"first");
        assert!(transfer(std::slice::from_ref(&source), &source, false, true).is_err());
        assert!(transfer(&[moved.join("新不存在.txt")], &dest, false, true).is_err());
        let nested = vec![
            Rename {
                source: source.join("folder"),
                name: "renamed".into(),
            },
            Rename {
                source: source.join("folder/child.txt"),
                name: "other.txt".into(),
            },
        ];
        assert!(rename(&nested, true).is_err());
        assert!(source.join("folder/child.txt").exists());
    }
}
