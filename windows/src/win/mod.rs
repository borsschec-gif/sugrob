//! Часть под Windows: окна, отрисовка, трей.
mod render;
mod tracker;

use std::fs::{self, File};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sugrob_core::sim::{v2, CapField, SnowField};
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use render::{Renderer, Target};
use tracker::Tracker;

const WM_TRAY: u32 = WM_APP + 1;
const MENU_QUIT: usize = 1;
static QUIT: AtomicBool = AtomicBool::new(false);
pub const OVERLAY_CLASS: PCWSTR = w!("SugrobOverlay");

pub struct Log(File);
impl Log {
    fn open() -> Option<Self> {
        let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Sugrob");
        fs::create_dir_all(&dir).ok()?;
        File::create(dir.join("spike.log")).ok().map(Log)
    }
    pub fn write(&mut self, line: &str) {
        let _ = writeln!(self.0, "{line}");
        let _ = self.0.flush();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY if (lparam.0 as u32) == WM_RBUTTONUP || (lparam.0 as u32) == WM_LBUTTONUP => {
            let menu = CreatePopupMenu().unwrap_or_default();
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Выйти"));
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let _ = SetForegroundWindow(hwnd);
            let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, None, hwnd, None);
            let _ = DestroyMenu(menu);
            LRESULT(0)
        }
        WM_COMMAND if wparam.0 & 0xffff == MENU_QUIT => {
            QUIT.store(true, Ordering::Relaxed);
            LRESULT(0)
        }
        WM_HOTKEY => {
            QUIT.store(true, Ordering::Relaxed);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Monitor {
    rect: RECT,
}

unsafe extern "system" fn monitor_proc(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data.0 as *mut Vec<Monitor>);
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        list.push(Monitor { rect: info.rcMonitor });
    }
    TRUE
}

use windows::core::BOOL;

fn monitors() -> Vec<Monitor> {
    let mut list = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(monitor_proc), LPARAM(&mut list as *mut Vec<Monitor> as isize));
    }
    list
}

/// Масштаб основного монитора: все размеры снега заданы в точках, как на Mac.
fn primary_scale() -> f32 {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let (mut x, mut y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y);
        x as f32 / 96.0
    }
}

fn refresh_rate() -> f32 {
    unsafe {
        let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        if EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut mode).as_bool() && mode.dmDisplayFrequency > 1 {
            mode.dmDisplayFrequency as f32
        } else {
            60.0
        }
    }
}

struct Screen {
    target: Target,
    field: SnowField,
    origin: sugrob_core::sim::V2,
    viewport: sugrob_core::sim::V2,
}

fn build_screens(renderer: &Renderer, list: &[Monitor], scale: f32, seed: u64, log: &mut Log) -> Result<Vec<Screen>> {
    let mut screens = Vec::new();
    for (index, monitor) in list.iter().enumerate() {
        let target = renderer.create_target(monitor.rect)?;
        let (w, h) = (monitor.rect.right - monitor.rect.left, monitor.rect.bottom - monitor.rect.top);
        let viewport = v2(w as f32 / scale, h as f32 / scale);
        log.write(&format!("screen {index}: {w}x{h} at {},{} scale {scale}", monitor.rect.left, monitor.rect.top));
        screens.push(Screen {
            target,
            field: SnowField::new(viewport.x, viewport.y, seed.wrapping_add(index as u64 * 31 + 1)),
            origin: v2(monitor.rect.left as f32 / scale, monitor.rect.top as f32 / scale),
            viewport,
        });
    }
    Ok(screens)
}

