import AppKit
import QuartzCore
import simd

/// Что рисовать на кадре: прямоугольник в локальных точках экрана и номер спрайта.
struct SantaDraw {
    var rect: SIMD4<Float>
    var frame: Int
}

/// Проезд Деда Мороза и его подарок. Живёт в координатах CoreGraphics основного экрана.
@MainActor
final class SantaController {
    private static let speed: Float = 230
    private static let phrases = [
        "С наступающим!",
        "Оливье само себя не нарежет",
        "Рендер закончится к Новому году",
        "Снег идёт. Дедлайн тоже",
        "Мандарины официально считаются овощем",
        "В этом году ты вёл себя… на монтаже",
        "Ёлку поливать не забывай",
        "Подарок был внутри тебя",
    ]

    private let caps: CapField
    private var x: Float?
    private var dropX: Float = 0
    private var didDrop = false
    private var ground: Float?
    private var lastTick: CFTimeInterval = 0
    private var nextRide: CFTimeInterval = 0
    private var animation: Float = 0
    var autoRides = true

    private var giftPanel: NSPanel?
    private var giftPosition = SIMD2<Float>.zero
    private var giftVelocity: Float = 0
    private var giftExpires: CFTimeInterval = 0

    init(caps: CapField) {
        self.caps = caps
    }

    private var screen: CGRect { CGDisplayBounds(CGMainDisplayID()) }

    /// Запускает проезд из меню, не дожидаясь расписания.
    var isRiding: Bool { x != nil }

    func rideNow() {
        guard x == nil else { return }
        startRide()
    }

    private func startRide() {
        let width = Float(screen.width)
        x = width + 20
        dropX = width * Float.random(in: 0.3...0.65)
        didDrop = false
    }

    /// Вызывается каждым экраном на каждом кадре, срабатывает один раз.
    func tick(now: CFTimeInterval) {
        guard now > lastTick else { return }
        let dt = Float(min(now - lastTick, 1.0 / 15))
        lastTick = now

        if nextRide == 0 { nextRide = now + 60 }
        if autoRides, x == nil, now >= nextRide {
            nextRide = now + Double.random(in: 240...480)
            startRide()
        }

        let target = groundLevel()
        ground = ground.map { $0 + (target - $0) * min(1, dt * 14) } ?? target
        updateRide(dt: dt, now: now)
        updateGift(dt: dt, now: now)
    }

    /// На видимом Dock тройка едет по его нижней кромке, иначе вдоль низа экрана.
    private func groundLevel() -> Float {
        let bottom = Float(screen.maxY)
        if let dock = caps.dockFrame, Float(dock.minY) < bottom - 1 {
            return min(Float(dock.maxY) - 6, bottom - 4)
        }
        return bottom - 4
    }

    private func updateRide(dt: Float, now: CFTimeInterval) {
        guard var position = x, let ground else { return }
        position -= Self.speed * dt
        animation += dt * 9

        // Из-под полоза летит снег.
        caps.spawnPuff(
            at: SIMD2(position + 190, ground - 3), count: 1, speed: 20...90, lift: 20...110, life: 0.3...0.6)

        if !didDrop, position + 150 <= dropX {
            didDrop = true
            dropGift(atX: position + 180, y: ground - 120, now: now)
        }
        x = position < -Float(SantaArt.width) - 20 ? nil : position
    }

    func drawItem(origin: SIMD2<Float>) -> SantaDraw? {
        guard let x, let ground else { return nil }
        let width = Float(SantaArt.width), height = Float(SantaArt.height)
        let frame = Int(animation) % SantaArt.frameCount
        return SantaDraw(
            rect: SIMD4(x - origin.x, ground - height - origin.y, width, height), frame: frame)
    }

    // MARK: Подарок

    private func dropGift(atX x: Float, y: Float, now: CFTimeInterval) {
        giftPanel?.close()
        let size = NSSize(width: 44, height: 44)
        let panel = NSPanel(
            contentRect: NSRect(origin: .zero, size: size),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        // Выше оверлея, иначе клик до окна не дойдёт.
        panel.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.dockWindow)) + 2)
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]

        let view = GiftView(frame: NSRect(origin: .zero, size: size))
        view.image = SantaArt.giftImage()
        view.onClick = { [weak self] in self?.openGift() }
        panel.contentView = view

        giftPanel = panel
        giftPosition = SIMD2(x, y)
        giftVelocity = 0
        giftExpires = now + 60
        moveGiftPanel()
        panel.orderFrontRegardless()
    }

    private func updateGift(dt: Float, now: CFTimeInterval) {
        guard giftPanel != nil, let ground else { return }
        if now >= giftExpires {
            removeGift()
            return
        }
        let rest = ground - 44
        if giftPosition.y < rest || giftVelocity != 0 {
            giftVelocity += 1300 * dt
            giftPosition.y += giftVelocity * dt
            if giftPosition.y >= rest {
                giftPosition.y = rest
                // Небольшой отскок, затем подарок лежит.
                giftVelocity = abs(giftVelocity) > 180 ? -abs(giftVelocity) * 0.32 : 0
            }
        } else {
            giftPosition.y = rest
        }
        moveGiftPanel()
    }

    private func moveGiftPanel() {
        let primaryHeight = Float(screen.height)
        let origin = NSPoint(
            x: CGFloat(giftPosition.x), y: CGFloat(primaryHeight - giftPosition.y - 44))
        if giftPanel?.frame.origin != origin { giftPanel?.setFrameOrigin(origin) }
    }

    func removeGiftIfAny() {
        removeGift()
    }

    private func removeGift() {
        giftPanel?.close()
        giftPanel = nil
    }

    private func openGift() {
        let center = giftPosition + SIMD2(22, 22)
        caps.spawnPuff(at: center, count: 170, speed: 80...420, lift: 120...620, life: 0.8...1.5)
        showPhrase(near: center)
        removeGift()
    }

    private func showPhrase(near center: SIMD2<Float>) {
        guard let phrase = Self.phrases.randomElement() else { return }
        let label = NSTextField(labelWithString: phrase)
        label.font = NSFont(name: "Georgia-BoldItalic", size: 19) ?? .boldSystemFont(ofSize: 19)
        label.textColor = NSColor(srgbRed: 1, green: 0.95, blue: 0.82, alpha: 1)
        label.sizeToFit()

        let padding: CGFloat = 18
        let size = NSSize(width: label.frame.width + padding * 2, height: label.frame.height + padding)
        let screenWidth = screen.width
        let x = min(max(12, CGFloat(center.x) - size.width / 2), screenWidth - size.width - 12)
        let y = screen.height - CGFloat(center.y) + 34

        let panel = NSPanel(
            contentRect: NSRect(x: x, y: y, width: size.width, height: size.height),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.ignoresMouseEvents = true
        panel.level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.dockWindow)) + 2)
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]

        let background = NSView(frame: NSRect(origin: .zero, size: size))
        background.wantsLayer = true
        background.layer?.backgroundColor = NSColor(srgbRed: 0.09, green: 0.13, blue: 0.27, alpha: 0.94).cgColor
        background.layer?.cornerRadius = 14
        label.frame.origin = NSPoint(x: padding, y: padding / 2)
        background.addSubview(label)
        panel.contentView = background
        panel.orderFrontRegardless()

        Task { @MainActor in
            try? await Task.sleep(for: .seconds(3.5))
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = 0.4
                panel.animator().alphaValue = 0
            }, completionHandler: {
                MainActor.assumeIsolated { panel.close() }
            })
        }
    }
}

private final class GiftView: NSImageView {
    var onClick: (() -> Void)?

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        onClick?()
    }
}
