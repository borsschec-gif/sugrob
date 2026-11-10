import Metal
import simd

/// Сплошной снег на весь экран после сна. Хранится как карта толщины в низком разрешении.
@MainActor
final class SnowSheet {
    private static let cell: Float = 6
    private static let wipeRadius: Float = 62
    /// Когда от нанесённого снега осталось меньше этой доли, остаток тает сам: дочищать углы незачем.
    private static let meltBelow: Float = 0.07
    private static let meltPerFrame: UInt8 = 14
    /// Самый большой занос: верх экрана остаётся чистым, снегу ещё есть куда сыпаться.
    static let fullCover: Float = 0.84
    /// За сколько секунд сна экран заносит полностью и с какого сна занос вообще заметен.
    static let fullCoverAfter: TimeInterval = 30 * 60
    static let minSleep: TimeInterval = 2 * 60

    /// Доля экрана под снегом после сна: растёт равномерно, 5 минут — 14 %, 10 — 28 %, 30 — 84 %.
    static func coverFraction(afterSleep seconds: TimeInterval) -> Float {
        guard seconds >= minSleep else { return 0 }
        return fullCover * Float(min(1, seconds / fullCoverAfter))
    }
    /// Толщина мягкого края у кромки сугроба, pt.
    private static let edge: Float = 18

    let texture: MTLTexture
    private let columns: Int
    private let rows: Int
    private var values: [UInt8]
    /// Волны кромки сугроба в каждом столбце, pt.
    private let waves: [Float]
    private let height: Float
    /// Какая доля экрана сейчас занесена и сколько клеток было под снегом сразу после заноса.
    private var fraction: Float = 0
    private var coveredAtStart = 0
    private var generator: SeededGenerator
    private var isDirty = false
    private var isMelting = false
    /// На сколько кромка осела вниз от рывков и наклона, pt.
    private var sink: Float = 0
    private var lastCursor: SIMD2<Float>?

    private(set) var isActive = false

    init?(device: MTLDevice, width: Float, height: Float, seed: UInt64) {
        let columns = max(2, Int((width / Self.cell).rounded(.up)))
        let rows = max(2, Int((height / Self.cell).rounded(.up)))
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .r8Unorm, width: columns, height: rows, mipmapped: false)
        descriptor.usage = .shaderRead
        descriptor.storageMode = .managed
        guard let texture = device.makeTexture(descriptor: descriptor) else { return nil }
        self.texture = texture
        self.columns = columns
        self.rows = rows
        values = [UInt8](repeating: 0, count: columns * rows)

