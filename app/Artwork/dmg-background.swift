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
// Flat fills only, as the icon's (AppIcon.svg): its lighter violet #7F5BE6
// as the ground, its yellow button, the hub, where the arrow sets out, and
// the arrow in white. Finder writes the icons' names under them itself, in
// black or in white, and that violet sets either about 4.5:1 apart.
import AppKit

let size = CGSize(width: 640, height: 400)

func color(_ hex: UInt32, _ alpha: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255, alpha: alpha)
}

func draw(scale: CGFloat) -> CGImage {
    let width = Int(size.width * scale), height = Int(size.height * scale)
    let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    context.scaleBy(x: scale, y: scale)
    // Finder's coordinates run down from the top; Core Graphics' run up.
    context.translateBy(x: 0, y: size.height)
    context.scaleBy(x: 1, y: -1)

    context.setFillColor(color(0x7F5BE6))
    context.fill(CGRect(origin: .zero, size: size))

    // The way from the app to Applications: the yellow hub, then an arrow.
    context.setFillColor(color(0xFFD23F))
    context.fillEllipse(in: CGRect(x: 262 - 7, y: 200 - 7, width: 14, height: 14))
    context.setStrokeColor(color(0xFFFFFF))
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
