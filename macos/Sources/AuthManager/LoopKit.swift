import SwiftUI

// The signature pieces of the 回环 design language: line icons and the brand mark drawn from the
// design's SVG data, seals, quota lines with a 见底线 notch, ring gauges, tickets and relay lines.

// MARK: SVG path data

// Reads the subset of SVG path data the design uses: M L H V C A Z, absolute and relative,
// with implicit repeats (a moveto followed by more pairs continues as lineto).
enum SVGPath {
    static func parse(_ data: String) -> Path {
        var path = Path()
        let tokens = tokenize(data)
        var index = 0
        var command: Character = "M"
        var current = CGPoint.zero
        var start = CGPoint.zero
        func numbers(_ count: Int) -> [CGFloat]? {
            guard index + count <= tokens.count else { return nil }
            var values: [CGFloat] = []
            for token in tokens[index..<index + count] {
                guard case .number(let value) = token else { return nil }
                values.append(value)
            }
            index += count
            return values
        }
        while index < tokens.count {
            if case .command(let letter) = tokens[index] {
                command = letter
                index += 1
                if letter == "Z" || letter == "z" {
                    path.closeSubpath()
                    current = start
                    continue
                }
            }
            let relative = command.isLowercase
            let base = relative ? current : .zero
            switch command.uppercased() {
            case "M":
                guard let v = numbers(2) else { return path }
                current = CGPoint(x: base.x + v[0], y: base.y + v[1])
                start = current
                path.move(to: current)
                command = relative ? "l" : "L"
            case "L":
                guard let v = numbers(2) else { return path }
                current = CGPoint(x: base.x + v[0], y: base.y + v[1])
                path.addLine(to: current)
            case "H":
                guard let v = numbers(1) else { return path }
                current = CGPoint(x: (relative ? current.x : 0) + v[0], y: current.y)
                path.addLine(to: current)
            case "V":
                guard let v = numbers(1) else { return path }
                current = CGPoint(x: current.x, y: (relative ? current.y : 0) + v[0])
                path.addLine(to: current)
            case "C":
                guard let v = numbers(6) else { return path }
                let end = CGPoint(x: base.x + v[4], y: base.y + v[5])
                path.addCurve(to: end, control1: CGPoint(x: base.x + v[0], y: base.y + v[1]),
                              control2: CGPoint(x: base.x + v[2], y: base.y + v[3]))
                current = end
            case "A":
                guard let v = numbers(7) else { return path }
                let end = CGPoint(x: base.x + v[5], y: base.y + v[6])
                arc(&path, from: current, to: end, rx: v[0], ry: v[1], rotation: v[2], large: v[3] != 0, sweep: v[4] != 0)
                current = end
            default:
                index += 1
            }
        }
        return path
    }

    private enum Token { case command(Character), number(CGFloat) }

    private static func tokenize(_ data: String) -> [Token] {
        var tokens: [Token] = []
        let chars = Array(data)
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if c.isLetter && c != "e" && c != "E" {
                tokens.append(.command(c))
                i += 1
            } else if c.isNumber || c == "-" || c == "+" || c == "." {
                var text = String(c)
                var dot = c == "."
                i += 1
                while i < chars.count {
                    let d = chars[i]
                    if d.isNumber { text.append(d) }
                    else if d == "." && !dot { dot = true; text.append(d) }
                    else if (d == "e" || d == "E") && i + 1 < chars.count {
                        text.append(d)
                        if chars[i + 1] == "-" || chars[i + 1] == "+" { i += 1; text.append(chars[i]) }
                    } else { break }
                    i += 1
                }
                if let value = Double(text) { tokens.append(.number(CGFloat(value))) }
            } else {
                i += 1
            }
        }
        return tokens
    }

    // SVG endpoint arcs to a centre arc (SVG 1.1 F.6.5). Path angles grow towards +y, as in SVG,
    // so the sweep flag maps straight onto the sign of the delta.
    private static func arc(_ path: inout Path, from p1: CGPoint, to p2: CGPoint, rx: CGFloat, ry: CGFloat,
                            rotation: CGFloat, large: Bool, sweep: Bool) {
        var rx = abs(rx), ry = abs(ry)
        guard rx > 0, ry > 0, p1 != p2 else { path.addLine(to: p2); return }
        let phi = rotation * .pi / 180
        let c = cos(phi), s = sin(phi)
        let dx = (p1.x - p2.x) / 2, dy = (p1.y - p2.y) / 2
        let x1 = c * dx + s * dy, y1 = -s * dx + c * dy
        let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry)
        if lambda > 1 { rx *= sqrt(lambda); ry *= sqrt(lambda) }
        let numerator = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1
        let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1
        var coefficient = sqrt(max(0, numerator / denominator))
        if large == sweep { coefficient = -coefficient }
        let cx1 = coefficient * rx * y1 / ry, cy1 = -coefficient * ry * x1 / rx
        let center = CGPoint(x: c * cx1 - s * cy1 + (p1.x + p2.x) / 2, y: s * cx1 + c * cy1 + (p1.y + p2.y) / 2)
        func angle(_ ux: CGFloat, _ uy: CGFloat, _ vx: CGFloat, _ vy: CGFloat) -> CGFloat {
            atan2(ux * vy - uy * vx, ux * vx + uy * vy)
        }
        let ux = (x1 - cx1) / rx, uy = (y1 - cy1) / ry
        let vx = (-x1 - cx1) / rx, vy = (-y1 - cy1) / ry
        let begin = angle(1, 0, ux, uy)
        var delta = angle(ux, uy, vx, vy)
        if !sweep && delta > 0 { delta -= 2 * .pi } else if sweep && delta < 0 { delta += 2 * .pi }
        let transform = CGAffineTransform(translationX: center.x, y: center.y).rotated(by: phi).scaledBy(x: rx, y: ry)
        path.addRelativeArc(center: .zero, radius: 1, startAngle: .radians(begin), delta: .radians(delta), transform: transform)
    }
}

