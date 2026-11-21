import SwiftUI

// First-run guidance for the Codex and Claude pages, kept to two things: how this Mac uses the tool, and the first
// account. It shows only to someone whose group had no account when they first opened it; turning it down, or
// reaching two accounts with the relay on, ends it for good. Anyone who already had accounts never sees it.
enum Onboarding {
    static let codexKey = "onboarding.codex"
    static let claudeKey = "onboarding.claude"
    static let started = "started"
    static let done = "done"

    // The starter card replaces the empty page until the guide is finished or turned down.
    static func starting(_ state: String, empty: Bool) -> Bool { state != done && empty }

    // First sight decides: no account yet starts the guide, accounts already there mean it is not a first run.
    static func settle(_ state: String, empty: Bool, finished: Bool) -> String {
        if state.isEmpty { return empty ? started : done }
        return state == started && finished ? done : state
    }

    // Nothing left to point at: a spare account to switch to and the relay on.
    static func finished(_ directory: AccountDirectory, provider: String, count: Int? = nil) -> Bool {
        (count ?? directory.relay(provider).count) >= 2 && directory.strategy?.config.enabled == true
    }

    // The login this Mac already has, as 客户端与服务 reads it (~/.codex/auth.json, or Claude CLI signed in).
    @MainActor static func localLogin(_ service: ServiceController, provider: String) -> ProviderStatus? {
        service.snapshot?.providers.first { $0.id == provider && $0.state == "configured" }
    }
}

// After the first account: one line with the one next step, gone once dismissed.
struct OnboardingNext {
    enum Go { case add(String), relay }
    let text: String
    let action: String
    let go: Go

    static func codex(_ directory: AccountDirectory, accounts: [ManagedAccount]) -> OnboardingNext? {
        // A desktop band about the connection speaks first; never two bands at once.
        if let desktop = directory.desktop, DesktopWiring(desktop, current: directory.current("codex")).attention { return nil }
        // With hot switching off Codex never switches on its own, so there is no relay to send anyone to.
        if accounts.count >= 2, directory.desktop?.enabled != true { return nil }
        return next(names: accounts.map(\.displayName), provider: "codex", directory: directory)
    }

    static func claude(_ directory: AccountDirectory, names: [String]) -> OnboardingNext? {
        // Accounts only Claude 桌面 knows cannot relay: two of those are not yet a reason to open the rules.
        if names.count >= 2, directory.relay("claude").count < 2 { return nil }
        return next(names: names, provider: "claude", directory: directory)
    }

    private static func next(names: [String], provider: String, directory: AccountDirectory) -> OnboardingNext? {
        if names.count == 1 {
            return OnboardingNext(text: "已添加「\(names[0])」。再加一个备用号，就能一键换号或开启自动接力。", action: "添加备用号", go: .add(provider))
        }
        if names.count >= 2, directory.strategy?.config.enabled != true {
            return OnboardingNext(text: "已有 \(names.count) 个账号，想在额度见底时自动换号，可以去确认接力规则并开启。", action: "去换号与重置", go: .relay)
        }
        return nil
    }
}

struct NextStrip: View {
    let next: OnboardingNext
    let open: (AccountSheet) -> Void
    let dismiss: () -> Void

    var body: some View {
        HStack(spacing: 9) {
            BrandMark(color: Palette.brand).frame(width: 14, height: 14)
            Text(next.text).font(.system(size: 12)).foregroundStyle(Palette.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            Button(next.action) {
                switch next.go {
                case .add(let provider): open(.login(provider: provider, accountID: nil))
                case .relay: AppNavigation.shared.show(.automation)
                }
            }
            .buttonStyle(.pill(.primary, height: 24))
            Button(action: dismiss) {
                Icon(.close, size: 9, color: Palette.deep, weight: 1.8).frame(width: 22, height: 22).contentShape(Circle())
            }
            .buttonStyle(.plain)
            .help("关掉，不再提示")
            .accessibilityLabel("关闭提示，不再显示")
        }
        .padding(.leading, 12).padding(.trailing, 7).frame(minHeight: 41)
        .background(Palette.wash, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.brand.opacity(0.18), lineWidth: 1))
    }
}

// MARK: 起手卡

// Two numbered steps on one card, the brand-ringed numbers joined by a thin line.
private struct StarterCard<Steps: View>: View {
    let provider: String
    let title: String
    let lead: String
    let dismiss: () -> Void
    @ViewBuilder var steps: Steps

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 10) {
                Icon(.provider(provider), size: 17, color: Palette.brand)
                    .frame(width: 31, height: 31)
                    .background(Palette.wash, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 14, weight: .bold))
                    Text(lead).font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 10)
                Button("先不用", action: dismiss).buttonStyle(.plain).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    .help("收起引导，之后不再出现")
            }
            steps.padding(.top, 19)
            HStack(spacing: 6) {
                Icon(.relay, size: 12, color: Palette.brand)
                Text("加好两个以上账号后，可去「换号与重置」开启额度见底自动接力（默认关闭）。")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
            .padding(.top, 19)
        }
        .padding(.horizontal, 26).padding(.top, 24).padding(.bottom, 19)
        .frame(maxWidth: 527)
        .loopCard(radius: 19, raised: true)
        .frame(maxWidth: .infinity)
    }
}

