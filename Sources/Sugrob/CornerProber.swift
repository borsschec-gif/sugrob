import AppKit
@preconcurrency import ScreenCaptureKit

/// Измеряет настоящий радиус угла окна: macOS не сообщает его иначе, чем через снимок.
/// Снимается только левый верхний угол 48×48 pt, картинка сразу выбрасывается.
@MainActor
final class CornerProber {
    private let caps: CapField
    private var measured = Set<CGWindowID>()
    private var isRunning = false

    init(caps: CapField) {
        self.caps = caps
    }

    func update(windows: [TrackedWindow]) {
        guard !isRunning, CGPreflightScreenCaptureAccess() else { return }
        let pending = windows.filter { $0.holdsSnow && !$0.isFixedSurface && !measured.contains($0.id) }
        guard !pending.isEmpty else { return }
        isRunning = true
        let batch = Array(pending.prefix(4))
        Task { @MainActor in
            await probe(batch)
            isRunning = false
        }
    }

    private func probe(_ windows: [TrackedWindow]) async {
        guard let content = try? await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true) else { return }
        for window in windows {
            measured.insert(window.id)
            guard let target = content.windows.first(where: { $0.windowID == window.id }),
                  let radius = await measureRadius(of: target) else { continue }
            Log.shared.write(String(format: "corner: %@ %.1f pt", window.owner, radius))
            caps.setCornerRadius(radius, for: window.id)
        }
    }

    private func measureRadius(of window: SCWindow) async -> Float? {
        let side = 96
        let configuration = SCStreamConfiguration()
        configuration.sourceRect = CGRect(x: 0, y: 0, width: 48, height: 48)
        configuration.width = side
        configuration.height = side
        configuration.showsCursor = false
        configuration.ignoreShadowsSingleWindow = true

        let filter = SCContentFilter(desktopIndependentWindow: window)
        guard let image = try? await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
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

        // На диагонали край круга радиуса R находится на расстоянии 0,293·R от угла.
        guard let firstOpaque = (0..<side).first(where: { pixels[($0 * side + $0) * 4 + 3] > 96 }) else { return nil }
        let radius = Float(firstOpaque) / 0.293 / 2
        return radius <= 45 ? radius : nil
    }
}