// MARK: Line icons

// The design's 20 × 20 line icons. Strokes scale with the drawn size, like the SVG originals.
enum Glyph: Sendable {
    case accounts, relay, desktop, clients, cloud, refresh, plus, sun, shield, sync, info, clock, alert
    case check, chevronDown, chevronRight, external, server, mac, window, codex, claude

    fileprivate var data: [String] {
        switch self {
        case .accounts: [circle(7.5, 6.5, 3), "M2 16.5c.5-3 2.6-4.8 5.5-4.8s5 1.8 5.5 4.8", "M13 3.8a2.7 2.7 0 0 1 0 5.4", "M15 11.9c1.7.5 2.8 2 3.1 4.6"]
        case .relay: ["M3 7h13M12.5 3.5L16 7l-3.5 3.5", "M17 13H4M7.5 9.5L4 13l3.5 3.5"]
        case .desktop: [rect(2, 3.5, 16, 11, 2.5), "M7 17.5h6M10 14.5v3"]
        case .clients: [rect(3, 3, 5.5, 5.5, 1.5), rect(11.5, 3, 5.5, 5.5, 1.5), rect(3, 11.5, 5.5, 5.5, 1.5), rect(11.5, 11.5, 5.5, 5.5, 1.5)]
        case .cloud: ["M5.8 15.5h8.7a3.6 3.6 0 0 0 .4-7.2 5.2 5.2 0 0 0-10-.2 3.7 3.7 0 0 0 .9 7.4z"]
        case .refresh: ["M16.5 10a6.5 6.5 0 1 1-2-4.7", "M15 2.5v3.6h-3.6"]
        case .plus: ["M10 4v12M4 10h12"]
        case .sun: [circle(10, 11, 3.4), "M10 3.2v1.6M3.6 11H2M18 11h-1.6M5.1 6.1 4 5M14.9 6.1 16 5M3 16.5h14"]
        case .shield: ["M10 2.5l6 2.3v4.6c0 3.9-2.6 6.8-6 8.1-3.4-1.3-6-4.2-6-8.1V4.8z", "M7.4 10.1l1.8 1.8 3.5-3.6"]
        case .sync: ["M3.8 9a6.3 6.3 0 0 1 10.9-3.4L16 7", "M16 3.4V7h-3.6", "M16.2 11a6.3 6.3 0 0 1-10.9 3.4L4 13", "M4 16.6V13h3.6"]
        case .info: [circle(10, 10, 7.5), "M10 9v5M10 6.2v.1"]
        case .clock: [circle(10, 10, 7.2), "M10 6v4.2l2.8 1.8"]
        case .alert: [circle(10, 10, 7.5), "M10 6v4.6M10 13.6v.1"]
        case .check: ["M4 10.5l3.6 3.6L16 5.8"]
        case .chevronDown: ["M6 8l4 4 4-4"]
        case .chevronRight: ["M8 5.5l4.5 4.5L8 14.5"]
        case .external: ["M8 4H4.5A1.5 1.5 0 0 0 3 5.5v10A1.5 1.5 0 0 0 4.5 17h10a1.5 1.5 0 0 0 1.5-1.5V12M11 3h6v6M17 3l-8 8"]
        case .server: [rect(3, 3, 14, 5.5, 1.8), rect(3, 11.5, 14, 5.5, 1.8), "M6 5.75h.1M6 14.25h.1"]
        case .mac: [rect(3, 4, 14, 9.5, 1.8), "M1.5 16h17"]
        case .window: [rect(2.5, 3.5, 15, 13, 3), "M7.5 3.5v13"]
        case .codex: ["M5.6 15.2h8.6a3.4 3.4 0 0 0 .3-6.8 4.7 4.7 0 0 0-9.1-.6 3.7 3.7 0 0 0 .2 7.4z", "M7.7 10.3l1.6 1.3-1.6 1.3M10.8 13h2.2"]
        case .claude: ["M10 3v14M3 10h14M5.05 5.05l9.9 9.9M14.95 5.05l-9.9 9.9"]
        }
    }

