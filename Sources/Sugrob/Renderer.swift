import CoreGraphics
import Foundation
import ImageIO
import Metal
import QuartzCore
import simd

struct RectInstance {
    /// x, y, ширина, высота в точках, y вниз.
    var rect: SIMD4<Float>
    var color: SIMD4<Float>
}

/// Буферы хлопьев и шапок на несколько кадров вперёд, чтобы не выделять память каждый кадр.
@MainActor
final class FrameRing {
    fileprivate static let capVertexCapacity = 64_000

    fileprivate let flakeBuffers: [MTLBuffer]
    fileprivate let capBuffers: [MTLBuffer]
    fileprivate let slots = DispatchSemaphore(value: 3)
    fileprivate var draws: [CapDraw] = []
    private var index = 0

    init?(device: MTLDevice, flakeCapacity: Int) {
        let flakeLength = MemoryLayout<FlakeInstance>.stride * max(flakeCapacity, 1)
        let capLength = MemoryLayout<CapVertex>.stride * Self.capVertexCapacity
        let flakeBuffers = (0..<3).compactMap { _ in device.makeBuffer(length: flakeLength, options: .storageModeShared) }
        let capBuffers = (0..<3).compactMap { _ in device.makeBuffer(length: capLength, options: .storageModeShared) }
        guard flakeBuffers.count == 3, capBuffers.count == 3 else { return nil }
        self.flakeBuffers = flakeBuffers
        self.capBuffers = capBuffers
    }

    fileprivate func next() -> (flakes: MTLBuffer, caps: MTLBuffer) {
        defer { index = (index + 1) % flakeBuffers.count }
        return (flakeBuffers[index], capBuffers[index])
    }
}

/// Всё, что нужно для кадра одного экрана.
struct SceneInput {
    var time: Float
    var viewport: SIMD2<Float>
    var origin: SIMD2<Float>
    var debugRects: [RectInstance]
    var santa: SantaDraw? = nil
    var sheet: MTLTexture? = nil
}

@MainActor
final class Renderer {
    let device: MTLDevice
    private let queue: MTLCommandQueue
    private let flakePipeline: MTLRenderPipelineState
    private let rectPipeline: MTLRenderPipelineState
    private let capPipeline: MTLRenderPipelineState
    private let erasePipeline: MTLRenderPipelineState
    private let ballPipeline: MTLRenderPipelineState
    private let spritePipeline: MTLRenderPipelineState
    private let sheetPipeline: MTLRenderPipelineState
    private var santaFrames: [MTLTexture] = []

