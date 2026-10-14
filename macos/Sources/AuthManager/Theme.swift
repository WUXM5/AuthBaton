import AppKit
import SwiftUI

// 回环: one brand colour, tool colours only inside small icons, amber only where something needs a hand.
// Every token has a light and a dark value; dark lifts the brand by one step.
enum Palette {
    // 无垠青 #0897B8. solid carries white text (buttons, switches, the in-use seal), deep is brand text on a
    // tint, soft is tracks and bench seals, wash is tiles and chips.
    static let brand = dynamic(0x0897B8, 0x46B1CA)
    static let solid = dynamic(0x07809C, 0x07809C)
    static let deep = dynamic(0x066A81, 0x7DCBDD)
    static let soft = dynamic(0xCEEAF1, 0x1D4A57)
    static let wash = dynamic(0xEBF7F9, 0x15323B)

    static let ink = dynamic(0x15171C, 0xF1F2F4)
    static let ink2 = dynamic(0x484C55, 0xC3C7CE)
    static let ink3 = dynamic(0x687079, 0x8D939C)
    static let disabled = dynamic(0x9AA0A9, 0x5D626A)
    static let ring = dynamic(0x15171C, 0xFFFFFF, alpha: (0.06, 0.08))
    static let edge = dynamic(0x15171C, 0xFFFFFF, alpha: (0.11, 0.16))
    static let divider = dynamic(0xEEF0F3, 0x2B2D32)
    static let dash = dynamic(0xD5D9DF, 0x3E4148)
    static let dashStrong = dynamic(0xA3A9B3, 0x5D626A)
    static let rail = dynamic(0xECEEF1, 0x18191C)
    static let railEdge = dynamic(0xDDE0E5, 0x26282D)
    static let ground = dynamic(0xF4F5F7, 0x141518)
    static let card = dynamic(0xFFFFFF, 0x1E2024)
    static let chip = dynamic(0xF1F2F5, 0x2A2C31)
    static let panel = dynamic(0xF6F7F9, 0x25272C)
    static let field = dynamic(0xF4F5F7, 0x2A2C31)
    static let hover = dynamic(0xF8F9FA, 0x23252A)

    static let attention = dynamic(0xB56200, 0xF0A632)
    static let attentionText = dynamic(0x8F4C00, 0xF4B860)
    static let attentionFill = dynamic(0xFBEBD3, 0x3B2C15)
    static let attentionWash = dynamic(0xFDF4E6, 0x2A2217)
    static let attentionInk = dynamic(0x6B4A1C, 0xE6CB9C)
    static let attentionDot = dynamic(0xE58A00, 0xF0A632)
    static let attentionDash = dynamic(0xE3A33E, 0xC98A2C)
    static let danger = dynamic(0xB3261E, 0xFF7B72)

    static let codex = dynamic(0x5E62E8, 0x8D90F6)
    static let codexTile = dynamic(0xF1F2F8, 0x26283F)
    static let claude = dynamic(0xD9774F, 0xE38A66)
    static let claudeTile = dynamic(0xFBF1EC, 0x3A2A23)

    private static func dynamic(_ light: UInt32, _ dark: UInt32, alpha: (CGFloat, CGFloat) = (1, 1)) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            let hex = isDark ? dark : light
            return NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                           blue: CGFloat(hex & 0xFF) / 255, alpha: isDark ? alpha.1 : alpha.0)
        })
    }
}

// Normal states stay uncoloured; only what needs a hand turns amber.
enum Tone {
    case neutral, active, attention
    var color: Color {
        switch self {
        case .neutral: Palette.ink3
        case .active: Palette.deep
        case .attention: Palette.attentionText
        }
    }
}

// MARK: Surfaces

extension View {
    // Hero cards are raised; everything else sits flat with a hairline.
    // The shadow belongs to the card's shape; on the whole view SwiftUI would give every child its own.
    func loopCard(radius: CGFloat = 15, raised: Bool = false) -> some View {
        self.background {
            RoundedRectangle(cornerRadius: radius, style: .continuous).fill(Palette.card)
                .shadow(color: .black.opacity(raised ? 0.06 : 0), radius: raised ? 12 : 0, y: raised ? 9 : 0)
        }
        .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
    }

