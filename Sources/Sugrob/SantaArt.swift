import AppKit
import CoreGraphics

/// Спрайты Деда Мороза и подарка, нарисованные кодом. Тройка едет влево.
enum SantaArt {
    static let width = 210
    static let height = 76
    static let frameCount = 4

    private static func color(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> CGColor {
        CGColor(srgbRed: r, green: g, blue: b, alpha: a)
    }

    /// BGRA с премультиплицированной альфой, первая строка — верх картинки.
    static func makeFrame(_ frame: Int, scale: Int) -> (pixels: [UInt8], width: Int, height: Int)? {
        let pixelWidth = width * scale, pixelHeight = height * scale
        var pixels = [UInt8](repeating: 0, count: pixelWidth * pixelHeight * 4)
        let info = CGBitmapInfo.byteOrder32Little.rawValue | CGImageAlphaInfo.premultipliedFirst.rawValue
        let drawn = pixels.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(
                data: buffer.baseAddress, width: pixelWidth, height: pixelHeight, bitsPerComponent: 8,
                bytesPerRow: pixelWidth * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: info)
            else { return false }
            context.scaleBy(x: CGFloat(scale), y: CGFloat(scale))
            context.setLineCap(.round)
            context.setLineJoin(.round)
            draw(in: context, frame: frame)
            return true
        }
        return drawn ? (pixels, pixelWidth, pixelHeight) : nil
    }

    private static func draw(in c: CGContext, frame: Int) {
        let phase = CGFloat(frame) / CGFloat(frameCount) * 2 * .pi

        // Дальние лошади темнее и выше: получается тройка в глубину.
        drawHorse(c, x: 22, y: 12, phase: phase + 2.1, body: color(0.27, 0.17, 0.12), mane: color(0.10, 0.07, 0.06))
        drawHorse(c, x: 30, y: 8, phase: phase + 1.0, body: color(0.36, 0.23, 0.15), mane: color(0.12, 0.08, 0.07))
        drawHorse(c, x: 14, y: 3, phase: phase, body: color(0.50, 0.31, 0.19), mane: color(0.14, 0.09, 0.07))

        // Дуга с колокольчиком над центральной лошадью.
        c.setStrokeColor(color(0.78, 0.16, 0.14))
        c.setLineWidth(3.2)
        c.addArc(center: CGPoint(x: 52, y: 40), radius: 15, startAngle: 0.15, endAngle: .pi - 0.15, clockwise: false)
        c.strokePath()
        c.setFillColor(color(0.95, 0.76, 0.26))
        c.fillEllipse(in: CGRect(x: 48.5, y: 44 + sin(phase) * 1.2, width: 7, height: 7))

        // Вожжи.
        c.setStrokeColor(color(0.20, 0.13, 0.09))
        c.setLineWidth(1)
        c.move(to: CGPoint(x: 42, y: 39))
        c.addQuadCurve(to: CGPoint(x: 134, y: 37), control: CGPoint(x: 90, y: 26))
        c.strokePath()

        drawSleigh(c)
        drawGrandfather(c, bob: sin(phase * 2) * 0.7)
    }

    private static func drawHorse(_ c: CGContext, x: CGFloat, y: CGFloat, phase: CGFloat, body: CGColor, mane: CGColor) {
        // Ноги: галоп по двум парам в противофазе.
        c.setStrokeColor(body)
        c.setLineWidth(3.4)
        let legs: [(CGFloat, CGFloat)] = [(9, 0), (15, 0.7), (37, .pi), (43, .pi + 0.7)]
        for (hip, offset) in legs {
            let swing = sin(phase + offset) * 7
            let knee = CGPoint(x: x + hip + swing * 0.7, y: y + 9)
            c.move(to: CGPoint(x: x + hip, y: y + 20))
            c.addLine(to: knee)
            c.addLine(to: CGPoint(x: knee.x + swing * 0.5, y: y + 1 + abs(swing) * 0.25))
            c.strokePath()
        }

        c.setFillColor(body)
        c.fillEllipse(in: CGRect(x: x + 3, y: y + 15, width: 46, height: 18))

        // Шея и морда смотрят влево.
        c.move(to: CGPoint(x: x + 13, y: y + 31))
        c.addLine(to: CGPoint(x: x + 3, y: y + 47))
        c.addLine(to: CGPoint(x: x + 12, y: y + 50))
        c.addLine(to: CGPoint(x: x + 23, y: y + 29))
        c.closePath()
        c.fillPath()
        c.fillEllipse(in: CGRect(x: x - 9, y: y + 41, width: 20, height: 10))
        c.move(to: CGPoint(x: x + 5, y: y + 50))
        c.addLine(to: CGPoint(x: x + 8, y: y + 57))
        c.addLine(to: CGPoint(x: x + 10, y: y + 49))
        c.closePath()
        c.fillPath()

        // Грива и хвост.
        c.setStrokeColor(mane)
        c.setLineWidth(3)
        c.move(to: CGPoint(x: x + 11, y: y + 51))
        c.addQuadCurve(to: CGPoint(x: x + 23, y: y + 31), control: CGPoint(x: x + 21, y: y + 44))
        c.strokePath()
        c.setLineWidth(3.6)
        c.move(to: CGPoint(x: x + 48, y: y + 29))
        c.addQuadCurve(
            to: CGPoint(x: x + 59, y: y + 12 + sin(phase) * 3), control: CGPoint(x: x + 58, y: y + 29))
        c.strokePath()

        // Глаз.
        c.setFillColor(color(0.05, 0.04, 0.04))
        c.fillEllipse(in: CGRect(x: x - 1, y: y + 46, width: 2.2, height: 2.2))
    }

