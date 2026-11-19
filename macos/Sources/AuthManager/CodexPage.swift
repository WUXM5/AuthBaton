import SwiftUI

// How this Mac uses Codex. Codex has one login (~/.codex/auth.json) that the CLI, Codex 桌面, the VS Code plugin
// and pi all read, so the choice never splits a switch the way Claude's does; it decides how a switch happens.
// 只用 CLI is a plain switch with hot switch off, so the desktop connection never stands in the way; either
// desktop choice brings back the hot switch setting the person had the last time they used the desktop.
enum CodexUse: String, CaseIterable {
    case cli, desktop, both

    static let key = "codex.use"
    // Hot switch as it was while the desktop was last in use; restored when coming back from 只用 CLI.
    static let hotKey = "codex.hotSwitch"

    var title: String {
        switch self {
        case .cli: "只用 CLI"
        case .desktop: "只用桌面"
        case .both: "CLI 与桌面"
        }
    }

    var menuTitle: String {
        switch self {
        case .cli: "只用 Codex CLI"
        case .desktop: "只用 Codex 桌面"
        case .both: "CLI 与桌面都用"
        }
    }

    var desk: Bool { self != .cli }

    // Not chosen yet: hot switch already on, or Codex 桌面 installed, means the desktop is in use.
    static func resolve(_ setting: String, _ directory: AccountDirectory) -> CodexUse {
        if let use = CodexUse(rawValue: setting) { return use }
        return directory.desktop?.enabled == true || desktopInstalled ? .both : .cli
    }

    // Where the service looks for Codex 桌面 (desktop.rs DESKTOP_APPS).
    static var desktopInstalled: Bool {
        ["/Applications/ChatGPT.app", "/Applications/Codex.app"].contains { FileManager.default.fileExists(atPath: $0) }
    }

    // nil is 按这台 Mac 自动判断.
    @MainActor static func choose(_ use: CodexUse?, service: ServiceController) {
        guard let directory = service.directory else { return }
        let defaults = UserDefaults.standard
        let before = resolve(defaults.string(forKey: key) ?? "auto", directory)
        defaults.set(use?.rawValue ?? "auto", forKey: key)
        let after = resolve(defaults.string(forKey: key) ?? "auto", directory)
        let enabled = directory.desktop?.enabled == true
        if !after.desk, enabled {
            defaults.set(true, forKey: hotKey)
            Task { await service.accountOperation("accounts.desktop", params: LoginParams(enabled: false)) }
        } else if after.desk, !before.desk, !enabled, defaults.object(forKey: hotKey) as? Bool ?? true {
            Task { await service.accountOperation("accounts.desktop", params: LoginParams(enabled: true)) }
        }
    }
}

