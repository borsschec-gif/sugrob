import AppKit
import IOKit.ps

/// Настройки из меню, сохраняются между запусками.
@MainActor
final class Settings {
    private let defaults = UserDefaults.standard

    init() {
        defaults.register(defaults: [
            "enabled": true, "intensity": 1.0, "cursorEffects": true, "santa": true,
            "pauseInEditors": true, "batterySaving": false, "lidEffects": true, "coverAfterSleep": true,
        ])
    }

    var coverAfterSleep: Bool {
        get { defaults.bool(forKey: "coverAfterSleep") }
        set { defaults.set(newValue, forKey: "coverAfterSleep") }
    }

    var lidEffects: Bool {
        get { defaults.bool(forKey: "lidEffects") }
        set { defaults.set(newValue, forKey: "lidEffects") }
    }

    var enabled: Bool {
        get { defaults.bool(forKey: "enabled") }
        set { defaults.set(newValue, forKey: "enabled") }
    }
    /// Густота снегопада: 0,5, 1 или 1,5.
    var intensity: Float {
        get { defaults.float(forKey: "intensity") }
        set { defaults.set(newValue, forKey: "intensity") }
    }
    var cursorEffects: Bool {
        get { defaults.bool(forKey: "cursorEffects") }
        set { defaults.set(newValue, forKey: "cursorEffects") }
    }
    var santa: Bool {
        get { defaults.bool(forKey: "santa") }
        set { defaults.set(newValue, forKey: "santa") }
    }
    var pauseInEditors: Bool {
        get { defaults.bool(forKey: "pauseInEditors") }
        set { defaults.set(newValue, forKey: "pauseInEditors") }
    }
    var batterySaving: Bool {
        get { defaults.bool(forKey: "batterySaving") }
        set { defaults.set(newValue, forKey: "batterySaving") }
    }
}

/// Решает, идёт ли снег сейчас, с какой частотой и густотой.
@MainActor
final class PowerGovernor {
    private static let editorPrefixes = [
        "com.blackmagic-design.DaVinciResolve", "com.adobe.PremierePro", "com.adobe.AfterEffects", "com.apple.FinalCut",
    ]

    private let settings: Settings
    private let tracker: WindowTracker
    private var timer: Timer?
    private var isScreenAsleep = false
    private var lastReason = ""

    private(set) var isPaused = false
    private(set) var fps: Float = Config.fps
    private(set) var density: Float = 1
    var onPauseChange: ((Bool) -> Void)?
    /// Экран проснулся; аргумент — сколько секунд он спал.
    var onScreensWoke: ((TimeInterval) -> Void)?
    private var sleptAt: Date?

    init(settings: Settings, tracker: WindowTracker) {
        self.settings = settings
        self.tracker = tracker
    }

    func start() {
        let center = NSWorkspace.shared.notificationCenter
        center.addObserver(self, selector: #selector(screensSlept), name: NSWorkspace.screensDidSleepNotification, object: nil)
        center.addObserver(self, selector: #selector(screensWoke), name: NSWorkspace.screensDidWakeNotification, object: nil)
        center.addObserver(self, selector: #selector(evaluateNow), name: NSWorkspace.didActivateApplicationNotification, object: nil)
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.evaluate() }
        }
        evaluate()
    }

    @objc private func screensSlept() {
        isScreenAsleep = true
        if sleptAt == nil { sleptAt = Date() }
        evaluate()
    }

    @objc private func screensWoke() {
        isScreenAsleep = false
        if let sleptAt { onScreensWoke?(Date().timeIntervalSince(sleptAt)) }
        sleptAt = nil
        evaluate()
    }

    @objc private func evaluateNow() {
        evaluate()
    }

    func evaluate() {
        let saving = settings.batterySaving && Self.isSavingPower
        fps = saving || Self.isIdle ? min(Config.fps, 30) : Config.fps
        density = settings.intensity * (saving ? 0.5 : 1)

        let reason = pauseReason()
        if reason != lastReason {
            lastReason = reason
            Log.shared.write("governor: \(reason.isEmpty ? "running" : "paused (\(reason))") fps=\(fps) density=\(density)")
        }
        let shouldPause = !reason.isEmpty
        if shouldPause != isPaused {
            isPaused = shouldPause
            onPauseChange?(shouldPause)
        }
    }

    private func pauseReason() -> String {
        if !settings.enabled { return "выключено" }
        if isScreenAsleep { return "сон экрана" }
        if settings.pauseInEditors, let bundle = NSWorkspace.shared.frontmostApplication?.bundleIdentifier,
           Self.editorPrefixes.contains(where: { bundle.hasPrefix($0) }) {
            return "монтаж"
        }
        if isFullScreenAppInFront { return "полный экран" }
        return ""
    }

    private var isFullScreenAppInFront: Bool {
        let main = CGDisplayBounds(CGMainDisplayID())
        guard let front = tracker.snapshot.windows.first(where: { $0.holdsSnow && !$0.isFixedSurface }) else { return false }
        return front.frame.contains(main)
    }

    /// Через минуту без мыши и клавиатуры снег идёт на 30 кадрах: каждый кадр стоит пересборки всего экрана.
    /// Первое же движение возвращает 60 не позже чем через секунду, а окна при движении и так получают 60.
    private static let idleAfter: CFTimeInterval = 60

    private static var isIdle: Bool {
        guard let anyInput = CGEventType(rawValue: ~0) else { return false }
        return CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: anyInput) > idleAfter
    }

    private static var isSavingPower: Bool {
        if ProcessInfo.processInfo.isLowPowerModeEnabled { return true }
        guard let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue(),
              let source = IOPSGetProvidingPowerSourceType(info)?.takeUnretainedValue() else { return false }
        return (source as String) == kIOPSBatteryPowerValue
    }
}
