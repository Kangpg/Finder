//! CF_HDROP + Preferred DropEffect interoperability with Explorer.
//! Owned global memory is released unless ownership transfers to Windows.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};
use windows_sys::Win32::{
    Foundation::{GlobalFree, HGLOBAL, HWND},
    System::{DataExchange::*, Memory::*},
    UI::{Shell::DragQueryFileW, WindowsAndMessaging::*},
};
const CF_HDROP: u32 = 15;
const MAX_BYTES: usize = 8 * 1024 * 1024;
pub use crate::fileops::Clipboard as Files;
struct Open;
impl Open {
    fn new(owner: HWND) -> Result<Self> {
        ensure!(
            unsafe { OpenClipboard(owner) } != 0,
            crate::i18n::Message::new(
                "클립보드를 다른 프로그램이 사용 중입니다",
                "Another application is using the clipboard"
            )
        );
        Ok(Self)
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}
struct Memory(HGLOBAL);
impl Memory {
    fn new(bytes: &[u8]) -> Result<Self> {
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len()) };
        ensure!(
            !handle.is_null(),
            crate::i18n::Message::new(
                "클립보드 메모리 할당 실패",
                "Could not allocate clipboard memory"
            )
        );
        let owned = Self(handle);
        let ptr = unsafe { GlobalLock(handle) };
        ensure!(
            !ptr.is_null(),
            crate::i18n::Message::new(
                "클립보드 메모리 잠금 실패",
                "Could not lock clipboard memory"
            )
        );
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
            GlobalUnlock(handle)
        };
        Ok(owned)
    }
    fn publish(mut self, format: u32) -> Result<()> {
        ensure!(
            !unsafe { SetClipboardData(format, self.0) }.is_null(),
            crate::i18n::Message::new("클립보드 쓰기 실패", "Could not write to the clipboard")
        );
        self.0 = std::ptr::null_mut();
        Ok(())
    }
}
impl Drop for Memory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { GlobalFree(self.0) };
        }
    }
}
pub struct Owner {
    window: HWND,
    last_sequence: Option<u32>,
}
impl Owner {
    pub fn new() -> Result<Self> {
        let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        ensure!(
            !hwnd.is_null(),
            crate::i18n::Message::new(
                "클립보드 소유 창 생성 실패",
                "Could not create clipboard owner window"
            )
        );
        Ok(Self {
            window: hwnd,
            last_sequence: None,
        })
    }
    pub fn set(&mut self, paths: &[PathBuf], cut: bool, expected_sequence: u32) -> Result<()> {
        let bytes = encode(paths)?;
        let block = Memory::new(&bytes)?;
        let effect = Memory::new(&(if cut { 2u32 } else { 1u32 }).to_le_bytes())?;
        let format = effect_format()?;
        let _open = Open::new(self.window)?;
        let current = sequence();
        ensure!(
            current == expected_sequence || self.last_sequence == Some(current),
            crate::i18n::Message::new(
                "이후에 클립보드가 변경되어 이전 파일 복사를 취소했습니다",
                "The clipboard changed; the previous file copy was canceled"
            )
        );
        ensure!(
            unsafe { EmptyClipboard() } != 0,
            crate::i18n::Message::new("클립보드 초기화 실패", "Could not initialize clipboard")
        );
        // Publish the effect first: if the file list fails, no actionable cut list is exposed.
        effect.publish(format)?;
        block.publish(CF_HDROP)?;
        self.last_sequence = Some(sequence());
        Ok(())
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.window) };
    }
}
pub fn pump_messages() {
    let mut message = unsafe { std::mem::zeroed() };
    while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message)
        };
    }
}
fn effect_format() -> Result<u32> {
    let name: Vec<u16> = "Preferred DropEffect"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
    ensure!(
        format != 0,
        crate::i18n::Message::new(
            "파일 클립보드 형식 등록 실패",
            "Could not register file clipboard format"
        )
    );
    Ok(format)
}
pub fn read() -> Result<Files> {
    let _open = Open::new(std::ptr::null_mut())?;
    let handle = unsafe { GetClipboardData(CF_HDROP) };
    ensure!(
        !handle.is_null(),
        crate::i18n::Message::new(
            "클립보드에 복사된 파일/폴더가 없습니다",
            "No files or folders on the clipboard"
        )
    );
    let paths = decode(handle)?;
    let effect = unsafe { GetClipboardData(effect_format()?) };
    let mut cut = false;
    if !effect.is_null() {
        ensure!(
            unsafe { GlobalSize(effect) } >= 4,
            crate::i18n::Message::new(
                "파일 클립보드 효과 값이 손상됐습니다",
                "The clipboard file effect is corrupt"
            )
        );
        let ptr = unsafe { GlobalLock(effect) };
        ensure!(
            !ptr.is_null(),
            crate::i18n::Message::new("클립보드 효과 읽기 실패", "Could not read clipboard effect")
        );
        let value = unsafe { std::ptr::read_unaligned(ptr.cast::<u32>()) };
        unsafe { GlobalUnlock(effect) };
        ensure!(
            value == 1 || value == 2,
            crate::msgf!(
                "복사/이동 외 클립보드 효과는 지원하지 않습니다: {value}",
                "Unsupported clipboard effect (expected copy or move): {value}"
            )
        );
        cut = value == 2;
    }
    Ok(Files {
        paths,
        cut,
        sequence: unsafe { GetClipboardSequenceNumber() },
    })
}
pub fn encode(paths: &[PathBuf]) -> Result<Vec<u8>> {
    ensure!(
        !paths.is_empty() && paths.len() <= crate::fileops::MAX_ITEMS,
        crate::i18n::Message::new(
            "클립보드 파일은 1~5000개여야 합니다",
            "Select between 1 and 5,000 clipboard files"
        )
    );
    let mut names = Vec::new();
    let mut size = 22usize;
    for path in paths {
        ensure!(
            path.is_absolute(),
            crate::i18n::Message::new("절대 경로가 필요합니다", "An absolute path is required")
        );
        let wide: Vec<u16> = dunce::simplified(path).as_os_str().encode_wide().collect();
        ensure!(
            !wide.contains(&0) && wide.len() <= 32767,
            crate::i18n::Message::new(
                "지원하지 않는 파일 경로 길이/NUL",
                "Unsupported path length or embedded NUL"
            )
        );
        size = size
            .checked_add((wide.len() + 1) * 2)
            .context(crate::i18n::Message::new(
                "클립보드 크기 초과",
                "Clipboard size limit exceeded",
            ))?;
        ensure!(
            size <= MAX_BYTES,
            crate::i18n::Message::new("파일 경로 목록이 8 MB를 넘습니다", "File paths exceed 8 MB")
        );
        names.push(wide);
    }
    let mut bytes = Vec::with_capacity(size);
    bytes.resize(20, 0);
    bytes[0..4].copy_from_slice(&20u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    for name in names {
        for c in name.into_iter().chain(Some(0)) {
            bytes.extend(c.to_le_bytes());
        }
    }
    bytes.extend([0, 0]);
    Ok(bytes)
}
pub(crate) fn decode(handle: HGLOBAL) -> Result<Vec<PathBuf>> {
    let size = unsafe { GlobalSize(handle) };
    ensure!(
        (20..=MAX_BYTES).contains(&size),
        crate::i18n::Message::new(
            "클립보드 파일 목록 크기 오류",
            "Invalid clipboard file list size"
        )
    );
    // Validate the complete owned block before invoking the Shell's HDROP reader.
    let ptr = unsafe { GlobalLock(handle) };
    ensure!(
        !ptr.is_null(),
        crate::i18n::Message::new(
            "클립보드 읽기 잠금 실패",
            "Could not lock clipboard for reading"
        )
    );
    let validation = (|| {
        let b = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), size) };
        let offset = u32::from_le_bytes(b[0..4].try_into().unwrap()) as usize;
        let unicode = u32::from_le_bytes(b[16..20].try_into().unwrap()) != 0;
        ensure!(
            offset >= 20 && offset < size,
            crate::i18n::Message::new("클립보드 경로 오프셋 오류", "Invalid clipboard path offset")
        );
        if unicode {
            ensure!(
                offset.is_multiple_of(2) && size - offset >= 4,
                crate::i18n::Message::new(
                    "UTF-16 파일 목록 경계 오류",
                    "Invalid UTF-16 file list boundary"
                )
            );
            let mut previous_zero = false;
            let mut terminated = false;
            for unit in b[offset..].chunks_exact(2) {
                let zero = unit == [0, 0];
                if zero && previous_zero {
                    terminated = true;
                    break;
                }
                previous_zero = zero;
            }
            ensure!(
                terminated,
                crate::i18n::Message::new(
                    "UTF-16 파일 목록 종료 오류",
                    "Invalid UTF-16 file list terminator"
                )
            );
        } else {
            ensure!(
                size - offset >= 2 && b[offset..].windows(2).any(|w| w == [0, 0]),
                crate::i18n::Message::new("파일 목록 종료 오류", "Invalid file list terminator")
            );
        }
        Ok(())
    })();
    unsafe { GlobalUnlock(handle) };
    validation?;
    let count = unsafe { DragQueryFileW(handle, u32::MAX, std::ptr::null_mut(), 0) } as usize;
    ensure!(
        (1..=crate::fileops::MAX_ITEMS).contains(&count),
        crate::i18n::Message::new(
            "클립보드 파일 수는 1~5000개여야 합니다",
            "The clipboard must contain 1–5,000 files"
        )
    );
    let mut paths = Vec::with_capacity(count);
    let mut total = 0usize;
    for n in 0..count {
        let length = unsafe { DragQueryFileW(handle, n as u32, std::ptr::null_mut(), 0) } as usize;
        ensure!(
            (1..=32767).contains(&length),
            crate::i18n::Message::new("클립보드 경로 길이 오류", "Invalid clipboard path length")
        );
        total += (length + 1) * 2;
        ensure!(
            total <= MAX_BYTES,
            crate::i18n::Message::new(
                "클립보드 경로 총 크기 초과",
                "Clipboard path size limit exceeded"
            )
        );
        let mut wide = vec![0; length + 1];
        let actual =
            unsafe { DragQueryFileW(handle, n as u32, wide.as_mut_ptr(), wide.len() as u32) };
        ensure!(
            actual as usize == length,
            crate::i18n::Message::new("클립보드 경로 읽기 실패", "Could not read clipboard paths")
        );
        wide.truncate(length);
        let path = PathBuf::from(OsString::from_wide(&wide));
        ensure!(
            path.is_absolute(),
            crate::i18n::Message::new(
                "클립보드에 상대 경로가 있습니다",
                "The clipboard contains a relative path"
            )
        );
        paths.push(path);
    }
    Ok(paths)
}
pub fn clear_if_current(sequence: u32) -> Result<()> {
    let _open = Open::new(std::ptr::null_mut())?;
    if unsafe { GetClipboardSequenceNumber() } == sequence {
        ensure!(
            unsafe { EmptyClipboard() } != 0,
            crate::i18n::Message::new("클립보드 정리 실패", "Could not clear clipboard")
        );
    }
    Ok(())
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hdrop_roundtrip_uses_real_shell_decoder_without_touching_clipboard() {
        let paths = vec![
            PathBuf::from(r"C:\한국 자료\a.txt"),
            PathBuf::from(r"D:\folder"),
        ];
        let mem = Memory::new(&encode(&paths).unwrap()).unwrap();
        assert_eq!(decode(mem.0).unwrap(), paths);
        let mut malformed = encode(&paths).unwrap();
        malformed[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        let mem = Memory::new(&malformed).unwrap();
        assert!(decode(mem.0).is_err());
        assert!(encode(&[PathBuf::from("relative.txt")]).is_err());
    }
}
