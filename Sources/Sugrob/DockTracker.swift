import AppKit
import ApplicationServices

/// Рамка Dock через Accessibility. Вызывается только с очереди трекера.
final class DockTracker {
    static let windowID: CGWindowID = 0xFFFF_FFF0

    private var list: AXUIElement?
    private var isTrusted = false
    private var lastTrustCheck: CFTimeInterval = 0
    private var cachedFrame: CGRect?
    private var lastRead: CFTimeInterval = -.infinity
    /// Dock сбоку не поддерживается; искать его заново раз в несколько секунд, а не на каждом опросе.
    private var sideDockUntil: CFTimeInterval = 0
    /// Рамка Dock меняется редко: в покое хватает раза в секунду, при активности — каждый опрос.
    private static let idleRefresh: CFTimeInterval = 1

    /// `fresh` — нужна свежая рамка: двигаются окна или курсор у Dock с авто-скрытием.
    func frame(now: CFTimeInterval, fresh: Bool) -> CGRect? {
        guard fresh || now - lastRead >= Self.idleRefresh else { return cachedFrame }
        lastRead = now
        cachedFrame = readFrame(now: now)
        return cachedFrame
    }

    private func readFrame(now: CFTimeInterval) -> CGRect? {
        if now - lastTrustCheck > 2 {
            lastTrustCheck = now
            let trusted = AXIsProcessTrusted()
            if trusted != isTrusted { Log.shared.write("dock: accessibility \(trusted)") }
            isTrusted = trusted
        }
        guard isTrusted else { return nil }

        guard now >= sideDockUntil else { return nil }
        if list == nil { list = findList() }
        guard let list, let frame = Self.frame(of: list) else {
            self.list = nil
            return nil
        }
        // Dock сбоку пока не поддерживаем.
        guard frame.width > frame.height else {
            sideDockUntil = now + 5
            return nil
        }
        // Рамка AXList выше видимой плашки: верх стекла ниже на 6 pt.
        return CGRect(x: frame.minX, y: frame.minY + 6, width: frame.width, height: frame.height - 6)
    }

    private func findList() -> AXUIElement? {
        guard let dock = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.dock").first
        else { return nil }
        let app = AXUIElementCreateApplication(dock.processIdentifier)
        AXUIElementSetMessagingTimeout(app, 0.2)
        let children = Self.attribute(app, "AXChildren") as? [AXUIElement] ?? []
        return children.first { (Self.attribute($0, "AXRole") as? String) == "AXList" }
    }

    private static func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
    }

    private static func frame(of element: AXUIElement) -> CGRect? {
        guard let position = attribute(element, "AXPosition"), let size = attribute(element, "AXSize"),
              CFGetTypeID(position) == AXValueGetTypeID(), CFGetTypeID(size) == AXValueGetTypeID()
        else { return nil }
        var origin = CGPoint.zero
        var extent = CGSize.zero
        guard AXValueGetValue(position as! AXValue, .cgPoint, &origin),
              AXValueGetValue(size as! AXValue, .cgSize, &extent) else { return nil }
        return CGRect(origin: origin, size: extent)
    }
}