// Codex: every account as a card, in relay order. Codex 桌面 no longer has a page of its own: while the desktop
// is in use, a capsule in the header says how it is wired and opens the connection sheet, and a band appears
// only when the connection keeps a switch from happening.
struct CodexPage: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @AppStorage(CodexUse.key) private var useSetting = "auto"
    @AppStorage(Onboarding.codexKey) private var onboarding = ""
    @State private var sheet: AccountSheet?

    var body: some View {
        Group {
            if let directory = service.directory {
                let use = CodexUse.resolve(useSetting, directory)
                let accounts = directory.relay("codex")
                let starting = Onboarding.starting(onboarding, empty: accounts.isEmpty)
                ScrollViewReader { proxy in
                    PageScaffold(title: "Codex", subtitle: Text(subtitle(directory, accounts, use: use)), provider: "codex") {
                        // On a first run the starter card is the page; only 添加账号 stays in the header.
                        if !starting {
                            modeMenu(use)
                            if use.desk, let desktop = directory.desktop { capsule(desktop, current: directory.current("codex")) }
                            RefreshPill(checkedAt: accounts.compactMap { $0.usage?.checkedAt }.max(), busy: service.isRefreshing) {
                                Task {
                                    await service.refresh()
                                    await service.refreshQuotaIfNeeded()
                                }
                            }
                            .fixedSize()
                        }
                        Button { sheet = .login(provider: "codex", accountID: nil) } label: {
                            HStack(spacing: 5) {
                                Icon(.plus, size: 12, color: .white, weight: 2)
                                Text("添加账号")
                            }
                        }
                        .buttonStyle(.pill(.primary, height: 27))
                        .fixedSize()
                        .disabled(service.accountBusy || directory.busy || directory.recoveryNeeded)
                    } content: {
                        if directory.recoveryNeeded { RecoveryBanner() }
                        if let error = service.accountError, service.accountErrorID == nil {
                            Banner(text: error) { Button("知道了") { service.clearAccountError() }.buttonStyle(.pill(.onAttention, height: 20)) }
                        }
                        if !accounts.isEmpty { desktopBand(directory) }
                        if starting {
                            CodexStarter(directory: directory, use: use, open: { sheet = $0 }, dismiss: { onboarding = Onboarding.done })
                        } else if accounts.isEmpty {
                            AddCard(provider: "codex", open: { sheet = $0 }, lead: "还没有 Codex 账号", detail: "登录一个，或导入这台 Mac 上已有的登录。")
                        } else {
                            if onboarding == Onboarding.started, let next = OnboardingNext.codex(directory, accounts: accounts) {
                                NextStrip(next: next, open: { sheet = $0 }, dismiss: { onboarding = Onboarding.done })
                            }
                            // Two cards a row, each row as tall as its taller card so the footers line up.
                            Grid(horizontalSpacing: 14, verticalSpacing: 14) {
                                ForEach(Array(stride(from: 0, to: accounts.count, by: 2)), id: \.self) { start in
                                    GridRow {
                                        ForEach(accounts[start..<min(start + 2, accounts.count)]) { account in
                                            CodexCard(account: account, directory: directory, selected: nav.selection == account.id) { sheet = $0 }
                                                .frame(maxHeight: .infinity, alignment: .top)
                                                .id(account.id)
                                        }
                                        if start + 1 == accounts.count { Color.clear.frame(maxWidth: .infinity, maxHeight: 1) }
                                    }
                                }
                            }
                            if directory.strategy != nil {
                                HStack(spacing: 0) {
                                    Text(use.desk ? "卡片按接力顺序排；改顺序和规则去" : "卡片按接力顺序排；只用 CLI 时见底只会重置，改顺序和规则去")
                                    Button("换号与重置") { nav.show(.automation) }.buttonStyle(.pill(.link))
                                    Text("。")
                                }
                                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                            }
                        }
                    }
                    .onAppear { if let id = nav.selection { proxy.scrollTo(id, anchor: .center) } }
                    .onChange(of: nav.selection) { _, id in if let id { withAnimation { proxy.scrollTo(id, anchor: .center) } } }
                    .onAppear { onboarding = Onboarding.settle(onboarding, empty: accounts.isEmpty, finished: Onboarding.finished(directory, provider: "codex")) }
                    .onChange(of: accounts.count) { _, _ in
                        onboarding = Onboarding.settle(onboarding, empty: accounts.isEmpty, finished: Onboarding.finished(directory, provider: "codex"))
                    }
                }
            } else {
                Color.clear
            }
        }
        .task { await service.refreshQuotaIfNeeded() }
        .onAppear { openRequestedSheet() }
        .onChange(of: nav.desktopSheet) { _, _ in openRequestedSheet() }
        .sheet(item: $sheet) { item in
            AccountSheetView(item: item, close: { sheet = nil }).environmentObject(service)
        }
    }

    // Another page or the menu bar asked for the connection sheet.
    private func openRequestedSheet() {
        guard nav.desktopSheet else { return }
        nav.desktopSheet = false
        sheet = .desktopConnection
    }

    private func subtitle(_ directory: AccountDirectory, _ accounts: [ManagedAccount], use: CodexUse) -> String {
        if accounts.isEmpty { return "还没有账号" }
        var parts = ["\(accounts.count) 个账号"]
        if let current = directory.current("codex") { parts.append("在用 \(current.displayName)") }
        let pi = service.snapshot?.providers.first(where: { $0.id == "pi" })?.state == "configured"
        switch use {
        case .both: parts.append(pi ? "CLI 与 pi 同步切换" : "CLI 同步切换")
        case .desktop: break
        case .cli: parts.append(pi ? "CLI、VS Code、pi 共用这份登录" : "CLI、VS Code 共用这份登录")
        }
        return parts.joined(separator: " · ")
    }

    private func modeMenu(_ use: CodexUse) -> some View {
        Menu {
            ForEach(CodexUse.allCases, id: \.self) { option in
                Button { CodexUse.choose(option, service: service) } label: {
                    if useSetting == option.rawValue { Label(option.menuTitle, systemImage: "checkmark") } else { Text(option.menuTitle) }
                }
            }
            Divider()
            Button { CodexUse.choose(nil, service: service) } label: {
                if CodexUse(rawValue: useSetting) == nil { Label("按这台 Mac 自动判断", systemImage: "checkmark") } else { Text("按这台 Mac 自动判断") }
            }
        } label: {
            HStack(spacing: 5) {
                Text(use.title)
                Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4)
            }
            .font(.system(size: 12)).foregroundStyle(Palette.ink2)
            .padding(.horizontal, 11).frame(height: 27)
            .background(Capsule().fill(Palette.card))
            .overlay(Capsule().strokeBorder(Palette.ring, lineWidth: 1))
            .contentShape(Capsule())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        .help("只用 CLI 时不连 Codex 桌面，换号前先退出 CLI；用桌面时经桌面热切换，不用退出桌面")
        .accessibilityLabel("使用方式：\(use.menuTitle)")
        .disabled(service.accountBusy || service.directory?.desktop?.busy == true)
    }

    // How the desktop is wired, at a glance; the sheet behind it holds everything the old Codex 桌面 page had.
    private func capsule(_ desktop: DesktopConnection, current: ManagedAccount?) -> some View {
        let state = DesktopWiring(desktop, current: current)
        return Button { sheet = .desktopConnection } label: {
            HStack(spacing: 5) {
                if let dot = state.dot { Circle().fill(dot).frame(width: 6, height: 6) }
                Icon(.desktop, size: 13, color: state.attention ? Palette.attentionText : state.plain ? Palette.ink3 : Palette.deep)
                Text(state.short)
            }
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(state.attention ? Palette.attentionText : state.plain ? Palette.ink2 : Palette.ink)
            .padding(.horizontal, 10).frame(height: 27)
            .background(Capsule().fill(state.attention ? Palette.attentionFill : Palette.card))
            .overlay(Capsule().strokeBorder(state.attention ? Palette.attentionDash.opacity(0.35) : Palette.ring, lineWidth: 1))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .fixedSize()
        .help(state.help(port: desktop.port))
        .accessibilityLabel("Codex 桌面连接：\(state.short)")
    }

    // The two connection states that keep a switch from happening, each with what fixes it. A lost connection
    // and a desktop still on its own login are the only reasons this band appears.
    @ViewBuilder private func desktopBand(_ directory: AccountDirectory) -> some View {
        if let desktop = directory.desktop, desktop.enabled {
            let current = directory.current("codex")
            if !desktop.connected {
                Banner(glyph: .desktop, text: directory.strategy?.config.enabled == true
                       ? "Codex 桌面没连上，现在换不了号，见底时只会重置。" : "Codex 桌面没连上，现在换不了号。") {
                    Button("连接详情") { sheet = .desktopConnection }.buttonStyle(.pill(.link))
                    Button(desktop.busy ? "正在重开…" : "以调试模式重新打开") {
                        Task { await service.accountOperation("accounts.desktop_launch") }
                    }
                    .buttonStyle(.pill(.onAttention, height: 22))
                    .disabled(service.accountBusy || directory.busy || directory.recoveryNeeded || desktop.busy)
                }
            } else if let current, desktop.selected != current.id {
                Banner(glyph: .desktop, text: "Codex 桌面已连上，但还在用自己的登录；接管后才能热切换（当前账号不变）。") {
                    Button("连接详情") { sheet = .desktopConnection }.buttonStyle(.pill(.link))
                    Button("接管桌面") { sheet = .switchTo(current.id) }
                        .buttonStyle(.pill(.primary, height: 22))
                        .disabled(service.accountBusy || directory.switchDisabled(for: current))
                }
            }
        }
    }
}

