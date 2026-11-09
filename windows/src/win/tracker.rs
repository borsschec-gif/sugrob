//! Список окон из системы: порядок, видимая рамка, радиус углов, панель задач.
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use sugrob_core::sim::{dock_is_hidden, v2, Rect, Snapshot, TrackedWindow, V2};
use windows::core::{Result, BOOL};
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::desktop::DesktopIcons;
use super::Log;

static DIRTY: AtomicBool = AtomicBool::new(true);
static DRAGGED: AtomicIsize = AtomicIsize::new(0);

const IDLE_INTERVAL: f64 = 0.2;
/// Окно могло исчезнуть во время перетаскивания, и конца перетаскивания тогда не будет.
const DRAG_RELEASE_GRACE: Duration = Duration::from_millis(500);

unsafe extern "system" fn on_win_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, _: i32, _: u32, _: u32) {
    if object != OBJID_WINDOW.0 {
        return;
    }
    DIRTY.store(true, Ordering::Relaxed);
    if event == EVENT_SYSTEM_MOVESIZESTART {
        DRAGGED.store(hwnd.0 as isize, Ordering::Relaxed);
    } else if event == EVENT_SYSTEM_MOVESIZEEND {
        DRAGGED.store(0, Ordering::Relaxed);
    }
}

fn visible_frame(hwnd: HWND) -> Option<RECT> {
    let mut frame = RECT::default();
    unsafe {
        let got = DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut RECT as *mut c_void, std::mem::size_of::<RECT>() as u32);
        if got.is_err() {
            GetWindowRect(hwnd, &mut frame).ok()?;
        }
    }
    Some(frame)
}

/// Основная кнопка физически зажата. С настройкой для левши это правая.
fn is_mouse_down() -> bool {
    unsafe {
        let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
        GetAsyncKeyState(button.0 as i32) as u16 & 0x8000 != 0
    }
}

/// Монитор ищем по верхнему краю: скрытая панель почти вся заходит на монитор ниже.
fn taskbar_is_hidden(px: RECT, scale: f32) -> bool {
    unsafe {
        let top = POINT { x: px.left + (px.right - px.left) / 2, y: px.top };
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(MonitorFromPoint(top, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() && dock_is_hidden(px.top as f32 / scale, info.rcMonitor.bottom as f32 / scale)
    }
}

/// Радиус углов из документации DWM: 8 единиц у обычных окон, 4 у малых,
/// развёрнутые окна и Windows 10 без скругления.
fn corner_radius(hwnd: HWND, is_windows11: bool, scale: f32) -> f32 {
    unsafe {
        if !is_windows11 || IsZoomed(hwnd).as_bool() {
            return 0.0;
        }
        let mut preference = DWMWCP_DEFAULT;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &mut preference as *mut _ as *mut c_void, 4);
        let units = match preference {
            DWMWCP_DONOTROUND => 0.0,
            DWMWCP_ROUNDSMALL => 4.0,
            _ => 8.0,
        };
        units * GetDpiForWindow(hwnd) as f32 / 96.0 / scale
    }
}

struct EnumState {
    windows: Vec<TrackedWindow>,
    scale: f32,
    primary_area: f32,
    is_windows11: bool,
    classes: Vec<(String, f32)>,
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) } as usize;
    String::from_utf16_lossy(&buffer[..len])
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = &mut *(lparam.0 as *mut EnumState);
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return TRUE;
    }
    // Окна других рабочих столов и спящие приложения Store скрыты через cloak.
    let mut cloaked = 0u32;
    let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
    if cloaked != 0 {
        return TRUE;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    if ex & WS_EX_TRANSPARENT.0 != 0 {
        return TRUE;
    }
    if ex & WS_EX_LAYERED.0 != 0 {
        let (mut alpha, mut flags) = (255u8, LAYERED_WINDOW_ATTRIBUTES_FLAGS(0));
        if GetLayeredWindowAttributes(hwnd, None, Some(&mut alpha), Some(&mut flags)).is_ok() && flags.0 & LWA_ALPHA.0 != 0 && alpha < 3 {
            return TRUE;
        }
    }
    let class = class_name(hwnd);
    if matches!(class.as_str(), "Progman" | "WorkerW" | "SugrobOverlay") {
        return TRUE;
    }
    let Some(px) = visible_frame(hwnd) else { return TRUE };
    let s = state.scale;
    let frame = Rect { x: px.left as f32 / s, y: px.top as f32 / s, w: (px.right - px.left) as f32 / s, h: (px.bottom - px.top) as f32 / s };
    if frame.w < 60.0 && frame.h < 60.0 {
        return TRUE;
    }

    let is_dock = matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd");
    if is_dock {
        // Снег копится только на горизонтальной панели.
        if frame.w > frame.h {
            state.windows.push(TrackedWindow { id: hwnd.0 as u64, frame, owner: 0, holds_snow: true, is_dock: true, is_dock_hidden: taskbar_is_hidden(px, s), is_desktop_icon: false, corner_radius: Some(0.0), top_profile: None });
        }
        return TRUE;
    }
    if frame.w < 60.0 || frame.h < 60.0 {
        return TRUE;
    }

    let has_caption = style & WS_CAPTION.0 == WS_CAPTION.0;
    let is_owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid());
    let holds_snow = ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) == 0 && (has_caption || !is_owned) && class != "#32768";
    // Системные прозрачные окна на весь экран не должны стирать снег.
    if !holds_snow && frame.w * frame.h >= state.primary_area * 0.8 {
        return TRUE;
    }
    let radius = corner_radius(hwnd, state.is_windows11, s);
    if holds_snow {
        state.classes.push((class, radius));
    }
    let mut owner = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut owner));
    state.windows.push(TrackedWindow { id: hwnd.0 as u64, frame, owner, holds_snow, is_dock: false, is_dock_hidden: false, is_desktop_icon: false, corner_radius: Some(radius), top_profile: None });
    TRUE
}

