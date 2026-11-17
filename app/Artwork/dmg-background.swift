// The background of the disk image's window (app/scripts/package.sh), drawn
// with Core Graphics at 1x and 2x and kept as one TIFF of both, which Finder
// shows at the screen's scale:
//
//   swift app/Artwork/dmg-background.swift app/Artwork
//   → app/Artwork/DMGBackground.tiff
//
// The window is 640 × 400 points. The app's icon stands at (160, 200) and the
// link to Applications at (480, 200), 128 points each (package.sh sets them).
// What to do is drawn, with no words, so it reads the same in every language:
// a dotted path leaves the icon's yellow button, the hub, and arcs over to
// Applications; on it the pointer drags the app's pocket, as a drag in
// Finder carries an icon; a little confetti is where the app lands.
//
// White, with flat fills only, as the icon's (AppIcon.svg): its violets for
// the path, the pocket as the icon draws it, its pink, yellow and violet for
// the confetti, its ink for the pointer. Finder writes the icons' names under
// them itself.
import AppKit

let size = CGSize(width: 640, height: 400)

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

let violet: UInt32 = 0x6A45D9, lightViolet: UInt32 = 0x7F5BE6, pink: UInt32 = 0xFF5F9E
let yellow: UInt32 = 0xFFD23F, ink: UInt32 = 0x171226

/// The point at `t` of the quadratic curve from `a` over `c` to `b`.
func curve(_ a: CGPoint, _ c: CGPoint, _ b: CGPoint, _ t: CGFloat) -> CGPoint {
    let u = 1 - t
    return CGPoint(x: u * u * a.x + 2 * u * t * c.x + t * t * b.x, y: u * u * a.y + 2 * u * t * c.y + t * t * b.y)
}

/// The icon's pocket (AppIcon.svg, in its 1024 units), its top left at
/// `origin`, `scale` points to a unit: the body in violet, its right half a
/// step lighter, the flap in pink and the button in yellow.
func pocket(_ context: CGContext, origin: CGPoint, scale: CGFloat) {
    context.saveGState()
    context.translateBy(x: origin.x - 262 * scale, y: origin.y - 256 * scale)
    context.scaleBy(x: scale, y: scale)
    let body = CGMutablePath()
    body.move(to: CGPoint(x: 318, y: 256))
    body.addArc(tangent1End: CGPoint(x: 762, y: 256), tangent2End: CGPoint(x: 762, y: 312), radius: 56)
    body.addArc(tangent1End: CGPoint(x: 762, y: 768), tangent2End: CGPoint(x: 537, y: 768), radius: 225)
    body.addArc(tangent1End: CGPoint(x: 262, y: 768), tangent2End: CGPoint(x: 262, y: 543), radius: 225)
    body.addArc(tangent1End: CGPoint(x: 262, y: 256), tangent2End: CGPoint(x: 318, y: 256), radius: 56)
    body.closeSubpath()
    context.addPath(body)
    context.setFillColor(color(violet))
    context.fillPath()
    context.saveGState()
    context.clip(to: CGRect(x: 512, y: 0, width: 512, height: 1024))
    context.addPath(body)
    context.setFillColor(color(lightViolet))
    context.fillPath()
    context.restoreGState()
    let flap = CGMutablePath()
    flap.move(to: CGPoint(x: 318, y: 256))
    flap.addArc(tangent1End: CGPoint(x: 762, y: 256), tangent2End: CGPoint(x: 762, y: 312), radius: 56)
    flap.addLine(to: CGPoint(x: 762, y: 367.66))
    flap.addLine(to: CGPoint(x: 512, y: 512))
    flap.addLine(to: CGPoint(x: 262, y: 367.66))
    flap.addArc(tangent1End: CGPoint(x: 262, y: 256), tangent2End: CGPoint(x: 318, y: 256), radius: 56)
    flap.closeSubpath()
    context.addPath(flap)
    context.setFillColor(color(pink))
    context.fillPath()
    context.setFillColor(color(yellow))
    context.fillEllipse(in: CGRect(x: 460, y: 460, width: 104, height: 104))
    context.restoreGState()
}

/// A shape of the confetti: turned by `angle` about `center`.
func piece(_ context: CGContext, at center: CGPoint, angle: CGFloat, _ hex: UInt32, _ draw: (CGContext) -> Void) {
    context.saveGState()
    context.translateBy(x: center.x, y: center.y)
    context.rotate(by: angle * .pi / 180)
    context.setFillColor(color(hex))
    context.setStrokeColor(color(hex))
    draw(context)
    context.restoreGState()
}