// The desktop connection boiled down for the header capsule, the sidebar and the connection sheet.
struct DesktopWiring {
    let desktop: DesktopConnection
    // Hot switch on and the desktop answers, but it still uses its own login.
    let ownLogin: Bool

    init(_ desktop: DesktopConnection, current: ManagedAccount?) {
        self.desktop = desktop
        ownLogin = desktop.enabled && desktop.connected && current.map { desktop.selected != $0.id } == true
    }

    var plain: Bool { !desktop.enabled }
    var lost: Bool { desktop.enabled && !desktop.connected }
    var attention: Bool { lost || ownLogin }
    var dot: Color? { plain ? nil : attention ? Palette.attentionDot : Palette.brand }
    var short: String { plain ? "普通切换" : lost ? "没连上" : ownLogin ? "未接管" : "热切换" }

    func help(port: UInt16) -> String {
        plain ? "热切换关着：换号前要先退出 Codex 桌面，点开可以重新打开"
            : lost ? "Codex 桌面没连上调试端口 \(port)，现在换不了号"
            : ownLogin ? "Codex 桌面已连上，但还在用自己的登录"
            : "Codex 桌面已连上并已接管（端口 \(port)）"
    }
}

private struct CodexCard: View {
    let account: ManagedAccount
    let directory: AccountDirectory
    let selected: Bool
    let open: (AccountSheet) -> Void

