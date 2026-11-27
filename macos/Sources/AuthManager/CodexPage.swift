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
                    PageScaffold(title: "Codex", subtitle: subtitle(directory, accounts, use: use), provider: "codex") {
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
                            RelayStatusSlot(directory: directory, provider: "codex", present: { sheet = $0 })
                            CodexStage(directory: directory, selection: nav.selection, open: { sheet = $0 })
                            CodexRelayDeck(directory: directory, note: use.desk || directory.strategy?.config.enabled != true ? nil
                                           : "只用 CLI 时见底\(directory.resets("codex") ? "只会重置" : "不会换号")。", open: { sheet = $0 })
                            // Outside the relay: by choice, new and without a rule, or handed over by another Mac. An external
                            // source's accounts outside it are in their own group below.
                            let out = accounts.filter { $0.external == nil && directory.rule(for: $0.id)?.participate != true }
                            if !out.isEmpty { CodexDrawer(accounts: out, directory: directory, open: { sheet = $0 }) }
                            ExternalGroups(provider: "codex", directory: directory, switchTitle: "换上",
                                           switchTo: { sheet = .switchTo($0.id) }, open: { sheet = $0 })
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
        .overlay(alignment: .bottom) { RelayUndoToast(provider: "codex") }
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

    private func subtitle(_ directory: AccountDirectory, _ accounts: [ManagedAccount], use: CodexUse) -> Text {
        if accounts.isEmpty { return Text("还没有账号") }
        // External sources' accounts are not counted (they have a group of their own); one in use says so.
        var parts = ["\(accounts.filter { $0.external == nil }.count) 个账号"]
        if let current = directory.current("codex") { parts.append("在用 \(current.displayName)" + (current.external != nil ? "（外部）" : "")) }
        let pi = service.snapshot?.piFollows == true
        switch use {
        case .both: parts.append(pi ? "CLI 与 pi 同步切换" : "CLI 同步切换")
        case .desktop: break
        case .cli: parts.append(pi ? "CLI、VS Code、pi 共用这份登录" : "CLI、VS Code 共用这份登录")
        }
        let text = Text(parts.joined(separator: " · "))
        // Accounts another Mac just handed to the centre show up here on their own.
        let fresh = accounts.filter { $0.cloud?.fresh == true }.map(\.displayName)
        return fresh.isEmpty ? text : text + Text(" · ") + Text("刚同步 " + fresh.joined(separator: "、")).fontWeight(.semibold).foregroundStyle(Palette.ink)
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
                Banner(glyph: .desktop, text: directory.strategy?.config.enabled == true && directory.resets("codex")
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

struct CodexCard: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let selected: Bool
    let open: (AccountSheet) -> Void
    // A queue row opened in place: the card folds back into its row.
    var collapse: (() -> Void)? = nil
    // 在用 with the relay's wing closing it on the right.
    var joined = false

    var body: some View {
        let rule = directory.rule(for: account.id)
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 9) {
                    Seal(account.displayName, style: account.external != nil ? .external : account.native ? .inUse : rule?.participate == false ? .out : .standby, size: 31)
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 7) {
                            Text(accountName: account.displayName).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                            CloudBadge(account: account)
                            if account.native { Pill("在用", style: .brand, small: true) }
                            if account.external != nil { ExternalModeButton(account: account, directory: directory, asTag: true, large: true) }
                            if !account.native, directory.desktop?.selected == account.id { Pill("桌面在用", style: .brand, small: true) }
                        }
                        Text(account.subtitle.isEmpty ? " " : account.subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                    }
                    Spacer(minLength: 7)
                    if let collapse {
                        Button(action: collapse) {
                            Icon(.chevronDown, size: 11, color: Palette.ink3, weight: 2).rotationEffect(.degrees(180))
                                .frame(width: 26, height: 26).contentShape(Circle())
                        }
                        .buttonStyle(.plain)
                        .help("收起")
                        .accessibilityLabel("收起 \(account.displayName)")
                    }
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
            .padding(.horizontal, 14).padding(.top, 15).padding(.bottom, 14)
            Spacer(minLength: 0)
            if account.transferFailed != nil { TransferBox(account: account).padding(.horizontal, 14).padding(.bottom, 10) }
            HStack(spacing: 5) {
                HStack(spacing: 5) {
                    if let rule {
                        if let rank = directory.rank(of: account) { Pill("第 \(rank) 位", style: rule.participate ? .neutral : .dashed) }
                        Pill("≤ \(rule.below)%", style: rule.participate ? .neutral : .dashed)
                        Pill(rule.order.label, style: rule.participate ? .neutral : .dashed).help(rule.order.explanation)
                        if !rule.participate { Pill("不参与") }
                    } else if directory.strategy != nil, account.external?.allowsSwitch != false {
                        // New, or came from another Mac: it stays out of the relay until someone adds it. Joining
                        // takes effect at once.
                        Pill("未加入接力", style: .dashed)
                        Button("加入接力") {
                            service.clearAccountError()
                            Task { await RelayRules.save(directory.relayConfig(account.id, participate: true), service) }
                        }
                        .buttonStyle(.pill(.link))
                        .disabled(service.accountBusy || (directory.strategy?.config.accounts.count ?? 0) >= RelayRules.limit)
                    }
                }
                .opacity(account.transferRunning ? 0.45 : 1)
                Spacer(minLength: 7)
                if let external = account.external {
                    ExternalWebLink(title: "去来源网页", address: external.manageUrl ?? directory.source(of: account)?.manageUrl)
                }
                // Taking the desktop over is the page band's job; the in-use card offers no 接管 of its own.
                AccountActions(account: account, directory: directory, open: open, showSwitch: !account.native)
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
            AccountStrips(account: account, directory: directory, open: open).padding(.horizontal, 14)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopCard(radius: 19, joined: joined, raised: account.native)
        .overlay {
            let shape = UnevenRoundedRectangle(topLeadingRadius: 19, bottomLeadingRadius: 19, bottomTrailingRadius: joined ? 0 : 19,
                                               topTrailingRadius: joined ? 0 : 19, style: .continuous)
            if selected {
                shape.strokeBorder(Palette.brand, lineWidth: 1.5)
            } else if account.transferFailed != nil {
                shape.strokeBorder(Palette.attention, lineWidth: 1.5)
            } else if account.cloud?.fresh == true {
                shape.strokeBorder(Palette.dashStrong, style: StrokeStyle(lineWidth: 1.2, dash: [5, 4]))
            }
        }
    }
}
