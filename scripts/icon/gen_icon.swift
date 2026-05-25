// gen_icon.swift — emit an AppIcon.iconset (all standard sizes) for tmnl.
//
//   swift scripts/icon/gen_icon.swift  # writes scripts/icon/AppIcon.iconset
//
// The companion `scripts/icon/build.sh` then runs `iconutil` to
// produce `tmnl.app/Contents/Resources/AppIcon.icns`.
//
// Design: a deep-charcoal rounded square with "tmnl" rendered in a
// monospace font (system font fallback covers users without the
// patched Nerd Font) and a thin orange "prompt" caret bar to telegraph
// "terminal". macOS rounding (~22% of side) matches Big Sur+ icon
// rounding.

import Foundation
import AppKit

let iconsetDir = URL(fileURLWithPath: CommandLine.arguments.count > 1
    ? CommandLine.arguments[1]
    : "scripts/icon/AppIcon.iconset")

try? FileManager.default.createDirectory(at: iconsetDir, withIntermediateDirectories: true)

// macOS App icon set — names + sizes that `iconutil` expects.
let sizes: [(String, Int)] = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]

func render(_ side: Int) -> Data? {
    let s = CGFloat(side)
    let cs = NSColorSpace.sRGB.cgColorSpace!
    guard let ctx = CGContext(
        data: nil,
        width: side,
        height: side,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: cs,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { return nil }

    // Outer bg (transparent margin — macOS expects ~10% padding inside
    // the canvas before the rounded-square art).
    ctx.setFillColor(CGColor(red: 0, green: 0, blue: 0, alpha: 0))
    ctx.fill(CGRect(x: 0, y: 0, width: s, height: s))

    // Rounded square.
    let inset = s * 0.06
    let body = CGRect(x: inset, y: inset, width: s - 2*inset, height: s - 2*inset)
    let radius = body.width * 0.22
    let path = CGMutablePath()
    path.addRoundedRect(in: body, cornerWidth: radius, cornerHeight: radius)
    ctx.addPath(path)
    // Gradient body — top: warm dark, bottom: cool dark. Matches
    // mnml/tmnl theme's bg → bg_darker shift.
    let topColor   = CGColor(red: 0.18, green: 0.20, blue: 0.24, alpha: 1.0)
    let botColor   = CGColor(red: 0.10, green: 0.12, blue: 0.14, alpha: 1.0)
    let gradient = CGGradient(
        colorsSpace: cs,
        colors: [topColor, botColor] as CFArray,
        locations: [0, 1]
    )!
    ctx.saveGState()
    ctx.clip()
    ctx.drawLinearGradient(
        gradient,
        start: CGPoint(x: 0, y: s),
        end: CGPoint(x: 0, y: 0),
        options: []
    )
    ctx.restoreGState()

    // Prompt caret: thin orange bar suggesting `▎` or `▍`. Sits in
    // the upper-left third of the body, vertically centered on the
    // top half. Width and position scale with `s`.
    let caretW = s * 0.045
    let caretH = s * 0.18
    let caretX = body.minX + body.width * 0.18
    let caretY = body.midY + body.height * 0.04
    ctx.setFillColor(CGColor(red: 0.85, green: 0.45, blue: 0.20, alpha: 1.0))
    ctx.fill(CGRect(x: caretX, y: caretY, width: caretW, height: caretH))

    // "tmnl" wordmark — bold sans-serif, centered horizontally,
    // bottom third of the body. NSGraphicsContext lets us use the
    // AppKit text APIs against our CGContext.
    let nsCtx = NSGraphicsContext(cgContext: ctx, flipped: false)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = nsCtx

    let label: NSString = "tmnl"
    let fontSize = s * 0.30
    let font = NSFont.monospacedSystemFont(ofSize: fontSize, weight: .bold)
    let para = NSMutableParagraphStyle()
    para.alignment = .center
    let attrs: [NSAttributedString.Key: Any] = [
        .font: font,
        .foregroundColor: NSColor(red: 0.95, green: 0.96, blue: 0.97, alpha: 1.0),
        .paragraphStyle: para,
        .kern: -fontSize * 0.04,
    ]
    let textSize = label.size(withAttributes: attrs)
    let textRect = CGRect(
        x: 0,
        y: body.minY + body.height * 0.18 - textSize.height * 0.10,
        width: s,
        height: textSize.height
    )
    label.draw(in: textRect, withAttributes: attrs)

    NSGraphicsContext.restoreGraphicsState()

    // Encode as PNG.
    guard let cg = ctx.makeImage() else { return nil }
    let rep = NSBitmapImageRep(cgImage: cg)
    return rep.representation(using: .png, properties: [:])
}

for (name, side) in sizes {
    guard let data = render(side) else {
        FileHandle.standardError.write("render \(side) failed\n".data(using: .utf8)!)
        exit(1)
    }
    let url = iconsetDir.appendingPathComponent(name)
    try data.write(to: url)
    print("wrote \(url.path)")
}