    var body: some View {
        let rule = directory.rule(for: account.id)
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 9) {
                    Seal(account.displayName, style: account.native ? .inUse : rule?.participate == false ? .out : .standby, size: 31)
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 7) {
                            Text(account.displayName).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                            if account.native { Pill("在用", style: .brand, small: true) }
                            if !account.native, directory.desktop?.selected == account.id { Pill("桌面在用", style: .brand, small: true) }
                        }
                        Text(account.subtitle.isEmpty ? " " : account.subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                    }
                    Spacer(minLength: 7)
                    AccountMenu(account: account, directory: directory, open: open)
                }
                QuotaFigures(account: account, notch: directory.notch(for: account), size: 36, inlineCaption: true) {
                    ChancesTicket(account: account)
                }
                .padding(.top, 14)
                if !account.extraWindows.isEmpty {
                    Text("独立额度：" + account.extraWindows.map { "\($0.scope ?? $0.label) 剩余 \($0.remaining)%" }.joined(separator: "，"))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                        .help("模型或功能单独的额度，不参与自动换号判断")
                        .padding(.top, 9)
                }
            }
            .padding(.horizontal, 17).padding(.top, 15).padding(.bottom, 14)
            Spacer(minLength: 0)
            HStack(spacing: 5) {
                if let rule {
                    if let rank = directory.rank(of: account) { Pill("第 \(rank) 位", style: rule.participate ? .neutral : .dashed) }
                    Pill("≤ \(rule.below)%", style: rule.participate ? .neutral : .dashed)
                    Pill(rule.order.label, style: rule.participate ? .neutral : .dashed).help(rule.order.explanation)
                    if !rule.participate { Pill("不参与") }
                } else if directory.strategy != nil {
                    Pill("还没有规则", style: .dashed).help("在「换号与重置」保存一次规则后加入接力")
                }
                Spacer(minLength: 7)
                // Taking the desktop over is the page band's job; the in-use card offers no 接管 of its own.
                AccountActions(account: account, directory: directory, open: open, showSwitch: !account.native)
            }
            .padding(.horizontal, 17).padding(.vertical, 10)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
            AccountStrips(account: account, directory: directory, open: open).padding(.horizontal, 17)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopCard(radius: 19, raised: account.native)
        .overlay {
            if selected { RoundedRectangle(cornerRadius: 19, style: .continuous).strokeBorder(Palette.brand, lineWidth: 1.5) }
        }
    }
}
