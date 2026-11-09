//! Настройки и решение, идёт ли снег сейчас. Перенос `Governor.swift`.
//! Работает в потоке трея, кадровый цикл только читает готовые значения.
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use sugrob_core::sim::{covers_monitor, is_editor_exe, is_shell_class, pause_reason, power_plan, PauseInput};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Power::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::Log;

const SETTINGS_KEY: PCWSTR = w!("Software\\Sugrob");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Sugrob");

static PAUSED: AtomicBool = AtomicBool::new(false);
static CURSOR_EFFECTS: AtomicBool = AtomicBool::new(true);
static SANTA: AtomicBool = AtomicBool::new(true);
static CALL_SANTA: AtomicBool = AtomicBool::new(false);
static FPS: AtomicU32 = AtomicU32::new(60.0f32.to_bits());
static DENSITY: AtomicU32 = AtomicU32::new(1.0f32.to_bits());

pub static DISPLAY_OFF: AtomicBool = AtomicBool::new(false);
pub static LOCKED: AtomicBool = AtomicBool::new(false);

pub fn paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}
pub fn cursor_effects() -> bool {
    CURSOR_EFFECTS.load(Ordering::Relaxed)
}
pub fn santa() -> bool {
    SANTA.load(Ordering::Relaxed)
}
/// Пункт «Позвать Деда Мороза»: кадровый цикл забирает запрос один раз.
pub fn take_santa_call() -> bool {
    CALL_SANTA.swap(false, Ordering::Relaxed)
}
pub fn call_santa() {
    CALL_SANTA.store(true, Ordering::Relaxed);
}
pub fn fps() -> f32 {
    f32::from_bits(FPS.load(Ordering::Relaxed))
}
pub fn density() -> f32 {
    f32::from_bits(DENSITY.load(Ordering::Relaxed))
}

fn read_dword(name: PCWSTR) -> Option<u32> {
    let (mut value, mut size) = (0u32, 4u32);
    let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, SETTINGS_KEY, name, RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut c_void), Some(&mut size)) };
    status.is_ok().then_some(value)
}

/// Настройки из меню, сохраняются в HKCU\Software\Sugrob.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub enabled: bool,
    /// Густота снегопада: 0,5, 1 или 1,5.
    pub intensity: f32,
    pub cursor_effects: bool,
    pub santa: bool,
    pub pause_in_editors: bool,
    pub battery_saving: bool,
}

impl Settings {
    fn load() -> Self {
        let flag = |name, default| read_dword(name).map_or(default, |v| v != 0);
        let intensity = match read_dword(w!("intensity")) {
            Some(50) => 0.5,
            Some(150) => 1.5,
            _ => 1.0,
        };
        Settings {
            enabled: flag(w!("enabled"), true),
            intensity,
            cursor_effects: flag(w!("cursorEffects"), true),
            santa: flag(w!("santa"), true),
            pause_in_editors: flag(w!("pauseInEditors"), true),
            battery_saving: flag(w!("batterySaving"), false),
        }
    }

    fn save(&self) -> std::result::Result<(), WIN32_ERROR> {
        let values = [
            (w!("enabled"), self.enabled as u32),
            (w!("intensity"), (self.intensity * 100.0).round() as u32),
            (w!("cursorEffects"), self.cursor_effects as u32),
            (w!("santa"), self.santa as u32),
            (w!("pauseInEditors"), self.pause_in_editors as u32),
            (w!("batterySaving"), self.battery_saving as u32),
        ];
        unsafe {
            let mut key = HKEY::default();
            let status = RegCreateKeyExW(HKEY_CURRENT_USER, SETTINGS_KEY, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None);
            if status.is_err() {
                return Err(status);
            }
            let mut result = Ok(());
            for (name, value) in values {
                let status = RegSetValueExW(key, name, None, REG_DWORD, Some(&value.to_ne_bytes()));
                if status.is_err() {
                    result = Err(status);
                }
            }
            let _ = RegCloseKey(key);
            result
        }
    }
}

fn run_command() -> Option<String> {
    Some(format!("\"{}\"", std::env::current_exe().ok()?.display()))
}

/// Автозапуск включён, только если запись ведёт на этот самый файл.
pub fn login_enabled() -> bool {
    let Some(expected) = run_command() else { return false };
    let mut buffer = [0u16; 1024];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr() as *mut c_void), Some(&mut size)) };
    if status.is_err() {
        return false;
    }
    let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length]).eq_ignore_ascii_case(&expected)
}

pub fn set_login(enable: bool, log: &mut Log) {
    let status = unsafe {
        if enable {
            let Some(command) = run_command() else {
                log.write("login item: не удалось узнать путь к программе");
                return;
            };
            let bytes: Vec<u8> = command.encode_utf16().chain(Some(0)).flat_map(u16::to_ne_bytes).collect();
            let mut key = HKEY::default();
            let opened = RegCreateKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None);
            if opened.is_ok() {
                let written = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(&bytes));
                let _ = RegCloseKey(key);
                written
            } else {
                opened
            }
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE)
        }
    };
    if status.is_err() {
        log.write(&format!("login item: ошибка {}", status.0));
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 64];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) } as usize;
    String::from_utf16_lossy(&buffer[..length])
}

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

