import CoreGraphics
import Foundation
import QuartzCore
import simd

/// Формат совпадает со структурой CapVertex в шейдере.
struct CapVertex {
    /// x, y в точках экрана; глубина от поверхности снега; координата вдоль шапки.
    var position: SIMD4<Float>
    var thickness: Float
}

/// Снежок для отрисовки: окна перед его владельцем его закрывают.
struct BallDraw {
    var center: SIMD2<Float>
    var radius: Float
    var rotation: Float
    var ownerID: CGWindowID
}

/// Одно окно в порядке отрисовки: стереть его прямоугольник и нарисовать его шапку.
struct CapDraw {
    var rect: SIMD4<Float>
    var vertexStart: Int
    var vertexCount: Int
    var windowID: CGWindowID
    var cornerRadius: Float
}

/// Частица сорвавшегося снега, в координатах CoreGraphics.
private struct SnowDebris {
    var position: SIMD2<Float>
    var velocity: SIMD2<Float>
    var radius: Float
    var alpha: Float
    var life: Float
    var gravity: Float = 900
    /// Окно, с которого сорвалась частица: обратно на него она не садится.
    var source: CGWindowID = 0
}

/// Снежок: катится по окну `windowID` или падает, если окна нет.
private struct SnowBall {
    var position: SIMD2<Float>
    var velocity: SIMD2<Float>
    var radius: Float
    var rotation: Float
    var windowID: CGWindowID?
    var droppedFrom: CGWindowID = 0
}

/// Карта высот снега вдоль верхнего края окна.
private final class SnowCap {
    static let step: Float = 2
    /// Настоящий радиус угла, если удалось измерить. Без него снег лежит только на ровном участке края.
    private(set) var cornerRadius: Float?
    private var snowStart: Float { cornerRadius.map { $0 * 0.36 } ?? 16 }
    private var snowTaper: Float { cornerRadius == nil ? 10 : 14 }

    private(set) var heights: [Float]
    private var detail: [Float] = []
    /// Сдвиг левого края в точках карты, чтобы неровности не прыгали при ресайзе слева.
    private var detailOffset = 0
    /// Остаток сдвига левого края меньше одного шага карты.
    private var leftCarry: Float = 0
    private(set) var width: Float
    private let phases: SIMD2<Float>
    var activeUntil: CFTimeInterval = 0
    /// Сглаженная скорость окна, pt/s.
    var velocity = SIMD2<Float>.zero
    /// Положения окна за последние 100 мс, пока его держат мышью.
    var trail: [(origin: SIMD2<Float>, time: CFTimeInterval)] = []
    var lastSeen: CFTimeInterval

    var count: Int { heights.count }

    init(width: Float, phases: SIMD2<Float>, now: CFTimeInterval) {
        self.width = width
        self.phases = phases
        lastSeen = now
        heights = [Float](repeating: 0, count: Self.sampleCount(for: width))
        rebuildDetail()
    }

    private static func sampleCount(for width: Float) -> Int {
        max(2, Int(width / step) + 1)
    }

    private func rebuildDetail() {
        detail = (0..<heights.count).map { index in
            let u = Float(index + detailOffset) * Self.step
            return 0.7 * sin(u * 0.13 + phases.x) + 0.4 * sin(u * 0.31 + phases.y)
        }
    }

    /// Начальный слой для окон, которые уже были открыты при запуске.
    func seed(using generator: inout SeededGenerator) {
        let base = Float.random(in: 7...10, using: &generator)
        let wave1 = Float.random(in: 0.02...0.05, using: &generator)
        let wave2 = Float.random(in: 0.06...0.11, using: &generator)
        let phase1 = Float.random(in: 0...6.28, using: &generator)
        let phase2 = Float.random(in: 0...6.28, using: &generator)
        for index in heights.indices {
            let u = Float(index) * Self.step
            heights[index] = max(0, base + 3 * sin(u * wave1 + phase1) + 1.5 * sin(u * wave2 + phase2))
        }
    }

    /// Шапка привязана к экранным координатам: с уходящей стороны снег ссыпается,
    /// на открывшийся участок ложится тонкий слой.
    func reshape(
        from old: CGRect, to new: CGRect, into debris: inout [SnowDebris], limit: Int,
        using generator: inout SeededGenerator, isVisible: (SIMD2<Float>) -> Bool
    ) {
        let oldSurface = (0..<heights.count).map { surfaceHeight(at: $0) }

        let leftShift = Float(new.minX - old.minX) + leftCarry
        let left = Int((leftShift / Self.step).rounded())
        leftCarry = leftShift - Float(left) * Self.step
        if left > 0 {
            heights.removeFirst(min(left, heights.count - 2))
        } else if left < 0 {
            heights.insert(contentsOf: repeatElement(0, count: -left), at: 0)
        }
        detailOffset += left

        let rightGrowth = Self.sampleCount(for: Float(new.width)) - heights.count
        if rightGrowth < 0 {
            heights.removeLast(min(-rightGrowth, heights.count - 2))
        } else if rightGrowth > 0 {
            heights.append(contentsOf: repeatElement(0, count: rightGrowth))
        }
        width = Float(new.width)
        rebuildDetail()

        // Снег, ушедший под скрытую зону у края, ссыпался и обратно не вернётся.
        let hidden = min(Int(snowStart / Self.step), heights.count)
        let leftShrank = new.minX > old.minX, rightShrank = new.maxX < old.maxX
        let leftGrew = new.minX < old.minX, rightGrew = new.maxX > old.maxX
        if leftShrank { for index in 0..<hidden { heights[index] = 0 } }
        if rightShrank { for index in (heights.count - hidden)..<heights.count { heights[index] = 0 } }

        // При расширении новый участок присыпается из общего сугроба: объём снега не растёт,
        // вся шапка становится тоньше.
        let total = heights.reduce(0, +)
        let fill = 0.7 * total / Float(heights.count)
        if fill > 0.3, leftGrew || rightGrew {
            if leftGrew { raise(0..<min(heights.count, hidden + max(0, -left)), to: fill) }
            if rightGrew { raise(max(0, heights.count - max(0, rightGrowth) - hidden)..<heights.count, to: fill) }
            let spread = total / heights.reduce(0, +)
            for index in heights.indices { heights[index] *= spread }
        }

        for (index, before) in oldSurface.enumerated() {
            let x = Float(old.minX) + Float(index) * Self.step
            let isLeftHalf = x < Float(new.midX)
            guard isLeftHalf ? leftShrank : rightShrank else { continue }
            let newIndex = Int(((x - Float(new.minX)) / Self.step).rounded())
            let after = heights.indices.contains(newIndex) ? surfaceHeight(at: newIndex) : 0
            let lost = before - after
            guard lost > 0.25, isVisible(SIMD2(x, Float(old.minY) - before / 2)) else { continue }

            let share = lost / 2.5
            let count = Int(share) + (Float.random(in: 0...1, using: &generator) < share - Float(Int(share)) ? 1 : 0)
            let direction: Float = isLeftHalf ? -1 : 1
            for _ in 0..<count where debris.count < limit {
                debris.append(SnowDebris(
                    position: SIMD2(x, Float(old.minY) - Float.random(in: after...before, using: &generator)),
                    velocity: SIMD2(
                        direction * Float.random(in: 0...70, using: &generator),
                        Float.random(in: -30...30, using: &generator)),
                    radius: Float.random(in: 1.8...4.6, using: &generator),
                    alpha: Float.random(in: 0.75...1, using: &generator),
                    life: Float.random(in: 0.9...1.6, using: &generator)))
            }
        }
    }