        var generator = SeededGenerator(state: seed)
        let phases = (0..<3).map { _ in Float.random(in: 0...6.28, using: &generator) }
        self.height = height
        waves = (0..<columns).map { column in
            let x = Float(column) * Self.cell
            return 22 * sin(x * 0.007 + phases[0]) + 12 * sin(x * 0.021 + phases[1]) + 5 * sin(x * 0.06 + phases[2])
        }
        self.generator = generator
    }

    /// Заносит нижнюю часть экрана. Неубранный снег с прошлого сна не становится ниже.
    func cover(fraction newFraction: Float = SnowSheet.fullCover) {
        fraction = min(Self.fullCover, max(newFraction, isActive && !isMelting ? fraction : 0))
        guard fraction > 0 else { return }
        sink = 0
        for row in 0..<rows {
            for column in 0..<columns { values[row * columns + column] = level(row: row, column: column) }
        }
        coveredAtStart = coveredCells
        isActive = true
        isMelting = false
        isDirty = true
        lastCursor = nil
    }

    func clear() {
        for index in values.indices { values[index] = 0 }
        isActive = false
        isMelting = false
        isDirty = true
    }

    /// `point` — курсор в точках этого экрана. Снег стирается полосой вдоль пути курсора.
    func wipe(to point: SIMD2<Float>, caps: CapField, origin: SIMD2<Float>) {
        defer { lastCursor = point }
        guard isActive, !isMelting, let last = lastCursor else { return }
        let delta = point - last
        let lengthSquared = simd_length_squared(delta)
        guard lengthSquared >= 0.25 else { return }

        let radius = Self.wipeRadius
        let firstColumn = max(0, Int((min(last.x, point.x) - radius) / Self.cell))
        let lastColumn = min(columns - 1, Int((max(last.x, point.x) + radius) / Self.cell))
        let firstRow = max(0, Int((min(last.y, point.y) - radius) / Self.cell))
        let lastRow = min(rows - 1, Int((max(last.y, point.y) + radius) / Self.cell))
        guard firstColumn <= lastColumn, firstRow <= lastRow else { return }

        let throwVelocity = simd_clamp(delta * 8, SIMD2(repeating: -500), SIMD2(repeating: 500))
        var changed = false
        for row in firstRow...lastRow {
            for column in firstColumn...lastColumn {
                let index = row * columns + column
                guard values[index] > 0 else { continue }
                let center = SIMD2(Float(column) + 0.5, Float(row) + 0.5) * Self.cell
                let along = max(0, min(1, simd_dot(center - last, delta) / lengthSquared))
                let distance = simd_distance(center, last + delta * along)
                guard distance < radius else { continue }
                let keep = max(0, (distance / radius - 0.65) / 0.35)
                let target = UInt8(255 * keep)
                guard values[index] > target else { continue }
                remove(index, to: target, center: center, velocity: throwVelocity, caps: caps, origin: origin)
                changed = true
            }
        }
        if changed { refreshCoverage() }
    }

    /// Толщина снега под кромкой: выше неё пусто, ниже сугроб.
    private func level(row: Int, column: Int) -> UInt8 {
        // У низкого сугроба волны ниже, иначе кромка уходила бы за край экрана.
        let top = height * (1 - fraction) + waves[column] * min(1, fraction / 0.2)
        let depth = (Float(row) + 0.5) * Self.cell - top - sink
        return UInt8(max(0, min(1, depth / Self.edge)) * 255)
    }

    /// Рывок и наклон осаживают сугроб вниз лавиной. Счищенное курсором не возвращается.
    func applyLid(_ state: LidState, caps: CapField, origin: SIMD2<Float>) {
        guard isActive, !isMelting else { return }
        let drop = state.shake * state.shake * 260 + (120 + 1400 * state.slip * state.slip) * state.dt * (state.slip > 0 ? 1 : 0)
        guard drop > 0 else { return }
        sink += drop

        let fall = SIMD2<Float>(0, max(0, min(260, abs(state.speed) * 0.7)))
        var changed = false
        for row in 0..<rows {
            for column in 0..<columns {
                let index = row * columns + column
                let target = level(row: row, column: column)
                guard target < values[index] else { continue }
                let center = SIMD2(Float(column) + 0.5, Float(row) + 0.5) * Self.cell
                remove(index, to: target, center: center, velocity: fall, caps: caps, origin: origin)
                changed = true
            }
        }
        if changed { refreshCoverage() }
    }

    private func remove(
        _ index: Int, to target: UInt8, center: SIMD2<Float>, velocity: SIMD2<Float>,
        caps: CapField, origin: SIMD2<Float>
    ) {
        let removed = Float(values[index] - target) / 255
        values[index] = target
        isDirty = true
        guard Float.random(in: 0...1, using: &generator) < removed * 0.18 else { return }
        caps.addFallingSnow(at: center + origin, velocity: velocity + SIMD2(
            Float.random(in: -60...60, using: &generator),
            Float.random(in: -90...20, using: &generator)))
    }

    private var coveredCells: Int {
        values.reduce(0) { $0 + ($1 > 40 ? 1 : 0) }
    }

    private func refreshCoverage() {
        if Float(coveredCells) < Float(coveredAtStart) * Self.meltBelow { isMelting = true }
    }

    /// Раз в кадр: дотаивание и загрузка карты в текстуру.
    func step() {
        if isMelting {
            var anyLeft = false
            for index in values.indices where values[index] > 0 {
                values[index] -= min(values[index], Self.meltPerFrame)
                if values[index] > 0 { anyLeft = true }
            }
            isDirty = true
            if !anyLeft {
                isMelting = false
                isActive = false
                Log.shared.write("sheet: экран очищен")
            }
        }
        guard isDirty else { return }
        isDirty = false
        values.withUnsafeBytes { bytes in
            texture.replace(
                region: MTLRegionMake2D(0, 0, columns, rows), mipmapLevel: 0,
                withBytes: bytes.baseAddress!, bytesPerRow: columns)
        }
    }
}