private struct StarterStep<Content: View>: View {
    let number: Int
    let title: String
    var hint: String? = nil
    var last = false
    @ViewBuilder var content: Content

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(spacing: 0) {
                Text("\(number)").font(.system(size: 10, weight: .bold, design: .rounded)).foregroundStyle(.white)
                    .frame(width: 17, height: 17).background(Palette.solid, in: Circle())
                if !last { Rectangle().fill(Palette.soft).frame(width: 1).frame(maxHeight: .infinity).padding(.vertical, 4) }
            }
            .frame(width: 17)
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 7) {
                    Text(title).font(.system(size: 12, weight: .semibold))
                    if let hint { Text(hint).font(.system(size: 11)).foregroundStyle(Palette.deep) }
                }
                .frame(minHeight: 17)
                content.padding(.top, 9)
            }
            .padding(.bottom, last ? 0 : 19)
        }
    }
}

// The login this Mac has, offered as one click; importing keeps the default login as it is.
private struct LocalLoginRow: View {
    @EnvironmentObject private var service: ServiceController
    let provider: String
    let glyph: Glyph
    let title: String
    let detail: String
    var primary = true
    var before: () -> Void = {}

    var body: some View {
        HStack(spacing: 10) {
            Icon(glyph, size: 15, color: primary ? Palette.deep : Palette.ink2)
                .frame(width: 27, height: 27)
                .background(primary ? Palette.card : Palette.field, in: RoundedRectangle(cornerRadius: 7.5, style: .continuous))
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.system(size: 12, weight: .semibold))
                Text(detail).font(.system(size: 11)).foregroundStyle(Palette.ink2).lineLimit(2).fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button(service.accountBusy ? "正在导入…" : primary ? "导入本机登录" : "导入") {
                before()
                Task { await service.accountOperation("accounts.import", params: LoginParams(provider: provider)) }
            }
            .buttonStyle(.pill(primary ? .primary : .secondary, height: 26))
            .disabled(service.accountBusy || service.directory?.busy == true)
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(primary ? Palette.wash : Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(primary ? Palette.brand.opacity(0.18) : Palette.ring, lineWidth: 1))
    }
}

private struct NewLoginRow: View {
    let provider: String
    let prominent: Bool
    var note: String? = nil
    let open: (AccountSheet) -> Void