    private func raise(_ range: Range<Int>, to level: Float) {
        for index in range {
            let wave = 0.75 + 0.25 * sin(Float(index + detailOffset) * 0.09 + phases.x)
            heights[index] = level * wave
        }
    }

    /// Четверть окружности: конец шапки круглый, без острого шипа.
    private func envelope(at u: Float) -> Float {
        let edge = min(u, width - u)
        let t = max(0, min(1, (edge - snowStart) / snowTaper))
        return (1 - (1 - t) * (1 - t)).squareRoot()
    }

    func canHoldSnow(atLocal u: Float) -> Bool {
        min(u, width - u) > snowStart
    }

    func setCornerRadius(_ radius: Float) {
        cornerRadius = radius
    }

    /// На сколько край окна опускается в этой точке из-за скругления угла.
    func baseDrop(atLocal u: Float) -> Float {
        guard let radius = cornerRadius else { return 0 }
        let edge = max(0, min(u, width - u))
        guard edge < radius else { return 0 }
        let offset = radius - edge
        return radius - (radius * radius - offset * offset).squareRoot()
    }

    /// Снег срывается частицами с видимых участков; вся шапка обнуляется.
    func shed(
        windowOrigin: SIMD2<Float>, direction: Float, fling: SIMD2<Float>?,
        into debris: inout [SnowDebris], limit: Int,
        using generator: inout SeededGenerator, isVisible: (SIMD2<Float>) -> Bool
    ) {
        for index in stride(from: 0, to: heights.count, by: 2) {
            let u = Float(index) * Self.step
            let height = surfaceHeight(at: index)
            guard height > 0.6 else { continue }
            let baseY = windowOrigin.y + baseDrop(atLocal: u)
            guard isVisible(SIMD2(windowOrigin.x + u, baseY - height / 2)) else { continue }

            for _ in 0..<(1 + Int(height / 5)) where debris.count < limit {
                let position = SIMD2(
                    windowOrigin.x + u + Float.random(in: -2...2, using: &generator),
                    baseY - Float.random(in: 0...height, using: &generator))
                let radius = Float.random(in: 1.8...4.6, using: &generator)
                let alpha = Float.random(in: 0.75...1, using: &generator)
                if let fling {
                    // Снег тяжёлый: срывается комьями и почти сразу падает.
                    debris.append(SnowDebris(
                        position: position,
                        velocity: fling * Float.random(in: 0.08...0.2, using: &generator) + SIMD2(
                            Float.random(in: -60...60, using: &generator),
                            Float.random(in: -60...40, using: &generator)),
                        radius: radius, alpha: alpha,
                        life: Float.random(in: 1...1.6, using: &generator),
                        gravity: 1300))
                } else {
                    debris.append(SnowDebris(
                        position: position,
                        velocity: SIMD2(
                            Float.random(in: -30...30, using: &generator) + direction * Float.random(in: 0...60, using: &generator),
                            Float.random(in: -20...60, using: &generator)),
                        radius: radius, alpha: alpha,
                        life: Float.random(in: 0.9...1.6, using: &generator)))
                }
            }
        }
        clear()
    }

    /// Окно тащат вбок: с отстающего края шапки снег сыплется тонкой струйкой.
    func shedTrailingEdge(
        windowOrigin: SIMD2<Float>, fromLeft: Bool, distance: Float, into debris: inout [SnowDebris], limit: Int,
        using generator: inout SeededGenerator, isVisible: (SIMD2<Float>) -> Bool
    ) {
        let found = fromLeft
            ? heights.indices.first { surfaceHeight(at: $0) > 1 }
            : heights.indices.last { surfaceHeight(at: $0) > 1 }
        guard let edge = found else { return }

        let band = 8
        let stepInward = fromLeft ? 1 : -1
        let share = distance / 6
        let count = Int(share) + (Float.random(in: 0...1, using: &generator) < share - Float(Int(share)) ? 1 : 0)
        for _ in 0..<count where debris.count < limit {
            let index = edge + stepInward * Int.random(in: 0..<band, using: &generator)
            guard heights.indices.contains(index) else { continue }
            let height = surfaceHeight(at: index)
            // Сразу за краем окна: над самим окном снег не упал бы на окно позади него.
            let gap = Float.random(in: 0...5, using: &generator)
            let x = fromLeft ? windowOrigin.x - gap : windowOrigin.x + width + gap
            guard height > 0.5, isVisible(SIMD2(x, windowOrigin.y - height / 2)) else { continue }
            debris.append(SnowDebris(
                position: SIMD2(x, windowOrigin.y - Float.random(in: 0...height, using: &generator)),
                velocity: SIMD2(
                    Float.random(in: -12...12, using: &generator),
                    Float.random(in: 0...30, using: &generator)),
                radius: Float.random(in: 1.4...3, using: &generator),
                alpha: Float.random(in: 0.75...1, using: &generator),
                life: Float.random(in: 0.9...1.4, using: &generator)))
        }

        for offset in 0..<band {
            let index = edge + stepInward * offset
            guard heights.indices.contains(index) else { continue }
            heights[index] = max(0, heights[index] - min(1.2, distance * 0.1))
        }
    }