/// На батарее или в режиме экономии Windows.
fn is_saving_power() -> bool {
    let mut status = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return false;
    }
    status.SystemStatusFlag == 1 || status.ACLineStatus == 0
}

/// Как на Mac: важен только основной монитор, видео на втором снег не останавливает.
/// Видимая рамка окна: `GetWindowRect` у развёрнутых окон шире монитора.
fn window_covers_primary_monitor(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            return false;
        }
        let mut frame = RECT::default();
        let got = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut RECT as *mut c_void, std::mem::size_of::<RECT>() as u32);
        if got.is_err() && GetWindowRect(hwnd, &mut frame).is_err() {
            return false;
        }
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY), &mut info).as_bool() {
            return false;
        }
        let m = info.rcMonitor;
        covers_monitor([frame.left, frame.top, frame.right, frame.bottom], [m.left, m.top, m.right, m.bottom])
    }
}

/// Решает, идёт ли снег сейчас, с какой частотой и густотой.
pub struct Governor {
    pub settings: Settings,
    pub log: Log,
    base_fps: f32,
    main_thread: u32,
    last_state: Option<(Option<&'static str>, u32, u32)>,
    last_fullscreen_probe: (i32, bool),
    last_front: isize,
    /// Имя процесса спрашиваем только при смене окна.
    foreground: (isize, u32, bool),
}

impl Governor {
    pub fn new(base_fps: f32, main_thread: u32, log: Log) -> Self {
        Governor { settings: Settings::load(), log, base_fps, main_thread, last_state: None, last_fullscreen_probe: (-1, false), last_front: 0, foreground: (0, 0, false) }
    }

    pub fn save(&mut self) {
        if let Err(status) = self.settings.save() {
            self.log.write(&format!("settings: ошибка записи {}", status.0));
        }
    }

    fn is_editor_in_front(&mut self, hwnd: HWND) -> bool {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if self.foreground.0 != hwnd.0 as isize || self.foreground.1 != pid {
            let editor = process_path(pid).is_some_and(|path| is_editor_exe(&path));
            self.foreground = (hwnd.0 as isize, pid, editor);
        }
        self.foreground.2
    }

    fn is_fullscreen_in_front(&mut self, hwnd: HWND) -> bool {
        // Состояние оболочки само по себе бывает ложным, поэтому нужны оба признака.
        let state = unsafe { SHQueryUserNotificationState() }.unwrap_or(QUNS_ACCEPTS_NOTIFICATIONS);
        let class = if hwnd.is_invalid() { String::new() } else { class_name(hwnd) };
        let covers = !hwnd.is_invalid() && !is_shell_class(&class) && window_covers_primary_monitor(hwnd);
        if self.last_fullscreen_probe != (state.0, covers) {
            self.last_fullscreen_probe = (state.0, covers);
            self.log.write(&format!("governor: shell state {}, front {class:?} covers primary {covers}", state.0));
        }
        covers && matches!(state, QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
    }

    pub fn evaluate(&mut self) {
        let saving = self.settings.battery_saving && is_saving_power();
        let (fps, density) = power_plan(self.settings.intensity, saving, self.base_fps);

        // Открытое меню трея забирает фокус: пауза должна смотреть на окно под ним.
        let mut front = unsafe { GetForegroundWindow() };
        if !front.is_invalid() && class_name(front) == "SugrobTray" {
            front = HWND(self.last_front as *mut c_void);
        }
        self.last_front = front.0 as isize;
        let editor_in_front = self.settings.pause_in_editors && !front.is_invalid() && self.is_editor_in_front(front);
        let fullscreen_in_front = self.is_fullscreen_in_front(front);
        let reason = pause_reason(&PauseInput {
            enabled: self.settings.enabled,
            display_off: DISPLAY_OFF.load(Ordering::Relaxed),
            locked: LOCKED.load(Ordering::Relaxed),
            pause_in_editors: self.settings.pause_in_editors,
            editor_in_front,
            fullscreen_in_front,
        });

        let state = (reason, fps.to_bits(), density.to_bits());
        if self.last_state != Some(state) {
            self.last_state = Some(state);
            let text = reason.map_or("running".to_string(), |r| format!("paused ({r})"));
            self.log.write(&format!("governor: {text} fps={fps} density={density}"));
        }

        FPS.store(fps.to_bits(), Ordering::Relaxed);
        DENSITY.store(density.to_bits(), Ordering::Relaxed);
        CURSOR_EFFECTS.store(self.settings.cursor_effects, Ordering::Relaxed);
        SANTA.store(self.settings.santa, Ordering::Relaxed);
        if PAUSED.swap(reason.is_some(), Ordering::Relaxed) != reason.is_some() {
            self.wake_main();
        }
    }

    /// Кадровый цикл на паузе спит, пока его не разбудят.
    pub fn wake_main(&self) {
        unsafe {
            let _ = PostThreadMessageW(self.main_thread, WM_NULL, WPARAM(0), LPARAM(0));
        }
    }
}
