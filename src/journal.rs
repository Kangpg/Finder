//! Explicit advanced settings for an existing NTFS USN journal. No delete/disable/recreate path.
use serde::Serialize;
use std::{
    fmt,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Invalid(crate::i18n::Message),
    AccessDenied,
    Unavailable {
        code: i32,
    },
    Changed,
    System {
        operation: crate::i18n::Message,
        code: i32,
    },
    Backup(crate::i18n::Message),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s)=>f.write_str(s),
            Self::AccessDenied=>f.write_str(crate::i18n::text("USN 저널 접근 권한이 없습니다. 프로그램을 관리자 권한으로 실행한 뒤 다시 조회하세요 (Windows 5)", "Access to the USN journal was denied. Run as administrator and query again (Windows 5)")),
            Self::Unavailable{code}=>write!(f,"{}", crate::trf!("사용 가능한 기존 USN 저널이 없습니다 (Windows {code}). 저널 생성·삭제는 수행하지 않습니다", "No existing USN journal is available (Windows {code}). No journal was created or deleted")),
            Self::Changed=>f.write_str(crate::i18n::text("조회 이후 볼륨·저널 또는 크기 설정이 바뀌었습니다. 다시 조회한 뒤 적용하세요", "The volume, journal, or size settings changed. Query again before applying")),
            Self::System{operation,code}=>write!(f,"{}", crate::trf!("{operation} 실패 (Windows {code})", "{operation} failed (Windows {code})")),
            Self::Backup(s)=>write!(f,"{}", crate::trf!("기존 설정 백업 실패: {s}", "Could not back up previous settings: {s}")),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub maximum: u64,
    pub delta: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub drive: char,
    pub volume: String,
    pub serial: u64,
    pub cluster_bytes: u32,
    pub volume_bytes: u64,
    pub journal_id: u64,
    pub first_usn: i64,
    pub next_usn: i64,
    pub lowest_valid_usn: i64,
    pub limits: Limits,
}
impl Snapshot {
    fn same_journal(&self, other: &Self) -> bool {
        self.volume == other.volume
            && self.serial == other.serial
            && self.journal_id == other.journal_id
            && self.cluster_bytes == other.cluster_bytes
    }
    fn same_settings(&self, other: &Self) -> bool {
        self.same_journal(other) && self.limits == other.limits
    }
}
impl Limits {
    pub fn validate(self, snapshot: &Snapshot) -> Result<()> {
        let cluster = u64::from(snapshot.cluster_bytes);
        if cluster == 0 || self.delta == 0 || self.maximum <= self.delta {
            return Err(Error::Invalid(crate::i18n::Message::new(
                "최대 크기는 할당 변화폭보다 커야 하며, 두 값은 0보다 커야 합니다",
                "Maximum size must exceed allocation delta, and both values must be positive",
            )));
        }
        if !self.maximum.is_multiple_of(cluster) || !self.delta.is_multiple_of(cluster) {
            return Err(Error::Invalid(crate::msgf!(
                "두 크기는 클러스터 크기 {} B의 배수여야 합니다",
                "Both sizes must be multiples of the cluster size ({} B)",
                cluster
            )));
        }
        if self.maximum > snapshot.volume_bytes {
            return Err(Error::Invalid(crate::i18n::Message::new(
                "저널 최대 크기는 볼륨 전체 용량을 넘을 수 없습니다",
                "Journal maximum size cannot exceed volume capacity",
            )));
        }
        Ok(())
    }
}
pub struct Report {
    pub requested: Limits,
    pub observed: Result<Snapshot>,
    pub backup: Option<PathBuf>,
}
trait Device {
    fn query(&mut self) -> Result<Snapshot>;
    fn set(&mut self, limits: Limits) -> Result<()>;
}
fn apply_on(
    device: &mut impl Device,
    expected: &Snapshot,
    limits: Limits,
    backup_dir: &Path,
) -> Result<Report> {
    let before = device.query()?;
    if !before.same_settings(expected) {
        return Err(Error::Changed);
    }
    limits.validate(&before)?;
    if limits == before.limits {
        return Ok(Report {
            requested: limits,
            observed: Ok(before),
            backup: None,
        });
    }
    std::fs::create_dir_all(backup_dir)
        .map_err(|e| Error::Backup(crate::i18n::Message::error(e)))?;
    let mut file = tempfile::Builder::new()
        .prefix(&format!("usn-{}-", before.drive))
        .suffix(".json")
        .tempfile_in(backup_dir)
        .map_err(|e| Error::Backup(crate::i18n::Message::error(e)))?;
    #[derive(Serialize)]
    struct Backup<'a> {
        before: &'a Snapshot,
        requested: Limits,
    }
    serde_json::to_writer_pretty(
        file.as_file_mut(),
        &Backup {
            before: &before,
            requested: limits,
        },
    )
    .map_err(|e| Error::Backup(crate::i18n::Message::error(e)))?;
    file.as_file()
        .sync_all()
        .map_err(|e| Error::Backup(crate::i18n::Message::error(e)))?;
    let (_, path) = file
        .keep()
        .map_err(|e| Error::Backup(crate::i18n::Message::error(e)))?;
    if !device.query()?.same_settings(&before) {
        return Err(Error::Changed);
    }
    device.set(limits)?;
    // IOCTL success is retained even if the follow-up query fails or NTFS defers size changes.
    Ok(Report {
        requested: limits,
        observed: device.query().and_then(|after| {
            if after.same_journal(&before) {
                Ok(after)
            } else {
                Err(Error::Changed)
            }
        }),
        backup: Some(path),
    })
}
#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::c_void,
        mem::size_of,
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Storage::FileSystem::*,
        System::{IO::DeviceIoControl, Ioctl::*},
    };
    fn os_error(operation: crate::i18n::Message) -> Error {
        let code = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
        match code {
            x if x == ERROR_ACCESS_DENIED as i32 => Error::AccessDenied,
            x if x == ERROR_JOURNAL_NOT_ACTIVE as i32
                || x == ERROR_JOURNAL_DELETE_IN_PROGRESS as i32 =>
            {
                Error::Unavailable { code }
            }
            _ => Error::System { operation, code },
        }
    }
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn ioctl(
        handle: HANDLE,
        operation: crate::i18n::Message,
        code: u32,
        input: *const c_void,
        input_size: u32,
        output: &mut [u8],
    ) -> Result<usize> {
        let mut returned = 0;
        if unsafe {
            DeviceIoControl(
                handle,
                code,
                input,
                input_size,
                if output.is_empty() {
                    null_mut()
                } else {
                    output.as_mut_ptr().cast()
                },
                output.len() as u32,
                &mut returned,
                null_mut(),
            )
        } == 0
        {
            return Err(os_error(operation));
        }
        if returned as usize > output.len() {
            return Err(Error::Invalid(crate::i18n::Message::new(
                "볼륨 응답 길이 오류",
                "Invalid volume response length",
            )));
        }
        Ok(returned as usize)
    }
    pub(crate) fn read_journal(handle: HANDLE) -> Result<USN_JOURNAL_DATA_V0> {
        let mut bytes = [0u8; size_of::<USN_JOURNAL_DATA_V0>()];
        let n = ioctl(
            handle,
            crate::i18n::Message::new("USN 저널 조회", "Query USN journal"),
            FSCTL_QUERY_USN_JOURNAL,
            null(),
            0,
            &mut bytes,
        )?;
        if n < bytes.len() {
            return Err(Error::Invalid(crate::i18n::Message::new(
                "USN 저널 응답이 잘렸습니다",
                "Truncated USN journal response",
            )));
        }
        // Complete fixed-size Win32 POD; byte storage may be unaligned.
        Ok(unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) })
    }
    pub struct Volume {
        handle: Handle,
        drive: char,
        volume: String,
    }
    impl Volume {
        fn open(drive: char, volume: String, write: bool) -> Result<Self> {
            let guid = volume
                .strip_prefix(r"\\?\Volume{")
                .and_then(|s| s.strip_suffix("}\\"));
            let valid = guid.is_some_and(|s| {
                s.len() == 36
                    && s.bytes().enumerate().all(|(i, b)| {
                        if [8, 13, 18, 23].contains(&i) {
                            b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }
                    })
            });
            if !valid {
                return Err(Error::Invalid(crate::i18n::Message::new(
                    "잘못된 볼륨 식별자",
                    "Invalid volume identifier",
                )));
            }
            let path = wide(volume.trim_end_matches('\\'));
            let access = if write {
                GENERIC_READ | GENERIC_WRITE
            } else {
                GENERIC_READ
            };
            let handle = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    null(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(os_error(crate::i18n::Message::new(
                    "NTFS 볼륨 열기",
                    "Open NTFS volume",
                )));
            }
            Ok(Self {
                handle: Handle(handle),
                drive,
                volume,
            })
        }
    }
    impl Device for Volume {
        fn query(&mut self) -> Result<Snapshot> {
            let mut bytes = [0u8; size_of::<NTFS_VOLUME_DATA_BUFFER>()];
            let n = ioctl(
                self.handle.0,
                crate::i18n::Message::new("NTFS 볼륨 정보 조회", "Query NTFS volume information"),
                FSCTL_GET_NTFS_VOLUME_DATA,
                null(),
                0,
                &mut bytes,
            )?;
            if n < bytes.len() {
                return Err(Error::Invalid(crate::i18n::Message::new(
                    "NTFS 볼륨 응답이 잘렸습니다",
                    "Truncated NTFS volume response",
                )));
            }
            let data: NTFS_VOLUME_DATA_BUFFER =
                unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
            let volume_bytes = u64::try_from(data.TotalClusters)
                .ok()
                .and_then(|n| n.checked_mul(u64::from(data.BytesPerCluster)))
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    Error::Invalid(crate::i18n::Message::new(
                        "NTFS 볼륨 용량 오류",
                        "Invalid NTFS volume capacity",
                    ))
                })?;
            let journal = read_journal(self.handle.0)?;
            Ok(Snapshot {
                drive: self.drive,
                volume: self.volume.clone(),
                serial: data.VolumeSerialNumber as u64,
                cluster_bytes: data.BytesPerCluster,
                volume_bytes,
                journal_id: journal.UsnJournalID,
                first_usn: journal.FirstUsn,
                next_usn: journal.NextUsn,
                lowest_valid_usn: journal.LowestValidUsn,
                limits: Limits {
                    maximum: journal.MaximumSize,
                    delta: journal.AllocationDelta,
                },
            })
        }
        fn set(&mut self, limits: Limits) -> Result<()> {
            let request = CREATE_USN_JOURNAL_DATA {
                MaximumSize: limits.maximum,
                AllocationDelta: limits.delta,
            };
            ioctl(
                self.handle.0,
                crate::i18n::Message::new("USN 저널 크기 변경", "Change USN journal size"),
                FSCTL_CREATE_USN_JOURNAL,
                (&request as *const CREATE_USN_JOURNAL_DATA).cast(),
                size_of::<CREATE_USN_JOURNAL_DATA>() as u32,
                &mut [],
            )?;
            Ok(())
        }
    }
    fn resolve(drive: char) -> Result<(char, String)> {
        if !drive.is_ascii_alphabetic() {
            return Err(Error::Invalid(crate::i18n::Message::new(
                "드라이브 문자를 입력하세요 (예: D:)",
                "Enter a drive letter (for example D:)",
            )));
        }
        let drive = drive.to_ascii_uppercase();
        let root = wide(&format!("{drive}:\\"));
        let mut name = [0u16; 64];
        if unsafe {
            GetVolumeNameForVolumeMountPointW(root.as_ptr(), name.as_mut_ptr(), name.len() as u32)
        } == 0
        {
            return Err(os_error(crate::i18n::Message::new(
                "볼륨 식별자 조회",
                "Query volume identifier",
            )));
        }
        let len = name.iter().position(|c| *c == 0).ok_or_else(|| {
            Error::Invalid(crate::i18n::Message::new(
                "볼륨 식별자 길이 오류",
                "Invalid volume identifier length",
            ))
        })?;
        let volume = String::from_utf16(&name[..len]).map_err(|_| {
            Error::Invalid(crate::i18n::Message::new(
                "볼륨 식별자 인코딩 오류",
                "Invalid volume identifier encoding",
            ))
        })?;
        Ok((drive, volume))
    }
    pub fn query(drive: char) -> Result<Snapshot> {
        let (drive, volume) = resolve(drive)?;
        Volume::open(drive, volume, false)?.query()
    }
    pub fn apply(expected: &Snapshot, limits: Limits, backup_dir: &Path) -> Result<Report> {
        limits.validate(expected)?;
        if resolve(expected.drive)?.1 != expected.volume {
            return Err(Error::Changed);
        }
        let mut volume = Volume::open(expected.drive, expected.volume.clone(), true)?;
        apply_on(&mut volume, expected, limits, backup_dir)
    }
}
#[cfg(windows)]
pub(crate) use native::read_journal;
#[cfg(windows)]
pub use native::{apply, query};
#[cfg(not(windows))]
pub fn query(_: char) -> Result<Snapshot> {
    Err(Error::Invalid(crate::i18n::Message::new(
        "USN 저널 설정은 Windows NTFS에서 사용할 수 있습니다",
        "USN journal settings are available on Windows NTFS volumes",
    )))
}
#[cfg(not(windows))]
pub fn apply(_: &Snapshot, _: Limits, _: &Path) -> Result<Report> {
    Err(Error::Invalid(crate::i18n::Message::new(
        "USN 저널 설정은 Windows NTFS에서 사용할 수 있습니다",
        "USN journal settings are available on Windows NTFS volumes",
    )))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            drive: 'D',
            volume: r"\\?\Volume{00000000-0000-0000-0000-000000000000}\".into(),
            serial: 1,
            cluster_bytes: 4096,
            volume_bytes: 1 << 40,
            journal_id: 7,
            first_usn: 10,
            next_usn: 100,
            lowest_valid_usn: 0,
            limits: Limits {
                maximum: 32 << 20,
                delta: 8 << 20,
            },
        }
    }
    struct Fake {
        state: Snapshot,
        writes: usize,
        fail_write: bool,
        fail_after: bool,
    }
    impl Device for Fake {
        fn query(&mut self) -> Result<Snapshot> {
            if self.fail_after && self.writes > 0 {
                Err(Error::AccessDenied)
            } else {
                Ok(self.state.clone())
            }
        }
        fn set(&mut self, limits: Limits) -> Result<()> {
            self.writes += 1;
            if self.fail_write {
                Err(Error::AccessDenied)
            } else {
                self.state.limits = limits;
                Ok(())
            }
        }
    }
    #[test]
    fn changing_existing_limits_requires_identity_freshness_validation_and_durable_backup() {
        let expected = snapshot();
        let dir = tempfile::tempdir().unwrap();
        let limits = Limits {
            maximum: 128 << 20,
            delta: 8 << 20,
        };
        for field in 0..4 {
            let mut fake = Fake {
                state: expected.clone(),
                writes: 0,
                fail_write: false,
                fail_after: false,
            };
            match field {
                0 => fake.state.serial += 1,
                1 => fake.state.journal_id += 1,
                2 => fake.state.volume.push('x'),
                _ => fake.state.limits.maximum *= 2,
            }
            assert_eq!(
                apply_on(&mut fake, &expected, limits, dir.path()).err(),
                Some(Error::Changed)
            );
            assert_eq!(fake.writes, 0);
        }
        let mut fake = Fake {
            state: expected.clone(),
            writes: 0,
            fail_write: false,
            fail_after: false,
        };
        let report = apply_on(&mut fake, &expected, limits, dir.path()).unwrap();
        assert_eq!(report.observed.unwrap().limits, limits);
        assert_eq!(fake.writes, 1);
        let backup = report.backup.unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&backup).unwrap()).unwrap();
        assert_eq!(json["before"]["limits"]["maximum"], 32 << 20);
        assert_eq!(json["requested"]["maximum"], 128 << 20);
        let mut fake = Fake {
            state: expected.clone(),
            writes: 0,
            fail_write: false,
            fail_after: false,
        };
        assert!(
            apply_on(&mut fake, &expected, expected.limits, dir.path())
                .unwrap()
                .backup
                .is_none()
        );
        assert_eq!(fake.writes, 0);
        let invalid = Limits {
            maximum: 1024,
            delta: 1024,
        };
        assert!(apply_on(&mut fake, &expected, invalid, dir.path()).is_err());
        assert_eq!(fake.writes, 0);
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, b"untouched").unwrap();
        assert!(matches!(
            apply_on(&mut fake, &expected, limits, &file),
            Err(Error::Backup(_))
        ));
        assert_eq!(fake.writes, 0);
        fake.fail_write = true;
        assert_eq!(
            apply_on(&mut fake, &expected, limits, dir.path()).err(),
            Some(Error::AccessDenied)
        );
        assert!(backup.exists());
        assert_eq!(std::fs::read(file).unwrap(), b"untouched");
    }
    #[test]
    fn successful_request_retains_applied_fact_when_requery_fails() {
        let expected = snapshot();
        let mut fake = Fake {
            state: expected.clone(),
            writes: 0,
            fail_write: false,
            fail_after: true,
        };
        let dir = tempfile::tempdir().unwrap();
        let report = apply_on(
            &mut fake,
            &expected,
            Limits {
                maximum: 128 << 20,
                delta: 8 << 20,
            },
            dir.path(),
        )
        .unwrap();
        assert!(matches!(report.observed, Err(Error::AccessDenied)));
        assert!(report.backup.unwrap().exists());
        assert_eq!(fake.writes, 1);
        for invalid in [
            Limits {
                maximum: 128 << 20,
                delta: 0,
            },
            Limits {
                maximum: 128 << 20,
                delta: 1,
            },
            Limits {
                maximum: 1 << 41,
                delta: 8 << 20,
            },
        ] {
            assert!(invalid.validate(&expected).is_err());
        }
    }
}