    func loopPanel(radius: CGFloat = 12) -> some View {
        self.background(Palette.panel, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
    }

    // The page column: centred, at most 960 wide.
    func pageColumn() -> some View {
        self.padding(.horizontal, 27).padding(.top, 26).padding(.bottom, 41)
            .frame(maxWidth: 816, alignment: .leading)
            .frame(maxWidth: .infinity)
    }
}

struct PageHeader<Trailing: View>: View {
    let title: String
    var subtitle: Text? = nil
    var provider: String? = nil
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .bottom, spacing: 14) {
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 9) {
                    if let provider {
                        GlyphTile(provider: provider, size: 27)
                            .overlay(RoundedRectangle(cornerRadius: 7.5, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
                    }
                    Text(title).font(.system(size: 22, weight: .bold)).kerning(-0.26).foregroundStyle(Palette.ink)
                }
                if let subtitle {
                    subtitle.font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 14)
            HStack(spacing: 7) { trailing }
        }
    }
}

extension PageHeader where Trailing == EmptyView {
    init(title: String, subtitle: Text? = nil, provider: String? = nil) {
        self.init(title: title, subtitle: subtitle, provider: provider) { EmptyView() }
    }
}

// Every page: a header, then cards in one centred column.
struct PageScaffold<Trailing: View, Content: View>: View {
    let title: String
    var subtitle: Text? = nil
    var provider: String? = nil
    var spacing: CGFloat = 17
    @ViewBuilder var trailing: Trailing
    @ViewBuilder var content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: spacing) {
                PageHeader(title: title, subtitle: subtitle, provider: provider) { trailing }
                content
            }
            .pageColumn()
        }
    }
}

extension PageScaffold where Trailing == EmptyView {
    init(title: String, subtitle: Text? = nil, provider: String? = nil, spacing: CGFloat = 17, @ViewBuilder content: () -> Content) {
        self.init(title: title, subtitle: subtitle, provider: provider, spacing: spacing, trailing: { EmptyView() }, content: content)
    }
}

// MARK: Controls

struct PillButtonStyle: ButtonStyle {
    // danger is red text for a destructive link; destructive is the outlined confirm button in a sheet.
    enum Kind { case primary, secondary, quiet, link, onAttention, danger, destructive }
    var kind: Kind = .secondary
    var height: CGFloat = 24

    func makeBody(configuration: Configuration) -> some View {
        PillButtonBody(configuration: configuration, kind: kind, height: height)
    }
}

private struct PillButtonBody: View {
    @Environment(\.isEnabled) private var enabled
    let configuration: ButtonStyleConfiguration
    let kind: PillButtonStyle.Kind
    let height: CGFloat

    var body: some View {
        configuration.label
            .font(.system(size: height >= 27 ? 12 : 11, weight: kind == .quiet ? .regular : .semibold))
            .foregroundStyle(foreground)
            .lineLimit(1)
            .padding(.horizontal, kind == .link || kind == .danger ? 0 : height >= 27 ? 14 : height >= 24 ? 10 : 9)
            .frame(height: kind == .link || kind == .danger ? nil : height)
            .background(background, in: Capsule())
            .overlay { if kind == .secondary || kind == .destructive { Capsule().strokeBorder(Palette.edge, lineWidth: 1) } }
            .contentShape(Capsule())
            .opacity(configuration.isPressed ? 0.72 : 1)
    }

    private var foreground: Color {
        guard enabled else { return kind == .primary ? .white.opacity(0.9) : Palette.disabled }
        switch kind {
        case .primary: return .white
        case .secondary: return Palette.ink
        case .quiet: return Palette.ink2
        case .link: return Palette.solid
        case .onAttention: return Palette.attentionText
        case .danger, .destructive: return Palette.danger
        }
    }

    private var background: Color {
        switch kind {
        case .primary: enabled ? Palette.solid : Palette.solid.opacity(0.35)
        case .secondary, .onAttention, .destructive: Palette.card
        case .quiet: Palette.field
        case .link, .danger: .clear
        }
    }
}

extension ButtonStyle where Self == PillButtonStyle {
    static func pill(_ kind: PillButtonStyle.Kind = .secondary, height: CGFloat = 24) -> PillButtonStyle {
        PillButtonStyle(kind: kind, height: height)
    }
}

// Rule chips (第 1 位 · ≤ 3% · 先换号), state chips (在用) and their dashed form for what does not take part.
struct Pill: View {
    enum Style { case neutral, brand, attention, dashed }
    let text: String
    var style: Style = .neutral
    var small = false