    /// Снежок забирает снег под собой. Возвращает собранную площадь в pt².
    func carve(atLocal u: Float, halfWidth: Float, strength: Float) -> Float {
        guard u >= 0, u <= width else { return 0 }
        let center = Int(u / Self.step)
        let radius = max(1, Int(halfWidth / Self.step))
        var taken: Float = 0
        for index in max(0, center - radius)...min(heights.count - 1, max(0, center + radius)) {
            let amount = heights[index] * strength * 0.8
            heights[index] -= amount
            taken += amount * Self.step
        }
        return taken
    }

    /// Курсор прошёл сквозь снег: сбивает его вокруг себя.
    func brush(
        atLocal u: Float, windowOrigin: SIMD2<Float>, velocity cursorVelocity: SIMD2<Float>,
        into debris: inout [SnowDebris], limit: Int, using generator: inout SeededGenerator
    ) {
        let center = Int(u / Self.step)
        let radius = 6
        for index in max(0, center - radius)...min(heights.count - 1, center + radius) {
            let visible = surfaceHeight(at: index)
            let falloff = 1 - Float(abs(index - center)) / Float(radius + 1)
            let taken = heights[index] * falloff * 0.7
            heights[index] -= taken
            guard visible > 0.5 else { continue }

            let share = taken / 3
            let count = Int(share) + (Float.random(in: 0...1, using: &generator) < share - Float(Int(share)) ? 1 : 0)
            for _ in 0..<count where debris.count < limit {
                debris.append(SnowDebris(
                    position: SIMD2(
                        windowOrigin.x + Float(index) * Self.step,
                        windowOrigin.y - Float.random(in: 0...visible, using: &generator)),
                    velocity: cursorVelocity * Float.random(in: 0.1...0.3, using: &generator) + SIMD2(
                        Float.random(in: -50...50, using: &generator),
                        Float.random(in: -80...0, using: &generator)),
                    radius: Float.random(in: 1.6...3.8, using: &generator),
                    alpha: Float.random(in: 0.75...1, using: &generator),
                    life: Float.random(in: 0.9...1.4, using: &generator),
                    gravity: 1300))
            }
        }
    }

    /// Пока окно тащат, с шапки понемногу срываются комки.
    func crumble(
        windowOrigin: SIMD2<Float>, intensity: Float, into debris: inout [SnowDebris], limit: Int,
        using generator: inout SeededGenerator, isVisible: (SIMD2<Float>) -> Bool
    ) {
        for _ in 0..<(1 + Int(intensity * 5)) where debris.count < limit {
            let index = Int.random(in: heights.indices, using: &generator)
            let height = surfaceHeight(at: index)
            let u = Float(index) * Self.step
            guard height > 1.5, isVisible(SIMD2(windowOrigin.x + u, windowOrigin.y - height / 2)) else { continue }

            for neighbour in max(0, index - 2)...min(heights.count - 1, index + 2) {
                heights[neighbour] = max(0, heights[neighbour] - Float.random(in: 0.3...0.9, using: &generator))
            }
            debris.append(SnowDebris(
                position: SIMD2(windowOrigin.x + u, windowOrigin.y - height),
                velocity: velocity * Float.random(in: 0.05...0.2, using: &generator) + SIMD2(
                    Float.random(in: -40...40, using: &generator),
                    Float.random(in: -40...40, using: &generator)),
                radius: Float.random(in: 1.4...3, using: &generator),
                alpha: Float.random(in: 0.75...1, using: &generator),
                life: Float.random(in: 0.7...1.2, using: &generator)))
        }
    }

    func clear() {
        for index in heights.indices { heights[index] = 0 }
        activeUntil = 0
    }

    /// Высота поверхности с учётом скругления углов и мелких неровностей.
    func surfaceHeight(at index: Int) -> Float {
        let u = Float(index) * Self.step
        let raw = heights[index]
        return max(0, raw + detail[index] * min(1, raw / 5)) * envelope(at: u)
    }

    func height(atLocal u: Float) -> Float {
        let index = max(0, min(heights.count - 1, Int(u / Self.step)))
        return surfaceHeight(at: index)
    }

    func deposit(atLocal u: Float, amount: Float, maxHeight: Float) {
        let center = Int(u / Self.step)
        let radius = 12
        for index in max(0, center - radius)...min(heights.count - 1, center + radius) {
            let distance = Float(index - center) / 5
            let room = max(0, 1 - heights[index] / maxHeight)
            heights[index] += amount * exp(-0.5 * distance * distance) * room
        }
    }

    /// Излишек крутизны сползает к соседним точкам, общий объём снега не меняется.
    func relax() {
        let maxStep: Float = 0.45
        for index in 0..<(heights.count - 1) {
            let difference = heights[index] - heights[index + 1]
            let excess = abs(difference) - maxStep
            guard excess > 0 else { continue }
            let transfer = excess * 0.25 * (difference > 0 ? 1 : -1)
            heights[index] -= transfer
            heights[index + 1] += transfer
        }
    }
}