    fileprivate var weight: CGFloat {
        switch self {
        case .claude: 1.9
        case .plus, .check: 1.9
        case .alert, .refresh: 1.7
        default: 1.6
        }
    }

    var tint: Color? {
        switch self {
        case .codex: Palette.codex
        case .claude: Palette.claude
        default: nil
        }
    }

    static func provider(_ id: String) -> Glyph { id == "claude" ? .claude : .codex }

    private func circle(_ cx: CGFloat, _ cy: CGFloat, _ r: CGFloat) -> String {
        "M\(cx - r) \(cy)a\(r) \(r) 0 1 0 \(2 * r) 0a\(r) \(r) 0 1 0 \(-2 * r) 0z"
    }

    private func rect(_ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat, _ r: CGFloat) -> String {
        "M\(x + r) \(y)h\(w - 2 * r)a\(r) \(r) 0 0 1 \(r) \(r)v\(h - 2 * r)a\(r) \(r) 0 0 1 \(-r) \(r)h\(2 * r - w)a\(r) \(r) 0 0 1 \(-r) \(-r)v\(2 * r - h)a\(r) \(r) 0 0 1 \(r) \(-r)z"
    }
}

private struct GlyphShape: Shape {
    let glyph: Glyph
    func path(in rect: CGRect) -> Path {
        var path = Path()
        for part in glyph.data { path.addPath(SVGPath.parse(part)) }
        let scale = min(rect.width, rect.height) / 20
        return path.applying(CGAffineTransform(translationX: rect.minX, y: rect.minY).scaledBy(x: scale, y: scale))
    }
}

struct Icon: View {
    let glyph: Glyph
    var size: CGFloat = 15
    var color: Color? = nil
    var weight: CGFloat? = nil

    init(_ glyph: Glyph, size: CGFloat = 15, color: Color? = nil, weight: CGFloat? = nil) {
        self.glyph = glyph
        self.size = size
        self.color = color
        self.weight = weight
    }