    var body: some View {
        HStack(spacing: 9) {
            Button("登录新账号") { open(.login(provider: provider, accountID: nil)) }
                .buttonStyle(.pill(prominent ? .primary : .secondary, height: 26))
            Text(note ?? (prominent ? "浏览器授权单独保存，不会影响本机其他客户端。" : "浏览器授权单独保存，不影响当前登录"))
                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

struct CodexStarter: View {
    @EnvironmentObject private var service: ServiceController
    @AppStorage(CodexUse.key) private var useSetting = "auto"
    let directory: AccountDirectory
    let use: CodexUse
    let open: (AccountSheet) -> Void
    let dismiss: () -> Void

    var body: some View {
        let local = Onboarding.localLogin(service, provider: "codex")
        StarterCard(provider: "codex", title: "开始管理 Codex 账号",
                    lead: CodexUse.desktopInstalled ? "已检测到本机装有 Codex 桌面，选好使用方式就能导入账号。" : "选好使用方式，就能导入或登录账号。",
                    dismiss: dismiss) {
            StarterStep(number: 1, title: "平时怎么用 Codex？", hint: CodexUse(rawValue: useSetting) == nil ? "按本机预选" : nil) {
                PillSegments(segments: [.init(value: CodexUse.cli, title: "只用 CLI"), .init(value: .desktop, title: "只用桌面"), .init(value: .both, title: "CLI 与桌面")],
                             selection: use, select: { CodexUse.choose($0, service: service) }, fill: true)
                Text(note(use)).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true).padding(.top, 7)
            }
            StarterStep(number: 2, title: "放入第一个账号", last: true) {
                VStack(alignment: .leading, spacing: 9) {
                    if let local {
                        LocalLoginRow(provider: "codex", glyph: .codex, title: "本机已有 Codex 登录",
                                      detail: "来自 \(local.source.replacingOccurrences(of: FileManager.default.homeDirectoryForCurrentUser.path, with: "~")) · 导入不会换掉现在的默认登录")
                    }
                    NewLoginRow(provider: "codex", prominent: local == nil, open: open)
                }
            }
        }
    }

    private func note(_ use: CodexUse) -> String {
        switch use {
        case .both: "桌面、CLI、VS Code 插件、pi 共用 ~/.codex/auth.json；连上桌面可热切换。"
        case .desktop: "通过调试端口连上 Codex 桌面后，换号不用退出窗口。"
        case .cli: "仅改写 ~/.codex/auth.json，不连接桌面，换号前先退出 CLI。"
        }
    }
}

struct ClaudeStarter: View {
    @EnvironmentObject private var service: ServiceController
    @AppStorage(ClaudeUse.key) private var useSetting = "auto"
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void
    let addDesktop: () -> Void
    let dismiss: () -> Void

    var body: some View {
        let local = Onboarding.localLogin(service, provider: "claude")
        let desktop = directory.claudeDesktop?.installed == true
        // With no account yet the page's own guess has nothing to go on; preselect from what this Mac has instead.
        let use = ClaudeUse(rawValue: useSetting) ?? (local != nil && desktop ? .both : desktop ? .desktop : .code)
        // The first account is added under the mode shown, so the page that follows matches it.
        let keep = { if ClaudeUse(rawValue: useSetting) == nil { useSetting = use.rawValue } }
        StarterCard(provider: "claude", title: "开始管理 Claude 账号", lead: lead(code: local != nil, desktop: desktop), dismiss: dismiss) {
            StarterStep(number: 1, title: "平时怎么用 Claude？", hint: ClaudeUse(rawValue: useSetting) == nil ? "按本机预选" : nil) {
                PillSegments(segments: [.init(value: ClaudeUse.code, title: "只用 Code"), .init(value: .desktop, title: "只用桌面"), .init(value: .both, title: "Code 与桌面")],
                             selection: use, select: { useSetting = $0.rawValue }, fill: true)
                Text(note(use)).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true).padding(.top, 7)
            }
            StarterStep(number: 2, title: "导入本机账号或登录新号", last: true) {
                VStack(alignment: .leading, spacing: 8) {
                    if use.code, local != nil {
                        LocalLoginRow(provider: "claude", glyph: .terminal, title: "本机 Claude Code 登录", detail: "导入不会换掉现在的默认登录", before: keep)
                    }
                    if use.desk, desktop {
                        HStack(spacing: 10) {
                            Icon(.desktop, size: 15, color: Palette.ink2)
                                .frame(width: 27, height: 27)
                                .background(Palette.field, in: RoundedRectangle(cornerRadius: 7.5, style: .continuous))
                            VStack(alignment: .leading, spacing: 1) {
                                Text("Claude 桌面").font(.system(size: 12, weight: .semibold))
                                Text("桌面只需登录 1 个同 Team 账号（Chat / Cowork）；已登录的会自动认出").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            Button("在 Claude 桌面登录…") { keep(); addDesktop() }
                                .buttonStyle(.pill(.secondary, height: 26))
                                .disabled(service.accountBusy || directory.claudeDesktop?.busy == true)
                        }
                        .padding(.horizontal, 12).padding(.vertical, 10)
                        .background(Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
                    }
                    NewLoginRow(provider: "claude",
                                prominent: !(use.code && local != nil) && !(use.desk && desktop),
                                note: loginNote(use),
                                open: { keep(); open($0) })
                }
            }
        }
    }

    private func lead(code: Bool, desktop: Bool) -> String {
        if code && desktop { return "已检测到 Claude Code 和 Claude 桌面，先选使用方式再导入。" }
        if code { return "已检测到 Claude Code 的登录，先选使用方式再导入。" }
        if desktop { return "已检测到 Claude 桌面，先选使用方式再登录。" }
        return "先选使用方式，再登录账号。"
    }

    private func note(_ use: ClaudeUse) -> String {
        switch use {
        case .both: "桌面 Code 无感换号与终端 CLI 共用 AuthBaton 登录，额度与重置卡互通；桌面壳登录用于 Chat / Cowork。"
        case .code: "点击直接换号不弹确认框；只显示 Claude Code 相关项。"
        case .desktop: "桌面换号会重启桌面（用于 Chat / Cowork）；若常用桌面 Code，把同 Team 账号登录到 AuthBaton 并开「Code 无感换号」即可不重启切号。"
        }
    }

    private func loginNote(_ use: ClaudeUse) -> String? {
        switch use {
        case .desktop: "供桌面 Code 无感换号用（无需装独立 CLI），日后用终端 CLI 也可直接复用。"
        case .both: "桌面 Code 无感换号与终端 CLI 共用，浏览器授权单独保存，不影响当前登录。"
        case .code: nil
        }
    }
}
