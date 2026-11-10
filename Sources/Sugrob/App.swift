import AppKit
import ServiceManagement

@main
@MainActor
struct SugrobMain {
    static func main() {
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.accessory)
        withExtendedLifetime(delegate) { app.run() }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let tracker = WindowTracker()
    private let caps = CapField(seed: 2026)
    private lazy var santa = SantaController(caps: caps)
    private let settings = Settings()
    private lazy var cornerProber = CornerProber(caps: caps)
    private let updater = Updater()
    private lazy var lid = LidSensor()
    private var cornerTimer: Timer?
    private var permissionState: [Bool] = []
    private lazy var governor = PowerGovernor(settings: settings, tracker: tracker)
    private var renderer: Renderer!
    private var overlays: [OverlayWindow] = []
    private var statusItem: NSStatusItem!
    private var statsTimer: Timer?
    private var mouseMonitor: Any?

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let renderer = Renderer() else {
            Log.shared.write("FATAL: Metal недоступен")
            NSApp.terminate(nil)
            return
        }
        self.renderer = renderer

        if let path = Config.snapshotPath {
            let written = renderer.writeSnapshot(to: path, width: 1512, height: 982, scale: 2)
            print(written ? "snapshot: \(path)" : "snapshot failed")
            NSApp.terminate(nil)
            return
        }

        Log.shared.write("start pid=\(getpid()) fps=\(Config.fps) AXTrusted=\(AXIsProcessTrusted()) screenCapture=\(CGPreflightScreenCaptureAccess())")

