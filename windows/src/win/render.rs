//! Direct3D 11 и DirectComposition: перенос шейдеров и порядка отрисовки из `Renderer.swift`.
use std::ffi::c_void;

use sugrob_core::sim::{make_santa_frame, CapDraw, CapField, CapVertex, FlakeInstance, SantaDraw, SnowField, MAX_DEBRIS, SANTA_FRAME_COUNT, V2};
use windows::core::{s, Interface, Result, PCSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::Log;

const CAP_VERTEX_CAPACITY: usize = 64_000;
const FLAKE_CAPACITY: usize = 20_000 + MAX_DEBRIS;
const RECT_CAPACITY: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RectInstance {
    rect: [f32; 4],
    value: f32,
    padding: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Frame {
    viewport: [f32; 2],
    time: f32,
    padding: f32,
}

const SHADER: &str = r#"
cbuffer Frame : register(b0) { float2 viewport; float time; float pad; };

float4 toClip(float2 p) { return float4(p.x / viewport.x * 2 - 1, 1 - p.y / viewport.y * 2, 0, 1); }
float hash(float2 p) { return frac(sin(dot(p, float2(127.1, 311.7))) * 43758.5453); }
float valueNoise(float2 p) {
    float2 i = floor(p);
    float2 f = frac(p);
    float2 u = f * f * (3 - 2 * f);
    return lerp(lerp(hash(i), hash(i + float2(1, 0)), u.x),
                lerp(hash(i + float2(0, 1)), hash(i + float2(1, 1)), u.x), u.y);
}
float2 cornerOf(uint v) { return float2(v & 1, (v >> 1) & 1); }

struct FlakeIn { float4 rect : TEXCOORD0; float alpha : TEXCOORD1; float softness : TEXCOORD2; float ring : TEXCOORD3; uint v : SV_VertexID; };
struct FlakeOut { float4 position : SV_Position; float2 uv : TEXCOORD0; float alpha : TEXCOORD1; float softness : TEXCOORD2; float ring : TEXCOORD3; };
FlakeOut flakeVS(FlakeIn i) {
    float2 c = cornerOf(i.v);
    FlakeOut o;
    o.position = toClip(i.rect.xy + c * i.rect.zw);
    o.uv = c * 2 - 1; o.alpha = i.alpha; o.softness = i.softness; o.ring = i.ring;
    return o;
}
float4 flakePS(FlakeOut i) : SV_Target {
    float d = length(i.uv);
    float body = 1 - smoothstep(1 - max(i.softness, 0.001), 1, d);
    body *= body;
    float rim = smoothstep(0.55, 0.92, d) * body * i.ring;
    float a = (body * (1 - i.ring * 0.5) + rim) * i.alpha;
    return float4(float3(0.93, 0.96, 1.0) * a, a);
}

struct CapIn { float4 position : TEXCOORD0; float thickness : TEXCOORD1; };
struct CapOut { float4 position : SV_Position; float depth : TEXCOORD0; float u : TEXCOORD1; float thickness : TEXCOORD2; };
CapOut capVS(CapIn i) {
    CapOut o;
    o.position = toClip(i.position.xy); o.depth = i.position.z; o.u = i.position.w; o.thickness = i.thickness;
    return o;
}
float4 capPS(CapOut i) : SV_Target {
    float a = smoothstep(-0.7, 0.7, i.depth) * smoothstep(0.15, 1.5, i.thickness);
    float t = saturate(i.depth / max(i.thickness, 1.0));
    float grain = valueNoise(float2(i.u * 0.35, i.depth * 0.35));
    float3 lit = float3(0.97, 0.985, 1.0);
    float3 shade = float3(0.62, 0.70, 0.88);
    float3 color = lerp(lit, shade, saturate(smoothstep(0.25, 1.0, t) * 0.75 + (grain - 0.5) * 0.12));
    float2 scaled = float2(i.u, i.depth) * 0.55;
    float seed = hash(floor(scaled));
    float roundness = 1 - smoothstep(0.1, 0.5, length(frac(scaled) - 0.5));
    float glint = step(0.985, seed) * roundness * pow(max(0.0, sin(time * 2.0 + seed * 200.0)), 8.0);
    color += glint * 0.6;
    return float4(saturate(color) * a, a);
}

struct RectIn { float4 rect : TEXCOORD0; float value : TEXCOORD1; uint v : SV_VertexID; };
struct EraseOut { float4 position : SV_Position; float2 p : TEXCOORD0; float2 halfSize : TEXCOORD1; nointerpolation float radius : TEXCOORD2; };
EraseOut eraseVS(RectIn i) {
    float2 c = cornerOf(i.v);
    EraseOut o;
    o.position = toClip(i.rect.xy + c * i.rect.zw);
    o.p = (c - 0.5) * i.rect.zw; o.halfSize = i.rect.zw * 0.5; o.radius = i.value;
    return o;
}
float4 erasePS(EraseOut i) : SV_Target {
    float2 q = abs(i.p) - i.halfSize + i.radius;
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - i.radius;
    return float4(0, 0, 0, 1 - smoothstep(-0.5, 0.5, d));
}

struct BallOut { float4 position : SV_Position; float2 uv : TEXCOORD0; nointerpolation float rotation : TEXCOORD1; };
BallOut ballVS(RectIn i) {
    float2 c = cornerOf(i.v);
    BallOut o;
    o.position = toClip(i.rect.xy + c * i.rect.zw);
    o.uv = c * 2 - 1; o.rotation = i.value;
    return o;
}
float4 ballPS(BallOut i) : SV_Target {
    float d = length(i.uv);
    float a = 1 - smoothstep(0.95, 1.0, d);
    if (a <= 0) discard;
    float z = sqrt(max(0.0, 1 - d * d));
    float3 normal = float3(i.uv.x, -i.uv.y, z);
    float diffuse = saturate(dot(normal, normalize(float3(-0.35, 0.6, 0.72))));
    float c = cos(i.rotation), s = sin(i.rotation);
    float2 turned = float2(c * i.uv.x + s * i.uv.y, -s * i.uv.x + c * i.uv.y);
    float grain = valueNoise(turned * 7) * 0.6 + valueNoise(turned * 16) * 0.4;
    float3 lit = float3(0.98, 0.99, 1.0);
    float3 shade = float3(0.60, 0.68, 0.86);
    float3 color = lerp(shade, lit, saturate(smoothstep(0.05, 0.85, diffuse) + (grain - 0.5) * 0.28));
    return float4(saturate(color) * a, a);
}

Texture2D spriteTexture : register(t0);
SamplerState spriteSampler : register(s0);
struct SpriteOut { float4 position : SV_Position; float2 uv : TEXCOORD0; };
SpriteOut spriteVS(RectIn i) {
    float2 c = cornerOf(i.v);
    SpriteOut o;
    o.position = toClip(i.rect.xy + c * i.rect.zw);
    o.uv = c;
    return o;
}
// Текстура уже с премультиплицированной альфой.
float4 spritePS(SpriteOut i) : SV_Target { return spriteTexture.Sample(spriteSampler, i.uv); }
"#;

/// Система вернула успех, но без объекта: для кадра это такая же ошибка.
fn got<T>(value: Option<T>, what: &str) -> Result<T> {
    value.ok_or_else(|| windows::core::Error::new(E_FAIL, what))
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let mut code = None;
    let mut errors: Option<ID3DBlob> = None;
    let result = unsafe {
        D3DCompile(SHADER.as_ptr() as *const c_void, SHADER.len(), None, None, None, entry, target, 0, 0, &mut code, Some(&mut errors))
    };
    if let Err(error) = result {
        let detail = errors.map(|b| unsafe { String::from_utf8_lossy(blob_bytes(&b)).into_owned() }).unwrap_or_default();
        return Err(windows::core::Error::new(error.code(), detail));
    }
    got(code, "шейдер без кода")
}

unsafe fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
}

struct Pass {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    layout: Option<ID3D11InputLayout>,
}

struct SpritePass {
    pass: Pass,
    sampler: ID3D11SamplerState,
}

/// Оверлей одного монитора.
pub struct Target {
    hwnd: HWND,
    swap_chain: IDXGISwapChain1,
    rtv: ID3D11RenderTargetView,
    size: (f32, f32),
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
}

impl Target {
    /// Панель задач при клике поднимается в своём слое, поэтому оверлей возвращается наверх.
    pub fn keep_on_top(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        }
    }

    /// На паузе окно скрыто: компоновщик не тратит на него ничего.
    pub fn set_visible(&self, visible: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

pub struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    dxgi: IDXGIDevice,
    factory: IDXGIFactory2,
    composition: IDCompositionDevice,
    flake: Pass,
    cap: Pass,
    erase: Pass,
    ball: Pass,
    sprite: Option<SpritePass>,
    santa_frames: Vec<ID3D11ShaderResourceView>,
    flake_buffer: ID3D11Buffer,
    cap_buffer: ID3D11Buffer,
    rect_buffer: ID3D11Buffer,
    frame_buffer: ID3D11Buffer,
    normal_blend: ID3D11BlendState,
    erase_blend: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    flakes: Vec<FlakeInstance>,
    vertices: Vec<CapVertex>,
    draws: Vec<CapDraw>,
    rects: Vec<RectInstance>,
}

/// Кадры без текстуры пропускаем: снег важнее саней.
unsafe fn make_santa_textures(device: &ID3D11Device, log: &mut Log) -> Vec<ID3D11ShaderResourceView> {
    let mut views = Vec::with_capacity(SANTA_FRAME_COUNT);
    for frame in 0..SANTA_FRAME_COUNT {
        let Some((pixels, width, height)) = make_santa_frame(frame, 2) else {
            log.write(&format!("santa: кадр {frame} не нарисовался"));
            continue;
        };
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_IMMUTABLE,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let initial = D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr() as *const c_void, SysMemPitch: width * 4, SysMemSlicePitch: 0 };
        let mut texture: Option<ID3D11Texture2D> = None;
        if let Err(error) = device.CreateTexture2D(&desc, Some(&initial), Some(&mut texture)) {
            log.write(&format!("santa: текстура {frame}: {error}"));
            continue;
        }
        let Some(texture) = texture else { continue };
        let mut view = None;
        match device.CreateShaderResourceView(&texture, None, Some(&mut view)) {
            Ok(()) => views.extend(view),
            Err(error) => log.write(&format!("santa: вид текстуры {frame}: {error}")),
        }
    }
    views
}

