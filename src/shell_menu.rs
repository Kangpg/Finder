//! Windows owns menu contents, extension verbs and property pages.
//! The app window forwards only menu messages while its popup is active. No external hooks.
use anyhow::{Context, Result, ensure};
use std::{cell::Cell, mem::ManuallyDrop};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{Interface, PCSTR, PCWSTR, w},
};
const FIRST: u32 = 1;
const LAST: u32 = 0x6fff;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    CopyPaths,
    CopyNames,
    Rename,
}
impl Extra {
    fn command(self) -> u32 {
        match self {
            Self::CopyPaths => 0x7000,
            Self::CopyNames => 0x7001,
            Self::Rename => 0x7002,
        }
    }
    fn from_command(id: u32) -> Option<Self> {
        [Self::CopyPaths, Self::CopyNames, Self::Rename]
            .into_iter()
            .find(|x| x.command() == id)
    }
}
pub enum Outcome {
    Cancelled,
    Invoked,
    Extra(Extra),
}
struct Menu(HMENU);
impl Menu {
    fn new() -> Result<Self> {
        Ok(Self(unsafe { CreatePopupMenu() }?))
    }
    fn append(&self, flags: MENU_ITEM_FLAGS, id: usize, text: &str) -> Result<()> {
        let text: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
        unsafe { AppendMenuW(self.0, flags, id, PCWSTR(text.as_ptr())) }?;
        Ok(())
    }
}
impl Drop for Menu {
    fn drop(&mut self) {
        let _ = unsafe { DestroyMenu(self.0) };
    }
}
struct Handler {
    two: Option<IContextMenu2>,
    three: Option<IContextMenu3>,
    destroyed: Cell<bool>,
    retained: Cell<bool>,
}
impl Handler {
    fn message(&self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if !matches!(
            msg,
            WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR
        ) {
            return None;
        }
        if let Some(menu) = &self.three {
            let mut result = LRESULT(0);
            if unsafe { menu.HandleMenuMsg2(msg, wp, lp, Some(&mut result)) }.is_ok() {
                return Some(result);
            }
        }
        if msg != WM_MENUCHAR
            && let Some(menu) = &self.two
            && unsafe { menu.HandleMenuMsg(msg, wp, lp) }.is_ok()
        {
            return Some(LRESULT(isize::from(matches!(
                msg,
                WM_DRAWITEM | WM_MEASUREITEM
            ))));
        }
        None
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    reference: usize,
) -> LRESULT {
    let pointer = reference as *const Handler;
    let state = unsafe { &*pointer };
    if msg == WM_NCDESTROY {
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(window_proc), id) };
        state.destroyed.set(true);
        let retained = state.retained.get();
        let result = unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
        if retained {
            drop(unsafe { Box::from_raw(pointer as *mut Handler) });
        }
        return result;
    }
    if let Some(result) = state.message(msg, wp, lp) {
        return result;
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}
struct Host {
    window: Option<HWND>,
    state: Option<Box<Handler>>,
}
impl Host {
    fn new(context: &IContextMenu, owner: Option<HWND>) -> Result<Self> {
        let state = Box::new(Handler {
            two: context.cast().ok(),
            three: context.cast().ok(),
            destroyed: Cell::new(false),
            retained: Cell::new(false),
        });
        let reference = (&*state as *const Handler) as usize;
        if let Some(window) = owner {
            unsafe { SetWindowSubclass(window, Some(window_proc), reference, reference) }
                .ok()
                .context(crate::i18n::Message::new(
                    "앱 창의 메뉴 메시지 연결 실패",
                    "Could not attach menu messages to the app window",
                ))?;
        }
        Ok(Self {
            window: owner,
            state: Some(state),
        })
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Some(state) = self.state.take()
            && let Some(window) = self.window
            && !state.destroyed.get()
        {
            let reference = (&*state as *const Handler) as usize;
            if !unsafe { RemoveWindowSubclass(window, Some(window_proc), reference) }.as_bool() {
                // Retain callback state until WM_NCDESTROY if removal failed.
                state.retained.set(true);
                let _ = Box::into_raw(state);
            }
        }
    }
}
fn context(selection: &crate::shell_path::Selection, owner: HWND) -> Result<IContextMenu> {
    let pointers = selection.pointers();
    ensure!(
        !pointers.is_empty(),
        crate::i18n::Message::new("선택 항목이 없습니다", "No items selected")
    );
    let first = pointers[0];
    let last = unsafe { ILFindLastID(first) };
    let parent_len = last as usize - first as usize;
    let mut parent = unsafe { std::slice::from_raw_parts(first.cast::<u8>(), parent_len) }.to_vec();
    parent.extend_from_slice(&[0, 0]);
    if pointers
        .iter()
        .all(|p| unsafe { ILIsParent(parent.as_ptr().cast(), *p, true) }.as_bool())
    {
        let folder: IShellFolder = unsafe { SHBindToParent(first, None) }?;
        let children: Vec<_> = pointers
            .iter()
            .map(|p| unsafe { ILFindLastID(*p) } as *const _)
            .collect();
        return Ok(unsafe { folder.GetUIObjectOf(owner, &children, None) }?);
    }
    // Desktop is the Shell namespace root: absolute PIDLs are relative to it.
    // Keep all items in one data object; never run a multi-file command once per parent.
    let desktop = unsafe { SHGetDesktopFolder() }?;
    let mut pointers: Vec<_> = pointers.into_iter().map(|p| p as *mut _).collect();
    let mut config = DEFCONTEXTMENU {
        hwnd: owner,
        psf: ManuallyDrop::new(Some(desktop)),
        cidl: pointers.len() as u32,
        apidl: pointers.as_mut_ptr(),
        ..Default::default()
    };
    let result = unsafe { SHCreateDefaultContextMenu(&config) };
    unsafe { ManuallyDrop::drop(&mut config.psf) };
    Ok(result?)
}
struct NativeMenu {
    menu: Menu,
    context: IContextMenu,
    host: Host,
    count: u32,
}
impl NativeMenu {
    fn new(
        selection: &crate::shell_path::Selection,
        owner: Option<HWND>,
        extended: bool,
    ) -> Result<Self> {
        let context =
            context(selection, owner.unwrap_or_default()).context(crate::i18n::Message::new(
                "Windows 메뉴 가져오기 실패",
                "Could not retrieve Windows menu",
            ))?;
        let host = Host::new(&context, owner)?;
        let menu = Menu::new()?;
        let flags = CMF_NORMAL | CMF_CANRENAME | if extended { CMF_EXTENDEDVERBS } else { 0 };
        let status = unsafe { context.QueryContextMenu(menu.0, 0, FIRST, LAST, flags) };
        status.ok().context(crate::i18n::Message::new(
            "Windows 메뉴 구성 실패",
            "Could not build Windows menu",
        ))?;
        let count = (status.0 as u32) & 0xffff;
        ensure!(
            count <= LAST - FIRST + 1,
            crate::i18n::Message::new(
                "Windows 메뉴 명령 범위 오류",
                "Invalid Windows menu command range"
            )
        );
        Ok(Self {
            menu,
            context,
            host,
            count,
        })
    }
    fn add_extras(&self) -> Result<()> {
        let submenu = Menu::new()?;
        for (extra, label) in [
            (
                Extra::CopyPaths,
                crate::i18n::text("경로 복사\tCtrl+Shift+C", "Copy path\tCtrl+Shift+C"),
            ),
            (
                Extra::CopyNames,
                crate::i18n::text("이름 복사", "Copy name"),
            ),
            (
                Extra::Rename,
                crate::i18n::text("이름 변경…\tF2", "Rename…\tF2"),
            ),
        ] {
            submenu.append(MF_STRING, extra.command() as usize, label)?;
        }
        self.menu.append(MF_SEPARATOR, 0, "")?;
        self.menu
            .append(MF_POPUP, submenu.0.0 as usize, crate::product::NAME)?;
        // A submenu attached successfully is destroyed with its parent.
        std::mem::forget(submenu);
        Ok(())
    }
    fn invoke(&self, owner: HWND, offset: Option<u32>, point: POINT, extended: bool) -> Result<()> {
        let offset = offset.map(|n| n as usize);
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: SEE_MASK_UNICODE
                | CMIC_MASK_PTINVOKE
                | if extended { CMIC_MASK_SHIFT_DOWN } else { 0 },
            hwnd: owner,
            lpVerb: offset.map_or(PCSTR(c"properties".as_ptr().cast()), |n| {
                PCSTR(n as *const _)
            }),
            lpVerbW: offset.map_or(w!("properties"), |n| PCWSTR(n as *const _)),
            nShow: SW_SHOWNORMAL.0,
            ptInvoke: point,
            ..Default::default()
        };
        unsafe {
            self.context
                .InvokeCommand((&info as *const CMINVOKECOMMANDINFOEX).cast())
        }
        .context(crate::i18n::Message::new(
            "Windows 메뉴 명령 실행 실패",
            "Could not execute Windows menu command",
        ))
    }
}
pub fn screen_point(window: isize, x: f32, y: f32, scale: f32) -> Result<(i32, i32)> {
    let mut point = POINT {
        x: (x * scale).round() as i32,
        y: (y * scale).round() as i32,
    };
    unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(HWND(window as *mut _), &mut point) }
        .ok()?;
    Ok((point.x, point.y))
}
pub fn show(
    selection: &crate::shell_path::Selection,
    window: isize,
    point: (i32, i32),
    extended: bool,
) -> Result<Outcome> {
    let owner = HWND(window as *mut _);
    let menu = NativeMenu::new(selection, Some(owner), extended)?;
    menu.add_extras()?;
    let command = unsafe {
        TrackPopupMenuEx(
            menu.menu.0,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            point.0,
            point.1,
            menu.host.window.unwrap(),
            None,
        )
    }
    .0 as u32;
    if command == 0 {
        return Ok(Outcome::Cancelled);
    }
    if let Some(extra) = Extra::from_command(command) {
        return Ok(Outcome::Extra(extra));
    }
    ensure!(
        command >= FIRST && command < FIRST + menu.count,
        crate::i18n::Message::new(
            "Windows 메뉴 명령 ID 오류",
            "Invalid Windows menu command ID"
        )
    );
    let mut verb = [0u16; 64];
    if unsafe {
        menu.context.GetCommandString(
            (command - FIRST) as usize,
            GCS_VERBW,
            None,
            windows::core::PSTR(verb.as_mut_ptr().cast()),
            verb.len() as u32,
        )
    }
    .is_ok()
        && String::from_utf16_lossy(
            &verb[..verb.iter().position(|x| *x == 0).unwrap_or(verb.len())],
        ) == "rename"
    {
        return Ok(Outcome::Extra(Extra::Rename));
    }
    menu.invoke(
        owner,
        Some(command - FIRST),
        POINT {
            x: point.0,
            y: point.1,
        },
        extended,
    )?;
    Ok(Outcome::Invoked)
}
pub fn properties(selection: &crate::shell_path::Selection, window: isize) -> Result<()> {
    let owner = HWND(window as *mut _);
    NativeMenu::new(selection, Some(owner), false)?.invoke(owner, None, POINT::default(), false)
}
#[derive(Debug, serde::Serialize)]
pub struct Entry {
    pub label: String,
    pub verb: Option<String>,
    pub children: Vec<Entry>,
}
/// Build and initialize actual Shell menus without displaying a popup or invoking any command.
fn inspect_selection(
    selection: &crate::shell_path::Selection,
    extended: bool,
) -> Result<Vec<Entry>> {
    fn read(
        menu: &NativeMenu,
        handle: HMENU,
        depth: usize,
        budget: &mut usize,
    ) -> Result<Vec<Entry>> {
        ensure!(
            depth <= 8,
            crate::i18n::Message::new(
                "Shell 메뉴 깊이 한도 초과",
                "Shell menu depth limit exceeded"
            )
        );
        let _ = menu.host.state.as_ref().unwrap().message(
            WM_INITMENUPOPUP,
            WPARAM(handle.0 as usize),
            LPARAM(0),
        );
        let count = unsafe { GetMenuItemCount(Some(handle)) };
        ensure!(
            count >= 0,
            crate::i18n::Message::new(
                "Shell 메뉴 항목 조회 실패",
                "Could not read Shell menu item"
            )
        );
        let mut entries = Vec::new();
        for i in 0..count as u32 {
            ensure!(
                *budget > 0,
                crate::i18n::Message::new(
                    "Shell 메뉴 항목 한도 초과",
                    "Shell menu item limit exceeded"
                )
            );
            *budget -= 1;
            let mut text = vec![0u16; 1024];
            let mut info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING | MIIM_ID | MIIM_SUBMENU,
                dwTypeData: windows::core::PWSTR(text.as_mut_ptr()),
                cch: 1024,
                ..Default::default()
            };
            unsafe { GetMenuItemInfoW(handle, i, true, &mut info) }?;
            let label = String::from_utf16_lossy(&text[..info.cch as usize]);
            let verb = if info.wID >= FIRST && info.wID < FIRST + menu.count {
                let mut value = [0u16; 1024];
                unsafe {
                    menu.context.GetCommandString(
                        (info.wID - FIRST) as usize,
                        GCS_VERBW,
                        None,
                        windows::core::PSTR(value.as_mut_ptr().cast()),
                        value.len() as u32,
                    )
                }
                .ok()
                .map(|_| {
                    String::from_utf16_lossy(
                        &value[..value.iter().position(|x| *x == 0).unwrap_or(value.len())],
                    )
                })
            } else {
                None
            };
            let children = if info.hSubMenu.0.is_null() {
                Vec::new()
            } else {
                read(menu, info.hSubMenu, depth + 1, budget)?
            };
            entries.push(Entry {
                label,
                verb,
                children,
            });
        }
        Ok(entries)
    }
    let menu = NativeMenu::new(selection, None, extended)?;
    read(&menu, menu.menu.0, 0, &mut 4096)
}
/// Read the actual menu without showing it or executing any verb.
pub fn inspect(paths: &[std::path::PathBuf], extended: bool) -> Result<Vec<Entry>> {
    let epoch = std::sync::atomic::AtomicU64::new(1);
    let selection = crate::shell_path::prepare(paths, &epoch, 1)?;
    inspect_selection(&selection, extended)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "measures real root Shell menu preparation without displaying or invoking it"]
    fn menu_preparation_measurement() {
        let _ole = crate::native_drag::Apartment::initialize().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("menu.txt");
        std::fs::write(&path, "unchanged").unwrap();
        let epoch = std::sync::atomic::AtomicU64::new(1);
        let mut samples = Vec::new();
        for _ in 0..11 {
            let start = std::time::Instant::now();
            let selection =
                crate::shell_path::prepare(std::slice::from_ref(&path), &epoch, 1).unwrap();
            let prepare_ms = start.elapsed().as_secs_f64() * 1000.;
            let start = std::time::Instant::now();
            let menu = NativeMenu::new(&selection, None, false).unwrap();
            menu.add_extras().unwrap();
            samples.push(serde_json::json!({"pidl_ms":prepare_ms,"root_menu_ms":start.elapsed().as_secs_f64()*1000.,"command_range":menu.count}));
        }
        println!(
            "{}",
            serde_json::json!({"samples":samples,"scope":"real temporary txt file, root menu only, no popup/command/submenu initialization; first and 10 warm in same process; not an SVN working copy"})
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "unchanged");
    }
    #[test]
    fn real_single_same_parent_and_mixed_parent_menus_keep_properties_and_paths() {
        let _ole = crate::native_drag::Apartment::initialize().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("다른 폴더")).unwrap();
        let paths = vec![
            dir.path().join("한글.txt"),
            dir.path().join("다른 폴더/two.txt"),
            dir.path().join("three.txt"),
        ];
        for path in &paths {
            std::fs::write(path, b"data").unwrap();
        }
        let epoch = std::sync::atomic::AtomicU64::new(1);
        for items in [
            vec![paths[0].clone()],
            vec![paths[0].clone(), paths[2].clone()],
            vec![paths[0].clone(), paths[1].clone()],
        ] {
            let selection = crate::shell_path::prepare(&items, &epoch, 1).unwrap();
            let entries = inspect_selection(&selection, false).unwrap();
            assert!(
                entries
                    .iter()
                    .any(|x| x.verb.as_deref() == Some("properties")),
                "{entries:#?}"
            );
            assert!(
                entries.iter().any(|x| x.verb.as_deref() == Some("copy")),
                "{entries:#?}"
            );
            assert!(items.iter().all(|p| std::fs::read(p).unwrap() == b"data"));
        }
    }
    #[test]
    fn menu_messages_attach_only_to_the_owned_test_window_and_detach_after_drop() {
        let _ole = crate::native_drag::Apartment::initialize().unwrap();
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }
        let window = Window(
            unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("REA menu test"),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap(),
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, b"data").unwrap();
        let selection =
            crate::shell_path::prepare(&[path], &std::sync::atomic::AtomicU64::new(1), 1).unwrap();
        let menu = NativeMenu::new(&selection, Some(window.0), false).unwrap();
        let reference = (&**menu.host.state.as_ref().unwrap() as *const Handler) as usize;
        let mut found = 0;
        assert!(
            unsafe { GetWindowSubclass(window.0, Some(window_proc), reference, Some(&mut found)) }
                .as_bool()
        );
        assert_eq!(found, reference);
        let _ = unsafe {
            SendMessageW(
                window.0,
                WM_INITMENUPOPUP,
                Some(WPARAM(menu.menu.0.0 as usize)),
                Some(LPARAM(0)),
            )
        };
        drop(menu);
        assert!(
            !unsafe { GetWindowSubclass(window.0, Some(window_proc), reference, Some(&mut found)) }
                .as_bool()
        );
    }
}