    // Шейдеры собираются при запуске: утилиты metal в Command Line Tools нет.
    private static let shaderSource = """
    #include <metal_stdlib>
    using namespace metal;

    static float4 toClip(float2 p, float2 viewport) {
        return float4(p.x / viewport.x * 2 - 1, 1 - p.y / viewport.y * 2, 0, 1);
    }

    static float hash(float2 p) {
        return fract(sin(dot(p, float2(127.1, 311.7))) * 43758.5453);
    }

    static float valueNoise(float2 p) {
        float2 i = floor(p);
        float2 f = fract(p);
        float2 u = f * f * (3 - 2 * f);
        return mix(mix(hash(i), hash(i + float2(1, 0)), u.x),
                   mix(hash(i + float2(0, 1)), hash(i + float2(1, 1)), u.x), u.y);
    }

    struct FlakeInstance { float4 rect; float alpha; float softness; float ring; float padding; };
    struct FlakeOut {
        float4 position [[position]];
        float2 uv;
        float alpha;
        float softness;
        float ring;
    };

    vertex FlakeOut flakeVertex(uint v [[vertex_id]], uint i [[instance_id]],
                                const device FlakeInstance *instances [[buffer(0)]],
                                constant float2 &viewport [[buffer(1)]]) {
        float2 corner = float2(v & 1, (v >> 1) & 1);
        FlakeInstance flake = instances[i];
        FlakeOut out;
        out.position = toClip(flake.rect.xy + corner * flake.rect.zw, viewport);
        out.uv = corner * 2 - 1;
        out.alpha = flake.alpha;
        out.softness = flake.softness;
        out.ring = flake.ring;
        return out;
    }

    fragment float4 flakeFragment(FlakeOut in [[stage_in]]) {
        float d = length(in.uv);
        float body = 1 - smoothstep(1 - max(in.softness, 0.001), 1, d);
        body *= body;
        float rim = smoothstep(0.55, 0.92, d) * body * in.ring;
        float a = (body * (1 - in.ring * 0.5) + rim) * in.alpha;
        return float4(float3(0.93, 0.96, 1.0) * a, a);
    }

    struct CapVertex { float4 position; float thickness; };
    struct CapOut {
        float4 position [[position]];
        float depth;
        float u;
        float thickness;
    };

    vertex CapOut capVertex(uint v [[vertex_id]],
                            const device CapVertex *vertices [[buffer(0)]],
                            constant float2 &viewport [[buffer(1)]]) {
        CapVertex in = vertices[v];
        CapOut out;
        out.position = toClip(in.position.xy, viewport);
        out.depth = in.position.z;
        out.u = in.position.w;
        out.thickness = in.thickness;
        return out;
    }

    fragment float4 capFragment(CapOut in [[stage_in]], constant float &time [[buffer(0)]]) {
        float a = smoothstep(-0.7, 0.7, in.depth) * smoothstep(0.15, 1.5, in.thickness);
        float t = saturate(in.depth / max(in.thickness, 1.0));
        float grain = valueNoise(float2(in.u * 0.35, in.depth * 0.35));

        float3 lit = float3(0.97, 0.985, 1.0);
        float3 shade = float3(0.62, 0.70, 0.88);
        float3 color = mix(lit, shade, saturate(smoothstep(0.25, 1.0, t) * 0.75 + (grain - 0.5) * 0.12));

        // Редкие искры на снегу.
        float2 scaled = float2(in.u, in.depth) * 0.55;
        float seed = hash(floor(scaled));
        float roundness = 1 - smoothstep(0.1, 0.5, length(fract(scaled) - 0.5));
        float glint = step(0.985, seed) * roundness * pow(max(0.0, sin(time * 2.0 + seed * 200.0)), 8.0);
        color += glint * 0.6;

        return float4(saturate(color) * a, a);
    }

    struct Erase { float4 rect; };
    struct EraseOut {
        float4 position [[position]];
        float2 p;
        float2 halfSize;
    };

    vertex EraseOut eraseVertex(uint v [[vertex_id]],
                                constant Erase &erase [[buffer(0)]],
                                constant float2 &viewport [[buffer(1)]]) {
        float2 corner = float2(v & 1, (v >> 1) & 1);
        float2 halfSize = erase.rect.zw * 0.5;
        EraseOut out;
        out.position = toClip(erase.rect.xy + corner * erase.rect.zw, viewport);
        out.p = (corner - 0.5) * erase.rect.zw;
        out.halfSize = halfSize;
        return out;
    }

    fragment float4 eraseFragment(EraseOut in [[stage_in]], constant float &radius [[buffer(2)]]) {
        float2 q = abs(in.p) - in.halfSize + radius;
        float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
        return float4(0, 0, 0, 1 - smoothstep(-0.5, 0.5, d));
    }

    struct SpriteOut { float4 position [[position]]; float2 uv; };

    vertex SpriteOut spriteVertex(uint v [[vertex_id]],
                                  constant float4 &rect [[buffer(0)]],
                                  constant float2 &viewport [[buffer(1)]]) {
        float2 corner = float2(v & 1, (v >> 1) & 1);
        SpriteOut out;
        out.position = toClip(rect.xy + corner * rect.zw, viewport);
        out.uv = corner;
        return out;
    }

    // Текстура уже с премультиплицированной альфой.
    fragment float4 spriteFragment(SpriteOut in [[stage_in]], texture2d<float> sprite [[texture(0)]]) {
        constexpr sampler linear(filter::linear, address::clamp_to_edge);
        return sprite.sample(linear, in.uv);
    }

    // Сплошной снег: край рваный от шума, в глубине плотнее и светлее.
    fragment float4 sheetFragment(SpriteOut in [[stage_in]], texture2d<float> mask [[texture(0)]],
                                  constant float2 &size [[buffer(1)]], constant float &time [[buffer(0)]]) {
        constexpr sampler linear(filter::linear, address::clamp_to_edge);
        float m = mask.sample(linear, in.uv).r;
        if (m < 0.01) { discard_fragment(); }
        float2 p = in.uv * size;
        float grain = valueNoise(p * 0.05) * 0.6 + valueNoise(p * 0.17) * 0.4;
        float edge = m + (grain - 0.5) * 0.35;
        float a = smoothstep(0.33, 0.5, edge);
        float depth = smoothstep(0.5, 0.95, edge);

        float3 lit = float3(0.96, 0.98, 1.0);
        float3 shade = float3(0.66, 0.74, 0.92);
        float drift = valueNoise(p * 0.009) * 0.6 + valueNoise(p * 0.025) * 0.4;
        float3 color = mix(shade, lit, saturate(depth * 0.55 + drift * 0.45 + (grain - 0.5) * 0.14));

        float2 scaled = p * 0.4;
        float seed = hash(floor(scaled));
        float roundness = 1 - smoothstep(0.1, 0.5, length(fract(scaled) - 0.5));
        color += step(0.992, seed) * roundness * pow(max(0.0, sin(time * 2.0 + seed * 200.0)), 8.0) * 0.7 * depth;
        return float4(saturate(color) * a, a);
    }

    struct BallOut { float4 position [[position]]; float2 uv; };

    vertex BallOut ballVertex(uint v [[vertex_id]],
                              constant float4 &rect [[buffer(0)]],
                              constant float2 &viewport [[buffer(1)]]) {
        float2 corner = float2(v & 1, (v >> 1) & 1);
        BallOut out;
        out.position = toClip(rect.xy + corner * rect.zw, viewport);
        out.uv = corner * 2 - 1;
        return out;
    }

    fragment float4 ballFragment(BallOut in [[stage_in]], constant float &rotation [[buffer(1)]]) {
        float d = length(in.uv);
        float a = 1 - smoothstep(0.95, 1.0, d);
        if (a <= 0) { discard_fragment(); }

        // Сфера, свет сверху-слева; y экрана направлен вниз.
        float z = sqrt(max(0.0, 1 - d * d));
        float3 normal = float3(in.uv.x, -in.uv.y, z);
        float diffuse = saturate(dot(normal, normalize(float3(-0.35, 0.6, 0.72))));

        float c = cos(rotation), s = sin(rotation);
        float2 turned = float2(c * in.uv.x + s * in.uv.y, -s * in.uv.x + c * in.uv.y);
        float grain = valueNoise(turned * 7) * 0.6 + valueNoise(turned * 16) * 0.4;

        float3 lit = float3(0.98, 0.99, 1.0);
        float3 shade = float3(0.60, 0.68, 0.86);
        float3 color = mix(shade, lit, saturate(smoothstep(0.05, 0.85, diffuse) + (grain - 0.5) * 0.28));
        return float4(saturate(color) * a, a);
    }

    struct RectInstance { float4 rect; float4 color; };
    struct RectOut { float4 position [[position]]; float4 color; };

    vertex RectOut rectVertex(uint v [[vertex_id]], uint i [[instance_id]],
                              const device RectInstance *instances [[buffer(0)]],
                              constant float2 &viewport [[buffer(1)]]) {
        float2 corner = float2(v & 1, (v >> 1) & 1);
        RectOut out;
        out.position = toClip(instances[i].rect.xy + corner * instances[i].rect.zw, viewport);
        float4 c = instances[i].color;
        out.color = float4(c.rgb * c.a, c.a);
        return out;
    }

    fragment float4 rectFragment(RectOut in [[stage_in]]) { return in.color; }
    """

