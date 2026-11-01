//! Read-only NTFS enumeration and journal replay. Never creates/changes a journal.
use crate::{
    index::{Index, Source},
    usn,
};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_HANDLE_EOF, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, OPEN_EXISTING,
    },
    System::{
        IO::DeviceIoControl,
        Ioctl::{
            FSCTL_ENUM_USN_DATA, FSCTL_READ_USN_JOURNAL, MFT_ENUM_DATA_V0,
            READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
        },
    },
};
struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Volume {
    handle: OwnedHandle,
    serial: u32,
    root_frn: u64,
    drive: char,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn open(path: &str, access: u32, flags: u32) -> Result<OwnedHandle> {
    let path = wide(path);
    // SAFETY: NUL-terminated path lives through the synchronous call; no security/template pointer.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            flags,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(OwnedHandle(handle))
}
impl Volume {
    fn open(drive: char) -> Result<Self> {
        ensure!(drive.is_ascii_alphabetic(), "Drive must be an ASCII letter");
        let drive = drive.to_ascii_uppercase();
        let root = open(&format!("{drive}:\\"), 0, FILE_FLAG_BACKUP_SEMANTICS)?;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
        // SAFETY: info is a writable structure of the documented size; root is owned and live.
        if unsafe { GetFileInformationByHandle(root.0, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let handle=open(&format!("\\\\.\\{drive}:"),GENERIC_READ,0)
            .with_context(||crate::i18n::Message::capture(|| format!("Cannot open NTFS volume {drive}:; run as administrator for drive indexing, or use --folder")))?;
        Ok(Self {
            handle,
            serial: info.dwVolumeSerialNumber,
            root_frn: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
            drive,
        })
    }
    fn ioctl(
        &self,
        code: u32,
        input: *const c_void,
        input_size: u32,
        output: &mut [u8],
    ) -> std::io::Result<usize> {
        let mut returned = 0;
        // SAFETY: callers pass concrete initialized IOCTL structures with their exact size; output is
        // writable for its stated length. The owned synchronous handle is not closed during this call.
        let ok = unsafe {
            DeviceIoControl(
                self.handle.0,
                code,
                input,
                input_size,
                output.as_mut_ptr().cast(),
                output.len() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if returned as usize > output.len() {
            return Err(std::io::Error::other(
                "NTFS returned an invalid buffer length",
            ));
        }
        Ok(returned as usize)
    }
    fn journal(&self) -> Result<USN_JOURNAL_DATA_V0> {
        Ok(crate::journal::read_journal(self.handle.0)?)
    }
    fn check(&self, journal_id: u64, next_usn: i64) -> Result<USN_JOURNAL_DATA_V0> {
        let journal = self.journal()?;
        ensure!(
            journal.UsnJournalID == journal_id,
            "USN journal was replaced; rebuild the index"
        );
        ensure!(
            next_usn >= journal.FirstUsn
                && next_usn >= journal.LowestValidUsn
                && next_usn <= journal.NextUsn,
            "USN history no longer covers this index; rebuild the index"
        );
        Ok(journal)
    }
    fn replay(&self, index: &mut Index, epoch: Option<(&AtomicU64, u64)>) -> Result<usize> {
        let Source::Ntfs {
            drive,
            serial,
            journal_id,
            next_usn,
        } = index.source
        else {
            anyhow::bail!("Not an NTFS index")
        };
        ensure!(
            drive == self.drive && serial == self.serial && index.root_frn == self.root_frn,
            "Volume identity changed; rebuild the index"
        );
        let journal = self.check(journal_id, next_usn)?;
        let goal = journal.NextUsn;
        if next_usn < goal {
            index.invalidate_failed_metadata();
        }
        let mut cursor = next_usn;
        let mut changes = 0;
        let mut output = vec![0u8; 64 * 1024];
        while cursor < goal {
            ensure!(
                !epoch.is_some_and(|(v, n)| v.load(Ordering::Relaxed) != n),
                "Journal replay cancelled"
            );
            let request = READ_USN_JOURNAL_DATA_V0 {
                StartUsn: cursor,
                ReasonMask: u32::MAX,
                ReturnOnlyOnClose: 0,
                Timeout: 0,
                BytesToWaitFor: 0,
                UsnJournalID: journal_id,
            };
            let n = self.ioctl(
                FSCTL_READ_USN_JOURNAL,
                (&request as *const READ_USN_JOURNAL_DATA_V0).cast(),
                size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                &mut output,
            )?;
            let (next, rows) = usn::decode_batch(&output[..n])?;
            let next = i64::try_from(next).context("Negative/overflowing USN cursor")?;
            ensure!(
                next > cursor,
                "USN replay made no progress; rebuild required"
            );
            for row in rows {
                // An MFT enumeration may already contain a more recent version of this file.
                if row.usn >= cursor && index.apply(&row)? {
                    changes += 1;
                }
            }
            cursor = next;
        }
        self.check(journal_id, cursor)?;
        index.source = Source::Ntfs {
            drive,
            serial,
            journal_id,
            next_usn: cursor,
        };
        if changes > 0 {
            index.compact_if_needed()?;
            index.rebuild_order(epoch)?;
        }
        Ok(changes)
    }
}
pub fn scan(drive: char, epoch: Option<(&AtomicU64, u64)>) -> Result<Index> {
    let volume = Volume::open(drive)?;
    let before = volume.journal()?;
    let mut index = Index::new(
        Source::Ntfs {
            drive: volume.drive,
            serial: volume.serial,
            journal_id: before.UsnJournalID,
            next_usn: before.NextUsn,
        },
        volume.root_frn,
    );
    let mut request = MFT_ENUM_DATA_V0 {
        StartFileReferenceNumber: 0,
        LowUsn: 0,
        HighUsn: i64::MAX,
    };
    let mut output = vec![0u8; 64 * 1024];
    loop {
        ensure!(
            !epoch.is_some_and(|(v, n)| v.load(Ordering::Relaxed) != n),
            "Indexing cancelled"
        );
        let n = match volume.ioctl(
            FSCTL_ENUM_USN_DATA,
            (&request as *const MFT_ENUM_DATA_V0).cast(),
            size_of::<MFT_ENUM_DATA_V0>() as u32,
            &mut output,
        ) {
            Ok(n) => n,
            Err(e) if e.raw_os_error() == Some(ERROR_HANDLE_EOF as i32) => break,
            Err(e) => return Err(e).context("MFT enumeration failed"),
        };
        let (next, rows) = usn::decode_batch(&output[..n])?;
        ensure!(
            next > request.StartFileReferenceNumber,
            "MFT enumeration made no progress"
        );
        for row in rows {
            index.upsert(&row)?;
        }
        request.StartFileReferenceNumber = next;
    }
    index.rebuild_order(epoch)?;
    volume.replay(&mut index, epoch)?;
    Ok(index)
}
pub fn refresh(index: &mut Index, epoch: Option<(&AtomicU64, u64)>) -> Result<usize> {
    let Source::Ntfs { drive, .. } = index.source else {
        anyhow::bail!("Folder snapshots need a rescan")
    };
    Volume::open(drive)?.replay(index, epoch)
}
pub fn has_changes(index: &Index) -> Result<bool> {
    let Source::Ntfs {
        drive,
        serial,
        journal_id,
        next_usn,
    } = index.source
    else {
        anyhow::bail!("Folder snapshots are not live; rescan for current results")
    };
    let volume = Volume::open(drive)?;
    ensure!(
        volume.serial == serial && volume.root_frn == index.root_frn,
        "Volume identity changed"
    );
    let journal = volume.check(journal_id, next_usn)?;
    Ok(journal.NextUsn > next_usn)
}
pub fn verify_freshness(index: &Index) -> Result<()> {
    ensure!(
        !has_changes(index)?,
        "Journal contains changes after the saved snapshot; refresh to search current data"
    );
    Ok(())
}
