import AppKit
import CoreGraphics
import QuartzCore

/// Координаты CoreGraphics: начало в левом верхнем углу основного экрана, y вниз.
struct TrackedWindow: Sendable {
    let id: CGWindowID
    let frame: CGRect
    let owner: String
    let hasName: Bool
    /// Обычное окно приложения. Окна других слоёв только перекрывают снег.
    let holdsSnow: Bool
    var isDock = false
    var layer = 0
}

struct WindowSnapshot: Sendable {
    var timestamp: CFTimeInterval = 0
    /// От переднего окна к заднему.
    var windows: [TrackedWindow] = []
    /// Окна сейчас двигаются: кадры нужны чаще.
    var isFast = false
    var isMouseDown = false
    var cursor = CGPoint.zero
}

/// Опрашивает WindowServer на своей очереди. Разрешений это не требует:
/// позиция, размер, слой и порядок окон доступны любому процессу.
final class WindowTracker: @unchecked Sendable {
    private let lock = NSLock()
    private var latest = WindowSnapshot()
    private let queue = DispatchQueue(label: "sugrob.tracker", qos: .userInteractive)
    private var timer: DispatchSourceTimer?
    private let ownPID = getpid()

    private static let idleHz = 5.0
    private static let fastHz = 60.0
    /// Сколько держать частый опрос после последнего движения окон.
    private static let activeHold = 0.4
    private var fixedHz: Double?
    private var isFast = false
    private var lastActivity: CFTimeInterval = 0

    private let dock = DockTracker()
    private var meter = DragMeter()
    private var pollCount = 0
    private var costSum = 0.0
    private var costMax = 0.0
    private var lastStatsAt = CACurrentMediaTime()

    var snapshot: WindowSnapshot {
        lock.lock(); defer { lock.unlock() }
        return latest
    }

    /// Без `fixedHz` опрос редкий в покое и частый, пока окна двигаются или зажата кнопка мыши.
    func start(fixedHz: Double?) {
        self.fixedHz = fixedHz
        Log.shared.write("tracker: \(fixedHz.map { "\($0) Hz" } ?? "adaptive")")
        let timer = DispatchSource.makeTimerSource(flags: .strict, queue: queue)
        schedule(timer, hz: fixedHz ?? Self.idleHz)
        timer.setEventHandler { [weak self] in self?.poll() }
        timer.resume()
        self.timer = timer
    }

    /// Нажатие кнопки мыши: переходим на частый опрос сразу, не дожидаясь редкого тика.
    func wake() {
        queue.async { [self] in
            lastActivity = CACurrentMediaTime()
            setFast(true)
            poll()
        }
    }

    private func schedule(_ timer: DispatchSourceTimer, hz: Double) {
        timer.schedule(deadline: .now() + 1 / hz, repeating: 1 / hz, leeway: .milliseconds(hz > 30 ? 1 : 20))
    }

    private func setFast(_ value: Bool) {
        guard value != isFast, fixedHz == nil, let timer else { return }
        isFast = value
        schedule(timer, hz: value ? Self.fastHz : Self.idleHz)
    }

    private func poll() {
        let started = CACurrentMediaTime()
        let raw = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] ?? []
        let cost = (CACurrentMediaTime() - started) * 1000

        let mainDisplay = CGDisplayBounds(CGMainDisplayID())
        var windows: [TrackedWindow] = []
        windows.reserveCapacity(raw.count)
        for info in raw {
            guard let layer = info[kCGWindowLayer as String] as? Int, layer >= 0,
                  (info[kCGWindowOwnerPID as String] as? pid_t) != ownPID,
                  ((info[kCGWindowAlpha as String] as? Double) ?? 1) > 0.01,
                  let number = info[kCGWindowNumber as String] as? CGWindowID,
                  let bounds = info[kCGWindowBounds as String] as? NSDictionary,
                  let frame = CGRect(dictionaryRepresentation: bounds),
                  frame.width >= 60, frame.height >= 60
            else { continue }
            let owner = info[kCGWindowOwnerName as String] as? String ?? "?"
            // Системные прозрачные окна на весь экран (Dock и подобные) не должны стирать снег.
            let isSystemSheet = layer != 0 && (
                owner == "Dock" || owner == "Window Server"
                    || frame.width * frame.height >= mainDisplay.width * mainDisplay.height * 0.8)
            guard !isSystemSheet else { continue }
            windows.append(TrackedWindow(
                id: number,
                frame: frame,
                owner: owner,
                hasName: info[kCGWindowName as String] != nil,
                holdsSnow: layer == 0,
                layer: layer))
        }

        // Dock лежит поверх обычных окон. Даже скрытый он остаётся в списке, за нижним краем экрана.
        var isNearDock = false
        if let frame = dock.frame(now: started) {
            windows.insert(
                TrackedWindow(id: DockTracker.windowID, frame: frame, owner: "Dock", hasName: false, holdsSnow: true, isDock: true, layer: 20),
                at: windows.firstIndex { $0.layer <= 20 } ?? windows.count)
            // Авто-скрытие длится 200 мс, редкий опрос заметно опоздал бы.
            if let cursor = CGEvent(source: nil)?.location {
                isNearDock = frame.insetBy(dx: -40, dy: -80).contains(cursor) || cursor.y >= mainDisplay.maxY - 8
            }
        }

        let isMouseDown = CGEventSource.buttonState(.combinedSessionState, button: .left)
        let snapshot = WindowSnapshot(
            timestamp: started, windows: windows, isFast: isFast || fixedHz != nil, isMouseDown: isMouseDown,
            cursor: CGEvent(source: nil)?.location ?? .zero)
        lock.lock()
        let previous = latest
        latest = snapshot
        lock.unlock()