/// Снег на всех окнах, в координатах CoreGraphics. Один на все экраны.
@MainActor
final class CapField {
    private static let maxHeight: Float = 16
    private static let maxCachedCaps = 64
    private static let landingChance: Float = 0.55
    static let maxDebris = 3000
    /// Сглаженная скорость окна при зажатой кнопке, pt/s: крошение и полный срыв шапки.
    private static let crumbleSpeed: Float = 500
    private static let flingSpeed: Float = 1500
    private static let closeDelay: CFTimeInterval = 0.15
    private static let maxBrushSpeed: Float = 1500
    /// Медленнее этого курсор скатывает снежок, а не стряхивает снег.
    private static let minBrushSpeed: Float = 250
    private static let maxBallRadius: Float = 34
    /// Меньший сдвиг за опрос считаем обычным движением окна, а не сменой Space.
    private static let slideThreshold: CGFloat = 12
    /// Сколько после последнего сдвига хлопья не садятся на окна.
    private static let slideHold: CFTimeInterval = 0.3

    private var caps: [CGWindowID: SnowCap] = [:]
    private var windows: [TrackedWindow] = []
    private var debris: [SnowDebris] = []
    private var lastDebrisStep: CFTimeInterval = 0
    private var slidingUntil: CFTimeInterval = 0
    private var pendingSlide: (list: [TrackedWindow], movers: Set<CGWindowID>, direction: Float, shift: Float)?
    private var pinnedDuringSlide = Set<CGWindowID>()
    private var pendingClosed: [(id: CGWindowID, list: [TrackedWindow], deadline: CFTimeInterval)] = []
    private var grabbedID: CGWindowID?
    private var wasMouseDown = false
    private var lastBrushTime: CFTimeInterval = 0
    private var lastBrushCursor: CGPoint?
    private var ball: SnowBall?
    var cursorEffectsEnabled = true
    private var dwellTime: Float = 0
    private var dwellPath: Float = 0
    private var lastSync: CFTimeInterval = -1
    private var lastRelax: CFTimeInterval = 0
    private var hasSeeded = false
    private var generator: SeededGenerator

    init(seed: UInt64) {
        generator = SeededGenerator(state: seed)
    }

    /// Снапшот окон применяется один раз, сколько бы экранов его ни запросили.
    func sync(_ snapshot: WindowSnapshot, now: CFTimeInterval) {
        guard snapshot.timestamp != lastSync else { return }
        lastSync = snapshot.timestamp
        let previous = windows
        windows = snapshot.windows
        handleSlide(from: previous, isMouseDown: snapshot.isMouseDown, now: now)

        let isFirstBatch = !hasSeeded && snapshot.timestamp > 0
        let previousFrames = Dictionary(previous.map { ($0.id, $0.frame) }, uniquingKeysWith: { first, _ in first })
        for (index, window) in windows.enumerated() where window.holdsSnow {
            let width = Float(window.frame.width)
            if let cap = caps[window.id] {
                if cap.width != width {
                    let firstNew = debris.count
                    defer { markSource(window.id, from: firstNew) }
                    let front = windows[..<index]
                    cap.reshape(
                        from: previousFrames[window.id]
                            ?? CGRect(origin: window.frame.origin, size: CGSize(width: CGFloat(cap.width), height: window.frame.height)),
                        to: window.frame, into: &debris, limit: Self.maxDebris, using: &generator
                    ) { point in
                        let location = CGPoint(x: CGFloat(point.x), y: CGFloat(point.y))
                        return !front.contains { $0.frame.contains(location) }
                    }
                    cap.activeUntil = now + 3
                }
                cap.lastSeen = now
            } else {
                let phases = SIMD2(
                    Float.random(in: 0...6.28, using: &generator),
                    Float.random(in: 0...6.28, using: &generator))
                let cap = SnowCap(width: width, phases: phases, now: now)
                if isFirstBatch { cap.seed(using: &generator) }
                caps[window.id] = cap
            }
        }
        if isFirstBatch { hasSeeded = true }
        shedClosedWindows(previous: previous, now: now)
        applyWindowMotion(previous: previous, snapshot: snapshot, now: now)
        evictIfNeeded()
    }

    /// При смене Space окна сдвигаются на одно и то же расстояние по горизонтали без мыши.
    /// Окна «на всех рабочих столах» стоят на месте, новые окна приходящего Space не сравниваются.
    private func detectSlide(
        from old: [TrackedWindow], isMouseDown: Bool, now: CFTimeInterval
    ) -> (shift: CGFloat, movers: Set<CGWindowID>, pinned: Set<CGWindowID>)? {
        guard !isMouseDown else { return nil }
        let oldByID = Dictionary(old.filter(\.holdsSnow).map { ($0.id, $0.frame) }, uniquingKeysWith: { first, _ in first })
        // Порог нужен только для начала сдвига: идущий сдвиг продолжает нести частицы.
        let threshold = now < slidingUntil ? 1 : Self.slideThreshold
        var shift: CGFloat?
        var movers = Set<CGWindowID>()
        var pinned = Set<CGWindowID>()
        for window in windows where window.holdsSnow && !window.isDock {
            guard let frame = oldByID[window.id] else { continue }
            if frame == window.frame {
                pinned.insert(window.id)
                continue
            }
            let dx = window.frame.minX - frame.minX
            guard frame.size == window.frame.size, frame.minY == window.frame.minY,
                  abs(dx) >= threshold, shift == nil || shift == dx else { return nil }
            shift = dx
            movers.insert(window.id)
        }
        return shift.map { ($0, movers, pinned) }
    }