fn run(log: &mut Log) -> Result<()> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        let instance = GetModuleHandleW(None)?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: OVERLAY_CLASS,
            ..Default::default()
        });

        let tray_window = CreateWindowExW(WS_EX_TOOLWINDOW, OVERLAY_CLASS, w!("SugrobTray"), WS_POPUP, 0, 0, 0, 0, None, None, Some(instance.into()), None)?;
        let mut icon = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: tray_window,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
            ..Default::default()
        };
        for (slot, unit) in icon.szTip.iter_mut().zip("Сугроб".encode_utf16()) {
            *slot = unit;
        }
        let _ = Shell_NotifyIconW(NIM_ADD, &icon);
        // Запасной выход, если значок в трее не появится.
        let _ = RegisterHotKey(Some(tray_window), 1, MOD_CONTROL | MOD_ALT | MOD_SHIFT, 'Q' as u32);

        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        let mut renderer = Renderer::new()?;
        let mut monitor_list = monitors();
        let mut scale = primary_scale();
        let mut screens = build_screens(&renderer, &monitor_list, scale, seed, log)?;
        let mut caps = CapField::new(seed ^ 0x5555);
        let mut tracker = Tracker::new()?;

        let base_fps: f32 = std::env::var("SUGROB_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(60.0);
        // Пока окно тащат, шапка должна идти за ним с частотой экрана, иначе она дёргается.
        let drag_fps = refresh_rate().max(base_fps);
        log.write(&format!("fps: {base_fps}, while dragging {drag_fps}"));

        let timer = CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0)?;
        let started = Instant::now();
        let mut next = started;
        let mut previous = started;
        let mut last_stats = started;
        let mut last_second = started;
        let (mut frames, mut work_ms) = (0u32, 0f32);

        'main: loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = DispatchMessageW(&msg);
            }
            if QUIT.load(Ordering::Relaxed) {
                break 'main;
            }

            let frame_start = Instant::now();
            if frame_start < next {
                let due = -(((next - frame_start).as_nanos() / 100) as i64);
                let _ = SetWaitableTimer(timer, &due, 0, None, None, false);
                let _ = MsgWaitForMultipleObjects(Some(&[timer]), false, INFINITE, QS_ALLINPUT);
                continue;
            }
            let fps = if tracker.is_dragging() { drag_fps } else { base_fps };
            let interval = Duration::from_secs_f32(1.0 / fps);
            next = (next + interval).max(frame_start);

            if frame_start - last_second >= Duration::from_secs(1) {
                last_second = frame_start;
                for screen in &screens {
                    screen.target.keep_on_top();
                }
                // Монитор подключили, сняли или поменяли масштаб.
                let (now_list, now_scale) = (monitors(), primary_scale());
                if now_list != monitor_list || now_scale != scale {
                    monitor_list = now_list;
                    scale = now_scale;
                    screens.clear();
                    screens = build_screens(&renderer, &monitor_list, scale, seed, log)?;
                }
            }

            let dt = (frame_start - previous).as_secs_f32().min(1.0 / 15.0);
            previous = frame_start;
            let now = (frame_start - started).as_secs_f64() + 1.0;
            let time = (now % 100_000.0) as f32;

            let snapshot = tracker.poll(now, scale, log);
            caps.sync(snapshot, now);
            for screen in &mut screens {
                screen.field.step(dt, time, &mut caps, screen.origin, now);
            }
            caps.relax(now);
            caps.brush(tracker.cursor(scale), now);
            caps.step_debris(now);

            for screen in &screens {
                renderer.draw(&screen.target, &screen.field, &caps, screen.origin, screen.viewport, time)?;
            }
            work_ms += frame_start.elapsed().as_secs_f32() * 1000.0;
            frames += 1;

            let since = last_stats.elapsed().as_secs_f32();
            if since >= 5.0 {
                let cost = tracker.take_stats(since);
                log.write(&format!("stats: {:.1} fps, work {:.2} ms/frame, {cost}, debris {}", frames as f32 / since, work_ms / frames.max(1) as f32, caps.debris_count()));
                frames = 0;
                work_ms = 0.0;
                last_stats = Instant::now();
            }
        }

        let _ = CloseHandle(timer);
        tracker.stop();
        screens.clear();
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
        log.write("exit");
        Ok(())
    }
}

pub fn main() {
    let mut log = Log::open().unwrap_or_else(|| std::process::exit(2));
    log.write(concat!("sugrob windows ", env!("CARGO_PKG_VERSION")));
    if let Err(error) = run(&mut log) {
        log.write(&format!("fatal: {error}"));
        let text: Vec<u16> = format!("Сугроб не запустился: {error}\0").encode_utf16().collect();
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), w!("Сугроб"), MB_ICONERROR);
        }
    }
}