func draw(scale: CGFloat) -> CGImage {
    let width = Int(size.width * scale), height = Int(size.height * scale)
    let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.scaleBy(x: scale, y: scale)
    // Finder's coordinates run down from the top; Core Graphics' run up.
    context.translateBy(x: 0, y: size.height)
    context.scaleBy(x: 1, y: -1)

    context.setFillColor(color(0xFFFFFF))
    context.fill(CGRect(origin: .zero, size: size))

    // The path: from the hub beside the app's icon, over, to Applications.
    let start = CGPoint(x: 244, y: 168), over = CGPoint(x: 320, y: 78), end = CGPoint(x: 396, y: 168)
    context.setStrokeColor(color(lightViolet))
    context.setLineWidth(5)
    context.setLineCap(.round)
    context.setLineDash(phase: 0, lengths: [0.01, 12])
    context.move(to: curve(start, over, end, 0.08))
    context.addQuadCurve(to: curve(start, over, end, 0.86), control: curve(start, over, end, 0.47))
    context.strokePath()
    context.setLineDash(phase: 0, lengths: [])

    // Its head, along the curve where it ends.
    let tail = curve(start, over, end, 0.86), tip = end
    let angle = atan2(tip.y - tail.y, tip.x - tail.x)
    context.saveGState()
    context.translateBy(x: tip.x, y: tip.y)
    context.rotate(by: angle)
    context.setFillColor(color(violet))
    context.move(to: CGPoint(x: 7, y: 0))
    context.addLine(to: CGPoint(x: -9, y: -9))
    context.addLine(to: CGPoint(x: -9, y: 9))
    context.closePath()
    context.fillPath()
    context.restoreGState()

    // The hub it sets out from: the icon's button on its violet.
    context.setFillColor(color(violet))
    context.fillEllipse(in: CGRect(x: start.x - 11, y: start.y - 11, width: 22, height: 22))
    context.setFillColor(color(yellow))
    context.fillEllipse(in: CGRect(x: start.x - 6, y: start.y - 6, width: 12, height: 12))

    // The app's pocket carried along the path, and the pointer dragging it:
    // the system's arrow, in ink, edged in white.
    let at = curve(start, over, end, 0.5)
    pocket(context, origin: CGPoint(x: at.x - 17, y: at.y - 22), scale: 34.0 / 512)
    let arrow: [CGPoint] = [(0, 0), (0, 21), (5, 16.5), (8.5, 24.5), (12.5, 22.8), (9, 15), (15, 15)].map { CGPoint(x: $0.0, y: $0.1) }
    context.saveGState()
    context.translateBy(x: at.x + 6, y: at.y + 2)
    context.scaleBy(x: 1.3, y: 1.3)
    context.addLines(between: arrow)
    context.closePath()
    context.setLineJoin(.round)
    context.setLineWidth(3)
    context.setStrokeColor(color(0xFFFFFF))
    context.setFillColor(color(ink))
    context.drawPath(using: .fillStroke)
    context.restoreGState()

    // Confetti where the app lands: a burst over Applications.
    piece(context, at: CGPoint(x: 444, y: 116), angle: 0, pink) { $0.fillEllipse(in: CGRect(x: -4.5, y: -4.5, width: 9, height: 9)) }
    piece(context, at: CGPoint(x: 456, y: 88), angle: -30, yellow) { $0.fill(CGRect(x: -8, y: -3, width: 16, height: 6)) }
    piece(context, at: CGPoint(x: 482, y: 74), angle: 45, lightViolet) { $0.fill(CGRect(x: -5, y: -5, width: 10, height: 10)) }
    piece(context, at: CGPoint(x: 510, y: 84), angle: 0, pink) {
        $0.setLineWidth(3.5); $0.setLineCap(.round)
        $0.move(to: CGPoint(x: -6, y: 0)); $0.addLine(to: CGPoint(x: 6, y: 0))
        $0.move(to: CGPoint(x: 0, y: -6)); $0.addLine(to: CGPoint(x: 0, y: 6)); $0.strokePath()
    }
    piece(context, at: CGPoint(x: 526, y: 110), angle: 35, yellow) { $0.fillEllipse(in: CGRect(x: -4, y: -4, width: 8, height: 8)) }
    piece(context, at: CGPoint(x: 470, y: 108), angle: 60, violet) { $0.fill(CGRect(x: -6, y: -2.5, width: 12, height: 5)) }
    piece(context, at: CGPoint(x: 498, y: 106), angle: 0, yellow) { $0.fillEllipse(in: CGRect(x: -2.5, y: -2.5, width: 5, height: 5)) }

    return context.makeImage()!
}

let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : ".", isDirectory: true)
let images = [1.0, 2.0].map(draw)
// One TIFF with both, each with its size in points, as `tiffutil -cathidpicheck` makes it.
let data = NSMutableData()
let tiff = CGImageDestinationCreateWithData(data, "public.tiff" as CFString, images.count, nil)!
for (image, scale) in zip(images, [1.0, 2.0]) {
    let dpi = 72 * scale
    CGImageDestinationAddImage(tiff, image, [kCGImagePropertyDPIWidth: dpi, kCGImagePropertyDPIHeight: dpi,
                                             kCGImagePropertyTIFFDictionary: [kCGImagePropertyTIFFCompression: 5]] as CFDictionary)
}
CGImageDestinationFinalize(tiff)
try data.write(to: out.appending(path: "DMGBackground.tiff"))
// The 2x picture alone, to look at.
let png = NSBitmapImageRep(cgImage: images[1]).representation(using: .png, properties: [:])!
try png.write(to: URL(fileURLWithPath: NSTemporaryDirectory()).appending(path: "DMGBackground@2x.png"))
print(out.appending(path: "DMGBackground.tiff").path)