#[derive(Default)]
struct DragMeter {
    hwnd: isize,
    grab: (f32, f32),
    size: (i32, i32),
    resized: bool,
    last_cursor: (f32, f32),
    path: f32,
    lags: Vec<f32>,
    started: Option<Instant>,
    released: Option<Instant>,
}

impl DragMeter {
    fn update(&mut self, log: &mut Log) {
        let current = DRAGGED.load(Ordering::Relaxed);
        if current != self.hwnd {
            self.finish(log);
            *self = DragMeter { hwnd: current, ..DragMeter::default() };
        }
        if current == 0 {
            return;
        }
        self.released = if is_mouse_down() { None } else { self.released.or(Some(Instant::now())) };
        let let_go = self.released.is_some_and(|at| at.elapsed() >= DRAG_RELEASE_GRACE);
        if let_go || !unsafe { IsWindow(Some(HWND(current as *mut c_void))) }.as_bool() {
            // Настоящий конец перетаскивания мог успеть раньше: чужое значение не трогаем.
            let _ = DRAGGED.compare_exchange(current, 0, Ordering::Relaxed, Ordering::Relaxed);
            self.finish(log);
            *self = DragMeter::default();
            return;
        }
        let Some(frame) = visible_frame(HWND(current as *mut c_void)) else { return };
        let mut point = POINT::default();
        if unsafe { GetCursorPos(&mut point) }.is_err() {
            return;
        }
        let cursor = (point.x as f32, point.y as f32);
        let size = (frame.right - frame.left, frame.bottom - frame.top);
        let offset = (cursor.0 - frame.left as f32, cursor.1 - frame.top as f32);
        if self.started.is_none() {
            self.started = Some(Instant::now());
            self.grab = offset;
            self.size = size;
            self.last_cursor = cursor;
            return;
        }
        self.resized |= size != self.size;
        self.path += (cursor.0 - self.last_cursor.0).hypot(cursor.1 - self.last_cursor.1);
        self.last_cursor = cursor;
        if !self.resized {
            self.lags.push((offset.0 - self.grab.0).hypot(offset.1 - self.grab.1));
        }
    }

    fn finish(&mut self, log: &mut Log) {
        let Some(started) = self.started else { return };
        let seconds = started.elapsed().as_secs_f32().max(0.001);
        let speed = self.path / seconds;
        let average = self.lags.iter().sum::<f32>() / self.lags.len().max(1) as f32;
        let max = self.lags.iter().cloned().fold(0.0, f32::max);
        let ms = if speed > 1.0 { average / speed * 1000.0 } else { 0.0 };
        log.write(&format!("drag[{}]: {seconds:.2} s, cursor {speed:.0} px/s, lag avg {average:.1} px, max {max:.1} px, ~{ms:.0} ms", if self.resized { "resize" } else { "move" }));
    }
}

