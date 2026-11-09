//! Папки на рабочем столе. Обход идёт в своём потоке: Проводник может зависнуть,
//! а кадр ждать его не должен. Перезапуск Проводника переживается сам: связь
//! с ним берётся заново на каждый обход.
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sugrob_core::sim::{desktop_icon_id, holds_folder_snow, icon_shape, icon_side, IconShape, Rect, TrackedWindow};
use windows::core::{w, Error, Interface, Result, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::SystemServices::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi, SystemParametersInfoForDpi};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::Log;

const REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const MAX_SHAPES: usize = 512;

struct Shared {
    fresh: Mutex<Option<Vec<TrackedWindow>>>,
    busy: AtomicBool,
    paused: AtomicBool,
    scale_bits: AtomicU32,
    /// Событие будит поток; хранится числом, потому что HANDLE нельзя передать между потоками.
    wake: isize,
}

impl Shared {
    fn wake(&self) {
        unsafe {
            let _ = SetEvent(HANDLE(self.wake as *mut c_void));
        }
    }
}

/// Сторона кадра: состояние для потока и готовый список папок.
pub struct DesktopIcons {
    shared: Option<Arc<Shared>>,
}

impl DesktopIcons {
    /// Если поток не запустился, снега на папках просто не будет.
    pub fn start(log: &Log) -> Self {
        DesktopIcons { shared: Self::spawn(log) }
    }

    fn spawn(log: &Log) -> Option<Arc<Shared>> {
        let mut worker_log = log.try_clone()?;
        let wake = match unsafe { CreateEventW(None, false, false, PCWSTR::null()) } {
            Ok(handle) => handle.0 as isize,
            Err(error) => {
                worker_log.write(&format!("desktop: no event: {error}"));
                return None;
            }
        };
        let shared = Arc::new(Shared { fresh: Mutex::new(None), busy: AtomicBool::new(false), paused: AtomicBool::new(false), scale_bits: AtomicU32::new(0), wake });
        let for_worker = shared.clone();
        // Завершается вместе с процессом: ждать зависший Проводник при выходе нельзя.
        match std::thread::Builder::new().name("desktop-icons".into()).spawn(move || worker(for_worker, worker_log)) {
            Ok(_) => Some(shared),
            Err(_) => None,
        }
    }

    /// Вызывается с кадра, только атомики. Событие — лишь при смене состояния.
    pub fn set_state(&self, scale: f32, busy: bool) {
        let Some(shared) = &self.shared else { return };
        let bits = scale.to_bits();
        let was_busy = shared.busy.swap(busy, Ordering::Relaxed);
        if shared.scale_bits.swap(bits, Ordering::Relaxed) != bits || (was_busy && !busy) {
            shared.wake();
        }
    }

    pub fn set_paused(&self, paused: bool) {
        let Some(shared) = &self.shared else { return };
        shared.paused.store(paused, Ordering::Relaxed);
        shared.wake();
    }

    /// Не ждёт: если поток как раз кладёт результат, заберём на следующем кадре.
    pub fn take_fresh(&self) -> Option<Vec<TrackedWindow>> {
        self.shared.as_ref()?.fresh.try_lock().ok()?.take()
    }
}

fn worker(shared: Arc<Shared>, mut log: Log) {
    // Оболочка требует STA; сообщения поток разбирает сам, иначе рассылки системы ждали бы его.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if init.is_err() {
        log.write(&format!("desktop: com failed: {init:?}"));
        return;
    }
    let automation: IUIAutomation = match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
        Ok(a) => a,
        Err(error) => {
            log.write(&format!("desktop: no ui automation: {error}"));
            return;
        }
    };
    let mut reader = Reader { automation, shapes: HashMap::new(), status: String::new(), logged_status: String::new(), last_error: String::new() };
    let mut last_run: Option<Instant> = None;
    let mut published: Option<Vec<TrackedWindow>> = None;
    loop {
        let bits = shared.scale_bits.load(Ordering::Relaxed);
        let scale = f32::from_bits(bits);
        let blocked = bits == 0 || shared.paused.load(Ordering::Relaxed) || shared.busy.load(Ordering::Relaxed);
        let wait = last_run.map_or(Duration::ZERO, |t| REFRESH_INTERVAL.saturating_sub(t.elapsed()));
        if blocked || !wait.is_zero() {
            let timeout = if blocked { INFINITE } else { wait.as_millis() as u32 + 1 };
            pump_and_wait(shared.wake, timeout);
            continue;
        }
        last_run = Some(Instant::now());
        match reader.read(scale) {
            Ok(list) => {
                reader.last_error.clear();
                if published.as_ref() != Some(&list) {
                    published = Some(list.clone());
                    if let Ok(mut slot) = shared.fresh.lock() {
                        *slot = Some(list);
                    }
                }
            }
            // Проводник перезапускается или ещё не готов: старый список остаётся.
            Err(error) => {
                let text = error.to_string();
                if text != reader.last_error {
                    log.write(&format!("desktop: {text}"));
                    reader.last_error = text;
                }
            }
        }
        if reader.status != reader.logged_status {
            log.write(&reader.status);
            reader.logged_status = reader.status.clone();
        }
    }
}