    init(_ text: String, style: Style = .neutral, small: Bool = false) {
        self.text = text
        self.style = style
        self.small = small
    }

    var body: some View {
        Text(text)
            .font(.system(size: small ? 10 : 11, weight: style == .brand || style == .attention ? .semibold : .regular))
            .foregroundStyle(foreground)
            .monospacedDigit()
            .lineLimit(1)
            .padding(.horizontal, 7)
            .frame(height: small ? 17 : 19)
            .background(background, in: Capsule())
            .overlay { if style == .dashed { Capsule().strokeBorder(Palette.dashStrong.opacity(0.7), style: StrokeStyle(lineWidth: 1, dash: [3, 2])) } }
    }

    private var foreground: Color {
        switch style {
        case .neutral: Palette.ink2
        case .brand: Palette.deep
        case .attention: Palette.attentionText
        case .dashed: Palette.ink3
        }
    }

    private var background: Color {
        switch style {
        case .neutral: Palette.chip
        case .brand: Palette.wash
        case .attention: Palette.attentionFill
        case .dashed: .clear
        }
    }
}

// The header's refresh control says how old the numbers are.
struct RefreshPill: View {
    let checkedAt: Double?
    let busy: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Icon(.refresh, size: 12, color: Palette.ink2)
                Text(busy ? "刷新中" : checkedAt.map { When.text($0) } ?? "刷新")
            }
        }
        .buttonStyle(.pill(.secondary, height: 27))
        .help("额度每 5 分钟自动查询；点一下立即刷新")
        .disabled(busy)
    }
}

// A segmented choice drawn as pills on a rail.
struct PillSegments<Value: Hashable>: View {
    struct Segment: Identifiable {
        let value: Value
        let title: String
        var glyph: Glyph? = nil
        var id: String { title }
    }
    let segments: [Segment]
    let selection: Value
    let select: (Value) -> Void
    var fill = false