fn install_hooks() -> Vec<HWINEVENTHOOK> {
    let ranges = [
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
        (EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_REORDER, EVENT_OBJECT_REORDER),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
    ];
    ranges.iter().map(|&(a, b)| unsafe { SetWinEventHook(a, b, None, Some(on_win_event), 0, 0, WINEVENT_OUTOFCONTEXT) }).collect()
}

pub struct Tracker {
    hooks: Vec<HWINEVENTHOOK>,
    snapshot: Snapshot,
    last_poll: f64,
    is_windows11: bool,
    meter: DragMeter,
    logged_classes: HashSet<String>,
    polls: u32,
    cost_ms: f32,
    cost_max: f32,
    desktop: DesktopIcons,
    folders: Vec<TrackedWindow>,
}

impl Tracker {
    pub fn new(log: &Log) -> Result<Self> {
        let mut version = OSVERSIONINFOW { dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32, ..Default::default() };
        unsafe {
            let _ = RtlGetVersion(&mut version);
        }
        Ok(Tracker {
            hooks: install_hooks(),
            snapshot: Snapshot::default(),
            last_poll: -1.0,
            is_windows11: version.dwBuildNumber >= 22000,
            meter: DragMeter::default(),
            logged_classes: HashSet::new(),
            polls: 0,
            cost_ms: 0.0,
            cost_max: 0.0,
            desktop: DesktopIcons::start(log),
            folders: Vec::new(),
        })
    }

    pub fn stop(&mut self) {
        for hook in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
    }

    /// На паузе подписки снимаются, иначе каждое движение в системе будило бы поток.
    pub fn suspend(&mut self) {
        self.stop();
        DRAGGED.store(0, Ordering::Relaxed);
        self.desktop.set_paused(true);
    }

    pub fn resume(&mut self) {
        if self.hooks.is_empty() {
            self.hooks = install_hooks();
        }
        self.desktop.set_paused(false);
        DIRTY.store(true, Ordering::Relaxed);
    }

    pub fn is_dragging(&self) -> bool {
        DRAGGED.load(Ordering::Relaxed) != 0
    }

    pub fn cursor(&self, scale: f32) -> V2 {
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
        }
        v2(point.x as f32 / scale, point.y as f32 / scale)
    }

    /// Новый снимок только при событиях, зажатой кнопке или раз в 200 мс.
    pub fn poll(&mut self, now: f64, scale: f32, log: &mut Log) -> &Snapshot {
        self.meter.update(log);
        let down = is_mouse_down();
        // Пока окна двигают, обход иконок откладывается, как на Mac.
        self.desktop.set_state(scale, down || self.is_dragging());
        let fresh = self.desktop.take_fresh();
        let has_fresh = fresh.is_some();
        if let Some(list) = fresh {
            self.folders = list;
        }
        if !DIRTY.swap(false, Ordering::Relaxed) && !has_fresh && !down && !self.snapshot.is_mouse_down && now - self.last_poll < IDLE_INTERVAL {
            return &self.snapshot;
        }
        self.last_poll = now;
        let started = Instant::now();
        let (primary_w, primary_h, screen_bottom) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32 / scale, GetSystemMetrics(SM_CYSCREEN) as f32 / scale, (GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN)) as f32 / scale) };
        let mut state = EnumState { windows: Vec::with_capacity(64), scale, primary_area: primary_w * primary_h, is_windows11: self.is_windows11, classes: Vec::new() };
        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut state as *mut EnumState as isize));
        }
        // Иконки лежат под всеми окнами, поэтому в конце списка.
        state.windows.extend(self.folders.iter().cloned());
        for (class, radius) in state.classes.drain(..) {
            if self.logged_classes.insert(class.clone()) {
                log.write(&format!("corner: {class} {radius:.1} pt"));
            }
        }
        self.snapshot = Snapshot { timestamp: now, windows: state.windows, is_mouse_down: down, cursor: self.cursor(scale), screen_bottom };
        let cost = started.elapsed().as_secs_f32() * 1000.0;
        self.polls += 1;
        self.cost_ms += cost;
        self.cost_max = self.cost_max.max(cost);
        &self.snapshot
    }

    pub fn take_stats(&mut self, seconds: f32) -> String {
        let text = format!(
            "enum {:.1}/s avg {:.2} ms max {:.2} ms, windows {}",
            self.polls as f32 / seconds,
            self.cost_ms / self.polls.max(1) as f32,
            self.cost_max,
            self.snapshot.windows.len()
        );
        self.polls = 0;
        self.cost_ms = 0.0;
        self.cost_max = 0.0;
        text
    }
}