fn pump_and_wait(wake: isize, timeout: u32) {
    unsafe {
        let _ = MsgWaitForMultipleObjects(Some(&[HANDLE(wake as *mut c_void)]), false, timeout, QS_ALLINPUT);
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = DispatchMessageW(&msg);
        }
    }
}

/// Ссылка на элемент, которую выдала оболочка и которую надо вернуть.
struct Pidl(*mut ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const c_void)) };
    }
}

fn take_string(text: PWSTR) -> String {
    unsafe {
        let value = text.to_string().unwrap_or_default();
        CoTaskMemFree(Some(text.0 as *const c_void));
        value
    }
}

struct Desktop {
    view: IFolderView2,
    folder: IShellFolder,
    list: HWND,
}

/// Документированный путь Рэймонда Чена к виду рабочего стола.
unsafe fn find_desktop() -> Result<Desktop> {
    let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
    let (location, root) = (VARIANT::from(0i32), VARIANT::default());
    let mut hwnd = 0i32;
    let dispatch = windows.FindWindowSW(&location, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)?;
    let service: IServiceProvider = dispatch.cast()?;
    let browser: IShellBrowser = service.QueryService(&SID_STopLevelBrowser)?;
    let shell_view = browser.QueryActiveShellView()?;
    let view: IFolderView2 = shell_view.cast()?;
    let folder: IShellFolder = view.GetFolder()?;
    let list = FindWindowExW(Some(shell_view.GetWindow()?), None, w!("SysListView32"), PCWSTR::null())?;
    Ok(Desktop { view, folder, list })
}

struct Folder {
    path: String,
    name: String,
    position: POINT,
    item: IShellItem,
}

unsafe fn read_folders(desktop: &Desktop) -> Result<Vec<Folder>> {
    let view = &desktop.view;
    let mut result = Vec::new();
    for index in 0..view.ItemCount(SVGIO_ALLVIEW)? {
        let Ok(raw) = view.Item(index) else { continue };
        let pidl = Pidl(raw);
        let Ok(item) = SHCreateItemWithParent::<_, IShellItem>(None, &desktop.folder, pidl.0) else { continue };
        let mask = SFGAO_FOLDER | SFGAO_FILESYSTEM | SFGAO_STREAM | SFGAO_LINK;
        let Ok(attrs) = item.GetAttributes(mask) else { continue };
        let has = |flag: SFGAO_FLAGS| attrs.0 & flag.0 != 0;
        if !holds_folder_snow(has(SFGAO_FOLDER), has(SFGAO_FILESYSTEM), has(SFGAO_STREAM), has(SFGAO_LINK)) {
            continue;
        }
        let (Ok(path), Ok(name), Ok(position)) = (item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING), item.GetDisplayName(SIGDN_NORMALDISPLAY), view.GetItemPosition(pidl.0)) else { continue };
        result.push(Folder { path: take_string(path), name: take_string(name), position, item });
    }
    Ok(result)
}

/// Настоящие рамки подписанных элементов списка.
unsafe fn read_item_rects(automation: &IUIAutomation, list: HWND) -> Result<Vec<(String, RECT)>> {
    let cache = automation.CreateCacheRequest()?;
    cache.AddProperty(UIA_NamePropertyId)?;
    cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
    let element = automation.ElementFromHandle(list)?;
    let items = element.FindAllBuildCache(TreeScope_Children, &automation.CreateTrueCondition()?, &cache)?;
    let mut result = Vec::new();
    for index in 0..items.Length()? {
        let Ok(item) = items.GetElement(index) else { continue };
        let (Ok(name), Ok(rect)) = (item.CachedName(), item.CachedBoundingRectangle()) else { continue };
        if rect.right > rect.left && rect.bottom > rect.top {
            result.push((name.to_string(), rect));
        }
    }
    Ok(result)
}

/// Высота строки подписи из настоящего шрифта подписей значков.
unsafe fn label_height(dpi: u32) -> f32 {
    let mut font = LOGFONTW::default();
    if SystemParametersInfoForDpi(SPI_GETICONTITLELOGFONT.0, std::mem::size_of::<LOGFONTW>() as u32, Some(&mut font as *mut LOGFONTW as *mut c_void), 0, dpi).is_err() {
        return 0.0;
    }
    let handle = CreateFontIndirectW(&font);
    if handle.is_invalid() {
        return 0.0;
    }
    let dc = GetDC(None);
    let old = SelectObject(dc, handle.into());
    let mut metrics = TEXTMETRICW::default();
    let got = GetTextMetricsW(dc, &mut metrics).as_bool();
    SelectObject(dc, old);
    ReleaseDC(None, dc);
    let _ = DeleteObject(handle.into());
    if got {
        metrics.tmHeight as f32
    } else {
        0.0
    }
}