fn element(index: u32, format: DXGI_FORMAT, offset: u32, per_instance: bool) -> D3D11_INPUT_ELEMENT_DESC {
    D3D11_INPUT_ELEMENT_DESC {
        SemanticName: s!("TEXCOORD"),
        SemanticIndex: index,
        Format: format,
        InputSlot: 0,
        AlignedByteOffset: offset,
        InputSlotClass: if per_instance { D3D11_INPUT_PER_INSTANCE_DATA } else { D3D11_INPUT_PER_VERTEX_DATA },
        InstanceDataStepRate: u32::from(per_instance),
    }
}

impl Renderer {
    pub fn new(log: &mut Log) -> Result<Self> {
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(&[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0]), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
            let device: ID3D11Device = got(device, "нет устройства D3D11")?;
            let context: ID3D11DeviceContext = got(context, "нет контекста D3D11")?;
            let dxgi: IDXGIDevice = device.cast()?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;

            let float4 = DXGI_FORMAT_R32G32B32A32_FLOAT;
            let float1 = DXGI_FORMAT_R32_FLOAT;
            let flake_layout = [element(0, float4, 0, true), element(1, float1, 16, true), element(2, float1, 20, true), element(3, float1, 24, true)];
            let cap_layout = [element(0, float4, 0, false), element(1, float1, 16, false)];
            let rect_layout = [element(0, float4, 0, true), element(1, float1, 16, true)];

            let make_pass = |vs: PCSTR, ps: PCSTR, layout: &[D3D11_INPUT_ELEMENT_DESC]| -> Result<Pass> {
                let vs_blob = compile(vs, s!("vs_4_0"))?;
                let ps_blob = compile(ps, s!("ps_4_0"))?;
                let mut vertex = None;
                device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vertex))?;
                let mut pixel = None;
                device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut pixel))?;
                let mut input = None;
                device.CreateInputLayout(layout, blob_bytes(&vs_blob), Some(&mut input))?;
                Ok(Pass { vs: got(vertex, "нет вершинного шейдера")?, ps: got(pixel, "нет пиксельного шейдера")?, layout: input })
            };
            let flake = make_pass(s!("flakeVS"), s!("flakePS"), &flake_layout)?;
            let cap = make_pass(s!("capVS"), s!("capPS"), &cap_layout)?;
            let erase = make_pass(s!("eraseVS"), s!("erasePS"), &rect_layout)?;
            let ball = make_pass(s!("ballVS"), s!("ballPS"), &rect_layout)?;
            // Без спрайта снег всё равно идёт: Дед Мороз просто не приедет.
            let sprite = make_pass(s!("spriteVS"), s!("spritePS"), &rect_layout).and_then(|pass| {
                let desc = D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: D3D11_FLOAT32_MAX,
                    ..Default::default()
                };
                let mut sampler = None;
                device.CreateSamplerState(&desc, Some(&mut sampler))?;
                let sampler = sampler.ok_or_else(|| windows::core::Error::new(E_FAIL, "сэмплер спрайта"))?;
                Ok(SpritePass { pass, sampler })
            });
            let sprite = match sprite {
                Ok(sprite) => Some(sprite),
                Err(error) => {
                    log.write(&format!("santa: шейдер спрайта: {error}"));
                    None
                }
            };
            let santa_frames = make_santa_textures(&device, log);

            let make_buffer = |bytes: usize, bind: D3D11_BIND_FLAG| -> Result<ID3D11Buffer> {
                let desc = D3D11_BUFFER_DESC { ByteWidth: bytes as u32, Usage: D3D11_USAGE_DYNAMIC, BindFlags: bind.0 as u32, CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32, ..Default::default() };
                let mut buffer = None;
                device.CreateBuffer(&desc, None, Some(&mut buffer))?;
                got(buffer, "нет буфера")
            };
            let flake_buffer = make_buffer(FLAKE_CAPACITY * std::mem::size_of::<FlakeInstance>(), D3D11_BIND_VERTEX_BUFFER)?;
            let cap_buffer = make_buffer(CAP_VERTEX_CAPACITY * std::mem::size_of::<CapVertex>(), D3D11_BIND_VERTEX_BUFFER)?;
            let rect_buffer = make_buffer(RECT_CAPACITY * std::mem::size_of::<RectInstance>(), D3D11_BIND_VERTEX_BUFFER)?;
            let frame_buffer = make_buffer(std::mem::size_of::<Frame>(), D3D11_BIND_CONSTANT_BUFFER)?;

            let make_blend = |source: D3D11_BLEND| -> Result<ID3D11BlendState> {
                let mut desc = D3D11_BLEND_DESC::default();
                desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                    BlendEnable: TRUE,
                    SrcBlend: source,
                    DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOp: D3D11_BLEND_OP_ADD,
                    SrcBlendAlpha: source,
                    DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
                    BlendOpAlpha: D3D11_BLEND_OP_ADD,
                    RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
                };
                let mut state = None;
                device.CreateBlendState(&desc, Some(&mut state))?;
                got(state, "нет состояния смешивания")
            };
            let normal_blend = make_blend(D3D11_BLEND_ONE)?;
            let erase_blend = make_blend(D3D11_BLEND_ZERO)?;

            // Вершины шапки идут в обратном обходе относительно хлопьев.
            let raster_desc = D3D11_RASTERIZER_DESC { FillMode: D3D11_FILL_SOLID, CullMode: D3D11_CULL_NONE, DepthClipEnable: TRUE, ..Default::default() };
            let mut rasterizer = None;
            device.CreateRasterizerState(&raster_desc, Some(&mut rasterizer))?;

            Ok(Renderer {
                device,
                context,
                dxgi,
                factory,
                composition,
                flake,
                cap,
                erase,
                ball,
                sprite,
                santa_frames,
                flake_buffer,
                cap_buffer,
                rect_buffer,
                frame_buffer,
                normal_blend,
                erase_blend,
                rasterizer: got(rasterizer, "нет растеризатора")?,
                flakes: Vec::with_capacity(FLAKE_CAPACITY),
                vertices: Vec::with_capacity(CAP_VERTEX_CAPACITY),
                draws: Vec::with_capacity(RECT_CAPACITY),
                rects: Vec::with_capacity(RECT_CAPACITY),
            })
        }
    }

    pub fn create_target(&self, rect: RECT) -> Result<Target> {
        unsafe {
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            let instance = GetModuleHandleW(None)?;
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                super::OVERLAY_CLASS,
                windows::core::w!("Sugrob"),
                WS_POPUP,
                rect.left,
                rect.top,
                width,
                height,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            // Цель пересоздаётся при сбоях: окно с ошибки не должно оставаться висеть.
            self.attach_swap_chain(hwnd, width, height).inspect_err(|_| {
                let _ = DestroyWindow(hwnd);
            })
        }
    }

    unsafe fn attach_swap_chain(&self, hwnd: HWND, width: i32, height: i32) -> Result<Target> {
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;

        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width as u32,
            Height: height as u32,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            ..Default::default()
        };
        let swap_chain = self.factory.CreateSwapChainForComposition(&self.dxgi, &desc, None)?;
        let target = self.composition.CreateTargetForHwnd(hwnd, true)?;
        let visual = self.composition.CreateVisual()?;
        visual.SetContent(&swap_chain)?;
        target.SetRoot(&visual)?;
        self.composition.Commit()?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let back_buffer: ID3D11Texture2D = swap_chain.GetBuffer(0)?;
        let mut rtv = None;
        self.device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv))?;
        Ok(Target { hwnd, swap_chain, rtv: got(rtv, "нет цели отрисовки")?, size: (width as f32, height as f32), _target: target, _visual: visual })
    }

    /// Раз в секунду: на паузе кадров нет, а компоновщик теряется и без ошибки в Present.
    pub fn is_alive(&self) -> bool {
        unsafe { self.device.GetDeviceRemovedReason().is_ok() && self.composition.CheckDeviceState().is_ok_and(|valid| valid.as_bool()) }
    }

    unsafe fn upload<T: Copy>(&self, buffer: &ID3D11Buffer, data: &[T]) -> Result<()> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context.Map(buffer, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), mapped.pData as *mut T, data.len());
        self.context.Unmap(buffer, 0);
        Ok(())
    }

    unsafe fn bind(&self, pass: &Pass, buffer: &ID3D11Buffer, stride: usize) {
        let ctx = &self.context;
        ctx.IASetInputLayout(pass.layout.as_ref());
        ctx.VSSetShader(&pass.vs, None);
        ctx.PSSetShader(&pass.ps, None);
        let (strides, offsets) = (stride as u32, 0u32);
        let slot = Some(buffer.clone());
        ctx.IASetVertexBuffers(0, 1, Some(&slot), Some(&strides), Some(&offsets));
    }

    pub fn draw(&mut self, target: &Target, field: &SnowField, caps: &CapField, santa: Option<SantaDraw>, origin: V2, viewport: V2, time: f32) -> Result<()> {
        unsafe {
            let mut flakes = std::mem::take(&mut self.flakes);
            let mut vertices = std::mem::take(&mut self.vertices);
            let mut draws = std::mem::take(&mut self.draws);
            let mut rects = std::mem::take(&mut self.rects);

            flakes.clear();
            field.write_instances(&mut flakes, time);
            caps.write_debris(&mut flakes, origin, viewport);
            flakes.truncate(FLAKE_CAPACITY);
            caps.build_mesh(&mut vertices, CAP_VERTEX_CAPACITY, origin, viewport, &mut draws);
            draws.truncate(RECT_CAPACITY - 2);

            // Первый слот для снежка, дальше окна в порядке отрисовки.
            let ball = caps.ball_for_drawing(origin);
            rects.clear();
            rects.push(ball.as_ref().map_or(RectInstance::default(), |b| RectInstance {
                rect: [b.center.x - b.radius, b.center.y - b.radius, b.radius * 2.0, b.radius * 2.0],
                value: b.rotation,
                padding: [0.0; 3],
            }));
            rects.extend(draws.iter().map(|d| RectInstance { rect: d.rect, value: d.corner_radius, padding: [0.0; 3] }));
            // Слот спрайта идёт за окнами, номер пригодится при отрисовке.
            let santa = santa.and_then(|item| self.santa_frames.get(item.frame).map(|view| (item, view.clone())));
            let santa_slot = rects.len() as u32;
            if let Some((item, _)) = &santa {
                rects.push(RectInstance { rect: item.rect, value: 0.0, padding: [0.0; 3] });
            }

            self.upload(&self.flake_buffer, &flakes)?;
            self.upload(&self.cap_buffer, &vertices)?;
            self.upload(&self.rect_buffer, &rects)?;
            self.upload(&self.frame_buffer, &[Frame { viewport: [viewport.x, viewport.y], time, padding: 0.0 }])?;

            let ctx = &self.context;
            ctx.ClearRenderTargetView(&target.rtv, &[0.0, 0.0, 0.0, 0.0]);
            ctx.OMSetRenderTargets(Some(&[Some(target.rtv.clone())]), None);
            ctx.RSSetState(&self.rasterizer);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: target.size.0, Height: target.size.1, MaxDepth: 1.0, ..Default::default() }]));
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            let constants = Some(self.frame_buffer.clone());
            ctx.VSSetConstantBuffers(0, Some(&[constants.clone()]));
            ctx.PSSetConstantBuffers(0, Some(&[constants]));

            // От задних окон к передним: окно стирает чужой снег под собой и получает свою шапку.
            let mut ball_drawn = false;
            let rect_stride = std::mem::size_of::<RectInstance>();
            let cap_stride = std::mem::size_of::<CapVertex>();
            let draw_ball = |this: &Self| {
                this.bind(&this.ball, &this.rect_buffer, rect_stride);
                this.context.DrawInstanced(4, 1, 0, 0);
            };
            for (index, draw) in draws.iter().enumerate() {
                ctx.OMSetBlendState(&self.erase_blend, None, 0xffff_ffff);
                self.bind(&self.erase, &self.rect_buffer, rect_stride);
                ctx.DrawInstanced(4, 1, 0, index as u32 + 1);
                ctx.OMSetBlendState(&self.normal_blend, None, 0xffff_ffff);
                if draw.vertex_count > 0 {
                    self.bind(&self.cap, &self.cap_buffer, cap_stride);
                    ctx.Draw(draw.vertex_count as u32, draw.vertex_start as u32);
                }
                // Сразу после своего окна: стирание передних окон закроет шар.
                if ball.as_ref().is_some_and(|b| b.owner == draw.window) {
                    draw_ball(self);
                    ball_drawn = true;
                }
            }
            ctx.OMSetBlendState(&self.normal_blend, None, 0xffff_ffff);
            if ball.is_some() && !ball_drawn {
                draw_ball(self);
            }

            // Между снежком и хлопьями, как в `Renderer.swift`: снег падает перед санями.
            if let (Some((_, view)), Some(sprite)) = (&santa, &self.sprite) {
                self.bind(&sprite.pass, &self.rect_buffer, rect_stride);
                ctx.PSSetShaderResources(0, Some(&[Some(view.clone())]));
                ctx.PSSetSamplers(0, Some(&[Some(sprite.sampler.clone())]));
                ctx.DrawInstanced(4, 1, 0, santa_slot);
            }

            self.bind(&self.flake, &self.flake_buffer, std::mem::size_of::<FlakeInstance>());
            ctx.DrawInstanced(4, flakes.len() as u32, 0, 0);

            self.flakes = flakes;
            self.vertices = vertices;
            self.draws = draws;
            self.rects = rects;

            // Частоту задаёт свой таймер, поэтому без ожидания кадрового импульса.
            target.swap_chain.Present(0, DXGI_PRESENT(0)).ok()
        }
    }
}