    /// Несколько окон подтверждают смену Space сразу. Одно окно могли просто перенести,
    /// поэтому для него нужен второй снимок подряд со сдвигом в ту же сторону.
    private func handleSlide(from previous: [TrackedWindow], isMouseDown: Bool, now: CFTimeInterval) {
        guard let slide = detectSlide(from: previous, isMouseDown: isMouseDown, now: now) else {
            pendingSlide = nil
            return
        }
        pinnedDuringSlide = slide.pinned
        let direction: Float = slide.shift < 0 ? -1 : 1
        var travelled = Float(slide.shift)

        if slide.movers.count >= 2 {
            pendingSlide = nil
            shedSnow(of: previous, only: slide.movers, direction: direction)
        } else if now < slidingUntil {
            pendingSlide = nil
        } else if let pending = pendingSlide, pending.direction == direction, pending.movers == slide.movers {
            pendingSlide = nil
            shedSnow(of: pending.list, only: pending.movers, direction: direction)
            travelled += pending.shift
        } else {
            pendingSlide = (previous, slide.movers, direction, Float(slide.shift))
            return
        }
        // Оверлей стоит на месте, поэтому частицы сдвигаем сами: они уезжают со старым Space
        // и не сыплются поверх нового.
        for index in debris.indices { debris[index].position.x += travelled }
        slidingUntil = now + Self.slideHold
    }

    /// Окно закрыли, свернули или спрятали: снег падает с места, где оно было.
    /// Решение откладывается: окна уходят из списка и при смене Space, и при коротком выпадении.
    private func shedClosedWindows(previous: [TrackedWindow], now: CFTimeInterval) {
        let current = Set(windows.map(\.id))
        let previousIDs = Set(previous.map(\.id))
        pendingClosed.removeAll { current.contains($0.id) }

        for window in previous where window.holdsSnow && !window.isDock && !current.contains(window.id) {
            // Вкладка окна: тот же кадр и приложение, но другой номер. Снег остаётся на месте.
            let twin = windows.first {
                $0.holdsSnow && !previousIDs.contains($0.id) && $0.frame == window.frame && $0.owner == window.owner
            }
            if let twin, let cap = caps[window.id] {
                caps[twin.id] = cap
                if ball?.windowID == window.id { ball?.windowID = twin.id }
                caps[window.id] = nil
            } else {
                pendingClosed.append((window.id, previous, now + Self.closeDelay))
            }
        }

        let expired = pendingClosed.filter { $0.deadline <= now }
        pendingClosed.removeAll { $0.deadline <= now }
        for entry in expired {
            if now >= slidingUntil { shedSnow(of: entry.list, only: [entry.id], direction: 0) }
            caps[entry.id] = nil
        }
    }

    private func applyWindowMotion(
        previous: [TrackedWindow], snapshot: WindowSnapshot, now: CFTimeInterval
    ) {
        if snapshot.isMouseDown && !wasMouseDown {
            grabbedID = windows.first { $0.frame.contains(snapshot.cursor) }?.id
        }
        wasMouseDown = snapshot.isMouseDown
        for cap in caps.values where !cap.trail.isEmpty && (grabbedID == nil || !snapshot.isMouseDown) {
            cap.trail.removeAll()
        }
        guard snapshot.isMouseDown, let grabbedID,
              let index = windows.firstIndex(where: { $0.id == grabbedID }),
              windows[index].holdsSnow, !windows[index].isDock,
              let cap = caps[grabbedID] else { return }

        let window = windows[index]
        guard let old = previous.first(where: { $0.id == grabbedID })?.frame, old.size == window.frame.size else {
            cap.trail.removeAll()
            return
        }

        let front = previous.prefix { $0.id != grabbedID }
        let isVisible: (SIMD2<Float>) -> Bool = { point in
            let location = CGPoint(x: CGFloat(point.x), y: CGFloat(point.y))
            return !front.contains { $0.frame.contains(location) }
        }

        let shiftX = Float(window.frame.minX - old.minX)
        if abs(shiftX) >= 1 {
            let firstNew = debris.count
            cap.shedTrailingEdge(
                windowOrigin: SIMD2(Float(old.minX), Float(old.minY)), fromLeft: shiftX > 0, distance: abs(shiftX),
                into: &debris, limit: Self.maxDebris, using: &generator, isVisible: isVisible)
            markSource(grabbedID, from: firstNew)
            cap.activeUntil = now + 3
        }

        // Скорость за последние 100 мс: один догоняющий шаг после паузы её не исказит.
        let origin = SIMD2(Float(window.frame.minX), Float(window.frame.minY))
        cap.trail.append((origin, snapshot.timestamp))
        cap.trail.removeAll { snapshot.timestamp - $0.time > 0.11 }
        guard let oldest = cap.trail.first else { return }
        let span = Float(snapshot.timestamp - oldest.time)
        guard span >= 0.06 else { return }

        cap.velocity = (origin - oldest.origin) / span
        let speed = simd_length(cap.velocity)
        guard speed >= Self.crumbleSpeed else { return }

        if speed >= Self.flingSpeed {
            Log.shared.write(String(format: "fling: %.0f pt/s", speed))
            shedSnow(of: previous, only: [grabbedID], direction: 0, fling: cap.velocity)
        } else {
            let firstNew = debris.count
            defer { markSource(grabbedID, from: firstNew) }
            cap.crumble(
                windowOrigin: SIMD2(Float(old.minX), Float(old.minY)),
                intensity: (speed - Self.crumbleSpeed) / (Self.flingSpeed - Self.crumbleSpeed),
                into: &debris, limit: Self.maxDebris, using: &generator, isVisible: isVisible)
            cap.activeUntil = now + 3
        }
    }

    private func shedSnow(
        of list: [TrackedWindow], only ids: Set<CGWindowID>?, direction: Float, fling: SIMD2<Float>? = nil
    ) {
        for (index, window) in list.enumerated() where window.holdsSnow && ids?.contains(window.id) != false {
            guard let cap = caps[window.id] else { continue }
            let firstNew = debris.count
            defer { markSource(window.id, from: firstNew) }
            cap.shed(
                windowOrigin: SIMD2(Float(window.frame.minX), Float(window.frame.minY)),
                direction: direction, fling: fling, into: &debris, limit: Self.maxDebris, using: &generator
            ) { point in
                let location = CGPoint(x: CGFloat(point.x), y: CGFloat(point.y))
                return !list[..<index].contains { $0.frame.contains(location) }
            }
        }
    }