    init?() {
        guard let device = MTLCreateSystemDefaultDevice(),
              let queue = device.makeCommandQueue(),
              let library = try? device.makeLibrary(source: Self.shaderSource, options: nil),
              let flakePipeline = Self.makePipeline(device, library, vertex: "flakeVertex", fragment: "flakeFragment"),
              let rectPipeline = Self.makePipeline(device, library, vertex: "rectVertex", fragment: "rectFragment"),
              let capPipeline = Self.makePipeline(device, library, vertex: "capVertex", fragment: "capFragment"),
              let erasePipeline = Self.makePipeline(device, library, vertex: "eraseVertex", fragment: "eraseFragment", erases: true),
              let ballPipeline = Self.makePipeline(device, library, vertex: "ballVertex", fragment: "ballFragment"),
              let spritePipeline = Self.makePipeline(device, library, vertex: "spriteVertex", fragment: "spriteFragment"),
              let sheetPipeline = Self.makePipeline(device, library, vertex: "spriteVertex", fragment: "sheetFragment")
        else { return nil }
        self.device = device
        self.queue = queue
        self.flakePipeline = flakePipeline
        self.rectPipeline = rectPipeline
        self.capPipeline = capPipeline
        self.erasePipeline = erasePipeline
        self.ballPipeline = ballPipeline
        self.spritePipeline = spritePipeline
        self.sheetPipeline = sheetPipeline
        santaFrames = (0..<SantaArt.frameCount).compactMap { makeSantaTexture(frame: $0) }
    }

