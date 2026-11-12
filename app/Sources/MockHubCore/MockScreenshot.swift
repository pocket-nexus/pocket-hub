import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// Draws a PNG shaped like a 3DS screenshot from the real core: the top
/// screen (400×240) above the bottom screen (320×240, centred), 400×480.
enum MockScreenshot {
    static func write(to url: URL, frame: Int) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let width = 400, height = 480
        guard let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { throw CocoaError(.fileWriteUnknown) }
        // CoreGraphics' origin is bottom-left: the top screen is y 240..<480.
        context.setFillColor(CGColor(red: 0.08, green: 0.08, blue: 0.1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))

        let hue = Double(frame % 360) / 360
        let top = CGRect(x: 0, y: 240, width: 400, height: 240)
        let colors = [color(hue: hue, brightness: 0.9), color(hue: hue + 0.15, brightness: 0.55)] as CFArray
        if let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: colors, locations: nil) {
            context.saveGState()
            context.clip(to: top)
            context.drawLinearGradient(gradient, start: CGPoint(x: 0, y: 480), end: CGPoint(x: 400, y: 240), options: [])
            context.restoreGState()
        }
        // A few "applet tiles" on the top screen.
        context.setFillColor(CGColor(gray: 1, alpha: 0.85))
        for column in 0..<4 {
            context.fill(CGRect(x: 30 + column * 90, y: 300, width: 70, height: 70))
        }
        let bottom = CGRect(x: 40, y: 0, width: 320, height: 240)
        context.setFillColor(CGColor(gray: 0.93, alpha: 1))
        context.fill(bottom)
        context.setFillColor(color(hue: hue + 0.5, brightness: 0.7))
        context.fill(CGRect(x: 60, y: 20 + (frame % 140), width: 280, height: 60))

        guard let image = context.makeImage(),
              let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { throw CocoaError(.fileWriteUnknown) }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
    }

    static func color(hue: Double, brightness: Double) -> CGColor {
        // HSB to RGB with full saturation scaled down a little.
        let h = (hue.truncatingRemainder(dividingBy: 1) + 1).truncatingRemainder(dividingBy: 1) * 6
        let saturation = 0.6
        let c = brightness * saturation
        let x = c * (1 - abs(h.truncatingRemainder(dividingBy: 2) - 1))
        let m = brightness - c
        let (r, g, b): (Double, Double, Double) = switch Int(h) {
        case 0: (c, x, 0)
        case 1: (x, c, 0)
        case 2: (0, c, x)
        case 3: (0, x, c)
        case 4: (x, 0, c)
        default: (c, 0, x)
        }
        return CGColor(red: r + m, green: g + m, blue: b + m, alpha: 1)
    }
}

/// A 128×128 rounded-square icon in a given hue.
enum MockIcon {
    static func write(to url: URL, hue: Double) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        let size = 128
        guard let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { throw CocoaError(.fileWriteUnknown) }
        let rect = CGRect(x: 8, y: 8, width: 112, height: 112)
        context.addPath(CGPath(roundedRect: rect, cornerWidth: 26, cornerHeight: 26, transform: nil))
        context.setFillColor(MockScreenshot.color(hue: hue, brightness: 0.85))
        context.fillPath()
        context.setFillColor(CGColor(gray: 1, alpha: 0.9))
        context.fillEllipse(in: CGRect(x: 40, y: 40, width: 48, height: 48))
        guard let image = context.makeImage(),
              let destination = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)
        else { throw CocoaError(.fileWriteUnknown) }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else { throw CocoaError(.fileWriteUnknown) }
    }
}