    /// Система сменила Space: на новом снег копится заново.
    func spaceDidChange(now: CFTimeInterval) {
        // Окна «на всех рабочих столах» никуда не уехали и снег сохраняют.
        var pinned = now < slidingUntil ? pinnedDuringSlide : []
        pinned.insert(DockTracker.windowID)
        // Здесь снег не сыплем: окна уже другие, и частицы упали бы поверх нового стола.
        for (id, cap) in caps where !pinned.contains(id) { cap.clear() }
        pendingClosed.removeAll()
        pendingSlide = nil
        slidingUntil = now + Self.slideHold
    }

    private func markSource(_ id: CGWindowID, from start: Int) {
        for index in start..<debris.count { debris[index].source = id }
    }

    func setCornerRadius(_ radius: Float, for id: CGWindowID) {
        caps[id]?.setCornerRadius(radius)
    }

    /// Вызывается каждый кадр. Медленное движение по снегу скатывает снежок,
    /// обычное стряхивает снег, резкий взмах ничего не задевает.
    func brush(cursor: CGPoint, now: CFTimeInterval) {
        guard now > lastBrushTime else { return }
        let interval = Float(min(now - lastBrushTime, 1.0 / 15))
        lastBrushTime = now
        defer { lastBrushCursor = cursor }

        stepFallingBall(dt: interval, now: now)
        guard cursorEffectsEnabled else {
            releaseBall()
            return
        }
        guard let last = lastBrushCursor else { return }
        guard !wasMouseDown, now >= slidingUntil else {
            releaseBall()
            dwellTime = 0
            dwellPath = 0
            return
        }

        let delta = SIMD2(Float(cursor.x - last.x), Float(cursor.y - last.y))
        let distance = simd_length(delta)
        var velocity = delta / max(interval, 0.004)
        let speed = simd_length(velocity)

        if ball?.windowID != nil {
            rollBall(cursor: cursor, dt: interval, now: now)
            return
        }
        if ball == nil, startBallIfDwelling(cursor: cursor, speed: speed, distance: distance, dt: interval) { return }

        guard distance >= 1, distance < 600, speed >= Self.minBrushSpeed, speed <= Self.maxBrushSpeed else { return }
        if speed > 900 { velocity *= 900 / speed }

        let steps = min(30, Int(distance / 5) + 1)
        for step in 1...steps {
            let t = Float(step) / Float(steps)
            knockSnow(atX: Float(last.x) + delta.x * t, y: Float(last.y) + delta.y * t, velocity: velocity, now: now)
        }
    }

    /// Окно с видимым снегом, в слое которого находится точка.
    private func snowSurface(atX x: Float, y: Float) -> (window: TrackedWindow, cap: SnowCap, height: Float)? {
        let point = CGPoint(x: CGFloat(x), y: CGFloat(y))
        for (index, window) in windows.enumerated() where window.holdsSnow {
            let minX = Float(window.frame.minX), minY = Float(window.frame.minY)
            guard x > minX, x < Float(window.frame.maxX), let cap = caps[window.id] else { continue }
            let height = cap.height(atLocal: x - minX)
            guard height > 0.5, y >= minY - height - 3, y <= minY + 1,
                  !windows[..<index].contains(where: { $0.frame.contains(point) }) else { continue }
            return (window, cap, height)
        }
        return nil
    }

    private func startBallIfDwelling(cursor: CGPoint, speed: Float, distance: Float, dt: Float) -> Bool {
        guard speed < Self.minBrushSpeed,
              let surface = snowSurface(atX: Float(cursor.x), y: Float(cursor.y)) else {
            dwellTime = 0
            dwellPath = 0
            return false
        }
        dwellTime += dt
        dwellPath += distance
        guard dwellTime >= 0.2, dwellPath >= 8 else { return false }

        dwellTime = 0
        dwellPath = 0
        let radius: Float = 6
        ball = SnowBall(
            position: SIMD2(Float(cursor.x), Float(surface.window.frame.minY) - surface.height - radius * 0.8),
            velocity: .zero, radius: radius, rotation: 0, windowID: surface.window.id)

        // Шар появляется не из воздуха: снег под ним вздымается облачком.
        let firstNew = debris.count
        let puffCenter = SIMD2(Float(cursor.x), Float(surface.window.frame.minY) - surface.height * 0.5)
        for _ in 0..<26 where debris.count < Self.maxDebris {
            debris.append(SnowDebris(
                position: puffCenter + SIMD2(
                    Float.random(in: -9...9, using: &generator),
                    Float.random(in: -4...3, using: &generator)),
                velocity: SIMD2(
                    Float.random(in: -95...95, using: &generator),
                    Float.random(in: -190 ... -40, using: &generator)),
                radius: Float.random(in: 1.2...3.2, using: &generator),
                alpha: Float.random(in: 0.7...1, using: &generator),
                life: Float.random(in: 0.35...0.7, using: &generator),
                gravity: 900))
        }
        markSource(surface.window.id, from: firstNew)
        return true
    }

