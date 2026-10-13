import AppKit
import SwiftUI

// Colors follow the system appearance; every token has a light and a dark value.
enum Palette {
    static let canvas = dynamic(0xF4F4F6, 0x161618)
    static let card = dynamic(0xFFFFFF, 0x1F1F23)
    static let raised = dynamic(0xF7F7F9, 0x27272C)
    static let hairline = dynamic(0x000000, 0xFFFFFF, alpha: (0.08, 0.09))
    static let track = dynamic(0x000000, 0xFFFFFF, alpha: (0.07, 0.10))
    static let accent = dynamic(0x5B5BD6, 0x8B8DF5)
    static let success = dynamic(0x1E8A4C, 0x4CC38A)
    static let warning = dynamic(0xB25E00, 0xF0A632)
    static let danger = dynamic(0xCE2C31, 0xFF6B6B)
    static let muted = Color(nsColor: .secondaryLabelColor)
    static let codex = dynamic(0x24242A, 0x4A4A54)
    static let claude = dynamic(0xD97757, 0xD97757)

    private static func dynamic(_ light: UInt32, _ dark: UInt32, alpha: (CGFloat, CGFloat) = (1, 1)) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            let hex = isDark ? dark : light
            return NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                           blue: CGFloat(hex & 0xFF) / 255, alpha: isDark ? alpha.1 : alpha.0)
        })
    }
}

enum Tone {
    case neutral, accent, success, warning, danger
    var color: Color {
        switch self {
        case .neutral: Palette.muted
        case .accent: Palette.accent
        case .success: Palette.success
        case .warning: Palette.warning
        case .danger: Palette.danger
        }
    }
}

extension View {
    func card(padding: CGFloat = 18, highlighted: Bool = false) -> some View {
        self.padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Palette.card, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
                .strokeBorder(highlighted ? Palette.accent.opacity(0.5) : Palette.hairline, lineWidth: highlighted ? 1.5 : 1))
            .shadow(color: .black.opacity(0.04), radius: 3, y: 1)
    }

    func panel(padding: CGFloat = 14) -> some View {
        self.padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Palette.raised, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }
}

struct PageHeader<Trailing: View>: View {
    let title: String
    let subtitle: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .bottom, spacing: 16) {
            VStack(alignment: .leading, spacing: 5) {
                Text(title).font(.system(size: 26, weight: .bold))
                Text(subtitle).font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 16)
            trailing
        }
    }
}

extension PageHeader where Trailing == EmptyView {
    init(title: String, subtitle: String) {
        self.init(title: title, subtitle: subtitle) { EmptyView() }
    }
}

struct StatusPill: View {
    let text: String
    let tone: Tone

    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(tone.color).frame(width: 6, height: 6)
            Text(text)
        }
        .font(.system(size: 12, weight: .medium))
        .foregroundStyle(tone.color)
        .padding(.horizontal, 9).padding(.vertical, 4)
        .background(tone.color.opacity(0.12), in: Capsule())
    }
}

struct Tag: View {
    let text: String
    var tone: Tone = .neutral
    var filled = false

    var body: some View {
        Text(text)
            .font(.system(size: 10.5, weight: .semibold))
            .padding(.horizontal, 6).padding(.vertical, 2)
            .foregroundStyle(filled ? Color.white : tone.color)
            .background(filled ? tone.color : tone.color.opacity(0.13), in: RoundedRectangle(cornerRadius: 4, style: .continuous))
    }
}

struct Callout<Actions: View>: View {
    let icon: String
    let tone: Tone
    var title: String? = nil
    let message: String
    @ViewBuilder var actions: Actions

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(tone.color)
                .frame(width: 18)
                .padding(.top, 1)
            VStack(alignment: .leading, spacing: 6) {
                if let title { Text(title).font(.callout.weight(.semibold)) }
                Text(message).font(.callout)
                    .foregroundStyle(title == nil ? Color.primary : Palette.muted)
                    .fixedSize(horizontal: false, vertical: true)
                actions
            }
            Spacer(minLength: 0)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tone.color.opacity(0.08), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(tone.color.opacity(0.22)))
    }
}

extension Callout where Actions == EmptyView {
    init(icon: String, tone: Tone, title: String? = nil, message: String) {
        self.init(icon: icon, tone: tone, title: title, message: message) { EmptyView() }
    }
}

struct ProviderGlyph: View {
    let provider: String
    var size: CGFloat = 22

    var body: some View {
        Group {
            if provider == "pi" {
                Text("π").font(.system(size: size * 0.58, weight: .bold, design: .serif))
            } else {
                Image(systemName: provider == "codex" ? "terminal.fill" : "sparkle")
                    .font(.system(size: size * 0.46, weight: .bold))
            }
        }
        .foregroundStyle(.white)
        .frame(width: size, height: size)
        .background(tint.gradient, in: RoundedRectangle(cornerRadius: size * 0.28, style: .continuous))
        .accessibilityHidden(true)
    }

