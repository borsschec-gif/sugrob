import AppKit
import QuartzCore
import simd

@MainActor
final class OverlayWindow: NSWindow {
    let overlayView: OverlayView

    init(screen: NSScreen, tracker: WindowTracker, caps: CapField, renderer: Renderer, seed: UInt64) {
        overlayView = OverlayView(screen: screen, tracker: tracker, caps: caps, renderer: renderer, seed: seed)
        super.init(contentRect: screen.frame, styleMask: .borderless, backing: .buffered, defer: false)
        setFrame(screen.frame, display: false)
        isReleasedWhenClosed = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        ignoresMouseEvents = true
        level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.dockWindow)) + 1)
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        contentView = overlayView
    }

    func setPaused(_ paused: Bool) {
        overlayView.setPaused(paused)
        if paused { orderOut(nil) } else { orderFrontRegardless() }
    }

    /// `close()` не вынимает вид из окна, и счётчик кадров продолжил бы рисовать в закрытое окно.
    func tearDown() {
        overlayView.stop()
        orderOut(nil)
        contentView = nil
        close()
    }
}

@MainActor
final class OverlayView: NSView {
    private let tracker: WindowTracker
    private let caps: CapField
    private let renderer: Renderer
    private var currentFPS: Float = Config.fps
    /// Левый верхний угол экрана в координатах CoreGraphics.
    private let screenOrigin: CGPoint
    private let primaryHeight: CGFloat
    var santa: SantaController?
    var governor: PowerGovernor?
    var lid: LidSensor?
    private let sheet: SnowSheet?
    private let isBuiltIn: Bool
    private let screenBounds: CGRect
    private let metalLayer = CAMetalLayer()
    private let flakeLayers: FlakeLayers
    /// Шапки меняются медленно: без движения слой Metal перерисовывается редко.
    private static let stillInterval: CFTimeInterval = 0.1
    private var lastMetalDraw: CFTimeInterval = 0
    private var metalFrameCount = 0
    private var field: SnowField
    private let ring: FrameRing?
    private var link: CADisplayLink?
    private var lastTimestamp: CFTimeInterval = 0
    private var frameCount = 0

    private static let outline = SIMD4<Float>(0.25, 0.85, 1.0, 0.9)
    private static let topEdge = SIMD4<Float>(1.0, 1.0, 1.0, 1.0)

    init(screen: NSScreen, tracker: WindowTracker, caps: CapField, renderer: Renderer, seed: UInt64) {
        self.tracker = tracker
        self.caps = caps
        self.renderer = renderer
        primaryHeight = NSScreen.screens.first?.frame.height ?? screen.frame.height
        screenOrigin = CGPoint(x: screen.frame.minX, y: primaryHeight - screen.frame.maxY)
        screenBounds = CGRect(origin: screenOrigin, size: screen.frame.size)
        let displayID = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? CGDirectDisplayID
        isBuiltIn = displayID.map { CGDisplayIsBuiltin($0) != 0 } ?? false
        field = SnowField(width: Float(screen.frame.width), height: Float(screen.frame.height), seed: seed)
        sheet = SnowSheet(device: renderer.device, width: Float(screen.frame.width), height: Float(screen.frame.height), seed: seed)
        ring = FrameRing(device: renderer.device, flakeCapacity: field.instanceCount + CapField.maxDebris)
        flakeLayers = FlakeLayers(count: field.instanceCount)
        super.init(frame: CGRect(origin: .zero, size: screen.frame.size))
        wantsLayer = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) не используется") }

    override func makeBackingLayer() -> CALayer {
        metalLayer.device = renderer.device
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.isOpaque = false
        metalLayer.framebufferOnly = true
        let root = CALayer()
        root.addSublayer(metalLayer)
        root.addSublayer(flakeLayers.host)
        return root
    }

    /// Авторазмер слоёв здесь не годится: корневой слой получает размер позже и растянул бы их вдвое.
    override func layout() {
        super.layout()
        fitLayers()
    }