    private static func drawSleigh(_ c: CGContext) {
        // Полоз с завитком впереди.
        c.setStrokeColor(color(0.93, 0.73, 0.25))
        c.setLineWidth(3)
        c.move(to: CGPoint(x: 196, y: 6))
        c.addLine(to: CGPoint(x: 112, y: 6))
        c.addCurve(
            to: CGPoint(x: 96, y: 28), control1: CGPoint(x: 94, y: 6), control2: CGPoint(x: 88, y: 18))
        c.strokePath()
        c.setLineWidth(2.2)
        for x in [128, 178] as [CGFloat] {
            c.move(to: CGPoint(x: x, y: 6))
            c.addLine(to: CGPoint(x: x, y: 15))
            c.strokePath()
        }

        // Короб.
        c.setFillColor(color(0.70, 0.13, 0.13))
        c.move(to: CGPoint(x: 112, y: 14))
        c.addLine(to: CGPoint(x: 192, y: 14))
        c.addCurve(
            to: CGPoint(x: 198, y: 46), control1: CGPoint(x: 200, y: 22), control2: CGPoint(x: 202, y: 36))
        c.addLine(to: CGPoint(x: 180, y: 38))
        c.addLine(to: CGPoint(x: 128, y: 36))
        c.addCurve(
            to: CGPoint(x: 112, y: 14), control1: CGPoint(x: 116, y: 36), control2: CGPoint(x: 108, y: 26))
        c.closePath()
        c.fillPath()

        c.setStrokeColor(color(0.95, 0.76, 0.26))
        c.setLineWidth(1.8)
        c.move(to: CGPoint(x: 120, y: 20))
        c.addLine(to: CGPoint(x: 190, y: 20))
        c.strokePath()
    }

    private static func drawGrandfather(_ c: CGContext, bob: CGFloat) {
        // Мешок.
        c.setFillColor(color(0.77, 0.18, 0.16))
        c.fillEllipse(in: CGRect(x: 164, y: 34, width: 34, height: 32))
        c.setStrokeColor(color(0.95, 0.76, 0.26))
        c.setLineWidth(2.2)
        c.move(to: CGPoint(x: 174, y: 63))
        c.addLine(to: CGPoint(x: 188, y: 62))
        c.strokePath()

        // Шуба.
        c.setFillColor(color(0.17, 0.35, 0.70))
        c.move(to: CGPoint(x: 134, y: 34))
        c.addCurve(
            to: CGPoint(x: 165, y: 34 + bob), control1: CGPoint(x: 136, y: 58 + bob), control2: CGPoint(x: 161, y: 60 + bob))
        c.closePath()
        c.fillPath()

        // Меховая оторочка и рука с вожжами.
        c.setFillColor(color(0.95, 0.96, 0.98))
        c.fill(CGRect(x: 133, y: 33, width: 33, height: 4))
        c.setStrokeColor(color(0.17, 0.35, 0.70))
        c.setLineWidth(5.5)
        c.move(to: CGPoint(x: 146, y: 47 + bob))
        c.addLine(to: CGPoint(x: 134, y: 39))
        c.strokePath()

        // Лицо, борода, шапка.
        let headY = 51 + bob
        c.setFillColor(color(0.95, 0.76, 0.64))
        c.fillEllipse(in: CGRect(x: 140, y: headY, width: 16, height: 15))
        c.setFillColor(color(0.96, 0.97, 0.99))
        c.move(to: CGPoint(x: 137, y: headY + 7))
        c.addCurve(
            to: CGPoint(x: 157, y: headY + 7), control1: CGPoint(x: 134, y: headY - 14), control2: CGPoint(x: 156, y: headY - 12))
        c.closePath()
        c.fillPath()
        c.setFillColor(color(0.17, 0.35, 0.70))
        c.fillEllipse(in: CGRect(x: 139, y: headY + 9, width: 19, height: 13))
        c.setFillColor(color(0.96, 0.97, 0.99))
        c.fill(CGRect(x: 138.5, y: headY + 9, width: 20, height: 3.6))
        c.setFillColor(color(0.84, 0.36, 0.34))
        c.fillEllipse(in: CGRect(x: 137.8, y: headY + 5.5, width: 4, height: 4))

        // Посох с навершием.
        c.setStrokeColor(color(0.93, 0.76, 0.32))
        c.setLineWidth(2.2)
        c.move(to: CGPoint(x: 160, y: 24))
        c.addLine(to: CGPoint(x: 169, y: 72))
        c.strokePath()
        c.setFillColor(color(0.72, 0.87, 1.0))
        c.fillEllipse(in: CGRect(x: 166, y: 69, width: 7, height: 7))
    }

    /// Подарок для окна, которое принимает клик.
    static func giftImage() -> NSImage {
        NSImage(size: NSSize(width: 44, height: 44), flipped: false) { _ in
            guard let c = NSGraphicsContext.current?.cgContext else { return false }
            c.setFillColor(color(0.78, 0.15, 0.15))
            c.addPath(CGPath(roundedRect: CGRect(x: 7, y: 4, width: 30, height: 25), cornerWidth: 3, cornerHeight: 3, transform: nil))
            c.fillPath()
            c.setFillColor(color(0.89, 0.20, 0.19))
            c.addPath(CGPath(roundedRect: CGRect(x: 4, y: 24, width: 36, height: 9), cornerWidth: 2.5, cornerHeight: 2.5, transform: nil))
            c.fillPath()
            c.setFillColor(color(0.96, 0.78, 0.28))
            c.fill(CGRect(x: 19, y: 4, width: 6, height: 29))
            c.fillEllipse(in: CGRect(x: 10, y: 31, width: 12, height: 9))
            c.fillEllipse(in: CGRect(x: 22, y: 31, width: 12, height: 9))
            return true
        }
    }
}