    private var tint: Color {
        switch provider {
        case "codex": Palette.codex
        case "claude": Palette.claude
        default: Palette.accent
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

// Quota bar: the filled part is what remains, colored by how little is left.
struct Meter: View {
    let remaining: Double

    static func tone(_ remaining: Double) -> Tone {
        remaining <= 10 ? .danger : remaining <= 30 ? .warning : .accent
    }

    var body: some View {
        GeometryReader { geometry in
            let fraction = min(1, max(0, remaining / 100))
            ZStack(alignment: .leading) {
                Capsule().fill(Palette.track)
                Capsule().fill(Self.tone(remaining).color)
                    .frame(width: fraction > 0 ? max(6, geometry.size.width * fraction) : 0)
            }
        }
        .frame(height: 6)
        .accessibilityHidden(true)
    }
}

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

    static func exact(_ date: Date) -> String { date.formatted(date: .abbreviated, time: .shortened) }
    static func exact(_ timestamp: Double) -> String { exact(Date(timeIntervalSince1970: timestamp)) }
}

// MARK: Workbench components

// Top bar of a page: title, one-line status, trailing controls.
// Page title and actions live in the window toolbar, so no strip is left empty above the page.
extension View {
    func pageBar<Actions: View>(_ title: String, subtitle: String? = nil, @ViewBuilder actions: () -> Actions) -> some View {
        navigationTitle(title)
            .navigationSubtitle(subtitle ?? "")
            .toolbar { ToolbarItemGroup(placement: .primaryAction) { actions() } }
    }

    func pageBar(_ title: String, subtitle: String? = nil) -> some View {
        pageBar(title, subtitle: subtitle) { EmptyView() }
    }
}

struct SectionLabel<Trailing: View>: View {
    let title: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack {
            Text(title).font(.system(size: 11, weight: .semibold)).foregroundStyle(.tertiary)
            Spacer()
            trailing.font(.caption).buttonStyle(.link)
        }
        .padding(.bottom, 6)
    }
}

extension SectionLabel where Trailing == EmptyView {
    init(_ title: String) { self.init(title: title) { EmptyView() } }
}

struct Dot: View {
    let tone: Tone
    var size: CGFloat = 7
    var body: some View { Circle().fill(tone.color).frame(width: size, height: size) }
}

struct HealthLabel: View {
    let health: Health
    var body: some View {
        HStack(spacing: 6) {
            Dot(tone: health.tone)
            Text(health.title).lineLimit(1)
        }
        .font(.system(size: 12, weight: .medium))
        .foregroundStyle(health.tone == .neutral ? Palette.muted : health.tone.color)
    }
}

// One "what happens / what you do" line in a confirmation sheet.
struct CheckItem: View {
    enum Kind { case system, you, info }
    let kind: Kind
    let text: String

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: kind == .system ? "checkmark.circle" : kind == .you ? "exclamationmark.triangle" : "info.circle")
                .foregroundStyle(kind == .system ? Palette.success : kind == .you ? Palette.warning : Palette.muted)
                .frame(width: 16)
            Text(text).fixedSize(horizontal: false, vertical: true)
        }
        .font(.callout)
    }
}

// Standard sheet layout: title, lead, content, trailing buttons.
struct SheetScaffold<Content: View, Actions: View>: View {
    let title: String
    var lead: String? = nil
    var width: CGFloat = 480
    @ViewBuilder var content: Content
    @ViewBuilder var actions: Actions

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.system(size: 16, weight: .semibold))
                if let lead {
                    Text(lead).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
            content
            HStack(spacing: 8) {
                Spacer()
                actions
            }
            .padding(.top, 4)
        }
        .padding(22)
        .frame(width: width)
    }
}

// Compact label · bar · percent · reset row used in tables and the menu bar panel.
struct QuotaLine: View {
    let quota: AccountQuota
    var labelWidth: CGFloat = 34
    var showReset = true

    var body: some View {
        let tone = Meter.tone(Double(quota.remaining))
        HStack(spacing: 6) {
            Text(quota.shortLabel).font(.system(size: 11)).foregroundStyle(.secondary)
                .frame(width: labelWidth, alignment: .leading).lineLimit(1)
            Meter(remaining: Double(quota.remaining)).frame(minWidth: 40)
            Text("\(quota.remaining)%").font(.system(size: 12, weight: .semibold)).monospacedDigit()
                .foregroundStyle(tone == .accent ? Color.primary : tone.color)
                .frame(width: 38, alignment: .trailing)
            if showReset {
                Text(quota.shortReset).font(.system(size: 11)).foregroundStyle(.secondary).monospacedDigit()
                    .frame(width: 58, alignment: .leading).lineLimit(1)
            }
        }
        .accessibilityElement(children: .combine)
    }
}

struct SwitchRow<Detail: View>: View {
    let title: String
    var subtitle: String? = nil
    @Binding var isOn: Bool
    @ViewBuilder var detail: Detail

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.callout)
                if let subtitle { Text(subtitle).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
            }
            Spacer(minLength: 8)
            detail
            Toggle(title, isOn: $isOn).labelsHidden().toggleStyle(.switch).controlSize(.small)
        }
    }
}

extension SwitchRow where Detail == EmptyView {
    init(title: String, subtitle: String? = nil, isOn: Binding<Bool>) {
        self.init(title: title, subtitle: subtitle, isOn: isOn) { EmptyView() }
    }
}