    private func fitLayers() {
        guard metalLayer.frame != bounds else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        metalLayer.frame = bounds
        flakeLayers.host.frame = bounds
        CATransaction.commit()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        link?.invalidate()
        link = nil
        guard window != nil else { return }
        let link = displayLink(target: self, selector: #selector(tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: Config.fps, maximum: Config.fps, preferred: Config.fps)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    func setPaused(_ paused: Bool) {
        link?.isPaused = paused
    }

    func stop() {
        link?.invalidate()
        link = nil
    }

    func coverWithSnow() {
        sheet?.cover()
    }

    func clearSnowCover() {
        sheet?.clear()
    }

    /// Кадры снегопада и кадры слоя Metal с прошлого вызова.
    func takeFrameCount() -> (flakes: Int, metal: Int) {
        defer { frameCount = 0; metalFrameCount = 0 }
        return (frameCount, metalFrameCount)
    }

    @objc private func tick(_ link: CADisplayLink) {
        guard let window, let ring else { return }
        // После сна или паузы шаг ограничен, чтобы снег не прыгал.
        let dt = lastTimestamp == 0 ? 0 : min(link.timestamp - lastTimestamp, 1.0 / 15)
        lastTimestamp = link.timestamp
        let time = Float(link.timestamp.truncatingRemainder(dividingBy: 100_000))
        let snapshot = tracker.snapshot
        let isMoving = snapshot.isFast || caps.hasFastMotion || santa?.isRiding == true
            || (lid.map { abs($0.flakeKick) > 1 } ?? false)
        updateFrameRate(of: link, isFast: isMoving)

        field.density = governor?.density ?? 1
        let origin = SIMD2(Float(screenOrigin.x), Float(screenOrigin.y))
        caps.sync(snapshot, now: link.timestamp)
        var kick: Float = 0
        if isBuiltIn, let lid {
            if let state = lid.tick(now: link.timestamp) {
                caps.applyLid(state, on: screenBounds, now: link.timestamp)
                sheet?.applyLid(state, caps: caps, origin: origin)
            }
            kick = lid.flakeKick
        }
        field.step(dt: Float(dt), time: time, caps: caps, origin: origin, now: link.timestamp, kick: kick)
        caps.relax(now: link.timestamp)
        // NSEvent не ходит за курсором в WindowServer, в отличие от CGEvent.
        let mouse = NSEvent.mouseLocation
        let cursor = CGPoint(x: mouse.x, y: primaryHeight - mouse.y)
        // Пока экран занесён, курсор счищает снег, а не катает снежок.
        if let sheet, sheet.isActive {
            sheet.wipe(to: SIMD2(Float(cursor.x), Float(cursor.y)) - origin, caps: caps, origin: origin)
            sheet.step()
        } else {
            caps.brush(cursor: cursor, now: link.timestamp)
        }
        caps.stepDebris(now: link.timestamp)
        santa?.tick(now: link.timestamp)

        fitLayers()
        flakeLayers.update(field: field, time: time)
        frameCount += 1

        let sheetIsActive = sheet?.isActive == true
        let needsMetal = isMoving || sheetIsActive || Config.debugOverlay
            || link.timestamp - lastMetalDraw >= Self.stillInterval
        guard needsMetal else { return }
        lastMetalDraw = link.timestamp
        metalFrameCount += 1

        let scale = window.backingScaleFactor
        // Лишняя установка свойств слоя стоит транзакции Core Animation на каждом кадре.
        let drawableSize = CGSize(width: bounds.width * scale, height: bounds.height * scale)
        if metalLayer.contentsScale != scale { metalLayer.contentsScale = scale }
        if metalLayer.drawableSize != drawableSize { metalLayer.drawableSize = drawableSize }

        renderer.draw(
            field: field, caps: caps,
            scene: SceneInput(
                time: time,
                viewport: SIMD2(Float(bounds.width), Float(bounds.height)),
                origin: origin,
                debugRects: Config.debugOverlay ? debugRects(from: snapshot) : [],
                santa: santa?.drawItem(origin: origin),
                sheet: sheetIsActive ? sheet?.texture : nil,
                drawsFlakes: false),
            ring: ring, into: metalLayer)
    }

    /// Пока что-то двигается быстро, кадров больше, иначе шапки дёргаются вместе с окном.
    private func updateFrameRate(of link: CADisplayLink, isFast: Bool) {
        let base = governor?.fps ?? Config.fps
        let target = isFast ? max(Config.motionFPS, base) : base
        guard target != currentFPS else { return }
        currentFPS = target
        link.preferredFrameRateRange = CAFrameRateRange(minimum: target, maximum: target, preferred: target)
    }

    private func debugRects(from snapshot: WindowSnapshot) -> [RectInstance] {
        var result: [RectInstance] = []
        func add(_ x: CGFloat, _ y: CGFloat, _ width: CGFloat, _ height: CGFloat, _ color: SIMD4<Float>) {
            result.append(RectInstance(rect: SIMD4(Float(x), Float(y), Float(width), Float(height)), color: color))
        }

        for index in snapshot.windows.indices.reversed() {
            let frame = snapshot.windows[index].frame.offsetBy(dx: -screenOrigin.x, dy: -screenOrigin.y)
            let thickness: CGFloat = 1.5
            add(frame.minX, frame.maxY - thickness, frame.width, thickness, Self.outline)
            add(frame.minX, frame.minY, thickness, frame.height, Self.outline)
            add(frame.maxX - thickness, frame.minY, thickness, frame.height, Self.outline)
            for segment in snapshot.visibleTopSegments(at: index) {
                add(segment.lowerBound - screenOrigin.x, frame.minY - 4,
                    segment.upperBound - segment.lowerBound, 4, Self.topEdge)
            }
        }
        return result
    }
}
