//! Filesystem parsing happens on the service STA. Only the native drag loop runs on the window STA.
use anyhow::Result;
use windows::Win32::{
    System::Ole::*,
    UI::Shell::{self},
};
pub struct Apartment;
impl Apartment {
    pub fn initialize() -> Result<Self> {
        unsafe { OleInitialize(None) }?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}
pub fn run(prepared: &crate::shell_path::Selection, window: isize) -> Result<u32> {
    let _ole = Apartment::initialize()?;
    let data = prepared.data_object()?;
    // The Shell owns optimized moves. Never delete source paths again on return.
    let effect = unsafe {
        Shell::SHDoDragDrop(
            Some(windows::Win32::Foundation::HWND(window as *mut _)),
            &data,
            None,
            DROPEFFECT_COPY | DROPEFFECT_MOVE,
        )
    }?;
    Ok(effect.0)
}
pub fn cursor(window: isize, scale: f32) -> Option<(f32, f32)> {
    let mut point = windows::Win32::Foundation::POINT::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point) }.ok()?;
    if !unsafe {
        windows::Win32::Graphics::Gdi::ScreenToClient(
            windows::Win32::Foundation::HWND(window as *mut _),
            &mut point,
        )
    }
    .as_bool()
    {
        return None;
    }
    Some((point.x as f32 / scale, point.y as f32 / scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    #[test]
    fn mixed_parent_shell_object_exposes_real_file_paths_without_using_clipboard() {
        unsafe { OleInitialize(None) }.unwrap();
        struct Ole;
        impl Drop for Ole {
            fn drop(&mut self) {
                unsafe { OleUninitialize() };
            }
        }
        let _ole = Ole;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("다른 폴더")).unwrap();
        let paths = vec![
            dir.path().join("한글.txt"),
            dir.path().join("다른 폴더/two.bin"),
        ];
        for path in &paths {
            std::fs::write(path, b"data").unwrap();
        }
        let epoch = AtomicU64::new(3);
        let prepared = crate::shell_path::prepare(&paths, &epoch, 3).unwrap();
        let object = prepared.data_object().unwrap();
        use windows::Win32::System::Com::*;
        let format = FORMATETC {
            cfFormat: 15,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut medium = unsafe { object.GetData(&format) }.unwrap();
        let actual = crate::file_clipboard::decode(unsafe { medium.u.hGlobal.0 }).unwrap();
        unsafe { ReleaseStgMedium(&mut medium) };
        assert_eq!(
            actual,
            paths
                .iter()
                .map(|p| dunce::simplified(p).to_path_buf())
                .collect::<Vec<_>>()
        );
        epoch.store(4, Ordering::Relaxed);
        assert!(crate::shell_path::prepare(&paths, &epoch, 3).is_err());
    }
}
