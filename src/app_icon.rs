//! One logo source for the native window and notification area.
const SIZE: u32 = 32;
pub fn data() -> eframe::egui::IconData {
    eframe::egui::IconData {
        rgba: crate::icon_art::rgba(SIZE),
        width: SIZE,
        height: SIZE,
    }
}
#[cfg(windows)]
pub struct Native(windows_sys::Win32::UI::WindowsAndMessaging::HICON);
#[cfg(windows)]
impl Native {
    pub fn new() -> anyhow::Result<Self> {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let dib = crate::icon_art::dib(SIZE);
        let handle = unsafe {
            CreateIconFromResourceEx(
                dib.as_ptr(),
                dib.len() as u32,
                1,
                0x30000,
                SIZE as i32,
                SIZE as i32,
                LR_DEFAULTCOLOR,
            )
        };
        anyhow::ensure!(
            !handle.is_null(),
            crate::msgf!(
                "앱 아이콘 생성 실패: {}",
                "Could not create app icon: {}",
                std::io::Error::last_os_error()
            )
        );
        Ok(Self(handle))
    }
    pub fn handle(&self) -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
        self.0
    }
}
#[cfg(windows)]
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(self.0);
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn native_icon_and_window_use_one_logo_and_release_handles() {
        let data = super::data();
        assert_eq!(data.rgba.len(), 32 * 32 * 4);
        assert_eq!(&data.rgba[0..4], &[0, 0, 0, 0]);
        let center = (16 * 32 + 16) * 4;
        assert_eq!(&data.rgba[center..center + 4], &[37, 65, 99, 255]);
        #[cfg(windows)]
        {
            use windows_sys::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};
            for _ in 0..32 {
                let native = super::Native::new().unwrap();
                let mut info: ICONINFO = unsafe { std::mem::zeroed() };
                assert_ne!(unsafe { GetIconInfo(native.handle(), &mut info) }, 0);
                assert_ne!(info.fIcon, 0);
                unsafe {
                    DeleteObject(info.hbmMask);
                    DeleteObject(info.hbmColor);
                }
            }
        }
    }
}