        let moved = !previous.hasSameGeometry(as: snapshot)
        if moved || isMouseDown || (isNearDock && snapshot.cursor != previous.cursor) {
            lastActivity = started
        }
        setFast(started - lastActivity < Self.activeHold)

        meter.update(snapshot: snapshot)
        recordStats(cost: cost, windows: windows)
    }

    private func recordStats(cost: Double, windows: [TrackedWindow]) {
        pollCount += 1
        costSum += cost
        costMax = max(costMax, cost)
        let now = CACurrentMediaTime()
        guard now - lastStatsAt >= 10 else { return }
        let named = windows.filter(\.hasName).count
        Log.shared.write(String(
            format: "tracker: %.1f polls/s, cost avg %.2f ms, max %.2f ms, windows %d, withName %d",
            Double(pollCount) / (now - lastStatsAt), costSum / Double(pollCount), costMax, windows.count, named))
        pollCount = 0; costSum = 0; costMax = 0; lastStatsAt = now
    }
}

/// Сравнивает положение окна из списка WindowServer с реальным курсором во время перетаскивания.
/// Смещение захвата фиксируется в момент нажатия, пока окно ещё неподвижно.
private struct DragMeter {
    private struct Session {
        let id: CGWindowID
        let owner: String
        let grabOffset: CGPoint
        let startFrame: CGRect
        let startedAt: CFTimeInterval
        var lastOrigin: CGPoint
        var lastCursor: CGPoint
        var lastTime: CFTimeInterval
        var updates = 0
        var lags: [Double] = []
        var cursorPath = 0.0
        var sizeChanged = false
    }

    private var session: Session?
    private var wasDown = false

    mutating func update(snapshot: WindowSnapshot) {
        let down = CGEventSource.buttonState(.combinedSessionState, button: .left)
        defer { wasDown = down }
        guard let cursor = CGEvent(source: nil)?.location else { return }

        if down && !wasDown {
            guard let top = snapshot.windows.first(where: { $0.frame.contains(cursor) }) else { return }
            session = Session(
                id: top.id, owner: top.owner,
                grabOffset: CGPoint(x: cursor.x - top.frame.minX, y: cursor.y - top.frame.minY),
                startFrame: top.frame, startedAt: snapshot.timestamp,
                lastOrigin: top.frame.origin, lastCursor: cursor, lastTime: snapshot.timestamp)
            return
        }

        if !down {
            if wasDown, let finished = session { report(finished, endedAt: snapshot.timestamp) }
            session = nil
            return
        }

        guard var current = session,
              let window = snapshot.windows.first(where: { $0.id == current.id }) else { return }

        if window.frame.size != current.startFrame.size { current.sizeChanged = true }
        current.cursorPath += hypot(cursor.x - current.lastCursor.x, cursor.y - current.lastCursor.y)
        if window.frame.origin != current.lastOrigin { current.updates += 1 }

        let moved = window.frame.origin != current.startFrame.origin
        if moved && !current.sizeChanged {
            let dx = cursor.x - window.frame.minX - current.grabOffset.x
            let dy = cursor.y - window.frame.minY - current.grabOffset.y
            current.lags.append(hypot(dx, dy))
        }
        current.lastOrigin = window.frame.origin
        current.lastCursor = cursor
        current.lastTime = snapshot.timestamp
        session = current
    }

    private func report(_ s: Session, endedAt: CFTimeInterval) {
        guard s.updates > 0 else { return }
        let duration = max(endedAt - s.startedAt, 0.001)
        let sorted = s.lags.sorted()
        let average = sorted.isEmpty ? 0 : sorted.reduce(0, +) / Double(sorted.count)
        let p95 = sorted.isEmpty ? 0 : sorted[min(sorted.count - 1, Int(Double(sorted.count) * 0.95))]
        let speed = s.cursorPath / duration
        let lagMs = speed > 1 ? average / speed * 1000 : 0
        Log.shared.write(String(
            format: "drag[%@] %@: %.2f s, %d updates (%.1f/s), cursor %.0f pt/s, lag avg %.1f pt, p95 %.1f pt, max %.1f pt, ~%.0f ms",
            s.sizeChanged ? "resize" : "move", s.owner, duration, s.updates, Double(s.updates) / duration,
            speed, average, p95, sorted.last ?? 0, lagMs))
    }
}

extension WindowSnapshot {
    func hasSameGeometry(as other: WindowSnapshot) -> Bool {
        windows.count == other.windows.count
            && zip(windows, other.windows).allSatisfy { $0.id == $1.id && $0.frame == $1.frame }
    }

    /// Участки верхнего края окна, не закрытые окнами перед ним. Понадобится для шапок снега.
    func visibleTopSegments(at index: Int) -> [ClosedRange<CGFloat>] {
        let frame = windows[index].frame
        var segments: [ClosedRange<CGFloat>] = [frame.minX...frame.maxX]
        let edgeY = frame.minY
        for front in windows[..<index] where front.frame.minY <= edgeY && edgeY < front.frame.maxY {
            segments = segments.flatMap { segment -> [ClosedRange<CGFloat>] in
                var pieces: [ClosedRange<CGFloat>] = []
                if segment.lowerBound < front.frame.minX {
                    pieces.append(segment.lowerBound...min(segment.upperBound, front.frame.minX))
                }
                if segment.upperBound > front.frame.maxX {
                    pieces.append(max(segment.lowerBound, front.frame.maxX)...segment.upperBound)
                }
                return pieces
            }
        }
        return segments
    }
}