    private func makeSantaTexture(frame: Int) -> MTLTexture? {
        guard let art = SantaArt.makeFrame(frame, scale: 2) else { return nil }
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: art.width, height: art.height, mipmapped: false)
        descriptor.usage = .shaderRead
        guard let texture = device.makeTexture(descriptor: descriptor) else { return nil }
        art.pixels.withUnsafeBytes { bytes in
            texture.replace(
                region: MTLRegionMake2D(0, 0, art.width, art.height), mipmapLevel: 0,
                withBytes: bytes.baseAddress!, bytesPerRow: art.width * 4)
        }
        return texture
    }

    private static func makePipeline(
        _ device: MTLDevice, _ library: MTLLibrary, vertex: String, fragment: String, erases: Bool = false
    ) -> MTLRenderPipelineState? {
        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction = library.makeFunction(name: vertex)
        descriptor.fragmentFunction = library.makeFunction(name: fragment)
        let attachment = descriptor.colorAttachments[0]!
        attachment.pixelFormat = .bgra8Unorm
        attachment.isBlendingEnabled = true
        attachment.rgbBlendOperation = .add
        attachment.alphaBlendOperation = .add
        attachment.sourceRGBBlendFactor = erases ? .zero : .one
        attachment.sourceAlphaBlendFactor = erases ? .zero : .one
        attachment.destinationRGBBlendFactor = .oneMinusSourceAlpha
        attachment.destinationAlphaBlendFactor = .oneMinusSourceAlpha
        return try? device.makeRenderPipelineState(descriptor: descriptor)
    }

    func draw(field: SnowField, caps: CapField, scene: SceneInput, ring: FrameRing, into layer: CAMetalLayer) {
        // Если GPU не успевает, пропускаем кадр, а не блокируем главный поток.
        guard ring.slots.wait(timeout: .now()) == .success else { return }
        let slots = ring.slots
        guard let drawable = layer.nextDrawable(),
              let commands = queue.makeCommandBuffer() else {
            slots.signal()
            return
        }

        let buffers = ring.next()
        let flakePointer = buffers.flakes.contents().assumingMemoryBound(to: FlakeInstance.self)
        let flakeCount = field.writeInstances(to: flakePointer, time: scene.time)
        let debrisCount = caps.writeDebris(
            to: flakePointer + flakeCount, origin: scene.origin, viewport: scene.viewport)
        caps.buildMesh(
            into: buffers.caps.contents().assumingMemoryBound(to: CapVertex.self),
            capacity: FrameRing.capVertexCapacity,
            origin: scene.origin, viewport: scene.viewport, draws: &ring.draws)

        let pass = Self.renderPass(texture: drawable.texture)
        encode(commands, pass: pass, flakes: buffers.flakes, flakeCount: flakeCount + debrisCount,
               capVertices: buffers.caps, draws: ring.draws,
               ball: caps.ballForDrawing(origin: scene.origin), scene: scene)
        commands.addCompletedHandler { _ in slots.signal() }
        commands.present(drawable)
        commands.commit()
    }

    private static func renderPass(texture: MTLTexture) -> MTLRenderPassDescriptor {
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        pass.colorAttachments[0].storeAction = .store
        return pass
    }

    private func encode(
        _ commands: MTLCommandBuffer, pass: MTLRenderPassDescriptor,
        flakes: MTLBuffer, flakeCount: Int,
        capVertices: MTLBuffer, draws: [CapDraw],
        ball: BallDraw?, scene: SceneInput
    ) {
        guard let encoder = commands.makeRenderCommandEncoder(descriptor: pass) else { return }
        var viewport = scene.viewport
        var time = scene.time
        encoder.setVertexBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
        encoder.setFragmentBytes(&time, length: MemoryLayout<Float>.stride, index: 0)

        // От задних окон к передним: окно стирает чужой снег под собой и получает свою шапку.
        var ballDrawn = false
        func drawBall(_ ball: BallDraw) {
            var rect = SIMD4(ball.center.x - ball.radius, ball.center.y - ball.radius, ball.radius * 2, ball.radius * 2)
            var rotation = ball.rotation
            encoder.setRenderPipelineState(ballPipeline)
            encoder.setVertexBytes(&rect, length: MemoryLayout<SIMD4<Float>>.stride, index: 0)
            encoder.setFragmentBytes(&rotation, length: MemoryLayout<Float>.stride, index: 1)
            encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
            ballDrawn = true
        }

        for draw in draws {
            var rect = draw.rect
            var eraseRadius = draw.cornerRadius
            encoder.setFragmentBytes(&eraseRadius, length: MemoryLayout<Float>.stride, index: 2)
            encoder.setRenderPipelineState(erasePipeline)
            encoder.setVertexBytes(&rect, length: MemoryLayout<SIMD4<Float>>.stride, index: 0)
            encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)

            if draw.vertexCount > 0 {
                encoder.setRenderPipelineState(capPipeline)
                encoder.setVertexBuffer(capVertices, offset: 0, index: 0)
                encoder.drawPrimitives(type: .triangleStrip, vertexStart: draw.vertexStart, vertexCount: draw.vertexCount)
            }
            // Сразу после своего окна: стирание передних окон закроет шар.
            if let ball, ball.ownerID == draw.windowID { drawBall(ball) }
        }
        if let ball, !ballDrawn { drawBall(ball) }

        if let santa = scene.santa, santaFrames.indices.contains(santa.frame) {
            var rect = santa.rect
            encoder.setRenderPipelineState(spritePipeline)
            encoder.setVertexBytes(&rect, length: MemoryLayout<SIMD4<Float>>.stride, index: 0)
            encoder.setFragmentTexture(santaFrames[santa.frame], index: 0)
            encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
        }

        if let sheet = scene.sheet {
            var rect = SIMD4<Float>(0, 0, viewport.x, viewport.y)
            encoder.setRenderPipelineState(sheetPipeline)
            encoder.setVertexBytes(&rect, length: MemoryLayout<SIMD4<Float>>.stride, index: 0)
            encoder.setFragmentBytes(&viewport, length: MemoryLayout<SIMD2<Float>>.stride, index: 1)
            encoder.setFragmentTexture(sheet, index: 0)
            encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
        }

        encoder.setRenderPipelineState(flakePipeline)
        encoder.setVertexBuffer(flakes, offset: 0, index: 0)
        encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4, instanceCount: flakeCount)

        if !scene.debugRects.isEmpty,
           let rects = device.makeBuffer(
               bytes: scene.debugRects,
               length: MemoryLayout<RectInstance>.stride * scene.debugRects.count,
               options: .storageModeShared) {
            encoder.setRenderPipelineState(rectPipeline)
            encoder.setVertexBuffer(rects, offset: 0, index: 0)
            encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4, instanceCount: scene.debugRects.count)
        }
        encoder.endEncoding()
    }

    /// Отладка: моделирует снег на синтетических окнах и пишет кадр в PNG.
    /// Настоящий оверлей недоступен снимкам экрана, поэтому смотреть результат можно только так.
    func writeSnapshot(to path: String, width: Int, height: Int, scale: Int) -> Bool {
        func window(_ id: CGWindowID, _ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> TrackedWindow {
            TrackedWindow(id: id, frame: CGRect(x: x, y: y, width: w, height: h), owner: "test", hasName: false, holdsSnow: true)
        }
        // От переднего к заднему. Окно 3 перекрывает полосу снега окна 1 своим нижним краем.
        let initial = [
            window(3, 300, 40, 260, 150),
            window(2, 640, 260, 700, 500),
            window(1, 200, 180, 640, 420),
        ]
        let opened = window(4, 1100, 560, 320, 300)

        var field = SnowField(width: Float(width), height: Float(height), seed: 42)
        let caps = CapField(seed: 11)
        let dt: Float = 1.0 / 30
        var time: Float = 0
        func simulate(frames: Int, windows: [TrackedWindow], mouseDown: Bool = false, cursor: CGPoint = CGPoint(x: 900, y: 420)) {
            caps.sync(WindowSnapshot(timestamp: Double(time) + 1, windows: windows, isMouseDown: mouseDown, cursor: cursor), now: Double(time))
            for _ in 0..<frames {
                time += dt
                field.step(dt: dt, time: time, caps: caps, origin: .zero, now: Double(time))
                caps.relax(now: Double(time))
                caps.stepDebris(now: Double(time))
            }
        }
        simulate(frames: 1, windows: initial)
        for item in initial { caps.setCornerRadius(Float([28, 27, 20, 14][Int(item.id) % 4]), for: item.id) }
        simulate(frames: 900, windows: initial)
        simulate(frames: 1800, windows: [opened] + initial)
        var windows = [opened] + initial
        if Config.snapshotBall {
            // Курсор медленно ведут по шапке окна 1 слева направо.
            for step in 0...110 {
                caps.brush(cursor: CGPoint(x: 580 + CGFloat(step) * 2, y: 172), now: Double(time))
                simulate(frames: 1, windows: windows)
            }
        }
        if Config.snapshotThrow {
            // Снежок скатали и бросили взмахом вверх-вправо.
            for step in 0...110 {
                caps.brush(cursor: CGPoint(x: 580 + CGFloat(step) * 1, y: 172), now: Double(time))
                simulate(frames: 1, windows: windows)
            }
            for step in 1...8 {
                caps.brush(cursor: CGPoint(x: 690 + CGFloat(step) * 45, y: 172 - CGFloat(step) * 10), now: Double(time))
                simulate(frames: 1, windows: windows)
            }
            for step in 1...6 {
                caps.brush(cursor: CGPoint(x: 1050 + CGFloat(step), y: 92), now: Double(time))
                simulate(frames: 1, windows: windows)
            }
        }
        if Config.snapshotBrush {
            // Курсор провели сквозь шапку окна 1 слева направо.
            for step in 0...12 {
                caps.brush(cursor: CGPoint(x: 600 + CGFloat(step) * 14, y: 174), now: Double(time))
                simulate(frames: 1, windows: windows)
            }
            simulate(frames: 3, windows: windows)
        }
        if Config.snapshotDrag {
            // Окно 3 тащат вправо над окном 1: с левого края снег сыплется на него.
            for step in 1...30 {
                windows = windows.map {
                    $0.id == 3 ? window(3, 300 + CGFloat(step) * 5, 40, 260, 150) : $0
                }
                simulate(frames: 1, windows: windows, mouseDown: true, cursor: CGPoint(x: 420, y: 100))
            }
            simulate(frames: 25, windows: windows)
        }
        if Config.snapshotFling {
            // Окно 2 резко бросили вниз, окно 4 закрыли.
            windows.removeAll { $0.id == 4 }
            simulate(frames: 1, windows: windows)
            for step in 1...6 {
                windows = windows.map {
                    $0.id == 2 ? window(2, 640, 260 + CGFloat(step) * 80, 700, 500) : $0
                }
                simulate(frames: 1, windows: windows, mouseDown: true)
            }
            simulate(frames: 8, windows: windows)
        }
        if Config.snapshotResize {
            // Окно 2 сужается справа, окно 1 расширяется слева, по шагам как при настоящем ресайзе.
            for step in 1...12 {
                let offset = CGFloat(step) * 12
                windows = windows.map { item in
                    switch item.id {
                    case 2: return window(2, 640, 260, 700 - offset, 500)
                    case 1: return window(1, 200 - offset, 180, 640 + offset, 420)
                    default: return item
                    }
                }
                simulate(frames: 2, windows: windows)
            }
            // Дать снегу долететь до окна ниже.
            simulate(frames: 45, windows: windows)
        }
        if let mode = Config.snapshotLid {
            // Те же расчёты, что на настоящей крышке, только углы заданы вручную.
            let screen = CGRect(x: 0, y: 0, width: width, height: height)
            var motion = LidMotion()
            var angle: Float = 110
            let frames = mode == "shake" ? 12 : 40
            for frame in 0..<frames {
                if mode == "shake" {
                    if frame < 6 { angle -= 14 }
                } else {
                    angle = max(60, angle - 3)
                }
                let state = motion.add(angle: angle, time: Double(time))
                caps.applyLid(state, on: screen, now: Double(time))
                simulate(frames: 1, windows: windows)
            }
        }
        var sheet: SnowSheet?
        if let mode = Config.snapshotSheet {
            sheet = SnowSheet(device: device, width: Float(width), height: Float(height), seed: 5)
            sheet?.cover()
            if mode == "wiped" {
                // Курсор прошёл по экрану дугой.
                for step in 0...40 {
                    let t = Float(step) / 40
                    sheet?.wipe(to: SIMD2(250 + t * 900, 520 - sin(t * 3.14) * 300), caps: caps, origin: .zero)
                    simulate(frames: 1, windows: windows)
                }
            }
            if mode == "shaken" {
                for _ in 0..<4 {
                    sheet?.applyLid(LidState(angle: 100, speed: 170, shake: 0.55, dt: dt), caps: caps, origin: .zero)
                    simulate(frames: 1, windows: windows)
                }
            }
            sheet?.step()
        }
        if Config.snapshotSlide {
            // Все окна уехали одинаково, как при смене Space: снег срывается и падает.
            windows = windows.map { window(CGWindowID($0.id), $0.frame.minX - 60, $0.frame.minY, $0.frame.width, $0.frame.height) }
            simulate(frames: 10, windows: windows)
        }

        let pixelWidth = width * scale, pixelHeight = height * scale
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: pixelWidth, height: pixelHeight, mipmapped: false)
        descriptor.usage = .renderTarget
        descriptor.storageMode = .managed
        guard let texture = device.makeTexture(descriptor: descriptor),
              let flakes = device.makeBuffer(
                  length: MemoryLayout<FlakeInstance>.stride * (field.instanceCount + CapField.maxDebris),
                  options: .storageModeShared),
              let capVertices = device.makeBuffer(
                  length: MemoryLayout<CapVertex>.stride * FrameRing.capVertexCapacity, options: .storageModeShared),
              let commands = queue.makeCommandBuffer() else { return false }

        let scene = SceneInput(
            time: time, viewport: SIMD2(Float(width), Float(height)), origin: .zero, debugRects: [],
            santa: Config.snapshotSanta ? SantaDraw(rect: SIMD4(620, 770, 420, 152), frame: 0) : nil,
            sheet: sheet?.texture)
        let flakePointer = flakes.contents().assumingMemoryBound(to: FlakeInstance.self)
        let flakeCount = field.writeInstances(to: flakePointer, time: time)
        let debrisCount = caps.writeDebris(to: flakePointer + flakeCount, origin: .zero, viewport: scene.viewport)
        var draws: [CapDraw] = []
        caps.buildMesh(
            into: capVertices.contents().assumingMemoryBound(to: CapVertex.self),
            capacity: FrameRing.capVertexCapacity, origin: .zero, viewport: scene.viewport, draws: &draws)
        encode(commands, pass: Self.renderPass(texture: texture), flakes: flakes, flakeCount: flakeCount + debrisCount,
               capVertices: capVertices, draws: draws,
               ball: caps.ballForDrawing(origin: scene.origin), scene: scene)
        if let blit = commands.makeBlitCommandEncoder() {
            blit.synchronize(resource: texture)
            blit.endEncoding()
        }
        commands.commit()
        commands.waitUntilCompleted()

        let bytesPerRow = pixelWidth * 4
        var pixels = [UInt8](repeating: 0, count: bytesPerRow * pixelHeight)
        texture.getBytes(
            &pixels, bytesPerRow: bytesPerRow,
            from: MTLRegionMake2D(0, 0, pixelWidth, pixelHeight), mipmapLevel: 0)

        let info = CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        guard let overlayContext = CGContext(
                  data: &pixels, width: pixelWidth, height: pixelHeight, bitsPerComponent: 8,
                  bytesPerRow: bytesPerRow, space: space, bitmapInfo: info),
              let overlay = overlayContext.makeImage(),
              let output = CGContext(
                  data: nil, width: pixelWidth, height: pixelHeight, bitsPerComponent: 8,
                  bytesPerRow: 0, space: space, bitmapInfo: info)
        else { return false }

        // Рабочий стол и окна снизу, снег поверх, как на настоящем экране.
        output.setFillColor(CGColor(srgbRed: 0.05, green: 0.08, blue: 0.18, alpha: 1))
        output.fill(CGRect(x: 0, y: 0, width: pixelWidth, height: pixelHeight))
        output.saveGState()
        output.scaleBy(x: CGFloat(scale), y: CGFloat(scale))
        for window in windows.reversed() {
            let rect = CGRect(
                x: window.frame.minX, y: CGFloat(height) - window.frame.maxY,
                width: window.frame.width, height: window.frame.height)
            // Радиусы разные, как у настоящих окон: снег не должен зависеть от них.
            let radius: CGFloat = [28, 27, 20, 14][Int(window.id) % 4]
            output.addPath(CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil))
            output.setFillColor(CGColor(srgbRed: 0.13, green: 0.14, blue: 0.16, alpha: 1))
            output.fillPath()
            output.addPath(CGPath(roundedRect: rect, cornerWidth: radius, cornerHeight: radius, transform: nil))
            output.setStrokeColor(CGColor(srgbRed: 0.32, green: 0.33, blue: 0.36, alpha: 1))
            output.strokePath()
        }
        output.restoreGState()
        output.draw(overlay, in: CGRect(x: 0, y: 0, width: pixelWidth, height: pixelHeight))

        guard let image = output.makeImage(),
              let destination = CGImageDestinationCreateWithURL(
                  URL(fileURLWithPath: path) as CFURL, "public.png" as CFString, 1, nil)
        else { return false }
        CGImageDestinationAddImage(destination, image, nil)
        return CGImageDestinationFinalize(destination)
    }
}