    var body: some View {
        GlyphShape(glyph: glyph)
            .stroke(color ?? glyph.tint ?? Palette.ink2,
                    style: StrokeStyle(lineWidth: (weight ?? glyph.weight) * size / 20, lineCap: .round, lineJoin: .round))
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

// Three dots for "更多操作".
struct MoreDots: View {
    var color: Color = Palette.ink3
    var body: some View {
        HStack(spacing: 2.2) {
            ForEach(0..<3, id: \.self) { _ in Circle().fill(color).frame(width: 2.8, height: 2.8) }
        }
        .frame(width: 14, height: 14)
        .accessibilityHidden(true)
    }
}

// MARK: Brand mark

// The 无垠 loop traced from Resources/BrandMark.png into a 100 × 100 outline, so it takes any colour.
private enum MarkData {
    static let outline = "M49 7.8 45.5 8.2 41 9.4 37.3 10.9 33.4 13.3 30.9 15.2 16 30.1 13.5 33.2 11.5 36.1 9.4 40.4 7.8 45.9 7.6 52.7 8.4 56.6 9.4 59.4 12.9 65.6 15 68.4 19.1 72.5 21.3 74.2 32.2 85.5 36.3 88.5 40.4 90.4 45.7 91.8 51.6 92 55.3 91.4 59.2 90.2 62.5 88.7 67 85.5 76 76.4 80.3 72.9 84.6 68.6 88.3 63.5 90.6 58.8 92 54.1 92.2 47.5 91.8 44.9 90.6 41 89.1 37.5 86.7 33.8 83.4 29.7 69.1 15.4 63.9 11.7 58.8 9.4 54.3 8.2ZM48.6 15.8 52.9 16 56.8 17 60.9 18.9 63.7 20.9 73 30.5 78.5 35.5 80.5 37.9 82.8 42 83.8 44.7 84.4 47.7 84.4 51.8 83.4 55.9 81.6 59.6 78.9 63.3 72.9 68.9 72.5 68.8 72.5 69.1 71.5 70.1 68.9 71.9 67 72.9 66.4 72.9 66 73.2 63.3 73.8 58.8 73.6 55.5 72.7 52.7 71.3 49.6 69.1 46.3 66.4 37.5 58.2 35 55.3 33.8 53.1 33.2 50.8 33.4 47.7 34.6 44.7 35.9 42.8 42 36.7 44.1 35.2 46.3 34.2 49.4 33.6 52.1 33.8 54.1 34.4 57.4 36.3 63.9 42.8 65.6 45.5 66.4 48 66.4 51.6 65.2 54.5 63.3 56.8 63.7 57 63.7 58.8 64.3 60.2 67.6 63.5 70.1 60.9 72.7 57.2 73.6 54.7 74.2 52.1 74.2 47.9 73.8 45.7 72.3 41.8 70.3 38.9 61.1 29.7 57 27.3 53.9 26.4 51.4 26 48.6 26 45.9 26.4 43.4 27.1 40.8 28.3 37.3 30.9 30.5 37.7 27.7 41.4 26.4 44.3 25.6 47.5 25.6 52.3 26.8 56.4 28.5 59.6 30.9 62.3 41.4 72.7 45.5 76 48.2 77.7 52.5 79.7 55.3 80.5 60 81.1 58.8 82 56.6 83 51.8 84.2 47.5 84.2 43.8 83.4 40.6 82 37.5 79.9 30.9 72.9 24.2 66.6 19.9 62.1 18.2 59.6 16.6 56.4 15.4 51.8 15.6 46.3 16.6 43 18.9 38.5 21.5 35.4 37.1 20.1 38.9 18.9 43 17 44.9 16.4Z"
    // The short segment where the inner loop crosses under the outer one, drawn half transparent.
    static let crossing = "M63.1 57 59.4 60.7 58.6 62.9 58.8 64.6 59.4 65.8 65.8 72.5 67 72.7 68.9 71.7 71.5 69.9 72.3 69.1 72.3 68.6 64.1 60.2 63.5 58.8 63.5 57Z"
}

private struct MarkShape: Shape {
    static let outline = SVGPath.parse(MarkData.outline)
    static let crossing = SVGPath.parse(MarkData.crossing)
    let path: Path
    func path(in rect: CGRect) -> Path {
        let scale = min(rect.width, rect.height) / 100
        return path.applying(CGAffineTransform(translationX: rect.minX, y: rect.minY).scaledBy(x: scale, y: scale))
    }
}

struct BrandMark: View {
    var color: Color = .white
    var body: some View {
        ZStack {
            MarkShape(path: MarkShape.outline).fill(color, style: FillStyle(eoFill: true))
            MarkShape(path: MarkShape.crossing).fill(color.opacity(0.5))
        }
        .aspectRatio(1, contentMode: .fit)
        .accessibilityLabel("BatonPass · 无垠")
    }
}

// The mark in a brand-coloured tile, as in the sidebar, the menu bar and the desktop page.
struct BrandTile: View {
    var size: CGFloat = 26
    var body: some View {
        BrandMark()
            .frame(width: size * 0.72, height: size * 0.72)
            .frame(width: size, height: size)
            .background(Palette.brand, in: RoundedRectangle(cornerRadius: size * 0.27, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: size * 0.27, style: .continuous).strokeBorder(.black.opacity(0.12), lineWidth: 0.5))
    }
}

// MARK: Tool tiles

// A tool's glyph in its own light tile; the tool colour stays inside the tile.
struct GlyphTile: View {
    let provider: String
    var size: CGFloat = 26

    var body: some View {
        Group {
            if provider == "pi" {
                Text("π").font(.system(size: size * 0.56, weight: .semibold, design: .serif)).foregroundStyle(Palette.ink2)
            } else {
                Icon(.provider(provider), size: size * 0.6)
            }
        }
        .frame(width: size, height: size)
        .background(tile, in: RoundedRectangle(cornerRadius: size * 0.29, style: .continuous))
        .accessibilityHidden(true)
    }

    private var tile: Color {
        switch provider {
        case "codex": Palette.codexTile
        case "claude": Palette.claudeTile
        default: Palette.chip
        }
    }

    static func name(_ provider: String) -> String {
        switch provider {
        case "codex": "Codex"
        case "claude": "Claude Code"
        default: "pi"
        }
    }
}

// MARK: Seal

// An account's monogram on a square turned 45°: solid while in use, soft on the bench,
// a dashed outline when it does not take part in switching.
struct Seal: View {
    enum Style { case inUse, standby, out }
    let letter: String
    var style: Style = .standby
    var size: CGFloat = 26

    init(_ name: String, style: Style = .standby, size: CGFloat = 26) {
        self.letter = String(name.trimmingCharacters(in: .whitespaces).first ?? "?").uppercased()
        self.style = style
        self.size = size
    }

    var body: some View {
        let side = size * 0.72
        let shape = RoundedRectangle(cornerRadius: side * 0.3, style: .continuous)
        ZStack {
            switch style {
            case .inUse: shape.fill(Palette.solid).frame(width: side, height: side).rotationEffect(.degrees(45))
            case .standby: shape.fill(Palette.soft).frame(width: side, height: side).rotationEffect(.degrees(45))
            case .out:
                shape.strokeBorder(Palette.dashStrong, style: StrokeStyle(lineWidth: 1.5, dash: [3, 2.5]))
                    .frame(width: side, height: side).rotationEffect(.degrees(45))
            }
            Text(letter)
                .font(.system(size: min(14, max(7, size * 0.4)), weight: .bold, design: .rounded))
                .foregroundStyle(style == .inUse ? Color.white : style == .out ? Palette.ink3 : Palette.ink)
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

// MARK: Quota

// What remains of a window. The notch marks the account's 见底线; at or under it the line turns amber.
struct QuotaBar: View {
    let remaining: Int
    var below: Int? = nil
    var height: CGFloat = 7

    var body: some View {
        let low = below.map { remaining <= $0 } ?? false
        GeometryReader { geometry in
            let width = geometry.size.width
            let fraction = CGFloat(min(100, max(0, remaining))) / 100
            ZStack(alignment: .leading) {
                Capsule().fill(low ? Palette.attentionFill : Palette.soft)
                Capsule().fill(low ? Palette.attentionDot : Palette.brand)
                    .frame(width: fraction > 0 ? max(height, width * fraction) : 0)
                if let below {
                    RoundedRectangle(cornerRadius: 1).fill(Palette.ink.opacity(0.55))
                        .frame(width: 2, height: height * 2)
                        .offset(x: width * CGFloat(min(100, max(0, below))) / 100 - 1)
                        .help("见底线：剩余 \(below)%")
                }
            }
        }
        .frame(height: height)
        .accessibilityElement()
        .accessibilityLabel(below.map { "剩余 \(remaining)%，见底线 \($0)%" } ?? "剩余 \(remaining)%")
    }
}

// The big rounded number with its percent sign.
struct Figure: View {
    let value: Int?
    var size: CGFloat = 49

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 2) {
            Text(value.map(String.init) ?? "—").font(.system(size: size, weight: .semibold, design: .rounded)).monospacedDigit()
                .kerning(-size * 0.02)
            if value != nil {
                Text("%").font(.system(size: size * 0.42, weight: .semibold, design: .rounded)).foregroundStyle(Palette.ink3)
            }
        }
        .foregroundStyle(Palette.ink)
        .lineLimit(1)
        .fixedSize()
    }
}

// One ring per window; with two, the outer one is the first window and the centre shows the lower value.
struct Ring: View {
    let values: [Int]
    var size: CGFloat = 65

    var body: some View {
        let scale = size / 76
        ZStack {
            if values.count >= 2 {
                arc(values[0], radius: 32, width: 6, color: Palette.brand)
                arc(values[1], radius: 23, width: 6, color: Palette.solid)
            } else {
                arc(values.first ?? 0, radius: 31, width: 7, color: Palette.brand)
            }
            HStack(alignment: .firstTextBaseline, spacing: 1) {
                Text(values.min().map(String.init) ?? "—")
                    .font(.system(size: (values.count >= 2 ? 17 : 22) * scale, weight: .semibold, design: .rounded)).monospacedDigit()
                Text("%").font(.system(size: (values.count >= 2 ? 10 : 11) * scale, weight: .semibold, design: .rounded))
                    .foregroundStyle(Palette.ink3)
            }
        }
        .frame(width: size, height: size)
    }

    private func arc(_ value: Int, radius: CGFloat, width: CGFloat, color: Color) -> some View {
        let scale = size / 76
        let fraction = CGFloat(min(100, max(0, value))) / 100
        return ZStack {
            Circle().stroke(Palette.soft, lineWidth: width * scale)
            Circle().trim(from: 0, to: fraction).stroke(color, style: StrokeStyle(lineWidth: width * scale, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .frame(width: radius * 2 * scale, height: radius * 2 * scale)
    }
}

// MARK: Ticket

// A reset chance: a chip with a half circle cut from each end.
struct TicketShape: Shape {
    var radius: CGFloat = 5
    var notch: CGFloat = 3.5

    func path(in r: CGRect) -> Path {
        var p = Path()
        let mid = r.midY
        p.move(to: CGPoint(x: r.minX + radius, y: r.minY))
        p.addLine(to: CGPoint(x: r.maxX - radius, y: r.minY))
        p.addArc(tangent1End: CGPoint(x: r.maxX, y: r.minY), tangent2End: CGPoint(x: r.maxX, y: r.minY + radius), radius: radius)
        p.addLine(to: CGPoint(x: r.maxX, y: mid - notch))
        p.addRelativeArc(center: CGPoint(x: r.maxX, y: mid), radius: notch, startAngle: .degrees(-90), delta: .degrees(-180))
        p.addLine(to: CGPoint(x: r.maxX, y: r.maxY - radius))
        p.addArc(tangent1End: CGPoint(x: r.maxX, y: r.maxY), tangent2End: CGPoint(x: r.maxX - radius, y: r.maxY), radius: radius)
        p.addLine(to: CGPoint(x: r.minX + radius, y: r.maxY))
        p.addArc(tangent1End: CGPoint(x: r.minX, y: r.maxY), tangent2End: CGPoint(x: r.minX, y: r.maxY - radius), radius: radius)
        p.addLine(to: CGPoint(x: r.minX, y: mid + notch))
        p.addRelativeArc(center: CGPoint(x: r.minX, y: mid), radius: notch, startAngle: .degrees(90), delta: .degrees(-180))
        p.addLine(to: CGPoint(x: r.minX, y: r.minY + radius))
        p.addArc(tangent1End: CGPoint(x: r.minX, y: r.minY), tangent2End: CGPoint(x: r.minX + radius, y: r.minY), radius: radius)
        p.closeSubpath()
        return p
    }
}

struct Ticket: View {
    let text: String
    var soon = false
    var large = false

    var body: some View {
        Text(text)
            .font(.system(size: large ? 12 : 11, weight: .semibold))
            .foregroundStyle(soon ? Palette.attentionText : large ? Palette.ink : Palette.ink2)
            .monospacedDigit()
            .lineLimit(1)
            .padding(.horizontal, large ? 11 : 9)
            .frame(height: large ? 24 : 20)
            .background(soon ? Palette.attentionFill : Palette.chip, in: TicketShape(radius: large ? 6 : 5, notch: large ? 4.5 : 3.5))
    }
}

// MARK: Relay line

// Between two stations: solid brand where a switch can happen, dashed amber where it cannot right now,
// dashed grey where a side does not take part.
struct RelayLine: View {
    enum Style { case open, blocked, idle }
    let style: Style
    var thickness: CGFloat = 2
    var arrow = true

    var body: some View {
        HStack(spacing: 0) {
            GeometryReader { geometry in
                Path { p in
                    p.move(to: CGPoint(x: 0, y: geometry.size.height / 2))
                    p.addLine(to: CGPoint(x: geometry.size.width, y: geometry.size.height / 2))
                }
                .stroke(color, style: StrokeStyle(lineWidth: thickness, lineCap: style == .open ? .round : .butt,
                                                  dash: style == .open ? [] : [thickness * 2.5, thickness * 2]))
            }
            .frame(height: thickness)
            if arrow && style != .idle {
                Icon(.chevronRight, size: 10, color: color, weight: 2.4).padding(.leading, -4)
            }
        }
        .accessibilityHidden(true)
    }

    private var color: Color {
        switch style {
        case .open: Palette.brand
        case .blocked: Palette.attentionDash
        case .idle: Palette.dash
        }
    }
}
