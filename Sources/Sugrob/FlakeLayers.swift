import QuartzCore

/// Падающие хлопья отдельными маленькими слоями.
/// Полноэкранный слой Metal заставляет систему пересобирать весь экран на каждый кадр,
/// а за маленькие слои она платит только их площадью: по замеру мощность GPU не отличается от фона.
@MainActor
final class FlakeLayers {
    let host = CALayer()
    private var layers: [CALayer] = []
    private var radii: [Float] = []
    private let plain = FlakeLayers.makeSprite(ring: 0)
    private let ringed = FlakeLayers.makeSprite(ring: 0.2)

    init(count: Int) {
        // Координаты снегопада идут сверху вниз.
        host.isGeometryFlipped = true
        let still: [String: any CAAction] = [
            "position": NSNull(), "bounds": NSNull(), "opacity": NSNull(), "hidden": NSNull(), "contents": NSNull(),
        ]
        host.actions = still
        for _ in 0..<count {
            let layer = CALayer()
            layer.actions = still
            layer.isHidden = true
            host.addSublayer(layer)
            layers.append(layer)
        }
        radii = [Float](repeating: -1, count: count)
    }

    func update(field: SnowField, time: Float) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        field.forEachInstance(time: time) { index, instance in
            let layer = layers[index]
            guard let instance else {
                if !layer.isHidden { layer.isHidden = true }
                return
            }
            if layer.isHidden { layer.isHidden = false }
            let side = instance.rect.z
            if radii[index] != side {
                radii[index] = side
                layer.bounds = CGRect(x: 0, y: 0, width: CGFloat(side), height: CGFloat(side))
                layer.contents = instance.ring > 0 ? ringed : plain
            }
            layer.position = CGPoint(x: CGFloat(instance.rect.x + side / 2), y: CGFloat(instance.rect.y + side / 2))
            layer.opacity = instance.alpha
        }
        CATransaction.commit()
    }

    /// Та же формула, что в шейдере `flakeFragment` при мягкости 1.
    private static func makeSprite(ring: Float) -> CGImage? {
        let side = 64
        var pixels = [UInt8](repeating: 0, count: side * side * 4)
        func smoothstep(_ low: Float, _ high: Float, _ x: Float) -> Float {
            let t = max(0, min(1, (x - low) / (high - low)))
            return t * t * (3 - 2 * t)
        }
        for y in 0..<side {
            for x in 0..<side {
                let u = (Float(x) + 0.5) / Float(side) * 2 - 1
                let v = (Float(y) + 0.5) / Float(side) * 2 - 1
                let d = (u * u + v * v).squareRoot()
                var body = 1 - smoothstep(0, 1, d)
                body *= body
                let rim = smoothstep(0.55, 0.92, d) * body * ring
                let a = body * (1 - ring * 0.5) + rim
                let offset = (y * side + x) * 4
                pixels[offset] = UInt8(max(0, min(255, 0.93 * a * 255)))
                pixels[offset + 1] = UInt8(max(0, min(255, 0.96 * a * 255)))
                pixels[offset + 2] = UInt8(max(0, min(255, a * 255)))
                pixels[offset + 3] = UInt8(max(0, min(255, a * 255)))
            }
        }
        guard let provider = CGDataProvider(data: Data(pixels) as CFData) else { return nil }
        return CGImage(
            width: side, height: side, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: side * 4,
            space: CGColorSpace(name: CGColorSpace.sRGB)!,
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }
}
