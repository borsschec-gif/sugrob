//! Этап 0: проверка оверлея на настоящей Windows.
//! Снег падает поверх экрана, клики проходят насквозь, на верхних краях окон лежит полоса.
//! Замеры пишутся в %LOCALAPPDATA%\Sugrob\spike.log.
#![windows_subsystem = "windows"]

use std::ffi::c_void;
use std::fs::{self, File};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use windows::core::{s, w, Interface, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const MENU_QUIT: usize = 1;
const CAP_HEIGHT: f32 = 14.0;
const FLAKES: usize = 400;
const MAX_VERTICES: usize = 6 * 4096;

static DIRTY: AtomicBool = AtomicBool::new(true);
static QUIT: AtomicBool = AtomicBool::new(false);
static DRAGGED: AtomicIsize = AtomicIsize::new(0);

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    x: f32,
    y: f32,
    u: f32,
    v: f32,
    alpha: f32,
    round: f32,
}

#[derive(Clone, Copy)]
struct Tracked {
    frame: RECT,
}

const SHADER: &str = r#"
struct VIn { float2 pos: POSITION; float2 uv: TEXCOORD0; float alpha: TEXCOORD1; float round_: TEXCOORD2; };
struct VOut { float4 pos: SV_Position; float2 uv: TEXCOORD0; float alpha: TEXCOORD1; float round_: TEXCOORD2; };
VOut vs(VIn i) { VOut o; o.pos = float4(i.pos, 0, 1); o.uv = i.uv; o.alpha = i.alpha; o.round_ = i.round_; return o; }
float4 ps(VOut i): SV_Target {
    float a = i.alpha;
    if (i.round_ > 0.5) a *= saturate((1 - length(i.uv)) * 3);
    return float4(a, a, a, a);
}
"#;

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 % 100_000) as f32 / 100_000.0
    }
}

struct Flake {
    x: f32,
    y: f32,
    size: f32,
    speed: f32,
    phase: f32,
}

struct Log(File);
impl Log {
    fn open() -> Option<Self> {
        let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Sugrob");
        fs::create_dir_all(&dir).ok()?;
        File::create(dir.join("spike.log")).ok().map(Log)
    }
    fn write(&mut self, line: &str) {
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
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
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
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()?;
    }
    Some(frame)
}

struct EnumState {
    own: [HWND; 2],
    windows: Vec<Tracked>,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = &mut *(lparam.0 as *mut EnumState);
    if state.own.contains(&hwnd) || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return TRUE;
    }
    let mut cloaked = 0u32;
    let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
    if cloaked != 0 {
        return TRUE;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & (WS_EX_TRANSPARENT.0 | WS_EX_TOOLWINDOW.0) != 0 {
        return TRUE;
    }
    let mut class = [0u16; 64];
    let len = GetClassNameW(hwnd, &mut class) as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        return TRUE;
    }
    if let Some(frame) = visible_frame(hwnd) {
        if frame.right - frame.left >= 60 && frame.bottom - frame.top >= 60 {
            state.windows.push(Tracked { frame });
        }
    }
    TRUE
}

/// Участки верхнего края окна с индексом `index`, не закрытые окнами перед ним.
fn visible_top_segments(windows: &[Tracked], index: usize) -> Vec<(i32, i32)> {
    let frame = windows[index].frame;
    let mut segments = vec![(frame.left, frame.right)];
    for front in &windows[..index] {
        let f = front.frame;
        if f.top <= frame.top && frame.top < f.bottom {
            segments = segments
                .into_iter()
                .flat_map(|(a, b)| {
                    let mut pieces = Vec::new();
                    if a < f.left {
                        pieces.push((a, b.min(f.left)));
                    }
                    if b > f.right {
                        pieces.push((a.max(f.right), b));
                    }
                    pieces
                })
                .collect();
        }
    }
    segments
}

struct Quads {
    out: Vec<Vertex>,
    width: f32,
    height: f32,
}

impl Quads {
    fn add(&mut self, x: f32, y: f32, w: f32, h: f32, alpha: f32, round: f32) {
        if self.out.len() + 6 > MAX_VERTICES {
            return;
        }
        let nx = |px: f32| px / self.width * 2.0 - 1.0;
        let ny = |py: f32| 1.0 - py / self.height * 2.0;
        let v = |px: f32, py: f32, u: f32, tv: f32| Vertex { x: nx(px), y: ny(py), u, v: tv, alpha, round };
        let quad = [
            v(x, y, -1.0, -1.0),
            v(x + w, y, 1.0, -1.0),
            v(x, y + h, -1.0, 1.0),
            v(x, y + h, -1.0, 1.0),
            v(x + w, y, 1.0, -1.0),
            v(x + w, y + h, 1.0, 1.0),
        ];
        self.out.extend_from_slice(&quad);
    }
}

fn compile(source: &str, entry: windows::core::PCSTR, target: windows::core::PCSTR) -> Result<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    let result = unsafe {
        D3DCompile(
            source.as_ptr() as *const c_void,
            source.len(),
            None,
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    result?;
    Ok(code.expect("шейдер скомпилирован"))
}

unsafe fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
}