    private func rollBall(cursor: CGPoint, dt: Float, now: CFTimeInterval) {
        guard var current = ball, let id = current.windowID,
              let index = windows.firstIndex(where: { $0.id == id }), let cap = caps[id] else {
            releaseBall()
            return
        }
        let window = windows[index]
        let minX = Float(window.frame.minX), minY = Float(window.frame.minY)

        let previousX = current.position.x
        current.position.x += (Float(cursor.x) - previousX) * min(1, dt * 18)
        let moved = current.position.x - previousX
        current.rotation += moved / current.radius
        current.velocity = SIMD2(max(-400, min(400, moved / max(dt, 0.004))), 0)

        let u = current.position.x - minX
        if u < 0 || u > Float(window.frame.width) {
            ball = current
            releaseBall()
            return
        }

        if abs(moved) > 0.05 {
            let taken = cap.carve(atLocal: u, halfWidth: current.radius * 0.8, strength: min(1, abs(moved) / 5))
            current.radius = min(Self.maxBallRadius, (current.radius * current.radius + taken * 0.25 / .pi).squareRoot())
            cap.activeUntil = now + 3
        }

        // Из-под катящегося шара летит немного снега: видно, что он идёт по сугробу.
        let groundHeight = cap.height(atLocal: u)
        // Снег проверяем перед шаром: под собой он его уже собрал.
        let snowAhead = cap.height(atLocal: u + (moved > 0 ? 1 : -1) * current.radius * 1.1)
        if abs(moved) > 0.05, snowAhead > 0.5 {
            let share = abs(moved) / 4 * (current.radius / 8)
            let count = Int(share) + (Float.random(in: 0...1, using: &generator) < share - Float(Int(share)) ? 1 : 0)
            let behind: Float = moved > 0 ? -1 : 1
            let firstNew = debris.count
            for _ in 0..<count where debris.count < Self.maxDebris {
                debris.append(SnowDebris(
                    position: SIMD2(
                        current.position.x + behind * current.radius * Float.random(in: 0.2...0.8, using: &generator),
                        minY - snowAhead * Float.random(in: 0.2...1, using: &generator)),
                    velocity: SIMD2(
                        behind * Float.random(in: 15...90, using: &generator),
                        Float.random(in: -130 ... -30, using: &generator)),
                    radius: Float.random(in: 1.2...3.2, using: &generator),
                    alpha: Float.random(in: 0.7...1, using: &generator),
                    life: Float.random(in: 0.35...0.7, using: &generator)))
            }
            markSource(id, from: firstNew)
        }

        let targetY = minY - groundHeight - current.radius * 0.8
        current.position.y += (targetY - current.position.y) * min(1, dt * 20)
        ball = current

        let center = CGPoint(x: CGFloat(current.position.x), y: CGFloat(current.position.y))
        let isCovered = windows[..<index].contains { $0.frame.contains(center) }
        let reach = current.radius * 2 + 30
        if isCovered || Float(cursor.y) < minY - reach || Float(cursor.y) > minY + 30 {
            releaseBall()
        }
    }

    private func releaseBall() {
        guard var current = ball, let id = current.windowID else { return }
        current.droppedFrom = id
        current.windowID = nil
        ball = current
    }

    private func stepFallingBall(dt: Float, now: CFTimeInterval) {
        guard var current = ball, current.windowID == nil else { return }
        let previousBottom = current.position.y + current.radius
        current.velocity.y += 1300 * dt
        current.position += current.velocity * dt
        current.rotation += current.velocity.x * dt / current.radius

        let bottom = current.position.y + current.radius
        let screenBottom = Float(CGDisplayBounds(CGMainDisplayID()).maxY)
        let landed = tryLand(
            x: current.position.x, fromY: previousBottom, toY: bottom,
            amount: min(8, current.radius * 0.4), now: now, chance: 1, excluding: current.droppedFrom, wholeEdge: true)
        if landed {
            shatter(current)
            ball = nil
        } else if bottom - current.radius * 2 > screenBottom {
            ball = nil
        } else {
            ball = current
        }
    }

    private func shatter(_ snowball: SnowBall) {
        let count = min(220, Int(snowball.radius * snowball.radius * 0.35))
        for _ in 0..<count where debris.count < Self.maxDebris {
            let angle = Float.random(in: 0...(2 * .pi), using: &generator)
            let direction = SIMD2(cos(angle), sin(angle))
            debris.append(SnowDebris(
                position: snowball.position + direction * Float.random(in: 0...snowball.radius, using: &generator),
                velocity: direction * Float.random(in: 60...220, using: &generator)
                    - SIMD2(0, Float.random(in: 40...200, using: &generator)),
                radius: Float.random(in: 1.6...4.2, using: &generator),
                alpha: Float.random(in: 0.75...1, using: &generator),
                life: Float.random(in: 0.7...1.3, using: &generator),
                gravity: 1300))
        }
    }

    /// Снежок для отрисовки в локальных точках экрана: x, y центра и радиус.
    func ballForDrawing(origin: SIMD2<Float>) -> BallDraw? {
        guard let ball else { return nil }
        return BallDraw(
            center: ball.position - origin, radius: ball.radius, rotation: ball.rotation,
            ownerID: ball.windowID ?? ball.droppedFrom)
    }

    private func knockSnow(atX x: Float, y: Float, velocity: SIMD2<Float>, now: CFTimeInterval) {
        let point = CGPoint(x: CGFloat(x), y: CGFloat(y))
        for (index, window) in windows.enumerated() where window.holdsSnow {
            let minX = Float(window.frame.minX), minY = Float(window.frame.minY)
            guard x > minX, x < Float(window.frame.maxX), let cap = caps[window.id] else { continue }
            let u = x - minX
            let height = cap.height(atLocal: u)
            guard height > 0.5, y >= minY - height - 3, y <= minY + 1,
                  !windows[..<index].contains(where: { $0.frame.contains(point) }) else { continue }

            let firstNew = debris.count
            cap.brush(
                atLocal: u, windowOrigin: SIMD2(minX, minY), velocity: velocity,
                into: &debris, limit: Self.maxDebris, using: &generator)
            markSource(window.id, from: firstNew)
            cap.activeUntil = now + 3
            return
        }
    }

    /// Рамка Dock, если он сейчас известен.
    var dockFrame: CGRect? { windows.first(where: \.isDock)?.frame }

    /// Облачко снега в координатах CoreGraphics: от полоза саней и из открытого подарка.
    func spawnPuff(
        at center: SIMD2<Float>, count: Int, speed: ClosedRange<Float>, lift: ClosedRange<Float>, life: ClosedRange<Float>
    ) {
        for _ in 0..<count where debris.count < Self.maxDebris {
            let side: Float = Bool.random(using: &generator) ? 1 : -1
            debris.append(SnowDebris(
                position: center + SIMD2(
                    Float.random(in: -14...14, using: &generator),
                    Float.random(in: -8...8, using: &generator)),
                velocity: SIMD2(
                    side * Float.random(in: speed, using: &generator),
                    -Float.random(in: lift, using: &generator)),
                radius: Float.random(in: 1.4...3.6, using: &generator),
                alpha: Float.random(in: 0.7...1, using: &generator),
                life: Float.random(in: life, using: &generator),
                gravity: 900))
        }
    }

