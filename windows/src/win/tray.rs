//! Значок в трее, меню и системные уведомления. Отдельный поток:
//! меню модальное и держит свой цикл сообщений, снег при этом идёт дальше.
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use sugrob_core::sim::snowflake_alpha;
use windows::core::{w, Error, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::*;
use windows::Win32::System::RemoteDesktop::*;
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::governor::{self, Governor};
use super::{update, Log, QUIT};

const WM_TRAY: u32 = WM_APP + 1;
const WM_TRAY_STOP: u32 = WM_APP + 2;
const CLASS: PCWSTR = w!("SugrobTray");
const TIMER_ID: usize = 1;
const EVALUATE_MS: u32 = 1000;
/// Голубой читается и на тёмной, и на светлой панели задач.
const ICON_RGB: (u8, u8, u8) = (0x2E, 0x9B, 0xE6);

const ID_ENABLED: u32 = 1;
const ID_CURSOR: u32 = 2;
const ID_EDITORS: u32 = 3;
const ID_BATTERY: u32 = 4;
const ID_LOGIN: u32 = 5;
const ID_QUIT: u32 = 6;
const ID_SANTA: u32 = 7;
const ID_CALL_SANTA: u32 = 8;
const ID_UPDATE: u32 = 9;
const ID_DENSITY: u32 = 10;
const DENSITIES: [(PCWSTR, f32); 3] = [(w!("Редкий"), 0.5), (w!("Обычный"), 1.0), (w!("Густой"), 1.5)];

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static MENU_OPEN: AtomicBool = AtomicBool::new(false);

struct State {
    governor: Governor,
    icon: NOTIFYICONDATAW,
    icon_added: bool,
    session_registered: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Занятое состояние значит вложенный вызов: его работу сделает следующий такт.
fn with_state<R>(action: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(action))
}

fn make_icon(size: i32) -> Option<HICON> {
    let side = size as usize;
    let alpha = snowflake_alpha(side);
    unsafe {
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(color.into());
            return None;
        }
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, side * side);
        let (r, g, b) = (ICON_RGB.0 as u32, ICON_RGB.1 as u32, ICON_RGB.2 as u32);
        for (pixel, &a) in pixels.iter_mut().zip(&alpha) {
            *pixel = (a as u32) << 24 | r << 16 | g << 8 | b;
        }
        // Маска обязательна, но при альфа-канале не используется. Строки выровнены по 16 бит.
        let mask_bits = vec![0u8; side.div_ceil(16) * 2 * side];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
        let icon_info = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&icon_info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

impl State {
    /// Размер берём у системы для DPI окна: панель задач показывает именно его.
    fn refresh_icon(&mut self, hwnd: HWND) {
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(hwnd)) }.clamp(16, 256);
        let old = self.icon.hIcon;
        match make_icon(size) {
            Some(icon) => {
                self.icon.hIcon = icon;
                if !old.is_invalid() {
                    unsafe {
                        let _ = DestroyIcon(old);
                    }
                }
            }
            None => {
                self.governor.log.write("tray: снежинка не создалась, значок системный");
                if old.is_invalid() {
                    self.icon.hIcon = unsafe { LoadIconW(None, IDI_APPLICATION) }.unwrap_or_default();
                }
            }
        }
    }

    fn add_icon(&mut self, hwnd: HWND) {
        self.refresh_icon(hwnd);
        self.icon_added = unsafe { Shell_NotifyIconW(NIM_ADD, &self.icon) }.as_bool();
        if self.icon_added {
            self.governor.log.write(&format!("tray: значок добавлен, {} px", unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForWindow(hwnd)) }));
        }
    }

    /// При раннем автозапуске службы могут быть ещё не готовы, поэтому повторяем.
    fn tick(&mut self, hwnd: HWND) {
        if !self.icon_added {
            self.add_icon(hwnd);
        }
        if !self.session_registered {
            self.session_registered = unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) }.is_ok();
            if self.session_registered {
                self.governor.log.write("governor: session notifications on");
            }
        }
        self.governor.evaluate();
    }

    fn apply(&mut self, command: u32) {
        let settings = &mut self.governor.settings;
        match command {
            ID_ENABLED => settings.enabled = !settings.enabled,
            ID_CURSOR => settings.cursor_effects = !settings.cursor_effects,
            ID_SANTA => settings.santa = !settings.santa,
            ID_EDITORS => settings.pause_in_editors = !settings.pause_in_editors,
            ID_BATTERY => settings.battery_saving = !settings.battery_saving,
            id if (ID_DENSITY..ID_DENSITY + DENSITIES.len() as u32).contains(&id) => {
                settings.intensity = DENSITIES[(id - ID_DENSITY) as usize].1;
            }
            ID_LOGIN => {
                governor::set_login(!governor::login_enabled(), &mut self.governor.log);
                return;
            }
            ID_CALL_SANTA => {
                governor::call_santa();
                self.governor.wake_main();
                return;
            }
            ID_UPDATE => {
                update::check_now();
                return;
            }
            ID_QUIT => {
                QUIT.store(true, Ordering::Relaxed);
                self.governor.wake_main();
                return;
            }
            _ => return,
        }
        self.governor.save();
        self.governor.evaluate();
    }
}

