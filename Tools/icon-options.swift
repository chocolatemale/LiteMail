// Renders four hand-drawn icon directions side by side for comparison.
// Usage: swift Tools/icon-options.swift <out-dir>
import AppKit

let cs = CGColorSpace(name: CGColorSpace.sRGB)!
func rgb(_ hex: UInt32, _ a: CGFloat = 1) -> CGColor {
    CGColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
            blue: CGFloat(hex & 0xff) / 255, alpha: a)
}
func poly(_ pts: [CGPoint]) -> CGPath { let p = CGMutablePath(); p.addLines(between: pts); p.closeSubpath(); return p }
func P(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: y) }

func canvas(_ draw: (CGContext) -> Void) -> CGImage {
    let ctx = CGContext(data: nil, width: 1024, height: 1024, bitsPerComponent: 8, bytesPerRow: 0, space: cs,
                        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    draw(ctx)
    return ctx.makeImage()!
}

let body = CGRect(x: 100, y: 100, width: 824, height: 824)
let squircle = CGPath(roundedRect: body, cornerWidth: 185, cornerHeight: 185, transform: nil)

func linear(_ c: CGContext, _ colors: [CGColor], _ a: CGPoint, _ b: CGPoint, _ locs: [CGFloat]? = nil) {
    let g = CGGradient(colorsSpace: cs, colors: colors as CFArray, locations: locs)!
    c.drawLinearGradient(g, start: a, end: b, options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
}
func radial(_ c: CGContext, _ colors: [CGColor], _ center: CGPoint, _ r: CGFloat) {
    let g = CGGradient(colorsSpace: cs, colors: colors as CFArray, locations: nil)!
    c.drawRadialGradient(g, startCenter: center, startRadius: 0, endCenter: center, endRadius: r, options: [])
}
func fill(_ c: CGContext, _ path: CGPath, _ colors: [CGColor], _ a: CGPoint, _ b: CGPoint) {
    c.saveGState(); c.addPath(path); c.clip(); linear(c, colors, a, b); c.restoreGState()
}
func tile(_ c: CGContext, base: UInt32, _ inside: (CGContext) -> Void) {
    c.saveGState()
    c.setShadow(offset: CGSize(width: 0, height: -10), blur: 24, color: rgb(0x2a1206, 0.32))
    c.addPath(squircle); c.setFillColor(rgb(base)); c.fillPath()
    c.restoreGState()
    c.saveGState(); c.addPath(squircle); c.clip()
    inside(c)
    c.restoreGState()
    c.addPath(squircle); c.setStrokeColor(rgb(0xffffff, 0.18)); c.setLineWidth(2); c.strokePath()
}
func shadowed(_ c: CGContext, blur: CGFloat, dy: CGFloat, _ color: CGColor, _ path: CGPath) {
    c.saveGState(); c.setShadow(offset: CGSize(width: 0, height: dy), blur: blur, color: color)
    c.addPath(path); c.setFillColor(rgb(0xf5efe5)); c.fillPath(); c.restoreGState()
}

// 1. Envelope sunrise
let sunrise = canvas { c in
    tile(c, base: 0xd97757) { c in
        linear(c, [rgb(0xf6b48f), rgb(0xe48a66), rgb(0xc65f3e)], P(512, 924), P(512, 100), [0, 0.5, 1])
        // sun rising behind the envelope's top edge
        radial(c, [rgb(0xfff2dc, 0.55), rgb(0xfff2dc, 0)], P(512, 560), 330)
        c.setFillColor(rgb(0xfdecd2)); c.fillEllipse(in: CGRect(x: 512 - 150, y: 560 - 150, width: 300, height: 300))
        // envelope
        let env = CGPath(roundedRect: CGRect(x: 196, y: 214, width: 632, height: 360), cornerWidth: 46, cornerHeight: 46, transform: nil)
        shadowed(c, blur: 40, dy: -18, rgb(0x5a2210, 0.4), env)
        c.saveGState(); c.addPath(env); c.clip()
        linear(c, [rgb(0xfffcf6), rgb(0xf2eadc)], P(512, 574), P(512, 214))
        // side and bottom folds
        fill(c, poly([P(196, 214), P(512, 430), P(828, 214)]), [rgb(0xf3ebde), rgb(0xe9dece)], P(512, 430), P(512, 214))
        fill(c, poly([P(196, 574), P(512, 352), P(828, 574)]), [rgb(0xfffefb), rgb(0xf4ecdf)], P(512, 574), P(512, 352))
        c.setStrokeColor(rgb(0xc9b59c, 0.55)); c.setLineWidth(4); c.setLineJoin(.round)
        c.move(to: P(196, 574)); c.addLine(to: P(512, 352)); c.addLine(to: P(828, 574)); c.strokePath()
        c.restoreGState()
    }
}

// 2. Wax-sealed envelope
let seal = canvas { c in
    tile(c, base: 0xd97757) { c in
        linear(c, [rgb(0xeb9472), rgb(0xd97757), rgb(0xbd5a3c)], P(300, 924), P(724, 100))
        radial(c, [rgb(0xffe6d2, 0.22), rgb(0xffe6d2, 0)], P(340, 800), 520)
        let env = CGPath(roundedRect: CGRect(x: 190, y: 268, width: 644, height: 452), cornerWidth: 50, cornerHeight: 50, transform: nil)
        shadowed(c, blur: 44, dy: -22, rgb(0x5a2210, 0.45), env)
        c.saveGState(); c.addPath(env); c.clip()
        linear(c, [rgb(0xfffcf6), rgb(0xf1e8da)], P(512, 720), P(512, 268))
        fill(c, poly([P(190, 268), P(512, 500), P(834, 268)]), [rgb(0xf6efe4), rgb(0xebe1d2)], P(512, 500), P(512, 268))
        c.saveGState()
        c.setShadow(offset: CGSize(width: 0, height: -8), blur: 14, color: rgb(0x6a3a1a, 0.28))
        c.addPath(poly([P(190, 720), P(512, 452), P(834, 720)])); c.setFillColor(rgb(0xfbf6ee)); c.fillPath()
        c.restoreGState()
        c.restoreGState()
        // wax seal at the flap tip
        let center = P(512, 452)
        c.saveGState(); c.setShadow(offset: CGSize(width: 0, height: -8), blur: 16, color: rgb(0x5a1e0c, 0.5))
        c.setFillColor(rgb(0xc4603f)); c.fillEllipse(in: CGRect(x: center.x - 92, y: center.y - 92, width: 184, height: 184)); c.restoreGState()
        c.saveGState(); c.addEllipse(in: CGRect(x: center.x - 92, y: center.y - 92, width: 184, height: 184)); c.clip()
        radial(c, [rgb(0xf09a76), rgb(0xd46f4d), rgb(0xb4523a)], P(center.x - 26, center.y + 30), 150); c.restoreGState()
        c.setStrokeColor(rgb(0x9e4430, 0.45)); c.setLineWidth(6)
        c.strokeEllipse(in: CGRect(x: center.x - 62, y: center.y - 62, width: 124, height: 124))
        c.setStrokeColor(rgb(0xffd9c4, 0.35)); c.setLineWidth(3)
        c.strokeEllipse(in: CGRect(x: center.x - 58, y: center.y - 56, width: 116, height: 116))
        // simple embossed mark: a small heart
        let h = CGMutablePath()
        h.move(to: P(center.x, center.y - 30))
        h.addCurve(to: P(center.x - 36, center.y + 10), control1: P(center.x - 20, center.y - 14), control2: P(center.x - 40, center.y - 6))
        h.addCurve(to: P(center.x, center.y + 18), control1: P(center.x - 32, center.y + 36), control2: P(center.x - 6, center.y + 34))
        h.addCurve(to: P(center.x + 36, center.y + 10), control1: P(center.x + 6, center.y + 34), control2: P(center.x + 32, center.y + 36))
        h.addCurve(to: P(center.x, center.y - 30), control1: P(center.x + 40, center.y - 6), control2: P(center.x + 20, center.y - 14))
        c.addPath(h); c.setFillColor(rgb(0xa94a32, 0.55)); c.fillPath()
    }
}

// 3. Terracotta plane on ivory
let plane = canvas { c in
    tile(c, base: 0xf4efe6) { c in
        linear(c, [rgb(0xfffdf9), rgb(0xf3ede2), rgb(0xe9e1d3)], P(512, 924), P(512, 100), [0, 0.6, 1])
        let trail: [(CGFloat, CGFloat, CGFloat)] = [(170, 230, 10), (208, 276, 11.5), (252, 318, 13), (302, 354, 14.5), (356, 384, 16)]
        for (x, y, r) in trail { c.setFillColor(rgb(0xd97757, 0.35 + (r - 10) * 0.06)); c.fillEllipse(in: CGRect(x: x - r, y: y - r, width: r * 2, height: r * 2)) }
        let tip = P(800, 752), tail = P(206, 546), crease = P(456, 446), keel = P(428, 236), fold = P(400, 398)
        shadowed(c, blur: 36, dy: -20, rgb(0x7a3a1c, 0.35), poly([tip, tail, crease, keel]))
        fill(c, poly([tip, tail, crease]), [rgb(0xee9877), rgb(0xe07f5d)], P(300, 600), P(760, 700))
        fill(c, poly([tip, crease, keel]), [rgb(0xcf6c4b), rgb(0xb9583a)], P(720, 700), P(460, 280))
        fill(c, poly([crease, fold, keel]), [rgb(0x9e4630), rgb(0xae5136)], P(400, 400), P(460, 270))
        c.setStrokeColor(rgb(0xffe2d2, 0.7)); c.setLineWidth(3); c.setLineCap(.round)
        c.move(to: tip); c.addLine(to: crease); c.strokePath()
    }
}

// 4. Stamped letter
let stamp = canvas { c in
    tile(c, base: 0xd97757) { c in
        linear(c, [rgb(0xeb9472), rgb(0xd97757), rgb(0xbd5a3c)], P(300, 924), P(724, 100))
        c.saveGState()
        c.translateBy(x: 512, y: 500); c.rotate(by: 6 * .pi / 180); c.translateBy(x: -512, y: -500)
        let env = CGPath(roundedRect: CGRect(x: 186, y: 296, width: 652, height: 420), cornerWidth: 34, cornerHeight: 34, transform: nil)
        shadowed(c, blur: 42, dy: -22, rgb(0x5a2210, 0.45), env)
        c.saveGState(); c.addPath(env); c.clip()
        linear(c, [rgb(0xfffcf6), rgb(0xf3ebdf)], P(512, 716), P(512, 296))
        // address lines
        c.setFillColor(rgb(0xd9c7b1))
        for (i, w) in [CGFloat(250), 300, 210].enumerated() {
            c.addPath(CGPath(roundedRect: CGRect(x: 262, y: 420 - CGFloat(i) * 46, width: w, height: 18), cornerWidth: 9, cornerHeight: 9, transform: nil)); c.fillPath()
        }
        c.restoreGState()
        // stamp with perforated edge
        let st = CGRect(x: 622, y: 520, width: 160, height: 150)
        c.saveGState(); c.setShadow(offset: CGSize(width: 0, height: -4), blur: 8, color: rgb(0x6a3a1a, 0.3))
        c.setFillColor(rgb(0xfffaf3)); c.fill(st); c.restoreGState()
        c.setFillColor(rgb(0xf6efe4))
        let r: CGFloat = 9
        for i in 0...7 { let x = st.minX + 4 + CGFloat(i) * 21.7; c.fillEllipse(in: CGRect(x: x - r, y: st.maxY - r, width: 2 * r, height: 2 * r)); c.fillEllipse(in: CGRect(x: x - r, y: st.minY - r, width: 2 * r, height: 2 * r)) }
        for i in 0...6 { let y = st.minY + 6 + CGFloat(i) * 23; c.fillEllipse(in: CGRect(x: st.minX - r, y: y - r, width: 2 * r, height: 2 * r)); c.fillEllipse(in: CGRect(x: st.maxX - r, y: y - r, width: 2 * r, height: 2 * r)) }
        let inner = st.insetBy(dx: 22, dy: 22)
        c.saveGState(); c.addRect(inner); c.clip()
        linear(c, [rgb(0xe8906d), rgb(0xc9643f)], P(inner.midX, inner.maxY), P(inner.midX, inner.minY))
        c.setFillColor(rgb(0xfde7cf)); c.fillEllipse(in: CGRect(x: inner.midX - 26, y: inner.minY + 24, width: 52, height: 52))
        c.restoreGState()
        // wavy postmark
        c.setStrokeColor(rgb(0x8a5a40, 0.4)); c.setLineWidth(6); c.setLineCap(.round)
        for k in 0..<3 {
            let y0 = 600 - CGFloat(k) * 34
            c.move(to: P(470, y0))
            for s in 0..<4 { let x = 470 + CGFloat(s) * 50; c.addCurve(to: P(x + 50, y0), control1: P(x + 16, y0 + 18), control2: P(x + 34, y0 - 18)) }
            c.strokePath()
        }
        c.restoreGState()
    }
}

let dir = CommandLine.arguments.dropFirst().first ?? "."
let items: [(String, CGImage)] = [("1-sunrise", sunrise), ("2-seal", seal), ("3-plane", plane), ("4-stamp", stamp)]
for (n, im) in items {
    try! NSBitmapImageRep(cgImage: im).representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "\(dir)/\(n).png"))
}
// comparison sheet: big icons on light, small sizes on light and dark
let W = 4 * 300 + 40, H = 560
let sheet = CGContext(data: nil, width: W, height: H, bitsPerComponent: 8, bytesPerRow: 0, space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
sheet.setFillColor(rgb(0xe9e6df)); sheet.fill(CGRect(x: 0, y: 160, width: W, height: H - 160))
sheet.setFillColor(rgb(0x1f1f21)); sheet.fill(CGRect(x: 0, y: 0, width: W, height: 160))
sheet.interpolationQuality = .high
for (i, (_, im)) in items.enumerated() {
    let x = CGFloat(20 + i * 300)
    sheet.draw(im, in: CGRect(x: x + 10, y: 230, width: 280, height: 280))
    sheet.draw(im, in: CGRect(x: x + 40, y: 172, width: 48, height: 48))
    sheet.draw(im, in: CGRect(x: x + 110, y: 180, width: 32, height: 32))
    sheet.draw(im, in: CGRect(x: x + 170, y: 188, width: 16, height: 16))
    sheet.draw(im, in: CGRect(x: x + 40, y: 50, width: 64, height: 64))
    sheet.draw(im, in: CGRect(x: x + 130, y: 66, width: 32, height: 32))
    sheet.draw(im, in: CGRect(x: x + 190, y: 74, width: 16, height: 16))
}
try! NSBitmapImageRep(cgImage: sheet.makeImage()!).representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "\(dir)/sheet.png"))
print("ok")
