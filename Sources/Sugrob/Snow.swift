import Foundation
import simd

/// Формат совпадает со структурой FlakeInstance в шейдере.
struct FlakeInstance {
    /// x, y, ширина, высота в точках, y вниз.
    var rect: SIMD4<Float>
    var alpha: Float
    /// Ширина мягкого края: 0 — резкий круг, 1 — мягкий от центра.
    var softness: Float
    /// Светлая кайма, как у боке.
    var ring: Float
    var padding: Float = 0
}

/// Снегопад одного экрана в его локальных точках, y вниз.
struct SnowField {
    private struct DepthLayer {
        let areaPerFlake: Float
        let radius: ClosedRange<Float>
        let fallSpeed: ClosedRange<Float>
        let alpha: ClosedRange<Float>
        let swayAmplitude: ClosedRange<Float>
        let parallax: Float
        let softness: Float
        let ring: Float
        /// Сколько снега добавляет упавший хлопок. 0 — пролетает мимо окон.
        let deposit: Float
    }

    private struct Flake {
        var x: Float
        var y: Float
        var radius: Float
        var fallSpeed: Float
        var alpha: Float
        var phase: Float
        var swayAmplitude: Float
        var swayRate: Float
        var layer: Int
        /// Хлопок виден, если его ранг не выше текущей густоты.
        var rank: Float
    }

    private static let layers = [
        // Дальний план: мелкая пыль.
        DepthLayer(areaPerFlake: 9_000, radius: 1.3...2.0, fallSpeed: 16...26, alpha: 0.4...0.65,
                   swayAmplitude: 3...7, parallax: 0.5, softness: 1, ring: 0, deposit: 0.9),
        // Средний план: основные хлопья, радиус включает мягкий ореол.
        DepthLayer(areaPerFlake: 8_000, radius: 2.6...4.2, fallSpeed: 30...48, alpha: 0.8...1,
                   swayAmplitude: 6...16, parallax: 1, softness: 1, ring: 0, deposit: 1.8),
        // У камеры: редкие крупные размытые хлопья.
        DepthLayer(areaPerFlake: 90_000, radius: 7...13, fallSpeed: 62...95, alpha: 0.2...0.34,
                   swayAmplitude: 10...24, parallax: 1.7, softness: 1, ring: 0.2, deposit: 0),
    ]

    private var flakes: [Flake] = []
    private var generator: SeededGenerator
    private let width: Float
    private let height: Float
    private let margin: Float = 24

    /// Сколько хлопьев есть всего: столько места нужно в буфере.
    var instanceCount: Int { flakes.count }
    /// Густота: 0,5 — редкий, 1 — обычный, 1,5 — густой снег.
    var density: Float = 1
    private static let maxDensity: Float = 1.5
    private var visibleRank: Float { density / Self.maxDensity }

    init(width: Float, height: Float, seed: UInt64) {
        self.width = width
        self.height = height
        generator = SeededGenerator(state: seed)
        let area = width * height
        for (index, layer) in Self.layers.enumerated() {
            for _ in 0..<max(1, Int(area * Self.maxDensity / layer.areaPerFlake)) {
                var flake = makeFlake(layer: index)
                flake.y = Float.random(in: -margin...(height + margin), using: &generator)
                flakes.append(flake)
            }
        }
    }

    private mutating func makeFlake(layer index: Int) -> Flake {
        let layer = Self.layers[index]
        return Flake(
            x: Float.random(in: -margin...(width + margin), using: &generator),
            y: -margin,
            radius: Float.random(in: layer.radius, using: &generator),
            fallSpeed: Float.random(in: layer.fallSpeed, using: &generator),
            alpha: Float.random(in: layer.alpha, using: &generator),
            phase: Float.random(in: 0...(2 * .pi), using: &generator),
            swayAmplitude: Float.random(in: layer.swayAmplitude, using: &generator),
            swayRate: Float.random(in: 2...5.5, using: &generator),
            layer: index,
            rank: Float.random(in: 0...1, using: &generator))
    }

    /// Ветер — чистая функция времени, поэтому на всех экранах он одинаковый.
    private static func wind(at time: Float) -> Float {
        let drift = 8 + 12 * sin(time * 0.11) + 6 * sin(time * 0.27 + 1.3)
        let gustShape = pow(max(0, sin(time * 0.047 + 0.7)), 12)
        let gustDirection: Float = sin(time * 0.013) >= 0 ? 1 : -1
        return drift + gustShape * gustDirection * 90
    }

    /// `origin` — левый верхний угол экрана в координатах CoreGraphics.
    @MainActor
    /// `kick` — толчок от крышки по вертикали, pt/s.
    mutating func step(dt: Float, time: Float, caps: CapField, origin: SIMD2<Float>, now: CFTimeInterval, kick: Float = 0) {
        let wind = Self.wind(at: time)
        let visibleRank = visibleRank
        for index in flakes.indices where flakes[index].rank <= visibleRank {
            var flake = flakes[index]
            let layer = Self.layers[flake.layer]
            let sway = cos(time * flake.swayRate + flake.phase) * flake.swayAmplitude
            let previousY = flake.y
            flake.x += (wind * layer.parallax + sway) * dt
            flake.y += (flake.fallSpeed + kick * layer.parallax) * dt

            let landed = layer.deposit > 0 && caps.tryLand(
                x: flake.x + origin.x, fromY: previousY + origin.y, toY: flake.y + origin.y,
                amount: layer.deposit, now: now)
            if landed || flake.y > height + margin {
                // Ранг постоянный, иначе видимых хлопьев становилось бы всё меньше.
                let rank = flake.rank
                flake = makeFlake(layer: flake.layer)
                flake.rank = rank
            } else if flake.x > width + margin {
                flake.x = -margin
            } else if flake.x < -margin {
                flake.x = width + margin
            }
            flakes[index] = flake
        }
    }

    /// Возвращает, сколько хлопьев записано.
    func writeInstances(to pointer: UnsafeMutablePointer<FlakeInstance>, time: Float) -> Int {
        var written = 0
        let visibleRank = visibleRank
        for flake in flakes where flake.rank <= visibleRank {
            let layer = Self.layers[flake.layer]
            let twinkle = 0.85 + 0.15 * sin(time * 1.7 + flake.phase * 3)
            pointer[written] = FlakeInstance(
                rect: SIMD4(flake.x - flake.radius, flake.y - flake.radius, flake.radius * 2, flake.radius * 2),
                alpha: flake.alpha * twinkle,
                softness: layer.softness,
                ring: layer.ring)
            written += 1
        }
        return written
    }
}

struct SeededGenerator: RandomNumberGenerator {
    var state: UInt64

    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}
