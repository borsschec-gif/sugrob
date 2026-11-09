import AppKit
import ApplicationServices

/// Папки на рабочем столе через Accessibility. Файлы снег не держат.
/// Вызывается только с очереди трекера.
final class DesktopIcons {
    private static let refreshInterval: CFTimeInterval = 3

    private var lastRefresh: CFTimeInterval = 0
    private var wasBusy = false
    private var folders: [TrackedWindow] = []
    private var isFolderByPath: [String: Bool] = [:]
    private var shapeByPath: [String: IconShape] = [:]

    /// Обход всех иконок заметен по времени, поэтому пока окна двигаются, он откладывается.
    func current(now: CFTimeInterval, isBusy: Bool) -> [TrackedWindow] {
        // Сразу после перетаскивания положение нужно свежим.
        if wasBusy && !isBusy { lastRefresh = 0 }
        wasBusy = isBusy
        if !isBusy, now - lastRefresh >= Self.refreshInterval {
            lastRefresh = now
            if let fresh = readFolders() { folders = fresh }
        }
        return folders
    }

    /// Иконки могут быть скрыты настройками, но Finder всё равно сообщает их координаты.
    private static var areIconsHidden: Bool {
        func flag(_ key: String, in domain: String) -> Bool? {
            CFPreferencesAppSynchronize(domain as CFString)
            return CFPreferencesCopyAppValue(key as CFString, domain as CFString) as? Bool
        }
        return flag("StandardHideDesktopIcons", in: "com.apple.WindowManager") == true
            || flag("CreateDesktop", in: "com.apple.finder") == false
    }

    private func readFolders() -> [TrackedWindow]? {
        if Self.areIconsHidden { return [] }
        guard AXIsProcessTrusted(),
              let finder = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.finder").first
        else { return nil }
        let app = AXUIElementCreateApplication(finder.processIdentifier)
        AXUIElementSetMessagingTimeout(app, 0.3)
        guard let area = Self.children(of: app).first(where: { Self.role(of: $0) == "AXScrollArea" }),
              let group = Self.children(of: area).first(where: { Self.role(of: $0) == "AXGroup" })
        else { return nil }

        var result: [TrackedWindow] = []
        for icon in Self.children(of: group) where Self.role(of: icon) == "AXImage" {
            guard let path = Self.path(of: icon), isFolder(path), let frame = Self.frame(of: icon) else { continue }
            guard let shape = shape(of: path) else { continue }
            let scale = frame.width / 52
            let surface = CGRect(
                x: frame.minX + shape.left * scale, y: frame.minY + shape.top * scale,
                width: shape.width * scale, height: frame.height - shape.top * scale)
            result.append(TrackedWindow(
                id: CGWindowID(truncatingIfNeeded: path.hashValue) & 0x0FFF_FFFF | 0xE000_0000,
                frame: surface, owner: "Finder", hasName: false, holdsSnow: true, isDesktopIcon: true,
                topProfile: shape.drops.map { $0 * Float(scale) }))
        }
        return result
    }

    private struct IconShape {
        let left: CGFloat
        let top: CGFloat
        let width: CGFloat
        let drops: [Float]
    }

    /// Контур верха настоящей иконки папки из её прозрачности, в точках иконки 52×52.
    private func shape(of path: String) -> IconShape? {
        if let known = shapeByPath[path] { return known }
        let side = 104
        var rect = CGRect(x: 0, y: 0, width: side, height: side)
        guard let image = NSWorkspace.shared.icon(forFile: path).cgImage(forProposedRect: &rect, context: nil, hints: nil)
        else { return nil }

        var pixels = [UInt8](repeating: 0, count: side * side * 4)
        let info = CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue
        let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(
                data: buffer.baseAddress, width: side, height: side, bitsPerComponent: 8,
                bytesPerRow: side * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: info)
            else { return false }
            context.draw(image, in: CGRect(x: 0, y: 0, width: side, height: side))
            return true
        }
        guard drawn else { return nil }

        let tops: [Int?] = (0..<side).map { x in
            (0..<side).first { y in pixels[(y * side + x) * 4 + 3] > 128 }
        }
        guard let first = tops.firstIndex(where: { $0 != nil }),
              let last = tops.lastIndex(where: { $0 != nil }), last > first,
              let highest = tops.compactMap({ $0 }).min() else { return nil }

        var drops: [Float] = []
        for x in stride(from: first, through: last, by: 2) {
            drops.append(Float((tops[x] ?? highest) - highest) / 2)
        }
        let result = IconShape(
            left: CGFloat(first) / 2, top: CGFloat(highest) / 2,
            width: CGFloat(last - first + 1) / 2, drops: drops)
        shapeByPath[path] = result
        return result
    }

    private func isFolder(_ path: String) -> Bool {
        if let known = isFolderByPath[path] { return known }
        let url = URL(fileURLWithPath: path)
        let values = try? url.resourceValues(forKeys: [.isDirectoryKey, .isPackageKey])
        let answer = values?.isDirectory == true && values?.isPackage != true
        isFolderByPath[path] = answer
        return answer
    }

    private static func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
    }

    private static func children(of element: AXUIElement) -> [AXUIElement] {
        attribute(element, "AXChildren") as? [AXUIElement] ?? []
    }

    private static func role(of element: AXUIElement) -> String {
        attribute(element, "AXRole") as? String ?? ""
    }

    private static func path(of element: AXUIElement) -> String? {
        if let url = attribute(element, "AXURL") as? URL { return url.path }
        return attribute(element, "AXFilename") as? String
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
