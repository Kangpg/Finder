//! Windows owns icon choice and overlay composition. No native handles cross threads.
use anyhow::{Context, Result, ensure};
use eframe::egui::ColorImage;
use std::{path::Path, sync::Arc};
use windows_sys::Win32::{
    Graphics::Gdi::*,
    UI::{Shell::*, WindowsAndMessaging::*},
};
const EDGE: usize = 32;
#[derive(Debug)]
pub struct Icon {
    pub image: ColorImage,
    /// Opaque Shell identifiers, never interpreted as SVN status.
    pub image_index: u32,
    pub overlay_index: u8,
}
struct OwnedIcon(HICON);
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DestroyIcon(self.0);
            }
        }
    }
}
struct Dib {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
}
impl Dib {
    fn new() -> Result<Self> {
        let mut dib = Self {
            dc: unsafe { CreateCompatibleDC(std::ptr::null_mut()) },
            bitmap: std::ptr::null_mut(),
            previous: std::ptr::null_mut(),
            bits: std::ptr::null_mut(),
        };
        ensure!(
            !dib.dc.is_null(),
            crate::i18n::Message::new("아이콘 DC 생성 실패", "Could not create icon DC")
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: EDGE as i32,
                biHeight: -(EDGE as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        dib.bitmap = unsafe {
            CreateDIBSection(
                dib.dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        dib.bits = bits.cast();
        ensure!(
            !dib.bitmap.is_null() && !dib.bits.is_null(),
            crate::i18n::Message::new("아이콘 비트맵 생성 실패", "Could not create icon bitmap")
        );
        dib.previous = unsafe { SelectObject(dib.dc, dib.bitmap) };
        ensure!(
            !dib.previous.is_null() && dib.previous as isize != -1,
            crate::i18n::Message::new("아이콘 비트맵 선택 실패", "Could not select icon bitmap")
        );
        Ok(dib)
    }
    fn draw(&mut self, icon: HICON, background: u8) -> Result<Vec<u8>> {
        // Flush GDI before both CPU writes and CPU reads of DIB storage.
        ensure!(
            unsafe { GdiFlush() } != 0,
            crate::i18n::Message::new("아이콘 GDI 동기화 실패", "Could not synchronize icon GDI")
        );
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.bits, EDGE * EDGE * 4) };
        pixels.fill(background);
        ensure!(
            unsafe {
                DrawIconEx(
                    self.dc,
                    0,
                    0,
                    icon,
                    EDGE as i32,
                    EDGE as i32,
                    0,
                    std::ptr::null_mut(),
                    DI_NORMAL,
                )
            } != 0,
            crate::i18n::Message::new("아이콘 그리기 실패", "Could not draw icon")
        );
        ensure!(
            unsafe { GdiFlush() } != 0,
            crate::i18n::Message::new("아이콘 GDI 동기화 실패", "Could not synchronize icon GDI")
        );
        Ok(pixels.to_vec())
    }
}
impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            if !self.dc.is_null() && !self.previous.is_null() && self.previous as isize != -1 {
                SelectObject(self.dc, self.previous);
            }
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            if !self.dc.is_null() {
                DeleteDC(self.dc);
            }
        }
    }
}
fn rgba(black: &[u8], white: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(EDGE * EDGE * 4);
    // Two backgrounds also support classic mask-based icons whose alpha bytes are zero.
    for (b, w) in black.chunks_exact(4).zip(white.chunks_exact(4)) {
        let transmission = (0..3).map(|c| w[c].saturating_sub(b[c])).max().unwrap();
        let alpha = 255 - transmission;
        for c in [2, 1, 0] {
            rgba.push(if alpha == 0 {
                0
            } else {
                ((b[c] as u32 * 255 + alpha as u32 / 2) / alpha as u32).min(255) as u8
            });
        }
        rgba.push(alpha);
    }
    rgba
}
pub fn read(path: &Path) -> Result<Arc<Icon>> {
    let pidl = crate::shell_path::Pidl::parse(path)?;
    let mut info = SHFILEINFOW::default();
    let result = unsafe {
        SHGetFileInfoW(
            pidl.0.cast(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_PIDL | SHGFI_ICON | SHGFI_LARGEICON | SHGFI_ADDOVERLAYS | SHGFI_OVERLAYINDEX,
        )
    };
    let icon = OwnedIcon(info.hIcon);
    ensure!(
        result != 0 && !icon.0.is_null(),
        crate::i18n::Message::new(
            "Windows 파일 아이콘 조회 실패",
            "Could not load Windows file icon"
        )
    );
    let mut dib = Dib::new()?;
    let black = dib.draw(icon.0, 0).context(crate::i18n::Message::new(
        "검정 배경 아이콘 렌더링",
        "Render icon on black background",
    ))?;
    let white = dib.draw(icon.0, 255).context(crate::i18n::Message::new(
        "흰 배경 아이콘 렌더링",
        "Render icon on white background",
    ))?;
    let pixels = rgba(&black, &white);
    Ok(Arc::new(Icon {
        image: ColorImage::from_rgba_unmultiplied([EDGE, EDGE], &pixels),
        image_index: info.iIcon as u32 & 0x00ff_ffff,
        overlay_index: (info.iIcon as u32 >> 24) as u8,
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_shell_icons_include_transparency_and_keep_handles_bounded() {
        // Shell/IFileOperation tests can initialize unrelated GDI caches concurrently.
        // Measure ownership in a dedicated test process, not the shared process count.
        if std::env::var("REA_ICON_RESOURCE_CHECK").as_deref() != Ok("isolated") {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args(["shell_icon::tests::real_shell_icons_include_transparency_and_keep_handles_bounded","--exact","--nocapture","--test-threads=1"]).env("REA_ICON_RESOURCE_CHECK","isolated");
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let _ole = crate::native_drag::Apartment::initialize().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("한글 문서.txt");
        std::fs::write(&file, b"data").unwrap();

        for path in [
            dir.path(),
            file.as_path(),
            Path::new(r"C:\Windows\explorer.exe"),
        ] {
            let icon = read(path).unwrap();
            assert_eq!(icon.image.size, [32, 32]);
            assert!(icon.image.pixels.iter().any(|c| c.a() != 0));
            assert!(icon.image.pixels.iter().any(|c| c.a() == 0));
        }
        let before = unsafe { GetGuiResources(GetCurrentProcess(), 0) };
        let before_user = unsafe { GetGuiResources(GetCurrentProcess(), 1) };
        for _ in 0..40 {
            drop(read(&file).unwrap());
        }
        let after = unsafe { GetGuiResources(GetCurrentProcess(), 0) };
        let after_user = unsafe { GetGuiResources(GetCurrentProcess(), 1) };
        assert!(
            after_user <= before_user + 20,
            "USER icon resources increased: {before_user} -> {after_user}"
        );
        assert!(
            after <= before + 20,
            "GDI resources increased: {before} -> {after}"
        );
    }
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetGuiResources};
    #[test]
    fn alpha_recovery_handles_transparent_opaque_and_semitransparent_pixels() {
        assert_eq!(rgba(&[0, 0, 0, 0], &[255, 255, 255, 0]), [0, 0, 0, 0]);
        assert_eq!(rgba(&[20, 40, 80, 0], &[20, 40, 80, 0]), [80, 40, 20, 255]);
        assert_eq!(rgba(&[0, 0, 128, 0], &[127, 127, 255, 0]), [255, 0, 0, 128]);
    }
    #[test]
    fn real_shortcut_icon_uses_the_shell_composed_overlay() {
        use windows::{
            Win32::{
                System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile},
                UI::Shell::{IShellLinkW, ShellLink},
            },
            core::{Interface, PCWSTR},
        };
        let _ole = crate::native_drag::Apartment::initialize().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"data").unwrap();
        let path = dir.path().join("바로 가기.lnk");
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let wide = crate::shell_path::wide(&target).unwrap();
        unsafe { link.SetPath(PCWSTR(wide.as_ptr())) }.unwrap();
        let persistent: IPersistFile = link.cast().unwrap();
        let wide = crate::shell_path::wide(&path).unwrap();
        unsafe { persistent.Save(PCWSTR(wide.as_ptr()), true) }.unwrap();
        drop(persistent);
        drop(link);
        assert!(std::fs::metadata(&path).unwrap().len() >= 76);
        let icon = read(&path).unwrap();
        assert!(
            icon.overlay_index > 0,
            "Shell did not provide a shortcut overlay"
        );
        assert!(icon.image.pixels.iter().any(|p| p.a() > 0));
    }
}