/// Альфа-канал настоящей иконки ровно в том размере, в каком её рисует рабочий стол.
unsafe fn read_shape(item: &IShellItem, side: i32) -> Option<IconShape> {
    let factory: IShellItemImageFactory = item.cast().ok()?;
    let bitmap = factory.GetImage(SIZE { cx: side, cy: side }, SIIGBF_ICONONLY).ok()?;
    let shape = alpha_of(bitmap);
    let _ = DeleteObject(bitmap.into());
    let (alpha, width, height) = shape?;
    icon_shape(&alpha, width, height)
}

unsafe fn alpha_of(bitmap: HBITMAP) -> Option<(Vec<u8>, usize, usize)> {
    let mut object = BITMAP::default();
    if GetObjectW(bitmap.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut object as *mut BITMAP as *mut c_void)) == 0 {
        return None;
    }
    let (width, height) = (object.bmWidth, object.bmHeight.abs());
    if width <= 0 || height <= 0 {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Отрицательная высота: строки сверху вниз.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let (w, h) = (width as usize, height as usize);
    let mut pixels = vec![0u8; w * h * 4];
    let dc = GetDC(None);
    let lines = GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
    ReleaseDC(None, dc);
    if lines != height {
        return None;
    }
    Some((pixels.chunks_exact(4).map(|p| p[3]).collect(), w, h))
}

struct Reader {
    automation: IUIAutomation,
    shapes: HashMap<(String, i32), IconShape>,
    status: String,
    logged_status: String,
    last_error: String,
}

impl Reader {
    fn read(&mut self, scale: f32) -> Result<Vec<TrackedWindow>> {
        unsafe {
            let desktop = find_desktop()?;
            // Галка «Отображать значки рабочего стола» и настройка видимости окна.
            let flags = desktop.view.GetCurrentFolderFlags().unwrap_or(0);
            if flags & FWF_NOICONS.0 as u32 != 0 || !IsWindowVisible(desktop.list).as_bool() {
                return Ok(Vec::new());
            }
            let folders = read_folders(&desktop)?;
            if folders.is_empty() {
                return Ok(Vec::new());
            }
            let mut rects = read_item_rects(&self.automation, desktop.list)?;
            if rects.is_empty() {
                return Err(Error::new(E_FAIL, "ui automation returned no items"));
            }

            let dpi = match GetDpiForWindow(desktop.list) {
                0 => 96,
                dpi => dpi,
            };
            let (mut mode, mut reported) = (FOLDERVIEWMODE::default(), 0i32);
            desktop.view.GetViewModeAndIconSize(&mut mode, &mut reported)?;
            let shortest = rects.iter().map(|(_, r)| r.bottom - r.top).min().unwrap_or(0) as f32;
            let label = label_height(dpi);
            let side = icon_side(reported as f32, dpi as f32 / 96.0, shortest, label).round() as i32;
            if side <= 0 {
                return Err(Error::new(E_FAIL, "icon size is unknown"));
            }
            let edge = GetSystemMetricsForDpi(SM_CYEDGE, dpi);
            let mut origin = POINT::default();
            let _ = ClientToScreen(desktop.list, &mut origin);

            if self.shapes.len() > MAX_SHAPES {
                self.shapes.clear();
            }
            let mut result = Vec::with_capacity(folders.len());
            let mut skipped = 0;
            for folder in &folders {
                // Рамка с тем же именем, ближайшая к положению из оболочки.
                let target = (folder.position.x + origin.x, folder.position.y + origin.y);
                let found = rects.iter().enumerate().filter(|(_, (name, _))| *name == folder.name).min_by_key(|(_, (_, r))| (r.left - target.0).abs() + (r.top - target.1).abs()).map(|(i, _)| i);
                let Some(found) = found else {
                    skipped += 1;
                    continue;
                };
                let (_, rect) = rects.swap_remove(found);
                let key = (folder.path.clone(), side);
                if !self.shapes.contains_key(&key) {
                    match read_shape(&folder.item, side) {
                        Some(shape) => {
                            self.shapes.insert(key.clone(), shape);
                        }
                        None => {
                            skipped += 1;
                            continue;
                        }
                    }
                }
                let Some(shape) = self.shapes.get(&key) else { continue };
                // Значок стоит по центру подписи и сверху элемента, с отступом в рамку.
                let image_left = (rect.left + rect.right - side) as f32 / 2.0;
                let image_top = (rect.top + edge) as f32;
                let image = Rect { x: image_left / scale, y: image_top / scale, w: side as f32 / scale, h: side as f32 / scale };
                let (frame, profile) = shape.place(image);
                result.push(TrackedWindow { id: desktop_icon_id(&folder.path), frame, owner: 0, holds_snow: true, is_dock: false, is_dock_hidden: false, is_desktop_icon: true, corner_radius: None, top_profile: Some(profile) });
            }
            self.status = format!("desktop: {} folders, {skipped} skipped, icon {side} px (reported {reported}, dpi {dpi}, item {shortest}, label {label}, edge {edge})", result.len());
            Ok(result)
        }
    }
}
