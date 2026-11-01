//! A private invisible window owns RegisterHotKey and the notification icon.
//! No keyboard hooks, foreign windows, or injected code.
use crate::hotkey::Hotkey;
use anyhow::{Context, Result, ensure};
use std::{
    cell::Cell,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey},
        Shell::*,
        WindowsAndMessaging::*,
    },
};
const TRAY_MESSAGE: u32 = WM_APP + 17;
const WORK_MESSAGE: u32 = WM_APP + 18;
const STOP_MESSAGE: u32 = WM_APP + 19;
const MENU_COMMAND_MESSAGE: u32 = WM_APP + 20;
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
const SHOW: u8 = 1;
const SETTINGS: u8 = 2;
const QUIT: u8 = 4;
#[derive(Default, Clone)]
pub struct Status {
    pub tray: bool,
    pub hotkey: Option<Hotkey>,
    pub error: crate::i18n::Message,
}
struct Shared {
    window: Mutex<Option<isize>>,
    status: Mutex<Status>,
    result: Mutex<Option<std::result::Result<Option<Hotkey>, crate::i18n::Message>>>,
    event: AtomicU8,
    stop: AtomicBool,
    wake: Arc<dyn Fn() + Send + Sync>,
}
pub enum Event {
    Show,
    Settings,
    Quit,
}
pub struct Controller {
    shared: Arc<Shared>,
    commands: mpsc::SyncSender<Option<Hotkey>>,
    join: Option<thread::JoinHandle<()>>,
    owner: isize,
}
impl Controller {
    pub fn start(
        owner: isize,
        hotkey: Option<Hotkey>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        Self::start_inner(owner, hotkey, wake, true)
    }
    #[cfg(test)]
    pub(crate) fn test_controller(hotkey: Hotkey) -> Result<Self> {
        Self::start_inner(0, Some(hotkey), Arc::new(|| {}), false)
    }
    fn start_inner(
        owner: isize,
        hotkey: Option<Hotkey>,
        wake: Arc<dyn Fn() + Send + Sync>,
        tray: bool,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            window: Mutex::new(None),
            status: Mutex::new(Status::default()),
            result: Mutex::new(None),
            event: AtomicU8::new(0),
            stop: AtomicBool::new(false),
            wake,
        });
        let (tx, rx) = mpsc::sync_channel(1);
        let state = shared.clone();
        let join = thread::Builder::new()
            .name("window-resident".into())
            .spawn(move || {
                let result = run(owner, hotkey, rx, state.clone(), tray);
                let error = result
                    .err()
                    .map(|e| {
                        crate::msgf!(
                            "백그라운드 창 제어 실패: {e:#}",
                            "Background window control failed: {e:#}"
                        )
                    })
                    .or_else(|| {
                        (!state.stop.load(Ordering::Acquire)).then(|| {
                            crate::i18n::Message::new(
                                "백그라운드 메시지 창이 종료되었습니다",
                                "The background message window stopped",
                            )
                        })
                    });
                {
                    let mut status = state.status.lock().unwrap();
                    status.tray = false;
                    status.hotkey = None;
                    if let Some(error) = error.clone() {
                        status.error = error;
                    }
                }
                if let Some(error) = error {
                    *state.result.lock().unwrap() = Some(Err(error));
                    (state.wake)();
                }
            })?;
        let controller = Self {
            shared,
            commands: tx,
            join: Some(join),
            owner,
        };
        if owner != 0 {
            ensure!(
                unsafe {
                    SetPropW(
                        owner as HWND,
                        windows_sys::w!("Kangpg.Finder.Resident.Owner"),
                        Arc::as_ptr(&controller.shared) as HANDLE,
                    )
                } != 0,
                crate::i18n::Message::new(
                    "자신의 창 식별 실패",
                    "Could not identify the app's own window"
                )
            );
        }
        Ok(controller)
    }
    pub fn apply(&self, hotkey: Option<Hotkey>) -> Result<()> {
        if let Some(h) = hotkey {
            h.validate()?;
        }
        self.commands
            .try_send(hotkey)
            .context(crate::i18n::Message::new(
                "단축키 변경 작업자가 응답하지 않습니다",
                "The shortcut worker is not responding",
            ))?;
        if let Some(window) = *self.shared.window.lock().unwrap() {
            unsafe {
                PostMessageW(window as HWND, WORK_MESSAGE, 0, 0);
            }
        }
        Ok(())
    }
    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }
    pub fn take_result(&self) -> Option<std::result::Result<Option<Hotkey>, crate::i18n::Message>> {
        self.shared.result.lock().unwrap().take()
    }
    pub fn take_event(&self) -> Option<Event> {
        let bits = self.shared.event.swap(0, Ordering::AcqRel);
        if bits & QUIT != 0 {
            Some(Event::Quit)
        } else if bits & SETTINGS != 0 {
            Some(Event::Settings)
        } else if bits & SHOW != 0 {
            Some(Event::Show)
        } else {
            None
        }
    }
    pub fn can_hide(&self) -> bool {
        let s = self.status();
        s.tray || s.hotkey.is_some()
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(window) = *self.shared.window.lock().unwrap() {
            unsafe {
                PostMessageW(window as HWND, STOP_MESSAGE, 0, 0);
            }
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        unsafe {
            if self.owner != 0
                && GetPropW(
                    self.owner as HWND,
                    windows_sys::w!("Kangpg.Finder.Resident.Owner"),
                ) == Arc::as_ptr(&self.shared) as HANDLE
            {
                RemovePropW(
                    self.owner as HWND,
                    windows_sys::w!("Kangpg.Finder.Resident.Owner"),
                );
            }
        }
    }
}
#[derive(Clone, Copy, Default)]
enum TrayProtocol {
    #[default]
    Legacy,
    Version4,
}
#[derive(Debug, PartialEq, Eq)]
enum TrayAction {
    Show,
    Menu,
}
impl TrayProtocol {
    fn action(self, wparam: WPARAM, lparam: LPARAM) -> Option<TrayAction> {
        let (icon, event) = match self {
            Self::Legacy => (wparam as u32, lparam as u32),
            Self::Version4 => ((lparam as u32) >> 16, (lparam as u32) & 0xffff),
        };
        if icon != 1 {
            return None;
        }
        match (self, event) {
            (Self::Version4, NIN_SELECT | NIN_KEYSELECT) | (Self::Legacy, WM_LBUTTONDBLCLK) => {
                Some(TrayAction::Show)
            }
            (Self::Version4, WM_CONTEXTMENU) | (Self::Legacy, WM_RBUTTONUP) => {
                Some(TrayAction::Menu)
            }
            _ => None,
        }
    }
}
struct MenuSession<'a>(&'a Cell<bool>);
impl<'a> MenuSession<'a> {
    fn begin(active: &'a Cell<bool>) -> Option<Self> {
        if active.replace(true) {
            None
        } else {
            Some(Self(active))
        }
    }
}
impl Drop for MenuSession<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
// Own the popup and its post-menu task-switch message, independently of the selected action.
struct TrayMenu {
    handle: HMENU,
    window: HWND,
    tracked: bool,
}
impl TrayMenu {
    fn new(window: HWND) -> Result<Self> {
        let menu = Self {
            handle: unsafe { CreatePopupMenu() },
            window,
            tracked: false,
        };
        ensure!(
            !menu.handle.is_null(),
            crate::i18n::Message::new("트레이 메뉴 생성 실패", "Could not create tray menu")
        );
        for (id, label) in [
            (1, crate::i18n::text("열기", "Open")),
            (2, crate::i18n::text("설정", "Settings")),
            (3, crate::i18n::text("종료", "Exit")),
        ] {
            let text: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
            ensure!(
                unsafe { AppendMenuW(menu.handle, MF_STRING, id, text.as_ptr()) } != 0,
                crate::i18n::Message::new(
                    "트레이 메뉴 항목 추가 실패",
                    "Could not add tray menu item"
                )
            );
        }
        Ok(menu)
    }
    fn track(&mut self) -> u32 {
        unsafe {
            let mut point = POINT::default();
            GetCursorPos(&mut point);
            // The zero-area popup owner stays available across menus. Hiding the foreground
            // owner here races activation of the main window after Settings/Open.
            ShowWindow(self.window, SW_SHOWNOACTIVATE);
            SetForegroundWindow(self.window);
            self.tracked = true;
            TrackPopupMenu(
                self.handle,
                TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
                point.x,
                point.y,
                0,
                self.window,
                std::ptr::null(),
            ) as u32
        }
    }
    fn finish(self, command: u32, icon: &NOTIFYICONDATAW) {
        let window = self.window;
        drop(self); // The popup and WM_NULL belong to this session, not to its command.
        unsafe {
            if command == 0 {
                Shell_NotifyIconW(NIM_SETFOCUS, icon);
            } else {
                PostMessageW(window, MENU_COMMAND_MESSAGE, command as usize, 0);
            }
        }
    }
}
impl Drop for TrayMenu {
    fn drop(&mut self) {
        unsafe {
            if self.tracked {
                PostMessageW(self.window, WM_NULL, 0, 0);
            }
            if !self.handle.is_null() {
                DestroyMenu(self.handle);
            }
        }
    }
}
struct State {
    owner: HWND,
    shared: Arc<Shared>,
    registered: Cell<Option<(i32, Hotkey)>>,
    commands: mpsc::Receiver<Option<Hotkey>>,
    tray_enabled: bool,
    tray_protocol: Cell<TrayProtocol>,
    menu_active: Cell<bool>,
    taskbar_message: u32,
    icon: Cell<NOTIFYICONDATAW>,
}
impl State {
    fn apply_pending(&self, window: HWND) {
        while let Ok(desired) = self.commands.try_recv() {
            let result = self
                .change_hotkey(window, desired)
                .map(|()| desired)
                .map_err(crate::i18n::Message::error);
            self.shared.status.lock().unwrap().error =
                result.as_ref().err().cloned().unwrap_or_default();
            *self.shared.result.lock().unwrap() = Some(result);
            (self.shared.wake)();
        }
    }
    fn signal(&self, event: u8) {
        self.shared.event.fetch_or(event, Ordering::AcqRel);
        (self.shared.wake)();
    }
    fn reveal(&self, event: u8) {
        unsafe {
            if !self.owner.is_null()
                && GetPropW(self.owner, windows_sys::w!("Kangpg.Finder.Resident.Owner"))
                    == Arc::as_ptr(&self.shared) as HANDLE
            {
                ShowWindowAsync(
                    self.owner,
                    if IsIconic(self.owner) != 0 {
                        SW_RESTORE
                    } else {
                        SW_SHOW
                    },
                );
                SetForegroundWindow(self.owner);
            }
        }
        self.signal(event);
    }
    fn add_tray(&self) {
        if !self.tray_enabled {
            return;
        }
        unsafe {
            let mut icon = self.icon.get();
            let added = Shell_NotifyIconW(NIM_ADD, &icon) != 0;
            self.tray_protocol.set(TrayProtocol::Legacy);
            if added {
                icon.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                if Shell_NotifyIconW(NIM_SETVERSION, &icon) != 0 {
                    self.tray_protocol.set(TrayProtocol::Version4);
                }
            }
            self.shared.status.lock().unwrap().tray = added;
            (self.shared.wake)();
        }
    }
    fn change_hotkey(&self, window: HWND, desired: Option<Hotkey>) -> Result<()> {
        if desired == self.registered.get().map(|(_, h)| h) {
            return Ok(());
        }
        if let Some(h) = desired {
            h.validate()?;
        }
        // Claim the replacement first. A collision leaves the old registration intact.
        let new_id = if self.registered.get().is_some_and(|(id, _)| id == 1) {
            2
        } else {
            1
        };
        if let Some(h) = desired {
            ensure!(
                unsafe { RegisterHotKey(window, new_id, h.modifiers() | MOD_NOREPEAT, h.key) } != 0,
                crate::msgf!(
                    "단축키 {} 등록 실패: {} · 다른 앱에서 사용 중일 수 있습니다",
                    "Could not register shortcut {}: {} · Another app may be using it",
                    h.label(),
                    std::io::Error::last_os_error()
                )
            );
        }
        if let Some((id, _)) = self.registered.get() {
            unsafe {
                UnregisterHotKey(window, id);
            }
        }
        self.registered.set(desired.map(|h| (new_id, h)));
        self.shared.status.lock().unwrap().hotkey = desired;
        Ok(())
    }
    fn menu_action(&self, command: usize) {
        match command {
            1 => self.reveal(SHOW),
            2 => self.reveal(SETTINGS),
            3 => self.signal(QUIT),
            _ => {}
        }
    }
    fn menu(&self, window: HWND) {
        let Some(_session) = MenuSession::begin(&self.menu_active) else {
            return;
        };
        match TrayMenu::new(window) {
            Ok(mut menu) => {
                let command = menu.track();
                // Queue the selected action only after menu cleanup and its neutral message.
                menu.finish(command, &self.icon.get());
            }
            Err(e) => {
                self.shared.status.lock().unwrap().error =
                    crate::i18n::Message::capture(|| format!("{e:#}"));
                (self.shared.wake)();
            }
        }
    }
}
unsafe extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let pointer = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut State;
        if !pointer.is_null() {
            let state = &*pointer;
            if message == MENU_COMMAND_MESSAGE {
                state.menu_action(wparam);
                return 0;
            }
            if message == WORK_MESSAGE {
                state.apply_pending(window);
                return 0;
            }
            if message == STOP_MESSAGE {
                EndMenu();
                PostQuitMessage(0);
                return 0;
            }
            if message == WM_HOTKEY {
                if state
                    .registered
                    .get()
                    .is_some_and(|(id, _)| id as usize == wparam)
                {
                    state.reveal(SHOW);
                }
                return 0;
            }
            if message == TRAY_MESSAGE {
                if !state.menu_active.get() {
                    match state.tray_protocol.get().action(wparam, lparam) {
                        Some(TrayAction::Show) => state.reveal(SHOW),
                        Some(TrayAction::Menu) => state.menu(window),
                        None => {}
                    }
                }
                return 0;
            }
            if message == state.taskbar_message && message != 0 {
                state.add_tray();
                return 0;
            }
            if message == WM_NCDESTROY {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            }
        }
        DefWindowProcW(window, message, wparam, lparam)
    }
}
struct Window {
    handle: HWND,
    class: Vec<u16>,
    instance: HINSTANCE,
    shared: Arc<Shared>,
}
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let mut window = self.shared.window.lock().unwrap();
            *window = None;
            DestroyWindow(self.handle);
            UnregisterClassW(self.class.as_ptr(), self.instance);
        }
    }
}
fn run(
    owner: isize,
    hotkey: Option<Hotkey>,
    commands: mpsc::Receiver<Option<Hotkey>>,
    shared: Arc<Shared>,
    tray: bool,
) -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        // Unique class keeps independent processes and test windows isolated.
        let class: Vec<u16> = format!(
            "Kangpg.Finder.Resident.{}.{:p}",
            std::process::id(),
            Arc::as_ptr(&shared)
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        ensure!(
            RegisterClassW(&wc) != 0,
            crate::msgf!(
                "창 클래스 등록 실패: {}",
                "Could not register window class: {}",
                std::io::Error::last_os_error()
            )
        );
        let mut state = Box::new(State {
            owner: owner as HWND,
            shared: shared.clone(),
            registered: Cell::new(None),
            commands,
            tray_enabled: tray,
            tray_protocol: Cell::new(TrayProtocol::Legacy),
            menu_active: Cell::new(false),
            taskbar_message: RegisterWindowMessageW(windows_sys::w!("TaskbarCreated")),
            icon: Cell::new(std::mem::zeroed()),
        });
        let handle = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            windows_sys::w!("Finder background"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            (&mut *state as *mut State).cast(),
        );
        if handle.is_null() {
            UnregisterClassW(class.as_ptr(), instance);
            anyhow::bail!(crate::msgf!(
                "백그라운드 창 생성 실패: {}",
                "Could not create background window: {}",
                std::io::Error::last_os_error()
            ));
        }
        let window = Window {
            handle,
            class,
            instance,
            shared: shared.clone(),
        };
        *shared.window.lock().unwrap() = Some(handle as isize);
        let native_icon = crate::app_icon::Native::new()?;
        let mut icon: NOTIFYICONDATAW = std::mem::zeroed();
        icon.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        icon.hWnd = handle;
        icon.uID = 1;
        icon.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        icon.uCallbackMessage = TRAY_MESSAGE;
        icon.hIcon = native_icon.handle();
        let tip: Vec<u16> = crate::product::NAME.encode_utf16().collect();
        icon.szTip[..tip.len()].copy_from_slice(&tip);
        state.icon.set(icon);
        state.add_tray();
        if let Err(e) = state.change_hotkey(handle, hotkey) {
            shared.status.lock().unwrap().error =
                crate::i18n::Message::capture(|| format!("{e:#}"));
        }
        (shared.wake)();
        state.apply_pending(handle);
        let mut message: MSG = std::mem::zeroed();
        while !shared.stop.load(Ordering::Acquire) {
            let result = GetMessageW(&mut message, std::ptr::null_mut(), 0, 0);
            if result <= 0 {
                break;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if state.tray_enabled {
            Shell_NotifyIconW(NIM_DELETE, &state.icon.get());
        }
        if let Some((id, _)) = state.registered.take() {
            UnregisterHotKey(handle, id);
        }
        drop(window); // WM_NCDESTROY runs while State is still alive.
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn test_tray_labels() -> Vec<String> {
    let menu = TrayMenu::new(std::ptr::null_mut()).unwrap();
    (0..3)
        .map(|position| {
            let mut text = [0u16; 128];
            let length = unsafe {
                GetMenuStringW(
                    menu.handle,
                    position,
                    text.as_mut_ptr(),
                    text.len() as i32,
                    MF_BYPOSITION,
                )
            };
            assert!(length > 0);
            String::from_utf16(&text[..length as usize]).unwrap()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn wait(mut check: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(3);
        while !check() {
            assert!(Instant::now() < end, "resident timeout");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn tray_callback_protocol_emits_one_menu_and_one_activation_per_gesture() {
        let packed = |event: u32| ((1u32 << 16) | event) as LPARAM;
        let modern = TrayProtocol::Version4;
        assert_eq!(
            modern.action(0, packed(WM_CONTEXTMENU)),
            Some(TrayAction::Menu)
        );
        for event in [WM_RBUTTONDOWN, WM_RBUTTONUP, WM_LBUTTONUP, WM_LBUTTONDBLCLK] {
            assert_eq!(modern.action(0, packed(event)), None);
        }
        for event in [NIN_SELECT, NIN_KEYSELECT] {
            assert_eq!(modern.action(0, packed(event)), Some(TrayAction::Show));
        }
        assert_eq!(modern.action(1, WM_CONTEXTMENU as LPARAM), None);
        let legacy = TrayProtocol::Legacy;
        assert_eq!(
            legacy.action(1, WM_RBUTTONUP as LPARAM),
            Some(TrayAction::Menu)
        );
        assert_eq!(
            legacy.action(1, WM_LBUTTONDBLCLK as LPARAM),
            Some(TrayAction::Show)
        );
        assert_eq!(legacy.action(1, WM_CONTEXTMENU as LPARAM), None);
        assert_eq!(legacy.action(2, WM_RBUTTONUP as LPARAM), None);
    }

    #[test]
    fn repeated_popup_completion_cleans_resources_before_queuing_settings_or_open() {
        struct TestWindow(HWND);
        impl Drop for TestWindow {
            fn drop(&mut self) {
                unsafe {
                    DestroyWindow(self.0);
                }
            }
        }
        let window = TestWindow(unsafe {
            CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                windows_sys::w!("REA tray queue test"),
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
        });
        assert!(!window.0.is_null());
        let active = Cell::new(false);
        for command in [2, 1, 2, 1, 3] {
            let session = MenuSession::begin(&active).unwrap();
            assert!(MenuSession::begin(&active).is_none());
            let mut menu = TrayMenu::new(window.0).unwrap();
            let handle = menu.handle;
            assert_eq!(unsafe { GetMenuItemCount(handle) }, 3);
            // Exercise the real completion boundary without tracking or showing a popup.
            menu.tracked = true;
            menu.finish(command, &unsafe { std::mem::zeroed() });
            assert_eq!(unsafe { IsMenu(handle) }, 0);
            unsafe {
                let mut message: MSG = std::mem::zeroed();
                assert_ne!(PeekMessageW(&mut message, window.0, 0, 0, PM_REMOVE), 0);
                assert_eq!(message.message, WM_NULL);
                assert_ne!(PeekMessageW(&mut message, window.0, 0, 0, PM_REMOVE), 0);
                assert_eq!(message.message, MENU_COMMAND_MESSAGE);
                assert_eq!(message.wParam, command as usize);
                assert_eq!(PeekMessageW(&mut message, window.0, 0, 0, PM_REMOVE), 0);
                assert_eq!(IsWindowVisible(window.0), 0);
            }
            drop(session);
            assert!(!active.get());
        }
    }

    #[test]
    #[ignore = "measures process CPU while the real message window is idle; no visible windows or tray icon"]
    fn resident_idle_measurement() {
        use windows_sys::Win32::System::{
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            Threading::{GetCurrentProcess, GetProcessTimes},
        };
        fn sample() -> (u64, usize) {
            unsafe {
                let mut created = std::mem::zeroed();
                let mut exited = std::mem::zeroed();
                let mut kernel: FILETIME = std::mem::zeroed();
                let mut user: FILETIME = std::mem::zeroed();
                assert_ne!(
                    GetProcessTimes(
                        GetCurrentProcess(),
                        &mut created,
                        &mut exited,
                        &mut kernel,
                        &mut user
                    ),
                    0
                );
                let ticks =
                    |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
                let mut memory: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
                memory.cb = std::mem::size_of_val(&memory) as u32;
                assert_ne!(
                    GetProcessMemoryInfo(
                        GetCurrentProcess(),
                        &mut memory,
                        std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32
                    ),
                    0
                );
                (ticks(kernel) + ticks(user), memory.PeakWorkingSetSize)
            }
        }
        let h = Hotkey {
            ctrl: true,
            alt: true,
            shift: true,
            win: false,
            key: 135,
        };
        let base_start = Instant::now();
        let before = sample();
        thread::sleep(Duration::from_secs(1));
        let after = sample();
        println!(
            "{}",
            serde_json::json!({"phase":"baseline_idle","elapsed_ms":base_start.elapsed().as_secs_f64()*1000.,"cpu_ms":(after.0-before.0) as f64/10000.,"peak_working_set":after.1})
        );
        let init = Instant::now();
        let controller = Controller::start_inner(0, Some(h), Arc::new(|| {}), false).unwrap();
        wait(|| controller.status().hotkey == Some(h));
        let first_ms = init.elapsed().as_secs_f64() * 1000.;
        let before = sample();
        let start = Instant::now();
        thread::sleep(Duration::from_secs(1));
        let after = sample();
        println!(
            "{}",
            serde_json::json!({"phase":"resident_idle","first_registration_ms":first_ms,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"cpu_ms":(after.0-before.0) as f64/10000.,"peak_working_set":after.1,"scope":"same process; no child processes; hidden Win32 controller only; GUI/GPU, tray Explorer work and indexes excluded"})
        );
    }

    #[test]
    fn real_hotkeys_dispatch_replace_preserve_collision_and_release_without_visible_windows() {
        let h = Hotkey {
            ctrl: true,
            alt: true,
            shift: true,
            win: false,
            key: 135,
        };
        let first = Controller::start_inner(0, Some(h), Arc::new(|| {}), false).unwrap();
        wait(|| first.status().hotkey == Some(h));
        let second = Controller::start_inner(0, None, Arc::new(|| {}), false).unwrap();
        wait(|| second.shared.window.lock().unwrap().is_some());
        second.apply(Some(h)).unwrap();
        let mut collision = None;
        wait(|| {
            collision = second.take_result();
            collision.is_some()
        });
        assert!(collision.unwrap().is_err());
        assert!(second.status().hotkey.is_none());
        let changed = Hotkey { key: 134, ..h };
        second.apply(Some(changed)).unwrap();
        let mut applied = None;
        wait(|| {
            applied = second.take_result();
            applied.is_some()
        });
        assert_eq!(applied.unwrap().unwrap(), Some(changed));
        second.apply(Some(h)).unwrap();
        let mut collision = None;
        wait(|| {
            collision = second.take_result();
            collision.is_some()
        });
        assert!(collision.unwrap().is_err());
        assert_eq!(second.status().hotkey, Some(changed));
        let window = second.shared.window.lock().unwrap().unwrap();
        unsafe {
            SendMessageW(window as HWND, WM_HOTKEY, 1, 0);
        }
        assert!(matches!(second.take_event(), Some(Event::Show)));
        unsafe {
            SendMessageW(window as HWND, TRAY_MESSAGE, 1, WM_LBUTTONDBLCLK as isize);
        }
        assert!(matches!(second.take_event(), Some(Event::Show)));
        unsafe {
            SendMessageW(window as HWND, MENU_COMMAND_MESSAGE, 1, 0);
        }
        assert!(matches!(second.take_event(), Some(Event::Show)));
        unsafe {
            SendMessageW(window as HWND, MENU_COMMAND_MESSAGE, 3, 0);
        }
        assert!(matches!(second.take_event(), Some(Event::Quit)));
        // Native menu notifications no longer execute actions inside TrackPopupMenu.
        unsafe {
            SendMessageW(window as HWND, WM_COMMAND, 2, 0);
        }
        assert!(second.take_event().is_none());
        for _ in 0..3 {
            unsafe {
                SendMessageW(window as HWND, MENU_COMMAND_MESSAGE, 2, 0);
            }
            assert!(matches!(second.take_event(), Some(Event::Settings)));
            unsafe {
                SendMessageW(window as HWND, MENU_COMMAND_MESSAGE, 1, 0);
            }
            assert!(matches!(second.take_event(), Some(Event::Show)));
        }
        unsafe {
            let mut rect = RECT::default();
            assert_ne!(GetWindowRect(window as HWND, &mut rect), 0);
            assert_eq!((rect.right - rect.left, rect.bottom - rect.top), (0, 0));
            assert_ne!(
                GetWindowLongPtrW(window as HWND, GWL_STYLE) as u32 & WS_POPUP,
                0
            );
        }
        drop(first);
        second.apply(Some(h)).unwrap();
        let mut applied = None;
        wait(|| {
            applied = second.take_result();
            applied.is_some()
        });
        assert_eq!(applied.unwrap().unwrap(), Some(h));
        let shared = second.shared.clone();
        let start = Instant::now();
        drop(second);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(shared.window.lock().unwrap().is_none());
        let third = Controller::start_inner(0, Some(h), Arc::new(|| {}), false).unwrap();
        wait(|| third.status().hotkey == Some(h));
    }
}