    func stepDebris(now: CFTimeInterval) {
        defer { lastDebrisStep = now }
        guard now > lastDebrisStep, lastDebrisStep > 0, !debris.isEmpty else { return }
        let dt = Float(min(now - lastDebrisStep, 1.0 / 15))
        for index in debris.indices {
            debris[index].velocity.y += debris[index].gravity * dt
            debris[index].velocity.x *= max(0, 1 - 1.2 * dt)
            let previousY = debris[index].position.y
            debris[index].position += debris[index].velocity * dt
            debris[index].life -= dt

            // Снег пересыпается на окна ниже и пополняет их сугроб.
            let particle = debris[index]
            if particle.velocity.y > 0, tryLand(
                x: particle.position.x, fromY: previousY, toY: particle.position.y,
                amount: 0.6, now: now, chance: 1, excluding: particle.source, wholeEdge: true) {
                debris[index].life = 0
            }
        }
        debris.removeAll { $0.life <= 0 }
    }

    /// Частицы этого экрана добавляются к хлопьям. Возвращает их количество.
    func writeDebris(
        to pointer: UnsafeMutablePointer<FlakeInstance>, origin: SIMD2<Float>, viewport: SIMD2<Float>
    ) -> Int {
        var written = 0
        for particle in debris {
            let local = particle.position - origin
            guard local.x > -8, local.x < viewport.x + 8, local.y > -8, local.y < viewport.y + 8 else { continue }
            pointer[written] = FlakeInstance(
                rect: SIMD4(local.x - particle.radius, local.y - particle.radius, particle.radius * 2, particle.radius * 2),
                alpha: particle.alpha * min(1, particle.life / 0.35),
                softness: 0.7,
                ring: 0)
            written += 1
        }
        return written
    }

    /// Окна на других Space пропадают из списка, поэтому снег хранится до вытеснения.
    private func evictIfNeeded() {
        guard caps.count > Self.maxCachedCaps else { return }
        let oldest = caps.sorted { $0.value.lastSeen < $1.value.lastSeen }
        for (id, _) in oldest.prefix(caps.count - Self.maxCachedCaps) {
            caps[id] = nil
        }
    }

    func relax(now: CFTimeInterval) {
        guard now - lastRelax >= 1.0 / 30 else { return }
        lastRelax = now
        for cap in caps.values where cap.activeUntil > now {
            cap.relax()
        }
    }

    /// Проверяет, упал ли хлопья на видимый снег или край окна на этом шаге.
    func tryLand(
        x: Float, fromY: Float, toY: Float, amount: Float, now: CFTimeInterval,
        chance: Float = landingChance, excluding: CGWindowID = 0, wholeEdge: Bool = false
    ) -> Bool {
        guard now >= slidingUntil else { return false }
        for (index, window) in windows.enumerated() where window.holdsSnow && window.id != excluding {
            let minX = Float(window.frame.minX)
            guard x > minX, x < Float(window.frame.maxX), let cap = caps[window.id] else { continue }
            if window.isDock, window.frame.minY >= CGDisplayBounds(CGMainDisplayID()).maxY - 1 { continue }
            let u = x - minX
            guard wholeEdge || cap.canHoldSnow(atLocal: u) else { continue }
            let surface = Float(window.frame.minY) + cap.baseDrop(atLocal: u) - cap.height(atLocal: u)
            guard fromY <= surface, toY >= surface else { continue }

            let point = CGPoint(x: CGFloat(x), y: CGFloat(surface))
            guard !windows[..<index].contains(where: { $0.frame.contains(point) }) else { continue }

            // Часть хлопьев пролетает дальше, иначе окна под верхним окном остались бы без снега.
            guard Float.random(in: 0...1, using: &generator) < chance else { continue }

            cap.deposit(atLocal: u, amount: amount, maxHeight: Self.maxHeight)
            cap.activeUntil = now + 3
            return true
        }
        return false
    }

    /// Строит сетки шапок для одного экрана от задних окон к передним.
    func buildMesh(
        into vertices: UnsafeMutablePointer<CapVertex>, capacity: Int,
        origin: SIMD2<Float>, viewport: SIMD2<Float>, draws: inout [CapDraw]
    ) {
        draws.removeAll(keepingCapacity: true)
        var used = 0
        for window in windows.reversed() {
            let x = Float(window.frame.minX) - origin.x
            var y = Float(window.frame.minY) - origin.y
            // Шапка скрытого Dock уходит за нижний край вместе с ним и не оставляет полосу.
            if window.isDock { y += max(0, y - (viewport.y - Self.maxHeight - 2)) }
            let width = Float(window.frame.width)
            let height = Float(window.frame.height)
            let isVisible = x < viewport.x && x + width > 0
                && y - Self.maxHeight - 4 < viewport.y && y + height > 0
            guard isVisible else { continue }

            var draw = CapDraw(rect: SIMD4(x, y, width, height), vertexStart: used, vertexCount: 0, windowID: window.id,
                cornerRadius: caps[window.id]?.cornerRadius ?? 20)
            if window.holdsSnow, let cap = caps[window.id], used + cap.count * 2 <= capacity {
                for index in 0..<cap.count {
                    let u = min(Float(index) * SnowCap.step, width)
                    let h = cap.surfaceHeight(at: index)
                    let base = y + cap.baseDrop(atLocal: u)
                    vertices[used] = CapVertex(position: SIMD4(x + u, base - h - 1, -1, u), thickness: h)
                    vertices[used + 1] = CapVertex(position: SIMD4(x + u, base + 1, h + 1, u), thickness: h)
                    used += 2
                }
                draw.vertexCount = cap.count * 2
            }
            draws.append(draw)
        }
    }
}