    var body: some View {
        HStack(spacing: 0) {
            ForEach(segments) { segment in
                let on = segment.value == selection
                Button { select(segment.value) } label: {
                    HStack(spacing: 5) {
                        if let glyph = segment.glyph { Icon(glyph, size: 13) }
                        Text(segment.title)
                    }
                    .font(.system(size: 12, weight: on ? .semibold : .regular))
                    .foregroundStyle(on ? Palette.ink : Palette.ink2)
                    .padding(.horizontal, 15)
                    .frame(maxWidth: fill ? .infinity : nil)
                    .frame(height: 24)
                    .background {
                        if on { Capsule().fill(Palette.card).shadow(color: .black.opacity(0.12), radius: 1, y: 1) }
                    }
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(3)
        .background(Palette.rail, in: Capsule())
    }
}

// A text field on the field tint, as in the login, rename and cloud forms.
struct LoopField: View {
    let placeholder: String
    @Binding var text: String
    var secure = false

    var body: some View {
        Group {
            if secure { SecureField(placeholder, text: $text) } else { TextField(placeholder, text: $text) }
        }
        .textFieldStyle(.plain)
        .font(.system(size: 12))
        .padding(.horizontal, 10)
        .frame(height: 31)
        .background(Palette.field, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
    }
}

// MARK: Problems in place

// A problem on a card sits in a strip along its bottom edge, with the one thing that fixes it.
struct IssueStrip<Actions: View>: View {
    var glyph: Glyph = .alert
    var busy = false
    let text: String
    var tone: Tone = .attention
    @ViewBuilder var actions: Actions

    var body: some View {
        HStack(spacing: 9) {
            if busy {
                ProgressView().controlSize(.small).frame(width: 14, height: 14)
            } else {
                Icon(glyph, size: 14, color: tone == .attention ? Palette.attention : Palette.ink3)
            }
            Text(text).font(.system(size: 11)).foregroundStyle(Palette.ink2)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            actions
        }
        .padding(.vertical, 10)
        .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
    }
}

extension IssueStrip where Actions == EmptyView {
    init(glyph: Glyph = .alert, busy: Bool = false, text: String, tone: Tone = .attention) {
        self.init(glyph: glyph, busy: busy, text: text, tone: tone) { EmptyView() }
    }
}

// A problem that belongs to no single card: an amber band at the top of the page.
struct Banner<Actions: View>: View {
    var glyph: Glyph = .alert
    var title: String? = nil
    let text: String
    @ViewBuilder var actions: Actions

    var body: some View {
        HStack(alignment: title == nil ? .center : .top, spacing: 9) {
            Icon(glyph, size: 14, color: Palette.attention).padding(.top, title == nil ? 0 : 1)
            VStack(alignment: .leading, spacing: 2) {
                if let title { Text(title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.attentionText) }
                Text(text).font(.system(size: 11)).foregroundStyle(Palette.attentionInk).fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 5) { actions }
        }
        .padding(.horizontal, 12).padding(.vertical, 9)
        .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

extension Banner where Actions == EmptyView {
    init(glyph: Glyph = .alert, title: String? = nil, text: String) {
        self.init(glyph: glyph, title: title, text: text) { EmptyView() }
    }
}

// MARK: Sheets

// Title, one line on what happens, the content, then the buttons on the right.
struct SheetScaffold<Content: View, Actions: View>: View {
    let title: String
    var lead: String? = nil
    var width: CGFloat = 408
    @ViewBuilder var content: Content
    @ViewBuilder var actions: Actions

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 5) {
                Text(title).font(.system(size: 14, weight: .bold)).foregroundStyle(Palette.ink)
                    .fixedSize(horizontal: false, vertical: true)
                if let lead {
                    Text(lead).font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                }
            }
            content
            HStack(spacing: 9) {
                Spacer(minLength: 0)
                actions
            }
            .padding(.top, 3.5)
        }
        .padding(20)
        .frame(width: width)
        .background(Palette.card)
    }
}

// One "what happens / what you do" line in a confirmation.
struct CheckItem: View {
    enum Kind { case system, you, info }
    let kind: Kind
    let text: String
    var glyph: Glyph? = nil

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Icon(glyph ?? (kind == .system ? .check : kind == .you ? .alert : .info), size: 14,
                 color: kind == .system ? Palette.solid : kind == .you ? Palette.attention : Palette.ink3)
                .padding(.top, 1)
            Text(text).font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: Time

// Short, human-readable times; the exact value stays available as a tooltip.
enum When {
    static func text(_ date: Date, now: Date = Date()) -> String {
        let seconds = date.timeIntervalSince(now)
        if abs(seconds) < 60 { return "刚刚" }
        if seconds < 0, seconds > -3600 { return "\(Int(-seconds / 60)) 分钟前" }
        if seconds > 0, seconds < 3600 { return "\(Int(seconds / 60)) 分钟后" }
        let calendar = Calendar.current
        let parts = calendar.dateComponents([.year, .month, .day, .hour, .minute], from: date)
        let time = String(format: "%02d:%02d", parts.hour ?? 0, parts.minute ?? 0)
        if calendar.isDateInToday(date) { return "今天 \(time)" }
        if calendar.isDateInYesterday(date) { return "昨天 \(time)" }
        if calendar.isDateInTomorrow(date) { return "明天 \(time)" }
        let day = "\(parts.month ?? 0)月\(parts.day ?? 0)日 \(time)"
        return calendar.isDate(date, equalTo: now, toGranularity: .year) ? day : "\(parts.year ?? 0)年\(day)"
    }

    static func text(_ timestamp: Double) -> String { text(Date(timeIntervalSince1970: timestamp)) }

    // "3 小时后" style countdown; nil once the moment has passed.
    static func countdown(to date: Date, now: Date = Date()) -> String? {
        let seconds = date.timeIntervalSince(now)
        guard seconds > 0 else { return nil }
        if seconds < 60 { return "1 分钟内" }
        if seconds < 3600 { return "\(Int(seconds / 60)) 分钟后" }
        if seconds < 86400 { return "\(Int(seconds / 3600)) 小时后" }
        let days = Int(seconds / 86400)
        let hours = Int(seconds.truncatingRemainder(dividingBy: 86400) / 3600)
        return hours > 0 && days < 3 ? "\(days) 天 \(hours) 小时后" : "\(days) 天后"
    }

    // Month and day only, for expiry dates.
    static func day(_ date: Date) -> String {
        let parts = Calendar.current.dateComponents([.month, .day], from: date)
        return "\(parts.month ?? 0)月\(parts.day ?? 0)日"
    }

    static func exact(_ date: Date) -> String { date.formatted(date: .abbreviated, time: .shortened) }
    static func exact(_ timestamp: Double) -> String { exact(Date(timeIntervalSince1970: timestamp)) }
}
