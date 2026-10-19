// Draws the LiteMail app icon (1024×1024 PNG): a stamped ivory letter on a warm
// terracotta squircle. Usage: swift Tools/make-icon.swift out.png
import AppKit

let cs = CGColorSpace(name: CGColorSpace.sRGB)!
func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255, alpha: a)
}
func P(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: y) }

let c = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8, bytesPerRow: 0, space: cs,
                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

func linear(_ colors: [CGColor], _ a: CGPoint, _ b: CGPoint, _ locs: [CGFloat]? = nil) {
    let g = CGGradient(colorsSpace: cs, colors: colors as CFArray, locations: locs)!
    c.drawLinearGradient(g, start: a, end: b, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
}
func radial(_ colors: [CGColor], _ center: CGPoint, _ r: CGFloat) {
    let g = CGGradient(colorsSpace: cs, colors: colors as CFArray, locations: nil)!
    c.drawRadialGradient(g, startCenter: center, startRadius: 0, endCenter: center, endRadius: r, options: [])
}
func rounded(_ r: CGRect, _ radius: CGFloat) -> CGPath {
    CGPath(roundedRect: r, cornerWidth: radius, cornerHeight: radius, transform: nil)
}

// Tile (macOS icon grid: 824 pt body, continuous corners).
let body = CGRect(x: 100, y: 100, width: 824, height: 824)
let squircle = rounded(body, 185)
c.saveGState()
c.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: rgb(0x2a1206, 0.32))
c.addPath(squircle); c.setFillColor(rgb(0xd97757)); c.fillPath()
c.restoreGState()

c.saveGState()
c.addPath(squircle); c.clip()
linear([rgb(0xee9a77), rgb(0xd97757), rgb(0xbb583a)], P(300, 924), P(724, 100))
radial([rgb(0xffe8d6, 0.24), rgb(0xffe8d6, 0)], P(320, 820), 560)

// Letter, tilted a little.
c.translateBy(x: 512, y: 506); c.rotate(by: 7 * .pi / 180); c.translateBy(x: -512, y: -506)
let letterRect = CGRect(x: 168, y: 288, width: 688, height: 436)
let letter = rounded(letterRect, 38)
c.saveGState()
c.setShadow(offset: CGSize(width: 0, height: -24), blur: 46, color: rgb(0x5a2210, 0.46))
c.addPath(letter); c.setFillColor(rgb(0xf7f1e7)); c.fillPath()
c.restoreGState()
c.saveGState()
c.addPath(letter); c.clip()
linear([rgb(0xfffdf9), rgb(0xf6efe4), rgb(0xede4d6)], P(512, letterRect.maxY), P(512, letterRect.minY), [0, 0.55, 1])
c.restoreGState()

// Address lines: bold enough to survive at 32 px.
c.setFillColor(rgb(0xd8c4ac))
for (i, w) in [CGFloat(268), 330, 230].enumerated() {
    c.addPath(rounded(CGRect(x: 238, y: 448 - CGFloat(i) * 54, width: w, height: 24), 12)); c.fillPath()
}

// Stamp with perforated edges.
let stamp = CGRect(x: 630, y: 516, width: 168, height: 160)
c.saveGState()
c.setShadow(offset: CGSize(width: 0, height: -5), blur: 10, color: rgb(0x6a3a1a, 0.28))
c.setFillColor(rgb(0xfffcf7)); c.fill(stamp)
c.restoreGState()
let hole: CGFloat = 8.5
c.setFillColor(rgb(0xf8f3ea))  // matches the paper under the stamp
func perforate(_ a: CGPoint, _ b: CGPoint) {
    let n = max(2, Int((hypot(b.x - a.x, b.y - a.y) / 24).rounded()))
    for i in 0...n {
        let t = CGFloat(i) / CGFloat(n)
        let p = P(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
        c.fillEllipse(in: CGRect(x: p.x - hole, y: p.y - hole, width: hole * 2, height: hole * 2))
    }
}
perforate(P(stamp.minX, stamp.maxY), P(stamp.maxX, stamp.maxY))
perforate(P(stamp.minX, stamp.minY), P(stamp.maxX, stamp.minY))
perforate(P(stamp.minX, stamp.minY), P(stamp.minX, stamp.maxY))
perforate(P(stamp.maxX, stamp.minY), P(stamp.maxX, stamp.maxY))

// Stamp artwork: a pale sun rising over a terracotta hill.
let art = stamp.insetBy(dx: 22, dy: 22)
c.saveGState()
c.addRect(art); c.clip()
linear([rgb(0xf3a682), rgb(0xdc7652)], P(art.midX, art.maxY), P(art.midX, art.minY))
c.setFillColor(rgb(0xfde9d2))
c.fillEllipse(in: CGRect(x: art.midX - 25, y: art.minY + 36, width: 50, height: 50))
c.setFillColor(rgb(0xb8573a))
c.fillEllipse(in: CGRect(x: art.minX - 40, y: art.minY - 74, width: art.width + 80, height: 116))
c.restoreGState()

// Postmark: a ring over the stamp's left edge, with wavy cancellation lines.
c.setStrokeColor(rgb(0x9a5a3e, 0.42)); c.setLineWidth(6); c.setLineCap(.round)
let ring = P(stamp.minX - 4, stamp.midY + 2)
c.strokeEllipse(in: CGRect(x: ring.x - 54, y: ring.y - 54, width: 108, height: 108))
for k in 0..<3 {
    let y0 = ring.y + 26 - CGFloat(k) * 26
    var x = ring.x - 190
    c.move(to: P(x, y0))
    for _ in 0..<3 {
        c.addCurve(to: P(x + 44, y0), control1: P(x + 14, y0 + 13), control2: P(x + 30, y0 - 13))
        x += 44
    }
    c.strokePath()
}

c.restoreGState()
c.addPath(squircle); c.setStrokeColor(rgb(0xffffff, 0.18)); c.setLineWidth(2); c.strokePath()

let out = CommandLine.arguments.dropFirst().first ?? "icon.png"
try! NSBitmapImageRep(cgImage: c.makeImage()!).representation(using: .png, properties: [:])!
    .write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