        // Accessibility нужен только для геометрии Dock, снег на окнах работает и без него.
        _ = AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary)
        setUpStatusItem()
        rebuildOverlays()
        NotificationCenter.default.addObserver(
            self, selector: #selector(screensChanged),
            name: NSApplication.didChangeScreenParametersNotification, object: nil)

        NSWorkspace.shared.notificationCenter.addObserver(
            self, selector: #selector(activeSpaceChanged),
            name: NSWorkspace.activeSpaceDidChangeNotification, object: nil)

        tracker.start(fixedHz: Config.fixedPollHz)
        governor.onPauseChange = { [weak self] paused in
            guard let self else { return }
            self.overlays.forEach { $0.setPaused(paused) }
            self.tracker.setPaused(paused)
            if paused { self.santa.removeGiftIfAny() }
        }
        caps.cursorEffectsEnabled = settings.cursorEffects
        santa.autoRides = settings.santa
        lid.isEnabled = settings.lidEffects
        governor.onScreensWoke = { [weak self] seconds in
            guard let self else { return }
            self.lid.hold(until: CACurrentMediaTime() + 3)
            // Без курсора счистить снег было бы нечем.
            guard self.settings.coverAfterSleep, self.settings.cursorEffects, seconds >= Config.sheetMinSleep else { return }
            Log.shared.write(String(format: "sheet: спал %.0f с, экран занесён", seconds))
            self.overlays.forEach { $0.overlayView.coverWithSnow() }
        }
        governor.start()
        updater.start()
        if !CGPreflightScreenCaptureAccess() { CGRequestScreenCaptureAccess() }
        cornerTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.refreshPermissions()
                guard !self.governor.isPaused else { return }
                self.cornerProber.update(windows: self.tracker.snapshot.windows)
            }
        }
        mouseMonitor = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDown) { [tracker] _ in
            tracker.wake()
        }

        statsTimer = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.logRenderStats() }
        }
    }

    private func setUpStatusItem() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem.button?.title = "❄︎"
        rebuildMenu()
    }

    private func rebuildMenu() {
        let menu = NSMenu()
        menu.autoenablesItems = false

        func toggle(_ title: String, _ isOn: Bool, _ action: Selector) {
            let item = menu.addItem(withTitle: title, action: action, keyEquivalent: "")
            item.target = self
            item.state = isOn ? .on : .off
        }

        let hasAccessibility = AXIsProcessTrusted()
        let hasScreenCapture = CGPreflightScreenCaptureAccess()
        permissionState = [hasAccessibility, hasScreenCapture]
        statusItem.button?.title = hasAccessibility && hasScreenCapture ? "❄︎" : "❄︎ !"
        if !hasAccessibility {
            menu.addItem(withTitle: "Разрешить: снег на Dock и папках…", action: #selector(openAccessibilitySettings), keyEquivalent: "").target = self
        }
        if !hasScreenCapture {
            menu.addItem(withTitle: "Разрешить: точные углы окон…", action: #selector(openScreenCaptureSettings), keyEquivalent: "").target = self
        }
        if !hasAccessibility || !hasScreenCapture {
            menu.addItem(withTitle: "Перезапустить Сугроб", action: #selector(relaunch), keyEquivalent: "").target = self
            menu.addItem(.separator())
        }

        toggle("Снег", settings.enabled, #selector(toggleEnabled))

        let intensityMenu = NSMenu()
        for (title, value) in [("Редкий", 0.5), ("Обычный", 1.0), ("Густой", 1.5)] as [(String, Float)] {
            let item = intensityMenu.addItem(withTitle: title, action: #selector(chooseIntensity(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = value
            item.state = settings.intensity == value ? .on : .off
        }
        menu.addItem(withTitle: "Густота", action: nil, keyEquivalent: "").submenu = intensityMenu

        menu.addItem(.separator())
        toggle("Стряхивание и снежок", settings.cursorEffects, #selector(toggleCursor))
        if lid.isAvailable { toggle("Реакция на крышку", settings.lidEffects, #selector(toggleLid)) }
        toggle("Заносить экран после сна", settings.coverAfterSleep, #selector(toggleCover))
        toggle("Дед Мороз", settings.santa, #selector(toggleSanta))
        menu.addItem(withTitle: "Позвать Деда Мороза", action: #selector(callSanta), keyEquivalent: "").target = self

        menu.addItem(.separator())
        toggle("Пауза в монтаже", settings.pauseInEditors, #selector(togglePauseInEditors))
        toggle("Экономить на батарее", settings.batterySaving, #selector(toggleBattery))
        toggle("Запускать при входе", SMAppService.mainApp.status == .enabled, #selector(toggleLogin))

        menu.addItem(.separator())
        let versionItem = menu.addItem(withTitle: "Версия \(Updater.currentVersion)", action: nil, keyEquivalent: "")
        versionItem.isEnabled = false
        menu.addItem(withTitle: "Проверить обновления", action: #selector(checkUpdates), keyEquivalent: "").target = self
        menu.addItem(.separator())
        menu.addItem(withTitle: "Выйти", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        statusItem.menu = menu
    }

    @objc private func openAccessibilitySettings() {
        openPrivacyPane("Privacy_Accessibility")
    }

    @objc private func openScreenCaptureSettings() {
        openPrivacyPane("Privacy_ScreenCapture")
    }

    private func openPrivacyPane(_ pane: String) {
        guard let url = URL(string: "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?\(pane)") else { return }
        NSWorkspace.shared.open(url)
    }

    /// Разрешение на запись экрана начинает действовать только после перезапуска.
    @objc private func relaunch() {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/bash")
        process.arguments = ["-c", "sleep 0.7; open \"$0\"", Bundle.main.bundlePath]
        guard (try? process.run()) != nil else { return }
        NSApp.terminate(nil)
    }

    /// Разрешения выдаются в Настройках: меню следит за ними само.
    private func refreshPermissions() {
        let current = [AXIsProcessTrusted(), CGPreflightScreenCaptureAccess()]
        guard current != permissionState else { return }
        let gotScreenCapture = permissionState.count == 2 && !permissionState[1] && current[1]
        if gotScreenCapture {
            relaunch()
        } else {
            rebuildMenu()
        }
    }

    @objc private func checkUpdates() {
        updater.check()
    }

    private func applySettings() {
        caps.cursorEffectsEnabled = settings.cursorEffects
        santa.autoRides = settings.santa
        lid.isEnabled = settings.lidEffects
        if !settings.coverAfterSleep || !settings.cursorEffects { overlays.forEach { $0.overlayView.clearSnowCover() } }
        governor.evaluate()
        rebuildMenu()
    }

    @objc private func toggleEnabled() {
        settings.enabled.toggle()
        applySettings()
    }

    @objc private func chooseIntensity(_ sender: NSMenuItem) {
        if let value = sender.representedObject as? Float { settings.intensity = value }
        applySettings()
    }

    @objc private func toggleCursor() {
        settings.cursorEffects.toggle()
        applySettings()
    }

    @objc private func toggleCover() {
        settings.coverAfterSleep.toggle()
        applySettings()
    }

    @objc private func toggleLid() {
        settings.lidEffects.toggle()
        applySettings()
    }

    @objc private func toggleSanta() {
        settings.santa.toggle()
        applySettings()
    }

    @objc private func togglePauseInEditors() {
        settings.pauseInEditors.toggle()
        applySettings()
    }

    @objc private func toggleBattery() {
        settings.batterySaving.toggle()
        applySettings()
    }

    @objc private func toggleLogin() {
        do {
            if SMAppService.mainApp.status == .enabled {
                try SMAppService.mainApp.unregister()
            } else {
                try SMAppService.mainApp.register()
            }
        } catch {
            Log.shared.write("login item: \(error.localizedDescription)")
        }
        rebuildMenu()
    }

    @objc private func callSanta() {
        santa.rideNow()
    }

    @objc private func activeSpaceChanged() {
        caps.spaceDidChange(now: CACurrentMediaTime())
    }

    /// Уведомление приходит и после сна: пересоздаём, только если экраны правда другие.
    @objc private func screensChanged() {
        guard Self.screenLayout() != screenLayout else { return }
        rebuildOverlays()
    }

    private struct ScreenKey: Equatable {
        let frame: CGRect
        let scale: CGFloat
    }

    private var screenLayout: [ScreenKey] = []

    private static func screenLayout() -> [ScreenKey] {
        NSScreen.screens.map { ScreenKey(frame: $0.frame, scale: $0.backingScaleFactor) }
    }

    private func rebuildOverlays() {
        screenLayout = Self.screenLayout()
        overlays.forEach { $0.tearDown() }
        overlays = NSScreen.screens.enumerated().map { index, screen in
            OverlayWindow(screen: screen, tracker: tracker, caps: caps, renderer: renderer, seed: UInt64(index) &+ 7)
        }
        overlays.forEach {
            $0.overlayView.santa = santa
            $0.overlayView.governor = governor
            $0.overlayView.lid = lid
            $0.setPaused(governor.isPaused)
        }
        Log.shared.write("overlays: \(overlays.count), level=\(overlays.first?.level.rawValue ?? -1)")
    }

    private func logRenderStats() {
        guard !governor.isPaused else { return }
        let frames = overlays.map { $0.overlayView.takeFrameCount() }
        let fps = frames.map { String(format: "%.1f", Double($0) / 10) }.joined(separator: ",")
        Log.shared.write("render fps=[\(fps)]")
    }
}
