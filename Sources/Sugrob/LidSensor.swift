import Foundation
import IOKit.hid
import QuartzCore

/// Что сейчас делает крышка.
struct LidState {
    var angle: Float = 0
    /// Градусы в секунду; плюс — крышку открывают.
    var speed: Float = 0
    /// Сила встряхивания: 0 — нет, 1 — снег срывается целиком.
    var shake: Float = 0
    /// Наклон вперёд: 0 — снег лежит, 1 — ссыпается быстрее всего.
    var slip: Float = 0
    /// Сколько секунд прошло с прошлого замера.
    var dt: Float = 0
}

/// Из замеров угла получает скорость, встряхивание и наклон.
struct LidMotion {
    /// Пороги скорости, °/с: начало осыпания и полный срыв шапок.
    private static let shakeStart: Float = 70
    private static let shakeFull: Float = 260
    /// Угол, °: с какого снег начинает ползти и при каком сыплется быстрее всего.
    private static let slipStart: Float = 85
    private static let slipFull: Float = 45
    /// Ниже этого крышка считается закрытой: экран погас, а внешний монитор не должен терять снег.
    private static let closedAngle: Float = 20
    private static let speedWindow: CFTimeInterval = 0.14
    private static let maxGap: CFTimeInterval = 0.3

    private var samples: [(angle: Float, time: CFTimeInterval)] = []

    mutating func add(angle: Float, time: CFTimeInterval) -> LidState {
        let previous = samples.last
        // После сна или паузы старые замеры дали бы ложный рывок.
        if let previous, time - previous.time > Self.maxGap { samples.removeAll() }
        samples.append((angle, time))
        samples.removeAll { time - $0.time > Self.speedWindow }

        var state = LidState(angle: angle)
        if let previous, time - previous.time <= Self.maxGap { state.dt = Float(time - previous.time) }
        if let oldest = samples.first, time - oldest.time >= 0.06 {
            state.speed = (angle - oldest.angle) / Float(time - oldest.time)
        }
        state.shake = max(0, min(1, (abs(state.speed) - Self.shakeStart) / (Self.shakeFull - Self.shakeStart)))
        if angle >= Self.closedAngle {
            state.slip = max(0, min(1, (Self.slipStart - angle) / (Self.slipStart - Self.slipFull)))
        }
        return state
    }
}

/// Датчик угла крышки: недокументированное устройство HID, есть не на всех Mac.
@MainActor
final class LidSensor {
    private static let pollInterval: CFTimeInterval = 1.0 / 30
    private static let maxFailures = 10
    private static let maxKick: Float = 450

    private let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
    private var device: IOHIDDevice?
    private var motion = LidMotion()
    private var lastPoll: CFTimeInterval = 0
    private var lastTick: CFTimeInterval = 0
    private var failures = 0
    private var readCount = 0
    private var readTime: CFTimeInterval = 0
    private var gesturePeak: Float = 0
    private var gestureStart: Float = 0
    private var wasSlipping = false
    private var holdUntil: CFTimeInterval = 0
    private var kickTarget: Float = 0

    var isEnabled = true
    var isAvailable: Bool { device != nil }
    /// Сдвиг хлопьев в воздухе, pt/s.
    private(set) var flakeKick: Float = 0

    init() {
        IOHIDManagerSetDeviceMatching(manager, [
            kIOHIDVendorIDKey: 0x05AC, kIOHIDProductIDKey: 0x8104,
            kIOHIDPrimaryUsagePageKey: 0x20, kIOHIDPrimaryUsageKey: 0x8A,
        ] as CFDictionary)
        let openResult = IOHIDManagerOpen(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        let found = IOHIDManagerCopyDevices(manager) as? Set<IOHIDDevice> ?? []
        for candidate in found where IOHIDDeviceOpen(candidate, IOOptionBits(kIOHIDOptionsTypeNone)) == kIOReturnSuccess {
            device = candidate
            if let angle = read() {
                Log.shared.write("lid: датчик есть, угол \(Int(angle))°")
                return
            }
            device = nil
        }
        Log.shared.write("lid: датчика нет (open=\(openResult), устройств \(found.count))")
    }

    /// После пробуждения крышку ещё открывают: это движение не должно сбить свежий снег.
    func hold(until time: CFTimeInterval) {
        holdUntil = time
    }

    /// Вызывается каждый кадр. Возвращает состояние, только когда есть новый замер.
    func tick(now: CFTimeInterval) -> LidState? {
        let frame = Float(min(now - lastTick, 1.0 / 15))
        lastTick = now
        guard isEnabled, isAvailable, now >= holdUntil else {
            flakeKick = 0
            return LidState()
        }
        var state: LidState?
        if now - lastPoll >= Self.pollInterval {
            lastPoll = now
            let started = CACurrentMediaTime()
            if let angle = read() {
                failures = 0
                state = motion.add(angle: angle, time: now)
                measure(since: started)
            } else {
                failures += 1
                if failures >= Self.maxFailures {
                    Log.shared.write("lid: датчик перестал отвечать, отключаю")
                    device = nil
                }
            }
        }
        if let state {
            report(state)
            kickTarget = state.shake > 0 ? max(-Self.maxKick, min(Self.maxKick, state.speed * 1.4)) : 0
        }
        flakeKick += (kickTarget - flakeKick) * min(1, frame * 18)
        return state
    }

    private func read() -> Float? {
        guard let device else { return nil }
        var buffer = [UInt8](repeating: 0, count: 8)
        var length = CFIndex(buffer.count)
        let result = IOHIDDeviceGetReport(device, kIOHIDReportTypeFeature, 1, &buffer, &length)
        guard result == kIOReturnSuccess, length >= 3 else { return nil }
        return Float(UInt16(buffer[1]) | UInt16(buffer[2]) << 8)
    }

    private func measure(since started: CFTimeInterval) {
        readTime += CACurrentMediaTime() - started
        readCount += 1
        if readCount == 300 {
            Log.shared.write(String(format: "lid: чтение датчика %.2f мс", readTime / 300 * 1000))
        }
    }

    /// Для подбора порогов: в логе остаётся пик скорости каждого движения.
    private func report(_ state: LidState) {
        if state.shake > 0 {
            if gesturePeak == 0 { gestureStart = state.angle }
            gesturePeak = max(gesturePeak, abs(state.speed))
        } else if gesturePeak > 0 {
            Log.shared.write(String(format: "lid: рывок %.0f°/с, угол %.0f → %.0f", gesturePeak, gestureStart, state.angle))
            gesturePeak = 0
        }
        let isSlipping = state.slip > 0
        if isSlipping != wasSlipping {
            wasSlipping = isSlipping
            Log.shared.write(String(format: "lid: наклон %@, угол %.0f", isSlipping ? "начался" : "закончился", state.angle))
        }
    }
}