#[derive(Default)]
struct Drag {
    hwnd: isize,
    grab: (f32, f32),
    size: (i32, i32),
    resized: bool,
    last_cursor: (f32, f32),
    path: f32,
    lags: Vec<f32>,
    started: Option<Instant>,
}

impl Drag {
    fn update(&mut self, log: &mut Log) {
        let current = DRAGGED.load(Ordering::Relaxed);
        if current != self.hwnd {
            self.finish(log);
            *self = Drag { hwnd: current, ..Drag::default() };
        }
        if current == 0 {
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
        let count = self.lags.len().max(1) as f32;
        let average = self.lags.iter().sum::<f32>() / count;
        let max = self.lags.iter().cloned().fold(0.0, f32::max);
        let ms = if speed > 1.0 { average / speed * 1000.0 } else { 0.0 };
        log.write(&format!(
            "drag[{}]: {seconds:.2} s, cursor {speed:.0} px/s, lag avg {average:.1} px, max {max:.1} px, ~{ms:.0} ms",
            if self.resized { "resize" } else { "move" }
        ));
    }
}

fn run(log: &mut Log) -> Result<()> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("SugrobOverlay");
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&class);

        let screen_width = GetSystemMetrics(SM_CXSCREEN);
        let screen_height = GetSystemMetrics(SM_CYSCREEN);
        log.write(&format!("screen: {screen_width}x{screen_height}"));

        let overlay = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Sugrob"),
            WS_POPUP,
            0,
            0,
            screen_width,
            screen_height,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        SetLayeredWindowAttributes(overlay, COLORREF(0), 255, LWA_ALPHA)?;

        let tray_window = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name,
            w!("SugrobTray"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let mut icon = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: tray_window,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
            ..Default::default()
        };
        for (slot, unit) in icon.szTip.iter_mut().zip("Сугроб: спайк".encode_utf16()) {
            *slot = unit;
        }
        let _ = Shell_NotifyIconW(NIM_ADD, &icon);
        // Запасной выход, если значок в трее не появится.
        let _ = RegisterHotKey(Some(tray_window), 1, MOD_CONTROL | MOD_ALT | MOD_SHIFT, 'Q' as u32);

        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device: ID3D11Device = device.expect("устройство D3D11");
        let context: ID3D11DeviceContext = context.expect("контекст D3D11");
        let dxgi: IDXGIDevice = device.cast()?;
        let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;

        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: screen_width as u32,
            Height: screen_height as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        };
        let swap_chain = factory.CreateSwapChainForComposition(&device, &desc, None)?;

        let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
        let target = composition.CreateTargetForHwnd(overlay, true)?;
        let visual = composition.CreateVisual()?;
        visual.SetContent(&swap_chain)?;
        target.SetRoot(&visual)?;
        composition.Commit()?;
        let _ = ShowWindow(overlay, SW_SHOWNOACTIVATE);

        let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
        let mut render_target = None;
        device.CreateRenderTargetView(&back_buffer, None, Some(&mut render_target))?;
        let render_target = render_target.expect("render target");

        let vs_blob = compile(SHADER, s!("vs"), s!("vs_4_0"))?;
        let ps_blob = compile(SHADER, s!("ps"), s!("ps_4_0"))?;
        let mut vertex_shader = None;
        device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vertex_shader))?;
        let mut pixel_shader = None;
        device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut pixel_shader))?;

        let element = |name, index, format, offset| D3D11_INPUT_ELEMENT_DESC {
            SemanticName: name,
            SemanticIndex: index,
            Format: format,
            InputSlot: 0,
            AlignedByteOffset: offset,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        };
        let elements = [
            element(s!("POSITION"), 0, DXGI_FORMAT_R32G32_FLOAT, 0),
            element(s!("TEXCOORD"), 0, DXGI_FORMAT_R32G32_FLOAT, 8),
            element(s!("TEXCOORD"), 1, DXGI_FORMAT_R32_FLOAT, 16),
            element(s!("TEXCOORD"), 2, DXGI_FORMAT_R32_FLOAT, 20),
        ];
        let mut layout = None;
        device.CreateInputLayout(&elements, blob_bytes(&vs_blob), Some(&mut layout))?;

        let buffer_desc = D3D11_BUFFER_DESC {
            ByteWidth: (MAX_VERTICES * std::mem::size_of::<Vertex>()) as u32,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let mut vertex_buffer = None;
        device.CreateBuffer(&buffer_desc, None, Some(&mut vertex_buffer))?;
        let vertex_buffer = vertex_buffer.expect("буфер вершин");

        let mut blend_desc = D3D11_BLEND_DESC::default();
        blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: TRUE,
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        let mut blend = None;
        device.CreateBlendState(&blend_desc, Some(&mut blend))?;

        let hook_range = [
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_REORDER, EVENT_OBJECT_REORDER),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        ];
        let hooks: Vec<HWINEVENTHOOK> = hook_range
            .iter()
            .map(|&(from, to)| SetWinEventHook(from, to, None, Some(on_win_event), 0, 0, WINEVENT_OUTOFCONTEXT))
            .collect();

        let mut rng = Rng(0x2F6E_2B1D);
        let (width, height) = (screen_width as f32, screen_height as f32);
        let mut flakes: Vec<Flake> = (0..FLAKES)
            .map(|_| Flake {
                x: rng.next() * width,
                y: rng.next() * height,
                size: 2.0 + rng.next() * 4.0,
                speed: 60.0 + rng.next() * 120.0,
                phase: rng.next() * 6.28,
            })
            .collect();

        let mut windows: Vec<Tracked> = Vec::new();
        let mut last_enum = Instant::now() - Duration::from_secs(1);
        let mut drag = Drag::default();
        let mut quads = Quads { out: Vec::with_capacity(MAX_VERTICES), width, height };

        let started = Instant::now();
        let mut previous = started;
        let mut last_stats = started;
        let (mut frames, mut enums, mut enum_ms, mut enum_max, mut work_ms) = (0u32, 0u32, 0f32, 0f32, 0f32);

        'main: loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = DispatchMessageW(&msg);
            }
            if QUIT.load(Ordering::Relaxed) {
                break 'main;
            }

            let now = Instant::now();
            let dt = (now - previous).as_secs_f32().min(0.1);
            previous = now;

            if DIRTY.swap(false, Ordering::Relaxed) || now - last_enum > Duration::from_millis(500) {
                let before = Instant::now();
                let mut state = EnumState { own: [overlay, tray_window], windows: Vec::with_capacity(64) };
                let _ = EnumWindows(Some(enum_proc), LPARAM(&mut state as *mut EnumState as isize));
                windows = state.windows;
                let cost = before.elapsed().as_secs_f32() * 1000.0;
                enums += 1;
                enum_ms += cost;
                enum_max = enum_max.max(cost);
                last_enum = now;
            }
            drag.update(log);

            let time = started.elapsed().as_secs_f32();
            quads.out.clear();
            for flake in &mut flakes {
                flake.y += flake.speed * dt;
                if flake.y > height {
                    flake.y = -flake.size;
                }
                let sway = (time * 0.8 + flake.phase).sin() * 14.0;
                quads.add(flake.x + sway, flake.y, flake.size, flake.size, 0.85, 1.0);
            }
            for index in 0..windows.len() {
                let top = windows[index].frame.top as f32;
                for (a, b) in visible_top_segments(&windows, index) {
                    if b > a {
                        quads.add(a as f32, top - CAP_HEIGHT, (b - a) as f32, CAP_HEIGHT, 0.95, 0.0);
                    }
                }
            }

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&vertex_buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
            std::ptr::copy_nonoverlapping(quads.out.as_ptr(), mapped.pData as *mut Vertex, quads.out.len());
            context.Unmap(&vertex_buffer, 0);

            context.ClearRenderTargetView(&render_target, &[0.0, 0.0, 0.0, 0.0]);
            context.OMSetRenderTargets(Some(&[Some(render_target.clone())]), None);
            context.OMSetBlendState(blend.as_ref(), None, 0xffff_ffff);
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: width,
                Height: height,
                MaxDepth: 1.0,
                ..Default::default()
            }]));
            context.IASetInputLayout(layout.as_ref());
            context.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            let stride = std::mem::size_of::<Vertex>() as u32;
            let offset = 0u32;
            let buffer = Some(vertex_buffer.clone());
            context.IASetVertexBuffers(0, 1, Some(&buffer), Some(&stride), Some(&offset));
            context.VSSetShader(vertex_shader.as_ref(), None);
            context.PSSetShader(pixel_shader.as_ref(), None);
            context.Draw(quads.out.len() as u32, 0);

            work_ms += now.elapsed().as_secs_f32() * 1000.0;
            swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
            frames += 1;

            let since = last_stats.elapsed().as_secs_f32();
            if since >= 5.0 {
                log.write(&format!(
                    "stats: {:.1} fps, work {:.2} ms/frame, enum {:.1}/s avg {:.2} ms max {:.2} ms, windows {}",
                    frames as f32 / since,
                    work_ms / frames.max(1) as f32,
                    enums as f32 / since,
                    enum_ms / enums.max(1) as f32,
                    enum_max,
                    windows.len()
                ));
                frames = 0;
                enums = 0;
                enum_ms = 0.0;
                enum_max = 0.0;
                work_ms = 0.0;
                last_stats = Instant::now();
            }
        }

        for hook in hooks {
            let _ = UnhookWinEvent(hook);
        }
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
        log.write("exit");
        Ok(())
    }
}

fn main() {
    let mut log = Log::open().unwrap_or_else(|| std::process::exit(2));
    log.write("sugrob windows spike 0.0.1");
    if let Err(error) = run(&mut log) {
        log.write(&format!("fatal: {error}"));
        let text: Vec<u16> = format!("Сугроб не запустился: {error}\0").encode_utf16().collect();
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), w!("Сугроб"), MB_ICONERROR);
        }
    }
}