unsafe fn append(menu: HMENU, id: u32, text: PCWSTR, checked: bool) {
    let flags = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
    let _ = AppendMenuW(menu, flags, id as usize, text);
}

/// Порядок пунктов как в `App.swift`.
unsafe fn build_menu(settings: &governor::Settings) -> Option<HMENU> {
    let menu = CreatePopupMenu().ok()?;
    append(menu, ID_ENABLED, w!("Снег"), settings.enabled);
    if let Ok(density) = CreatePopupMenu() {
        for (index, (title, value)) in DENSITIES.iter().enumerate() {
            append(density, ID_DENSITY + index as u32, *title, settings.intensity == *value);
        }
        let _ = AppendMenuW(menu, MF_POPUP, density.0 as usize, w!("Густота"));
    }
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    append(menu, ID_CURSOR, w!("Стряхивание и снежок"), settings.cursor_effects);
    append(menu, ID_SANTA, w!("Дед Мороз"), settings.santa);
    append(menu, ID_CALL_SANTA, w!("Позвать Деда Мороза"), false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    append(menu, ID_EDITORS, w!("Пауза в монтаже"), settings.pause_in_editors);
    append(menu, ID_BATTERY, w!("Экономить на батарее"), settings.battery_saving);
    append(menu, ID_LOGIN, w!("Запускать при входе"), governor::login_enabled());
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let version: Vec<u16> = concat!("Версия ", env!("CARGO_PKG_VERSION")).encode_utf16().chain(Some(0)).collect();
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(version.as_ptr()));
    append(menu, ID_UPDATE, w!("Проверить обновления"), false);
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    append(menu, ID_QUIT, w!("Выйти"), false);
    Some(menu)
}

unsafe fn show_menu(hwnd: HWND) {
    if MENU_OPEN.swap(true, Ordering::Relaxed) {
        return;
    }
    // Состояние не занято, пока меню открыто: в это время приходят такты таймера.
    let menu = with_state(|state| state.governor.settings).and_then(|settings| build_menu(&settings));
    if let Some(menu) = menu {
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, point.x, point.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        // Без этого меню может не закрыться при клике мимо.
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        with_state(|state| state.apply(command.0 as u32));
    }
    MENU_OPEN.store(false, Ordering::Relaxed);
}

unsafe extern "system" fn on_foreground(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    with_state(|state| state.governor.evaluate());
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let event = lparam.0 as u32 & 0xffff;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            with_state(|state| state.tick(hwnd));
            LRESULT(0)
        }
        WM_HOTKEY => {
            with_state(|state| state.apply(ID_QUIT));
            LRESULT(0)
        }
        WM_POWERBROADCAST if wparam.0 as u32 == PBT_POWERSETTINGCHANGE && lparam.0 != 0 => {
            let setting = &*(lparam.0 as *const POWERBROADCAST_SETTING);
            if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE && setting.DataLength >= 1 {
                // 0: выключен, 1: включён, 2: притушен.
                governor::DISPLAY_OFF.store(setting.Data[0] == 0, Ordering::Relaxed);
                with_state(|state| state.governor.evaluate());
            }
            LRESULT(1)
        }
        WM_WTSSESSION_CHANGE => {
            match wparam.0 as u32 {
                WTS_SESSION_LOCK => governor::LOCKED.store(true, Ordering::Relaxed),
                WTS_SESSION_UNLOCK => governor::LOCKED.store(false, Ordering::Relaxed),
                _ => {}
            }
            with_state(|state| state.governor.evaluate());
            LRESULT(0)
        }
        WM_TRAY_STOP => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        // Проводник перезапустился: значки надо добавить заново.
        _ if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with_state(|state| state.add_icon(hwnd));
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn create_window() -> Result<HWND> {
    let instance = GetModuleHandleW(None)?;
    RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: CLASS, ..Default::default() });
    CreateWindowExW(WS_EX_TOOLWINDOW, CLASS, w!("SugrobTray"), WS_POPUP, 0, 0, 0, 0, None, None, Some(instance.into()), None)
}

