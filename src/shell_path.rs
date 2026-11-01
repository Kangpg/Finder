//! Encode filesystem paths once at the Windows Shell parsing boundary.
use anyhow::{Context, Result, ensure};
use std::{
    os::windows::ffi::OsStrExt,
    path::{Component, Path},
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use windows::Win32::{
    System::Com::IDataObject,
    UI::Shell::{self},
};
pub(crate) fn wide(path: &Path) -> Result<Vec<u16>> {
    ensure!(
        path.is_absolute(),
        crate::i18n::Message::new("절대 경로가 필요합니다", "An absolute path is required")
    );
    let path = dunce::simplified(path);
    // Ordinary Windows paths accept both separators. Verbatim paths have different
    // slash semantics, so retain their exact spelling if simplification was unsafe.
    let verbatim =
        matches!(path.components().next(), Some(Component::Prefix(p)) if p.kind().is_verbatim());
    let mut wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|c| {
            if !verbatim && c == b'/' as u16 {
                b'\\' as u16
            } else {
                c
            }
        })
        .collect();
    ensure!(
        !wide.contains(&0),
        crate::i18n::Message::new("경로에 NUL 포함", "Path contains NUL")
    );
    wide.push(0);
    Ok(wide)
}
pub(crate) struct Pidl(pub *mut windows::Win32::UI::Shell::Common::ITEMIDLIST);
impl Pidl {
    pub fn parse(path: &Path) -> Result<Self> {
        let wide = wide(path)?;
        let mut pidl = Self(std::ptr::null_mut());
        unsafe {
            windows::Win32::UI::Shell::SHParseDisplayName(
                windows::core::PCWSTR(wide.as_ptr()),
                None,
                &mut pidl.0,
                0,
                None,
            )
        }?;
        ensure!(
            !pidl.0.is_null(),
            crate::i18n::Message::new(
                "Windows 파일 식별자 없음",
                "Windows item identifier is missing"
            )
        );
        Ok(pidl)
    }
}
impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(self.0.cast())) };
        }
    }
}
const MAX_BYTES: usize = 16 * 1024 * 1024;
pub struct Selection {
    pidls: Vec<Vec<u8>>,
}
pub fn prepare(paths: &[PathBuf], epoch: &AtomicU64, id: u64) -> Result<Selection> {
    ensure!(
        epoch.load(Ordering::Relaxed) == id,
        crate::i18n::Message::new(
            "Shell 선택 준비 취소",
            "Shell selection preparation canceled"
        )
    );
    // Reuse the path/count/representation budget that owns file-transfer inputs.
    let _ = crate::file_clipboard::encode(paths)?;
    let mut pidls = Vec::with_capacity(paths.len());
    let mut bytes = 0usize;
    for path in paths {
        ensure!(
            epoch.load(Ordering::Relaxed) == id,
            crate::i18n::Message::new(
                "Shell 선택 준비 취소",
                "Shell selection preparation canceled"
            )
        );
        let pidl = crate::shell_path::Pidl::parse(path).with_context(|| {
            crate::msgf!(
                "선택 파일 확인 실패: {}",
                "Could not verify selected file: {}",
                path.display()
            )
        })?;
        let size = unsafe { Shell::ILGetSize(Some(pidl.0)) } as usize;
        ensure!(
            size >= 2,
            crate::i18n::Message::new(
                "Shell 파일 식별자 길이 오류",
                "Invalid Shell item identifier length"
            )
        );
        bytes = bytes.checked_add(size).context(crate::i18n::Message::new(
            "선택 정보 크기 초과",
            "Selection data size limit exceeded",
        ))?;
        ensure!(
            bytes <= MAX_BYTES,
            crate::i18n::Message::new(
                "선택 파일 정보가 16 MB를 넘습니다",
                "Selected file data exceeds 16 MB"
            )
        );
        pidls.push(unsafe { std::slice::from_raw_parts(pidl.0.cast::<u8>(), size) }.to_vec());
    }
    ensure!(
        epoch.load(Ordering::Relaxed) == id,
        crate::i18n::Message::new(
            "Shell 선택 준비 취소",
            "Shell selection preparation canceled"
        )
    );
    Ok(Selection { pidls })
}
impl Selection {
    pub(crate) fn pointers(&self) -> Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST> {
        self.pidls.iter().map(|b| b.as_ptr().cast()).collect()
    }
    pub fn data_object(&self) -> Result<IDataObject> {
        let pointers = self.pointers();
        let items = unsafe { Shell::SHCreateShellItemArrayFromIDLists(&pointers) }?;
        Ok(unsafe { items.BindToHandler(None, &Shell::BHID_DataObject) }?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_separators_normalize_but_verbatim_identity_is_preserved() {
        assert_eq!(
            wide(Path::new(r"C:\parent/한글.txt")).unwrap(),
            "C:\\parent\\한글.txt\0".encode_utf16().collect::<Vec<_>>()
        );
        let reserved = Path::new(r"\\?\C:\parent\NUL/child");
        assert_eq!(
            wide(reserved).unwrap(),
            reserved
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        );
        assert!(wide(Path::new("relative/file")).is_err());
    }
}
