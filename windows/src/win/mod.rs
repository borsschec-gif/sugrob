//! Часть под Windows: окна, отрисовка, трей.
mod desktop;
mod gift;
mod governor;
mod render;
mod tracker;
mod tray;
mod update;

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sugrob_core::sim::{v2, CapField, Rect, Santa, SnowField};
use windows::core::{w, Error, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use gift::GiftLayer;
use render::{Renderer, Target};
use tracker::Tracker;

static QUIT: AtomicBool = AtomicBool::new(false);
pub const OVERLAY_CLASS: PCWSTR = w!("SugrobOverlay");
const RESTORE_RETRY: Duration = Duration::from_secs(1);
/// Столько новая версия должна проработать с кадрами, чтобы старую можно было удалить.
const CONFIRM_AFTER: Duration = Duration::from_secs(60);
const CONFIRM_FRAMES: u32 = 60;

fn log_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Sugrob").join("spike.log"))
}

pub struct Log(File);
impl Log {
    fn open() -> Option<Self> {
        let path = log_path()?;
        fs::create_dir_all(path.parent()?).ok()?;
        File::create(path).ok().map(Log)
    }
    /// Второй поток пишет в тот же файл через свою копию.
    fn try_clone(&self) -> Option<Self> {
        self.0.try_clone().ok().map(Log)
    }
    pub fn write(&mut self, line: &str) {
        // Одной записью, чтобы строки двух потоков не перемешивались.
        let _ = self.0.write_all(format!("{line}\n").as_bytes());
        let _ = self.0.flush();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
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

/// Границы основного монитора в точках: Дед Мороз ездит только по нему.
fn primary_bounds(list: &[Monitor], scale: f32) -> Rect {
    let found = list.iter().find(|m| m.rect.left == 0 && m.rect.top == 0).or(list.first());
    found.map_or(Rect::default(), |m| Rect { x: m.rect.left as f32 / scale, y: m.rect.top as f32 / scale, w: (m.rect.right - m.rect.left) as f32 / scale, h: (m.rect.bottom - m.rect.top) as f32 / scale })
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

/// Всё графическое заново; снег на окнах и Дед Мороз живут отдельно и остаются.
fn restore(renderer: &mut Renderer, list: &mut Vec<Monitor>, scale: &mut f32, seed: u64, log: &mut Log) -> Result<Vec<Screen>> {
    let fresh = Renderer::new(log)?;
    *list = monitors();
    *scale = primary_scale();
    let screens = build_screens(&fresh, list, *scale, seed, log)?;
    *renderer = fresh;
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

        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        let mut renderer = Renderer::new(log)?;
        let mut monitor_list = monitors();
        let mut scale = primary_scale();
        let mut screens = build_screens(&renderer, &monitor_list, scale, seed, log)?;
        let mut caps = CapField::new(seed ^ 0x5555);
        let mut santa = Santa::new(seed ^ 0xA5A5);
        let mut gifts = GiftLayer::new();
        let mut tracker = Tracker::new(log)?;

        let base_fps: f32 = std::env::var("SUGROB_FPS").ok().and_then(|v| v.parse().ok()).unwrap_or(60.0);
        // Пока окно тащат, шапка должна идти за ним с частотой экрана, иначе она дёргается.
        let drag_fps = refresh_rate().max(base_fps);
        log.write(&format!("fps: {base_fps}, while dragging {drag_fps}"));

        // Меню и опрос системы живут в своём потоке: открытое меню не останавливает снег.
        let tray_log = log.try_clone().ok_or_else(|| Error::new(E_FAIL, "лог не открывается второй раз"))?;
        let _tray = tray::Tray::start(base_fps, tray_log)?;
        match log.try_clone() {
            Some(update_log) => update::start(update_log),
            None => log.write("update: лог не открылся, обновления выключены"),
        }

        let timer = CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0)?;
        let started = Instant::now();
        let mut next = started;
        let mut previous = started;
        let mut last_stats = started;
        let mut last_second = started;
        let (mut frames, mut work_ms) = (0u32, 0f32);
        let mut is_paused = false;
        let mut show_after_draw = false;
        // Устройство потеряно: новая попытка не раньше этого времени.
        let mut retry: Option<Instant> = None;
        let mut restore_error = String::new();
        let mut drawn = 0u32;
        let mut confirmed = false;

        'main: loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = DispatchMessageW(&msg);
            }
            if QUIT.load(Ordering::Relaxed) {
                break 'main;
            }

            let paused = governor::paused();
            if paused != is_paused {
                is_paused = paused;
                if paused {
                    for screen in &screens {
                        screen.target.set_visible(false);
                    }
                    tracker.suspend();
                    santa.remove_gift();
                    gifts.sync(None, scale, log);
                } else {
                    tracker.resume();
                    let _ = tracker.take_stats(1.0);
                    let now = Instant::now();
                    next = now;
                    previous = now;
                    last_stats = now;
                    // За паузу мониторы могли смениться: проверка сразу, на первом кадре.
                    last_second = started;
                    (frames, work_ms) = (0, 0.0);
                    show_after_draw = true;
                }
            }
            if is_paused {
                // Поток спит: будит сообщение из трея, таймаут только на всякий случай.
                let _ = MsgWaitForMultipleObjects(None, false, 1000, QS_ALLINPUT);
                continue;
            }

            if let Some(at) = retry {
                let moment = Instant::now();
                if moment >= at {
                    match restore(&mut renderer, &mut monitor_list, &mut scale, seed, log) {
                        Ok(list) => {
                            screens = list;
                            retry = None;
                            restore_error.clear();
                            log.write("render: устройство восстановлено");
                            next = moment;
                            previous = moment;
                        }
                        Err(error) => {
                            // Раз в секунду, поэтому в лог только новая причина.
                            let text = error.to_string();
                            if text != restore_error {
                                log.write(&format!("render: не восстановилось: {text}"));
                                restore_error = text;
                            }
                            retry = Some(Instant::now() + RESTORE_RETRY);
                        }
                    }
                }
                if retry.is_some() {
                    // Сообщения из трея разбираются и сейчас: выйти можно всегда.
                    let _ = MsgWaitForMultipleObjects(None, false, RESTORE_RETRY.as_millis() as u32, QS_ALLINPUT);
                    continue;
                }
            }

            let frame_start = Instant::now();
            if frame_start < next {
                let due = -(((next - frame_start).as_nanos() / 100) as i64);
                let _ = SetWaitableTimer(timer, &due, 0, None, None, false);
                let _ = MsgWaitForMultipleObjects(Some(&[timer]), false, INFINITE, QS_ALLINPUT);
                continue;
            }
            let fps = if tracker.is_dragging() { drag_fps } else { governor::fps() };
            let interval = Duration::from_secs_f32(1.0 / fps);
            next = (next + interval).max(frame_start);

            if frame_start - last_second >= Duration::from_secs(1) {
                last_second = frame_start;
                for screen in &screens {
                    screen.target.keep_on_top();
                }
                gifts.keep_on_top();
                if !renderer.is_alive() {
                    log.write("render: устройство потеряно");
                    screens.clear();
                    retry = Some(frame_start);
                    continue;
                }
                // Монитор подключили, сняли или поменяли масштаб.
                let (now_list, now_scale) = (monitors(), primary_scale());
                if now_list != monitor_list || now_scale != scale {
                    monitor_list = now_list;
                    scale = now_scale;
                    screens.clear();
                    match build_screens(&renderer, &monitor_list, scale, seed, log) {
                        Ok(list) => screens = list,
                        Err(error) => {
                            log.write(&format!("render: оверлеи не создались: {error}"));
                            retry = Some(frame_start);
                            continue;
                        }
                    }
                }
            }

            let dt = (frame_start - previous).as_secs_f32().min(1.0 / 15.0);
            previous = frame_start;
            let now = (frame_start - started).as_secs_f64() + 1.0;
            let time = (now % 100_000.0) as f32;

            let snapshot = tracker.poll(now, scale, log);
            caps.sync(snapshot, now);
            caps.cursor_effects = governor::cursor_effects();
            let density = governor::density();
            for screen in &mut screens {
                screen.field.density = density;
                screen.field.step(dt, time, &mut caps, screen.origin, now);
            }
            caps.relax(now);
            caps.brush(tracker.cursor(scale), now);
            caps.step_debris(now);

            let primary = primary_bounds(&monitor_list, scale);
            if gifts.take_click() {
                if let Some(opened) = santa.open_gift(&mut caps) {
                    gifts.show_phrase(opened.phrase, opened.center, primary, scale, log);
                }
            }
            santa.auto_rides = governor::santa();
            if governor::take_santa_call() {
                santa.ride_now(primary);
            }
            santa.tick(now, primary, &mut caps);
            gifts.sync(santa.gift_position(), scale, log);

            let failed = screens.iter().find_map(|screen| renderer.draw(&screen.target, &screen.field, &caps, santa.draw_item(screen.origin), screen.origin, screen.viewport, time).err());
            if let Some(error) = failed {
                log.write(&format!("render: кадр не отрисован: {error}"));
                screens.clear();
                retry = Some(Instant::now());
                continue;
            }
            drawn = drawn.saturating_add(1);
            if !confirmed && drawn >= CONFIRM_FRAMES && frame_start - started >= CONFIRM_AFTER {
                confirmed = true;
                update::confirm(log);
            }
            // Показываем уже новый кадр, а не тот, что остался с начала паузы.
            if show_after_draw {
                show_after_draw = false;
                for screen in &screens {
                    screen.target.set_visible(true);
                    screen.target.keep_on_top();
                }
                gifts.keep_on_top();
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
        log.write("exit");
        Ok(())
    }
}

/// `Local\`: на сеанс пользователя. Живёт до конца процесса, снимает его система.
enum Instance {
    First,
    AlreadyRunning,
    Unknown(Error),
}

fn claim_single_instance() -> Instance {
    unsafe {
        match CreateMutexW(None, false, w!("Local\\SugrobSingleInstance")) {
            Ok(_) if GetLastError() == ERROR_ALREADY_EXISTS => Instance::AlreadyRunning,
            Ok(_) => Instance::First,
            // Мьютекс чужого уровня целостности: он есть, значит копия уже работает.
            Err(error) if error.code() == E_ACCESSDENIED => Instance::AlreadyRunning,
            Err(error) => Instance::Unknown(error),
        }
    }
}

pub fn main() {
    // Новая версия сперва дожидается старой, иначе сочла бы её второй копией.
    update::wait_for_previous();
    let instance = claim_single_instance();
    if matches!(instance, Instance::AlreadyRunning) {
        return;
    }
    let mut log = Log::open().unwrap_or_else(|| std::process::exit(2));
    log.write(concat!("sugrob windows ", env!("CARGO_PKG_VERSION")));
    if let Instance::Unknown(error) = instance {
        log.write(&format!("мьютекс не создан, вторая копия не исключена: {error}"));
    }
    update::clean_up(&mut log);
    let result = run(&mut log);
    // Иначе процесс мог бы завершиться между двумя переименованиями exe.
    update::finish_install();
    if let Err(error) = result {
        log.write(&format!("fatal: {error}"));
        if update::roll_back(&mut log) {
            // Старая версия начнёт лог заново, а причину отката надо сохранить.
            if let Some(path) = log_path() {
                let _ = fs::copy(&path, path.with_file_name("spike-rollback.log"));
            }
            return;
        }
        let text: Vec<u16> = format!("Сугроб не запустился: {error}\0").encode_utf16().collect();
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), w!("Сугроб"), MB_ICONERROR);
        }
    }
}