unsafe fn thread_main(hwnd: HWND, governor: Governor) {
    let mut icon = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    };
    for (slot, unit) in icon.szTip.iter_mut().zip("Сугроб".encode_utf16()) {
        *slot = unit;
    }
    STATE.with(|cell| *cell.borrow_mut() = Some(State { governor, icon, icon_added: false, session_registered: false }));

    TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
    // Запасной выход, если значок в трее не появится.
    let _ = RegisterHotKey(Some(hwnd), 1, MOD_CONTROL | MOD_ALT | MOD_SHIFT, 'Q' as u32);
    let power = RegisterPowerSettingNotification(HANDLE(hwnd.0), &GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_WINDOW_HANDLE);
    if let Err(error) = &power {
        with_state(|state| state.governor.log.write(&format!("governor: display notifications failed: {error}")));
    }
    let foreground = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_foreground), 0, 0, WINEVENT_OUTOFCONTEXT);
    if SetTimer(Some(hwnd), TIMER_ID, EVALUATE_MS, None) == 0 {
        with_state(|state| state.governor.log.write("governor: таймер не создан, опрос только по событиям"));
    }
    with_state(|state| state.tick(hwnd));

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    let _ = KillTimer(Some(hwnd), TIMER_ID);
    let _ = UnhookWinEvent(foreground);
    if let Ok(handle) = power {
        let _ = UnregisterPowerSettingNotification(handle);
    }
    with_state(|state| {
        if state.session_registered {
            let _ = WTSUnRegisterSessionNotification(hwnd);
        }
        let _ = Shell_NotifyIconW(NIM_DELETE, &state.icon);
        if !state.icon.hIcon.is_invalid() {
            let _ = DestroyIcon(state.icon.hIcon);
        }
    });
    let _ = UnregisterHotKey(Some(hwnd), 1);
    let _ = DestroyWindow(hwnd);
}

/// Пока объект жив, поток трея работает. При удалении значок убирается.
pub struct Tray {
    hwnd: isize,
    thread: Option<JoinHandle<()>>,
}

impl Tray {
    pub fn start(base_fps: f32, log: Log) -> Result<Self> {
        let main_thread = unsafe { GetCurrentThreadId() };
        let (sender, receiver) = mpsc::channel::<std::result::Result<isize, String>>();
        let thread = std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || unsafe {
                match create_window() {
                    Ok(hwnd) => {
                        let governor = Governor::new(base_fps, main_thread, log);
                        let _ = sender.send(Ok(hwnd.0 as isize));
                        thread_main(hwnd, governor);
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                    }
                }
            })
            .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        match receiver.recv() {
            Ok(Ok(hwnd)) => Ok(Tray { hwnd, thread: Some(thread) }),
            Ok(Err(text)) => Err(Error::new(E_FAIL, text)),
            Err(_) => Err(Error::new(E_FAIL, "поток трея не запустился")),
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let hwnd = HWND(self.hwnd as *mut c_void);
        unsafe {
            // Закрывает меню, если оно открыто, иначе поток ждал бы пользователя.
            let _ = PostMessageW(Some(hwnd), WM_CANCELMODE, WPARAM(0), LPARAM(0));
            if PostMessageW(Some(hwnd), WM_TRAY_STOP, WPARAM(0), LPARAM(0)).is_err() {
                return;
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
