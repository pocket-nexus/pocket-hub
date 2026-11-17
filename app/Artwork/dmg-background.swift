// The background of the disk image's window (app/scripts/package.sh), drawn
// with Core Graphics at 1x and 2x and kept as one TIFF of both, which Finder
// shows at the screen's scale:
//
//   swift app/Artwork/dmg-background.swift app/Artwork
//   → app/Artwork/DMGBackground.tiff
//
// The window is 640 × 400 points. The app's icon stands at (160, 200) and the
// link to Applications at (480, 200), 128 points each (package.sh sets them),
// and the arrow between them says what to do: no words, so it reads the
// same in every language.
//
// The colours are the icon's (AppIcon.svg): its violets as the ground, its
// pink as light at two corners, and its yellow button, the hub, where the
// arrow sets out. Finder writes the icons' names under them itself, black in
// the light appearance and white in the dark one, so the ground under the
// names is about the icon's lighter violet (#8866EA), which sets either about
// 4.5:1 apart or more; the light at the corners stays clear of them.
import AppKit

let size = CGSize(width: 640, height: 400)

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

/// Light that fades out from `center` over `radius`.
func glow(_ context: CGContext, center: CGPoint, radius: CGFloat, _ hex: UInt32, _ alpha: CGFloat) {
    let colors = [color(hex, alpha), color(hex, alpha * 0.45), color(hex, 0)] as CFArray
    let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: colors, locations: [0, 0.45, 1])!
    context.drawRadialGradient(gradient, startCenter: center, startRadius: 0, endCenter: center, endRadius: radius, options: [])
}

func draw(scale: CGFloat) -> CGImage {
    let width = Int(size.width * scale), height = Int(size.height * scale)
    let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.scaleBy(x: scale, y: scale)
    // Finder's coordinates run down from the top; Core Graphics' run up.
    context.translateBy(x: 0, y: size.height)
    context.scaleBy(x: 1, y: -1)

    // The ground: the icon's lighter violet, lit from above, the same along
    // the row of names so both read alike.
    let ground = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB),
                            colors: [color(0x9373F0), color(0x8866EA), color(0x7F5BE6)] as CFArray,
                            locations: [0, 0.6, 1])!
    context.drawLinearGradient(ground, start: CGPoint(x: 0, y: 0), end: CGPoint(x: 0, y: size.height), options: [])
    // A soft light behind the way between the two places.
    glow(context, center: CGPoint(x: 320, y: 200), radius: 190, 0xFFFFFF, 0.10)

    // The flap's pink as light at two corners, away from the names.
    glow(context, center: CGPoint(x: 610, y: -50), radius: 270, 0xFF5F9E, 0.6)
    glow(context, center: CGPoint(x: -50, y: 470), radius: 210, 0xFF5F9E, 0.28)

    // The way from the app to Applications: the yellow hub, then an arrow in
    // white with a shadow of the ink's violet.
    context.setShadow(offset: CGSize(width: 0, height: 2), blur: 6, color: color(0x2A1670, 0.35))
    context.setFillColor(color(0xFFD23F))
    context.fillEllipse(in: CGRect(x: 262 - 7, y: 200 - 7, width: 14, height: 14))
    context.setStrokeColor(color(0xFFFFFF, 0.95))
    context.setLineWidth(5)
    context.setLineCap(.round)
    context.setLineJoin(.round)
    context.move(to: CGPoint(x: 282, y: 200))
    context.addLine(to: CGPoint(x: 376, y: 200))
    context.move(to: CGPoint(x: 358, y: 182))
    context.addLine(to: CGPoint(x: 377, y: 200))
    context.addLine(to: CGPoint(x: 358, y: 218))
    context.strokePath()

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
