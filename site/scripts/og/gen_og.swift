// gen_og.swift — emit the 1200×630 OpenGraph / Twitter card hero for the
// tmnl site.
//
//   swift site/scripts/og/gen_og.swift                # writes site/src/assets/og/hero.png + mirrors to site/public/og/hero.png
//   swift site/scripts/og/gen_og.swift path/to/x.png  # writes to that path only
//
// Design follows scripts/icon/gen_icon.swift: deep-charcoal gradient body,
// rounded "terminal bezel" look, `> tmnl` wordmark with the warm-orange
// caret. Adds a tagline row below the wordmark sized for social previews.
//
// The master copy lives under src/assets/og/ (it's a designed asset, owned
// by the source tree). It also gets mirrored into public/og/ so it serves
// at the stable URL /og/hero.png — the path referenced from the og:image
// + twitter:image meta tags in astro.config.mjs.
//
// Output is a 1200×630 sRGB PNG.

import Foundation
import AppKit

let outURL = URL(fileURLWithPath: CommandLine.arguments.count > 1
    ? CommandLine.arguments[1]
    : "site/src/assets/og/hero.png")

try? FileManager.default.createDirectory(
    at: outURL.deletingLastPathComponent(),
    withIntermediateDirectories: true)

let width = 1200
let height = 630

func render() -> Data? {
    let w = CGFloat(width)
    let h = CGFloat(height)
    let cs = NSColorSpace.sRGB.cgColorSpace!
    guard let ctx = CGContext(
        data: nil,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: cs,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { return nil }

    // Deep-charcoal canvas — full bleed. Social card previews crop slightly
    // so we don't want a transparent margin like the .icns build.
    ctx.setFillColor(CGColor(red: 0.06, green: 0.07, blue: 0.09, alpha: 1.0))
    ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))

    // Bezel — a rounded charcoal "card" inset from the edges. Mirrors the
    // app-icon gradient so the OG image visually shares DNA with the icon.
    let inset = w * 0.045
    let body = CGRect(x: inset, y: inset, width: w - 2*inset, height: h - 2*inset)
    let radius = body.height * 0.07
    let path = CGMutablePath()
    path.addRoundedRect(in: body, cornerWidth: radius, cornerHeight: radius)

    ctx.saveGState()
    ctx.addPath(path)
    ctx.clip()
    let topColor = CGColor(red: 0.18, green: 0.20, blue: 0.24, alpha: 1.0)
    let botColor = CGColor(red: 0.10, green: 0.12, blue: 0.14, alpha: 1.0)
    let gradient = CGGradient(
        colorsSpace: cs,
        colors: [topColor, botColor] as CFArray,
        locations: [0, 1]
    )!
    ctx.drawLinearGradient(
        gradient,
        start: CGPoint(x: 0, y: h),
        end: CGPoint(x: 0, y: 0),
        options: []
    )
    ctx.restoreGState()

    // Subtle border so the bezel reads as a card on light social previews.
    ctx.saveGState()
    ctx.addPath(path)
    ctx.setStrokeColor(CGColor(red: 0.28, green: 0.31, blue: 0.36, alpha: 1.0))
    ctx.setLineWidth(1.5)
    ctx.strokePath()
    ctx.restoreGState()

    // Wordmark + tagline rendered through Cocoa text APIs.
    let nsCtx = NSGraphicsContext(cgContext: ctx, flipped: false)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = nsCtx

    let accent = NSColor(red: 0.85, green: 0.45, blue: 0.20, alpha: 1.0)
    let textColor = NSColor(red: 0.95, green: 0.96, blue: 0.97, alpha: 1.0)
    let mutedColor = NSColor(red: 0.62, green: 0.66, blue: 0.72, alpha: 1.0)

    // Wordmark: `> tmnl` in a chunky monospace.
    let wordmarkSize: CGFloat = 168
    let wordmarkFont = NSFont.monospacedSystemFont(ofSize: wordmarkSize, weight: .bold)
    let wordmark = NSMutableAttributedString()
    let para = NSMutableParagraphStyle()
    para.alignment = .center
    wordmark.append(NSAttributedString(string: "> ", attributes: [
        .font: wordmarkFont,
        .foregroundColor: accent,
        .paragraphStyle: para,
        .kern: -wordmarkSize * 0.04,
    ]))
    wordmark.append(NSAttributedString(string: "tmnl", attributes: [
        .font: wordmarkFont,
        .foregroundColor: textColor,
        .paragraphStyle: para,
        .kern: -wordmarkSize * 0.04,
    ]))

    let wordmarkSizeRect = wordmark.size()
    // Push the wordmark up so the tagline sits comfortably below center.
    let wordmarkRect = CGRect(
        x: (w - wordmarkSizeRect.width) / 2,
        y: h * 0.50,
        width: wordmarkSizeRect.width,
        height: wordmarkSizeRect.height
    )
    wordmark.draw(in: wordmarkRect)

    // Tagline.
    let taglineSize: CGFloat = 38
    let taglineFont = NSFont.systemFont(ofSize: taglineSize, weight: .regular)
    let tagline = NSAttributedString(
        string: "A GPU terminal with a structured-cell display surface.",
        attributes: [
            .font: taglineFont,
            .foregroundColor: mutedColor,
            .paragraphStyle: para,
            .kern: -0.4,
        ])
    let taglineSizeRect = tagline.size()
    let taglineRect = CGRect(
        x: (w - taglineSizeRect.width) / 2,
        y: h * 0.32,
        width: taglineSizeRect.width,
        height: taglineSizeRect.height
    )
    tagline.draw(in: taglineRect)

    // Footer URL — lower-right, tiny, in accent. Confirms domain at a glance.
    let urlSize: CGFloat = 26
    let urlFont = NSFont.monospacedSystemFont(ofSize: urlSize, weight: .medium)
    let url = NSAttributedString(string: "tmnl.sh", attributes: [
        .font: urlFont,
        .foregroundColor: accent,
        .kern: -0.2,
    ])
    let urlSizeRect = url.size()
    let urlRect = CGRect(
        x: body.maxX - urlSizeRect.width - 32,
        y: body.minY + 26,
        width: urlSizeRect.width,
        height: urlSizeRect.height
    )
    url.draw(in: urlRect)

    NSGraphicsContext.restoreGraphicsState()

    guard let cg = ctx.makeImage() else { return nil }
    let rep = NSBitmapImageRep(cgImage: cg)
    return rep.representation(using: .png, properties: [:])
}

guard let data = render() else {
    FileHandle.standardError.write("render failed\n".data(using: .utf8)!)
    exit(1)
}
try data.write(to: outURL)
print("wrote \(outURL.path) (\(width)x\(height))")

// Mirror into public/ when invoked with the default path so the meta tag
// URL resolves. Only fire when the caller didn't pass an explicit path —
// a custom path is "I know what I'm doing, don't touch public/".
if CommandLine.arguments.count <= 1 {
    let mirror = URL(fileURLWithPath: "site/public/og/hero.png")
    try? FileManager.default.createDirectory(
        at: mirror.deletingLastPathComponent(),
        withIntermediateDirectories: true)
    try data.write(to: mirror)
    print("mirrored to \(mirror.path)")
}
