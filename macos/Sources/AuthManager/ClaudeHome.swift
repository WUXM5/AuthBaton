import SwiftUI

// Claude: Claude Code and Claude 桌面 on one page. Quota and reset cards belong to the account, so each
// account shows once; which client it is switched into depends on how this Mac uses Claude.

// How this Mac uses Claude. Until someone picks, what is set up here decides.
enum ClaudeUse: String, CaseIterable {
    case code, desktop, both

    static let key = "claude.use"
    static let scopeKey = "claude.switchScope"
    static let driftKey = "claude.driftDismissed"

    var title: String {
        switch self {
        case .code: "只用 Code"
        case .desktop: "只用桌面"
        case .both: "Code 与桌面"
        }
    }

    var menuTitle: String {
        switch self {
        case .code: "只用 Claude Code"
        case .desktop: "只用 Claude 桌面"
        case .both: "Code 与桌面都用"
        }
    }

    var code: Bool { self != .desktop }
    var desk: Bool { self != .code }

    static func resolve(_ setting: String, _ directory: AccountDirectory) -> ClaudeUse {
        if let use = ClaudeUse(rawValue: setting) { return use }
        let code = !directory.relay("claude").isEmpty
        let desk = directory.claudeDesktop.map { $0.installed && !$0.known.isEmpty } ?? false
        return code && desk ? .both : desk ? .desktop : .code
    }
}

// Where 换上 sends an account when both clients are in use; remembered from the last choice.
enum SwitchScope: String, CaseIterable {
    case both, code, desktop

    // Until someone picks: with 无感换号 on the desktop's Code tab already follows Code, so 换上 moves Code
    // only and leaves Claude 桌面's chat / work where it is (11-22); without it, both.
    static func remembered(_ setting: String?, _ directory: AccountDirectory?) -> SwitchScope {
        SwitchScope(rawValue: setting ?? "") ?? (directory?.claudeDesktop?.codeRelay?.enabled == true ? .code : .both)
    }

    var title: String {
        switch self {
        case .both: "一起换"
        case .code: "只换 Code"
        case .desktop: "只换桌面"
        }
    }
}

// One Claude account: its AuthBaton login (what Claude Code uses, and what quota and reset cards need),
// its saved Claude 桌面 login, or both.
struct ClaudeEntry: Identifiable {
    let cli: ManagedAccount?
    let desk: ClaudeDesktopStatus.Account?

    var id: String { cli?.id ?? "desktop:" + (desk?.handle ?? "") }
    var name: String { cli?.displayName ?? desk?.display ?? "" }
    var inCode: Bool { cli?.native == true }
    var inDesk: Bool { desk?.inUse == true }
    var inUse: Bool { inCode || inDesk }

    // Code-mode pages list AuthBaton logins in relay order; desktop pages list Claude 桌面's numbered list first.
    static func all(_ directory: AccountDirectory, use: ClaudeUse) -> [ClaudeEntry] {
        let known = directory.claudeDesktop?.known ?? []
        let relay = directory.relay("claude")
        let linked = Dictionary(known.compactMap { desk in desk.linkedAccount.map { ($0, desk) } }, uniquingKeysWith: { first, _ in first })
        let paired = relay.map { ClaudeEntry(cli: $0, desk: use.desk ? linked[$0.id] : nil) }
        let deskOnly = known.filter { desk in desk.linkedAccount.map { id in !relay.contains { $0.id == id } } ?? true }
            .map { ClaudeEntry(cli: nil, desk: $0) }
        switch use {
        case .code: return paired
        case .both: return paired + deskOnly
        case .desktop:
            let numbered = (paired.filter { $0.desk != nil } + deskOnly).sorted { ($0.desk?.number ?? .max) < ($1.desk?.number ?? .max) }
            return numbered + paired.filter { $0.desk == nil }
        }
    }

    static func find(_ id: String?, in entries: [ClaudeEntry]) -> ClaudeEntry? {
        entries.first { $0.id == id || ($0.desk != nil && $0.desk?.handle == id) }
    }
}

// What 换上 can do for an entry right now, and what it does by default.
struct ClaudeSwitchPlan {
    let canCode: Bool
    let canDesk: Bool

    @MainActor init(_ entry: ClaudeEntry, use: ClaudeUse, directory: AccountDirectory, service: ServiceController) {
        let status = directory.claudeDesktop
        let codeRelayOn = status?.codeRelay?.enabled == true
        canCode = (use.code || codeRelayOn) && entry.cli.map { directory.primaryAction(for: $0) == .switchTo && !directory.switchDisabled(for: $0) } == true
            && !service.accountBusy
        canDesk = use.desk && entry.desk?.canSwitch == true && status?.switchSupported == true && status?.busy != true && !service.accountBusy
    }

    var any: Bool { canCode || canDesk }
    var choice: Bool { canCode && canDesk }

    // The remembered scope where it applies; when it moves nothing, the side that can still move.
    func targets(_ scope: SwitchScope) -> (code: Bool, desk: Bool) {
        let code = canCode && scope != .desktop
        let desk = canDesk && scope != .code
        return code || desk ? (code, desk) : (canCode, canDesk)
    }
}

// The request behind a switch sheet: which entry, which sides, and whether the sheet offers the choice.
struct ClaudeSwitchRequest: Identifiable {
    let entryID: String
    let code: Bool
    let desk: Bool
    let choice: Bool
    var id: String { "\(entryID)-\(code)-\(desk)-\(choice)" }
}

enum ClaudeSheet: Identifiable {
    case account(AccountSheet)
    case desktop(DesktopSheet)
    case switching(ClaudeSwitchRequest)
    case tokenQuota
    case codeRelay
    case codeRelayUninstall
    case chromeSync
    case chromeSyncUninstall

    var id: String {
        switch self {
        case .account(let sheet): "account-\(sheet.id)"
        case .desktop(let sheet): "desktop-\(sheet.id)"
        case .switching(let request): "switch-\(request.id)"
        case .tokenQuota: "token-quota"
        case .codeRelay: "code-relay"
        case .codeRelayUninstall: "code-relay-uninstall"
        case .chromeSync: "chrome-sync"
        case .chromeSyncUninstall: "chrome-sync-uninstall"
        }
    }
}

// MARK: 页面

struct ClaudeHomePage: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @AppStorage(ClaudeUse.key) private var useSetting = "auto"
    @AppStorage(ClaudeUse.scopeKey) private var scopeSetting = ""
    @AppStorage(ClaudeUse.driftKey) private var driftDismissed = ""
    @AppStorage(Onboarding.claudeKey) private var onboarding = ""
    @State private var sheet: ClaudeSheet?

    var body: some View {
        Group {
            if let directory = service.directory {
                let use = ClaudeUse.resolve(useSetting, directory)
                let entries = ClaudeEntry.all(directory, use: use)
                if let detail = nav.claudeDetail, let entry = ClaudeEntry.find(detail, in: entries) {
                    ClaudeAccountPage(entry: entry, entries: entries, use: use, directory: directory, open: { present($0, directory) },
                                      switchTo: { perform($0, $1, directory: directory, use: use) })
                } else {
                    overview(directory, use: use, entries: entries)
                        .onAppear { settle(directory, entries: entries) }
                        .onChange(of: entries.count) { _, _ in settle(directory, entries: entries) }
                }
            } else {
                Color.clear
            }
        }
        .task { await service.refreshQuotaIfNeeded() }
        .overlay(alignment: .bottom) { RelayUndoToast(provider: "claude") }
        .sheet(item: $sheet) { item in
            Group {
                switch item {
                case .account(let sheet): AccountSheetView(item: sheet, close: { self.sheet = nil })
                case .desktop(let sheet): DesktopSheetView(item: sheet, close: { self.sheet = nil })
                case .switching(let request): ClaudeSwitchSheet(request: request, close: { self.sheet = nil },
                                                                login: { self.sheet = .account(.login(provider: "claude", accountID: nil)) })
                case .tokenQuota: TokenQuotaSheet(close: { self.sheet = nil })
                case .codeRelay: CodeRelaySheet(close: { self.sheet = nil })
                case .codeRelayUninstall: CodeRelayUninstallSheet(close: { self.sheet = nil })
                case .chromeSync: ChromeSyncSheet(close: { self.sheet = nil }, openUninstall: { self.sheet = .chromeSyncUninstall })
                case .chromeSyncUninstall: ChromeSyncUninstallSheet(close: { self.sheet = nil })
                }
            }
            .environmentObject(service)
        }
    }

    private var scope: SwitchScope { SwitchScope.remembered(scopeSetting, service.directory) }

    // Sheets from cards, rows and the desktop's bar.
    private func present(_ item: ClaudeSheet, _ directory: AccountDirectory) {
        sheet = item
    }

    private func settle(_ directory: AccountDirectory, entries: [ClaudeEntry]) {
        onboarding = Onboarding.settle(onboarding, empty: entries.isEmpty,
                                       finished: Onboarding.finished(directory, provider: "claude", count: entries.count))
    }

    // 换上 with the remembered scope, or the scope picked from the ▾ menu (which becomes the new default).
    private func perform(_ entry: ClaudeEntry, _ picked: SwitchScope?, directory: AccountDirectory, use: ClaudeUse) {
        let plan = ClaudeSwitchPlan(entry, use: use, directory: directory, service: service)
        if let picked { scopeSetting = picked.rawValue }
        let targets = plan.targets(picked ?? scope)
        guard targets.code || targets.desk else { return }
        if targets.code && !targets.desk, let cli = entry.cli {
            // Claude Code switches in place within 30 s; nothing to confirm beyond the click.
            Task { await service.accountOperation("accounts.activate", params: LoginParams(id: cli.id, confirm: true)) }
        } else {
            sheet = .switching(ClaudeSwitchRequest(entryID: entry.id, code: targets.code, desk: targets.desk,
                                                   choice: plan.choice && picked == nil && targets.code && targets.desk))
        }
    }

    private func overview(_ directory: AccountDirectory, use: ClaudeUse, entries: [ClaudeEntry]) -> some View {
        let status = directory.claudeDesktop
        let codeNow = entries.first { $0.inCode }
        let deskNow = entries.first { $0.inDesk }
        // On a first run the starter card is the page; only 添加账号 stays in the header.
        let starting = Onboarding.starting(onboarding, empty: entries.isEmpty)
        let codeRelayOn = status?.codeRelay?.enabled == true
        // Accounts another Mac just handed to the centre show up here on their own.
        let fresh = entries.compactMap { $0.cli?.cloud?.fresh == true ? $0.name : nil }
        let lead = Text(starting ? "还没有账号" : subtitle(use, entries: entries, code: codeNow, desk: deskNow, codeRelayOn: codeRelayOn))
        return PageScaffold(title: "Claude", subtitle: starting || fresh.isEmpty ? lead
                                : lead + Text(" · ") + Text("刚同步 " + fresh.joined(separator: "、")).fontWeight(.semibold).foregroundStyle(Palette.ink),
                            provider: "claude", spacing: 14) {
            if !starting {
                modeMenu(use)
                RefreshPill(checkedAt: entries.compactMap { $0.cli?.usage?.checkedAt }.max(), busy: service.isRefreshing) {
                    Task {
                        await service.refresh()
                        await service.refreshQuotaIfNeeded()
                    }
                }
                .fixedSize()
            }
            addButton(use, directory: directory).fixedSize()
        } content: {
            if (use.code || codeRelayOn) && directory.recoveryNeeded { RecoveryBanner() }
            if let error = service.accountError, service.accountErrorID == nil {
                Banner(text: error) { Button("知道了") { service.clearAccountError() }.buttonStyle(.pill(.onAttention)) }
            }
            if use == .both, scope == .both, status?.codeRelay?.enabled != true, let code = codeNow, let desk = deskNow, code.id != desk.id,
               ClaudeSwitchPlan(code, use: use, directory: directory, service: service).canDesk, driftDismissed != "\(code.id)|\(desk.id)" {
                Banner(glyph: .relay, text: "Claude Code 在用 \(code.name)，Claude 桌面还在用 \(desk.name)。") {
                    Button("桌面也换上") { sheet = .switching(ClaudeSwitchRequest(entryID: code.id, code: false, desk: true, choice: false)) }
                        .buttonStyle(.pill(.onAttention))
                    Button("不用") { driftDismissed = "\(code.id)|\(desk.id)" }.buttonStyle(.pill(.link))
                }
            }
            if starting {
                ClaudeStarter(directory: directory, open: { sheet = .account($0) }, addDesktop: { sheet = .desktop(.add) },
                              dismiss: { onboarding = Onboarding.done })
            } else if entries.isEmpty {
                AddCard(provider: "claude", open: { sheet = .account($0) }, lead: "还没有 Claude 账号", detail: "登录一个，或导入这台 Mac 上已有的登录。")
            } else {
                if onboarding == Onboarding.started, let next = OnboardingNext.claude(directory, names: entries.map(\.name)) {
                    NextStrip(next: next, open: { sheet = .account($0) }, dismiss: { onboarding = Onboarding.done })
                }
                layers(directory, use: use, entries: entries)
            }
            if !starting {
                hints(use, directory: directory)
                footer(use, status: status, managed: entries.contains { $0.cli != nil }, deck: ClaudeUse.sides(use, directory).code && entries.count > 1)
            }
        }
    }

    private func deskLocked(_ status: ClaudeDesktopStatus) -> Bool {
        service.accountBusy || status.busy || !status.installed
    }

    private func subtitle(_ use: ClaudeUse, entries: [ClaudeEntry], code: ClaudeEntry?, desk: ClaudeEntry?, codeRelayOn: Bool) -> String {
        switch use {
        case .code: return "\(entries.count) 个账号 · " + (code.map { "在用 \($0.name)" } ?? "还没有在用的账号")
        case .desktop:
            if codeRelayOn, let code {
                if let desk, code.id == desk.id { return "\(entries.count) 个账号 · 在用 \(code.name)" }
                return "\(entries.count) 个账号 · Code 在用 \(code.name) · 桌面在用 \(desk?.name ?? "—")"
            }
            let count = entries.filter { $0.desk != nil }.count
            return "\(count) 个桌面账号 · " + (desk.map { "在用 \($0.desk?.number.map { "#\($0) " } ?? "")\($0.name)" } ?? "桌面还没认出在用的账号")
        case .both:
            if let code, let desk, code.id == desk.id { return "\(entries.count) 个账号 · 在用 \(code.name)" }
            return "\(entries.count) 个账号 · Code 在用 \(code?.name ?? "—") · 桌面在用 \(desk?.name ?? "—")"
        }
    }

    private func modeMenu(_ use: ClaudeUse) -> some View {
        Menu {
            ForEach(ClaudeUse.allCases, id: \.self) { option in
                Button { useSetting = option.rawValue } label: {
                    if useSetting == option.rawValue { Label(option.menuTitle, systemImage: "checkmark") } else { Text(option.menuTitle) }
                }
            }
            Divider()
            Button { useSetting = "auto" } label: {
                if ClaudeUse(rawValue: useSetting) == nil { Label("按这台 Mac 自动判断", systemImage: "checkmark") } else { Text("按这台 Mac 自动判断") }
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
        .help("只用一边时，页面只显示那一边；两边都用时，换上可以一起换或只换一边")
        .accessibilityLabel("使用方式：\(use.menuTitle)")
    }

    // In use on top, the relay queue below, the accounts outside it in a drawer at the bottom.
    @ViewBuilder private func layers(_ directory: AccountDirectory, use: ClaudeUse, entries: [ClaudeEntry]) -> some View {
        let sides = ClaudeUse.sides(use, directory)
        let present = { (item: ClaudeSheet) in self.present(item, directory) }
        let switchTo = { (entry: ClaudeEntry, scope: SwitchScope?) in perform(entry, scope, directory: directory, use: use) }
        // Claude 桌面's chat / work stays on one account while Code relays: it is a bar of its own, not a slot (11-25).
        if sides.desk && sides.code, let status = directory.claudeDesktop {
            ClaudeDeskBar(status: status, entries: entries, use: use, directory: directory, present: present)
        }
        if sides.code { RelayStatusSlot(directory: directory, provider: "claude", present: { sheet = .account($0) }) }
        ClaudeStage(entries: entries, use: use, directory: directory, present: present, switchTo: switchTo)
        if !sides.code {
            // Only Claude 桌面: it never switches by itself, so its numbered list stands in for the relay.
            ClaudeDeskRotation(entries: entries, use: use, directory: directory, present: present, switchTo: switchTo)
            // Outside the numbered list: sitting out of 下一个, or never signed in to Claude 桌面 (去桌面登录).
            let out = entries.filter { $0.desk == nil || ($0.desk?.participates != true && !$0.inDesk) }
            if !out.isEmpty {
                ClaudeDrawer(title: "不参与「下一个」", entries: out, use: use, directory: directory, present: present, switchTo: switchTo) { entry in
                    guard let desk = entry.desk, desk.known else { return nil }
                    return ("+ 参与「下一个」", { Task { await service.claudeDesktop("enable", account: desk.handle) } })
                }
            }
        } else if entries.count == 1 {
            ClaudeSecondAccount { sheet = .account(.login(provider: "claude", accountID: nil)) }
        } else {
            ClaudeRelayDeck(entries: entries, use: use, directory: directory, present: present, switchTo: switchTo)
            // Outside the relay: by choice, or with no Code login yet (only signed in to Claude 桌面).
            let out = entries.filter { entry in
                if let cli = entry.cli { return !entry.inUse && directory.rule(for: cli.id)?.participate != true }
                return sides.desk && !entry.inDesk
            }
            if !out.isEmpty {
                ClaudeDrawer(title: "不参与自动接力", entries: out, use: use, directory: directory, present: present, switchTo: switchTo) { entry in
                    guard let cli = entry.cli, let config = directory.relayConfig(cli.id, participate: true) else { return nil }
                    // Joining takes effect at once, as a drop in the queue does.
                    return ("加入接力", { Task { await RelayRules.save(config, service) } })
                }
            }
        }
    }

    @ViewBuilder private func addButton(_ use: ClaudeUse, directory: AccountDirectory) -> some View {
        let codeLocked = service.accountBusy || directory.busy || directory.recoveryNeeded
        let deskLocked = directory.claudeDesktop.map { self.deskLocked($0) || !$0.switchSupported } ?? true
        let codeRelayOn = directory.claudeDesktop?.codeRelay?.enabled == true
        let label = HStack(spacing: 5) {
            Icon(.plus, size: 12, color: .white, weight: 2)
            Text("添加账号")
        }
        switch use {
        case .code:
            Button { sheet = .account(.login(provider: "claude", accountID: nil)) } label: { label }
                .buttonStyle(.pill(.primary, height: 27)).disabled(codeLocked)
        case .desktop:
            if codeRelayOn {
                Menu {
                    Button("添加 Code 模式账号（无感换号 / CLI 共用）…") { sheet = .account(.login(provider: "claude", accountID: nil)) }.disabled(codeLocked)
                    Button("在 Claude 桌面登录新账号（Chat / Cowork）…") { sheet = .desktop(.add) }.disabled(deskLocked)
                } label: {
                    label.font(.system(size: 12, weight: .semibold)).foregroundStyle(.white)
                        .padding(.horizontal, 12).frame(height: 27)
                        .background(Capsule().fill(Palette.solid))
                        .contentShape(Capsule())
                }
                .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
            } else {
                Button { sheet = .desktop(.add) } label: { label }
                    .buttonStyle(.pill(.primary, height: 27)).disabled(deskLocked)
                    .help("Claude 桌面会退出并打开登录页，当前登录会先存好")
            }
        case .both:
            Menu {
                Button("登录到 AuthBaton（Code 模式 / CLI 共用）…") { sheet = .account(.login(provider: "claude", accountID: nil)) }.disabled(codeLocked)
                Button("在 Claude 桌面登录新账号…") { sheet = .desktop(.add) }.disabled(deskLocked)
            } label: {
                label.font(.system(size: 12, weight: .semibold)).foregroundStyle(.white)
                    .padding(.horizontal, 12).frame(height: 27)
                    .background(Capsule().fill(Palette.solid))
                    .contentShape(Capsule())
            }
            .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        }
    }

    @ViewBuilder private func hints(_ use: ClaudeUse, directory: AccountDirectory) -> some View {
        let status = directory.claudeDesktop
        if use == .code, let status {
            let relay = directory.relay("claude")
            let deskOnly = status.known.filter { desk in desk.linkedAccount.map { id in !relay.contains { $0.id == id } } ?? true }
            if !deskOnly.isEmpty {
                HStack(spacing: 10) {
                    Text("另有 \(deskOnly.count) 个只存了桌面登录的账号（\(deskOnly.map(\.display).joined(separator: "、"))），补登后可用于 Claude Code")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).frame(maxWidth: .infinity, alignment: .leading)
                    Button("补登") { sheet = .account(.login(provider: "claude", accountID: nil)) }.buttonStyle(.pill(.soft))
                }
                .padding(.horizontal, 12).padding(.vertical, 7)
                .loopCard(radius: 10)
            }
        }
        if use.desk, let status, !status.onlyFolders.isEmpty {
            Text("Claude 桌面里还有 \(status.onlyFolders.count) 个只有会话、没保存登录的旧账号（\(status.onlyFolders.map(\.display).joined(separator: "、"))）；在 Claude 桌面登录一次就会保存。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 3.5)
        }
    }

    // managed: some account is signed in to AuthBaton, so its page has 预热 and 续期 to show.
    private func footer(_ use: ClaudeUse, status: ClaudeDesktopStatus?, managed: Bool, deck: Bool) -> some View {
        let showCode = use.code || status?.codeRelay?.enabled == true
        return VStack(alignment: .leading, spacing: 5) {
            if deck || showCode || managed {
                Text("点卡片看 5 小时窗口预热与续期。")
            }
            if use.desk, let feature = status?.tokenQuota, feature.enabled {
                HStack(spacing: 5) {
                    Text("只在桌面登录过的账号用 Claude 桌面的登录查额度。")
                    Button("关掉") { Task { await service.claudeDesktop("token_quota", enabled: false) } }.buttonStyle(.pill(.link))
                }
            }
            if use.desk, let relay = status?.codeRelay, relay.isInstalled {
                HStack(spacing: 5) {
                    Text("Claude 桌面 Code 无感换号已安装系统插件。")
                    Button("卸载插件…") { sheet = .codeRelayUninstall }.buttonStyle(.pill(.link))
                        .disabled(relay.busy || service.accountBusy)
                }
            }
            if use.desk, let chrome = status?.chromeSync, chrome.isInstalled {
                HStack(spacing: 5) {
                    Text("Chrome 登录同步扩展已配置。")
                    Button("管理…") { sheet = .chromeSync }.buttonStyle(.pill(.link))
                        .disabled(chrome.busy || service.accountBusy)
                    Button("卸载扩展…") { sheet = .chromeSyncUninstall }.buttonStyle(.pill(.link))
                        .disabled(chrome.busy || service.accountBusy)
                }
            }
            if use.desk {
                if let retention = status?.retention, retention.kept {
                    HStack(spacing: 5) {
                        Text("桌面会话的对话原文已改为一直保留。")
                        if retention.canUndo {
                            Button("撤销") { Task { await service.claudeDesktop("retention_undo") } }.buttonStyle(.pill(.link))
                        }
                    }
                }
                Text("同步的是 Claude 桌面自己的会话列表和登录文件；对话原文本来就在 ~/.claude/projects，CLI 和桌面共用。连接器、远程控制、置顶和侧栏分组按账号各自保留。会话同步与桌面换号移植自 cc-same（MIT）。")
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .font(.system(size: 11)).foregroundStyle(Palette.ink3)
        .padding(.horizontal, 3.5)
    }
}

// MARK: 桌面会话

// Claude 桌面's own problems, each with the one thing that fixes it.
struct DesktopNotices: View {
    @EnvironmentObject private var service: ServiceController
    let status: ClaudeDesktopStatus
    let locked: Bool
    // Inside the desktop's slot: one line each under the card, not banners over the page (11-22).
    var compact = false
    let open: (DesktopSheet) -> Void

    var body: some View {
        if let operation = status.operation, ["switch", "add"].contains(operation.kind), operation.running {
            notice(.sync, operation.kind == "add" ? "正在添加 Claude 桌面账号…" : "Claude 桌面正在换上 \(operation.target ?? "账号")…", progress: true) {
                Button("查看") { open(.progress) }.buttonStyle(.pill(compact ? .soft : .onAttention))
            }
        } else if let operation = status.operation, !operation.running, operation.ok == false, let message = operation.message {
            notice(.alert, message) {
                Button("知道了") { Task { await service.claudeDesktop("dismiss") } }.buttonStyle(.pill(.onAttention))
            }
        }
        if !status.installed {
            notice(.info, "这台 Mac 上没找到 Claude 桌面。装好并登录以后，会话同步才能用。") { EmptyView() }
        } else if !status.switchSupported {
            notice(.info, "这一版 Claude 桌面不能在 AuthBaton 里换号；会话同步照常。") { EmptyView() }
        }
        if let retention = status.retention, retention.atRisk {
            notice(.clock, retention.days.map { "Claude Code 会在 \($0) 天后清掉桌面会话的对话原文，清掉的会话打开是空的。" }
                   ?? "Claude Code 会定期清掉桌面会话的对话原文，清掉的会话打开是空的。") {
                if retention.canKeep {
                    Button("一直保留") { Task { await service.claudeDesktop("retention_keep") } }
                        .buttonStyle(.pill(.onAttention)).disabled(locked)
                }
            }
        }
        if status.symlinks > 0 {
            notice(.alert, "有 \(status.symlinks) 个会话文件夹是软链，Claude 桌面会存不住会话。") {
                Button("改回真文件夹") { Task { await service.claudeDesktop("fix_symlinks", confirm: true) } }
                    .buttonStyle(.pill(.onAttention)).disabled(locked)
            }
        }
    }

    @ViewBuilder private func notice<Actions: View>(_ glyph: Glyph, _ text: String, progress: Bool = false,
                                                    @ViewBuilder actions: () -> Actions) -> some View {
        if compact {
            HStack(spacing: 8) {
                if progress { ProgressView().controlSize(.mini) } else { Icon(glyph, size: 13, color: Palette.attention) }
                Text(text).font(.system(size: 11)).foregroundStyle(progress ? Palette.deep : Palette.attentionInk)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                HStack(spacing: 5) { actions() }
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .background(progress ? Palette.wash.opacity(0.6) : Palette.attentionWash)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
        } else {
            Banner(glyph: glyph, text: text) { actions() }
        }
    }
}

// The session sync, folded to one quiet line: how many sessions, whether every account matches, and its controls.
struct SyncStrip: View {
    @EnvironmentObject private var service: ServiceController
    let status: ClaudeDesktopStatus
    let locked: Bool
    // The bottom line of the desktop's slot rather than a card of its own.
    var embedded = false
    let open: (DesktopSheet) -> Void

    var body: some View {
        if embedded {
            // Half a window wide there is no room for the state beside the controls: it gets its own line.
            VStack(alignment: .leading, spacing: 3) {
                controls
                stateLine.padding(.leading, 32)
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .modifier(StripChrome(embedded: true))
        } else {
            controls
                .padding(.horizontal, 16).frame(minHeight: 52)
                .modifier(StripChrome(embedded: false))
        }
    }

    private var controls: some View {
        HStack(spacing: 12) {
            ZStack {
                Circle().strokeBorder(Palette.soft, lineWidth: 2.4).frame(width: embedded ? 13 : 18, height: embedded ? 13 : 18).offset(x: embedded ? -2 : -3)
                Circle().strokeBorder(Palette.brand, lineWidth: 2.4).frame(width: embedded ? 13 : 18, height: embedded ? 13 : 18).offset(x: embedded ? 2 : 3)
            }
            .frame(width: embedded ? 20 : 28, height: embedded ? 20 : 28)
            .accessibilityHidden(true)
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text("\(status.sessions)").font(.system(size: embedded ? 13 : 15, weight: .bold, design: .rounded)).monospacedDigit()
                Text("个桌面会话").font(.system(size: embedded ? 12 : 13, weight: .semibold))
            }
            .fixedSize()
            if embedded { Spacer(minLength: 6) } else { stateLine.frame(maxWidth: .infinity, alignment: .leading) }
            if !embedded { Text("后台同步").font(.system(size: 12)).foregroundStyle(Palette.ink2) }
            Toggle("后台同步", isOn: Binding(get: { status.background }, set: { value in
                Task { await service.claudeDesktop("background", enabled: value) }
            }))
            .labelsHidden().toggleStyle(.switch).controlSize(.small)
            .disabled(service.accountBusy || !status.installed)
            .help("AuthBaton 开着时，每个账号的桌面会话列表一直保持一样")
            Button(status.syncing ? "同步中" : "立即同步") { Task { await service.claudeDesktop("sync") } }
                .buttonStyle(.pill(.secondary))
                .disabled(locked || status.syncing)
            Button(embedded ? "快照 ›" : "快照与恢复 ›") { open(.snapshots) }.buttonStyle(.pill(.link))
                .help("快照与恢复")
        }
    }

    @ViewBuilder private var stateLine: some View {
        Group {
            if !status.installed {
                Text("没找到 Claude 桌面").foregroundStyle(Palette.ink3)
            } else if status.syncing {
                HStack(spacing: 5) { ProgressView().controlSize(.mini); Text("正在同步…") }.foregroundStyle(Palette.ink2)
            } else if status.restartNeeded {
                Text("要重启一次 Claude 才看得到新会话").foregroundStyle(Palette.attentionText)
            } else if let behind = status.known.first(where: { $0.behind > 0 }) {
                Text("\(behind.display) 差 \(behind.behind) 个" + (status.background ? "，马上补上" : "；打开后台同步或立即同步"))
                    .foregroundStyle(Palette.ink2)
            } else if status.pending > 0 {
                Text("还有 \(status.pending) 处没同步" + (status.background ? "，马上补上" : "；打开后台同步或立即同步"))
                    .foregroundStyle(Palette.ink2)
            } else if let last = status.lastSync {
                if last.ok {
                    Text("各账号一样 · \(When.text(last.at)) 同步").foregroundStyle(Palette.ink3).help(When.exact(last.at))
                } else {
                    Text(last.message).foregroundStyle(Palette.attentionText).help(When.exact(last.at))
                }
            } else {
                Text("还没同步过").foregroundStyle(Palette.ink3)
            }
        }
        .font(.system(size: 12)).lineLimit(1)
    }
}

// Claude Desktop Code tab seamless same-Team account switching, folded to one quiet row beside SyncStrip.
struct CodeRelayStrip: View {
    @EnvironmentObject private var service: ServiceController
    let relay: ClaudeDesktopStatus.CodeRelayStatus
    let locked: Bool
    // The bottom line of the Code slot rather than a card of its own.
    var embedded = false
    let open: (ClaudeSheet) -> Void

    var body: some View {
        if embedded {
            VStack(alignment: .leading, spacing: 3) {
                controls
                stateLine.padding(.leading, 32)
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .modifier(StripChrome(embedded: true))
        } else {
            controls
                .padding(.horizontal, 16).frame(minHeight: 52)
                .modifier(StripChrome(embedded: false))
        }
    }

    private var controls: some View {
        HStack(spacing: 12) {
            ZStack {
                Circle()
                    .fill(relay.state == "active" ? Palette.wash : Palette.chip)
                    .frame(width: embedded ? 20 : 28, height: embedded ? 20 : 28)
                Icon(.relay, size: embedded ? 11 : 14, color: relay.state == "active" ? Palette.brand : Palette.ink3, weight: 2)
            }
            .accessibilityHidden(true)
            Text("Code 无感换号")
                .font(.system(size: embedded ? 12 : 13, weight: .semibold))
                .fixedSize()
            if embedded { Spacer(minLength: 6) } else { stateLine.frame(maxWidth: .infinity, alignment: .leading) }
            if relay.enabled && relay.state == "waiting_account" && service.directory?.relay("claude").isEmpty == true {
                Button("登录 Code 账号") {
                    open(.account(.login(provider: "claude", accountID: nil)))
                }
                .buttonStyle(.pill(.primary))
                .disabled(locked || service.directory?.busy == true)
            }
            if relay.enabled && relay.installState == "needs_update" {
                Button("更新插件") {
                    Task { await service.claudeDesktop("code_relay_install", confirm: true) }
                }
                .buttonStyle(.pill(.secondary))
                .disabled(locked)
            }
            if relay.installState == "blocked_remote" {
                Text("组织策略限制").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            } else if relay.enabled {
                Toggle("Code 无感换号", isOn: Binding(get: { relay.enabled }, set: { value in
                    if !value {
                        Task { await service.claudeDesktop("code_relay", enabled: false) }
                    }
                }))
                .labelsHidden().toggleStyle(.switch).controlSize(.small)
                .disabled(locked)
                .help("关闭后立即回退为 Claude 桌面默认登录")
            } else if relay.installState == "installed" {
                Toggle("Code 无感换号", isOn: Binding(get: { relay.enabled }, set: { value in
                    if value {
                        Task { await service.claudeDesktop("code_relay", enabled: true, confirm: true) }
                    }
                }))
                .labelsHidden().toggleStyle(.switch).controlSize(.small)
                .disabled(locked)
                .help("在同 Team 账号间无感切换，不重启 Claude 桌面、不中断 Code 对话")
            } else {
                Button("开启…") { open(.codeRelay) }
                    .buttonStyle(.pill(.secondary))
                    .disabled(locked)
                    .help("首次开启会说明原理并请求一次系统管理员授权")
            }
        }
    }

    @ViewBuilder private var stateLine: some View {
        Group {
            if relay.state == "installing" {
                HStack(spacing: 5) {
                    ProgressView().controlSize(.mini)
                    Text("正在安装系统插件，请在弹窗输入管理员密码…")
                }.foregroundStyle(Palette.ink2)
            } else if relay.state == "uninstalling" {
                HStack(spacing: 5) {
                    ProgressView().controlSize(.mini)
                    Text("正在卸载系统插件，请在弹窗输入管理员密码…")
                }.foregroundStyle(Palette.ink2)
            } else if relay.state == "active" {
                let countNote = relay.relayedCount > 0 ? " · 已发 \(relay.relayedCount) 次请求" : ""
                Text("当前账号 \(relay.activeLabel ?? "—")\(countNote)")
                    .foregroundStyle(Palette.ink2)
                    .help("切 Claude Code 账号时，桌面 Code 对话会立即切过去，无需重启桌面")
            } else if ["team_mismatch", "blocked_remote", "error"].contains(relay.state) {
                Text(relay.message).foregroundStyle(Palette.attentionText).help(relay.message)
            } else {
                Text(relay.message).foregroundStyle(Palette.ink3).help(relay.message)
            }
        }
        .font(.system(size: 12)).lineLimit(1)
    }
}

// A strip on its own is a card; inside a slot it is the slot's bottom line.
struct StripChrome: ViewModifier {
    let embedded: Bool

    func body(content: Content) -> some View {
        if embedded {
            content.overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
        } else {
            content.loopCard()
        }
    }
}

// MARK: 账号卡

struct ClaudeEntryCard: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    let entry: ClaudeEntry
    let use: ClaudeUse
    let directory: AccountDirectory
    let open: (ClaudeSheet) -> Void
    let switchTo: (SwitchScope?) -> Void
    // Inside a slot the slot draws the card.
    var chrome = true
    // Where the rules take Code next.
    var next = false
    // Opened in place from a queue row: folds back into the row.
    var collapse: (() -> Void)? = nil

    var body: some View {
        let rule = entry.cli.flatMap { directory.rule(for: $0.id) }
        let codeRelayOn = directory.claudeDesktop?.codeRelay?.enabled == true
        let showCode = use.code || codeRelayOn
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 10) {
                    Button { nav.claudeDetail = entry.id } label: {
                        HStack(spacing: 10) {
                            ClaudeSeal(entry: entry, size: 31, rule: rule)
                            VStack(alignment: .leading, spacing: 1) {
                                HStack(spacing: 6) {
                                    Text(accountName: entry.name).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                                    if let cli = entry.cli { CloudBadge(account: cli) }
                                    InUsePill(entry: entry, use: use, codeRelayOn: codeRelayOn)
                                    if let cli = entry.cli { LowPill(account: cli, notch: directory.notch(for: cli)) }
                                }
                                Text(subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                            }
                            Spacer(minLength: 0)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("\(entry.name) · 点开看账号详情")
                    if let collapse {
                        Button(action: collapse) {
                            Icon(.chevronDown, size: 11, color: Palette.ink3, weight: 2).rotationEffect(.degrees(180))
                                .frame(width: 26, height: 26).contentShape(Circle())
                        }
                        .buttonStyle(.plain)
                        .help("收起")
                        .accessibilityLabel("收起 \(entry.name)")
                    }
                    ClaudeEntryMenu(entry: entry, use: use, directory: directory, open: open)
                }
                if let cli = entry.cli {
                    QuotaFigures(account: cli, notch: directory.notch(for: cli), size: 36, inlineCaption: true,
                                 refillWhenLow: true, extras: cli.extraWindows) {
                        ChancesTicket(account: cli)
                        if cli.warmup?.enabled == true {
                            HStack(spacing: 4) {
                                Icon(.sun, size: 11, color: Palette.ink2)
                                Text("预热")
                            }
                            .font(.system(size: 11)).foregroundStyle(Palette.ink2)
                            .padding(.horizontal, 7).frame(height: 19)
                            .background(Palette.chip, in: Capsule())
                            .help("开着 5 小时窗口预热")
                        }
                        if use.desk && entry.desk == nil {
                            if directory.claudeDesktop?.codeRelay?.state == "active" {
                                Text("Code 无感可用").font(.system(size: 11)).foregroundStyle(Palette.deep).lineLimit(1)
                                    .help(entry.inCode ? "已开启 Code 无感换号：桌面 Code 对话已经在用这个账号，无需在桌面重复登录"
                                          : "已开启 Code 无感换号：同 Team 账号无需在桌面重复登录，点「换上」即可在桌面 Code 对话里直接切换")
                            } else {
                                Text("还没在桌面登录过").font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                            }
                        }
                    }
                    .padding(.top, 14)
                } else if let desk = entry.desk {
                    DeskOnlyQuota(desk: desk, login: { open(.account(.login(provider: "claude", accountID: nil))) }, turnOn: { open(.tokenQuota) })
                        .padding(.top, 14)
                    if use.desk && !desk.participates {
                        Text("不参与「下一个」").font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 9)
                    }
                }
            }
            .padding(.horizontal, 14).padding(.top, 15).padding(.bottom, 14)
            Spacer(minLength: 0)
            if let cli = entry.cli, cli.transferFailed != nil { TransferBox(account: cli).padding(.horizontal, 14).padding(.bottom, 10) }
            if hasFooter(rule, codeRelayOn: codeRelayOn) { footer(rule, showCode: showCode) }
            if let cli = entry.cli, showCode {
                AccountStrips(account: cli, directory: directory, open: { open(.account($0)) }).padding(.horizontal, 14)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .modifier(CardChrome(on: chrome, raised: entry.inUse))
        .overlay {
            if chrome, entry.cli?.transferFailed != nil {
                RoundedRectangle(cornerRadius: 17, style: .continuous).strokeBorder(Palette.attention, lineWidth: 1.5)
            }
        }
    }

    // The desktop's number, the relay rank, a reset card or a way to switch; without any of them no empty row.
    private func hasFooter(_ rule: CodexStrategyConfig.Rule?, codeRelayOn: Bool) -> Bool {
        let plan = ClaudeSwitchPlan(entry, use: use, directory: directory, service: service)
        let showCode = use.code || codeRelayOn
        return (use.desk && entry.desk != nil) || (showCode && rule != nil && entry.cli.flatMap(directory.rank(of:)) != nil)
            || entry.cli?.nextReset != nil || plan.any || (use == .desktop && !codeRelayOn && entry.desk == nil)
    }

    private func footer(_ rule: CodexStrategyConfig.Rule?, showCode: Bool) -> some View {
            HStack(spacing: 5) {
                if use.desk, let desk = entry.desk { DeskNumber(desk: desk) }
                if showCode, let cli = entry.cli, let rule {
                    if let rank = directory.rank(of: cli) {
                        let order = "自动接力：≤ \(rule.below)% · \(rule.order.label)"
                        if next && rule.participate {
                            PointerRank(text: "第 \(rank) 位", small: false, help: PointerRank.meaning + "\n" + order)
                        } else {
                            Pill("第 \(rank) 位", style: rule.participate ? .neutral : .dashed)
                                .help(order + (rule.participate ? "" : " · 不参与"))
                        }
                    }
                }
                Spacer(minLength: 7)
                if let cli = entry.cli { AccountActions(account: cli, directory: directory, open: { open(.account($0)) }, showSwitch: false, prominent: cli.weekSpent != nil) }
                ClaudeSwitchControl(entry: entry, use: use, directory: directory, switchTo: switchTo,
                                    login: { open(.desktop(.add)) })
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
    }

    private var subtitle: String {
        if let cli = entry.cli { return cli.subtitle.isEmpty ? " " : cli.subtitle }
        return entry.desk?.email ?? "只在 Claude 桌面登录过"
    }
}

struct CardChrome: ViewModifier {
    let on: Bool
    let raised: Bool

    func body(content: Content) -> some View {
        if on { content.loopCard(radius: 17, raised: raised) } else { content }
    }
}

// Seal: in use on either side is solid; an account with only a desktop login is dashed.
struct ClaudeSeal: View {
    let entry: ClaudeEntry
    var size: CGFloat = 31
    var rule: CodexStrategyConfig.Rule? = nil

    var body: some View {
        Seal(entry.name, style: entry.cli == nil ? .out : entry.inUse ? .inUse : rule?.participate == false ? .out : .standby, size: size)
    }
}

// 在用, with the client's glyph when both are in use on this Mac (or Code seamless switching is on).
struct InUsePill: View {
    let entry: ClaudeEntry
    let use: ClaudeUse
    var codeRelayOn: Bool = false

    var body: some View {
        let showCode = use.code || codeRelayOn
        let dual = use == .both || codeRelayOn
        let code = showCode && entry.inCode
        let desk = use.desk && entry.inDesk
        if code || desk {
            HStack(spacing: 3) {
                if dual && code { Icon(.terminal, size: 10, color: Palette.deep, weight: 2.2) }
                if dual && desk { Icon(.chat, size: 10, color: Palette.deep, weight: 2) }
                Text("在用")
            }
            .font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.deep)
            .padding(.horizontal, 7).frame(height: 17)
            .background(Palette.wash, in: Capsule())
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(dual ? [code ? "Claude Code" : nil, desk ? "Claude 桌面" : nil].compactMap { $0 }.joined(separator: "和") + "在用" : "在用")
        }
    }
}

// 已见底: a main window is at or under the account's 见底线, so the rules will not switch to it.
struct LowPill: View {
    let account: ManagedAccount
    let notch: Int?

    var body: some View {
        let windows = Array(account.primaryWindows.prefix(2))
        if let notch, let last = windows.indexRefillingLast(below: notch) {
            Text("已见底")
                .font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.attentionText)
                .lineLimit(1).fixedSize()
                .padding(.horizontal, 7).frame(height: 17)
                .background(Palette.attentionFill, in: Capsule())
                .help(help(windows.filter { $0.remaining <= notch }, waited: windows[last], notch: notch))
        }
    }

    private func help(_ low: [AccountQuota], waited: AccountQuota, notch: Int) -> String {
        let name = { (window: AccountQuota) in window.shortLabel.trimmingCharacters(in: .whitespaces) }
        if low.count > 1 {
            return low.map { "\(name($0))剩 \($0.remaining)%" }.joined(separator: "、") + "，都到了见底线 \(notch)%："
                + (account.native ? "" : "自动换号不会换到它；两个都回满才能再用，")
                + "最晚回满的是 \(name(waited))额度，\(waited.refill())。"
        }
        return "\(name(waited))剩 \(waited.remaining)%，到了见底线 \(notch)%：" + (account.native ? "" : "自动换号不会换到它，") + "\(waited.refill())。"
    }
}

// The desktop's number for 下一个账号; dashed when the account sits out.
struct DeskNumber: View {
    let desk: ClaudeDesktopStatus.Account

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 5, style: .continuous)
        Text(desk.number.map { "#\($0)" } ?? "–")
            .font(.system(size: 11, weight: .semibold, design: .rounded)).monospacedDigit()
            .foregroundStyle(desk.participates ? Palette.ink2 : Palette.ink3)
            .padding(.horizontal, 5).frame(minWidth: 19, minHeight: 19)
            .background { if desk.participates { shape.fill(Palette.chip) } }
            .overlay { if !desk.participates { shape.strokeBorder(Palette.dashStrong, style: StrokeStyle(lineWidth: 1, dash: [3, 2])) } }
            .help(desk.participates ? "Claude 桌面的「下一个账号」按这个编号轮换" : "不参与「下一个」，仍可直接换上")
    }
}

// An account only Claude 桌面 knows: AuthBaton has no login to ask for its quota with.
struct DeskOnlyQuota: View {
    @EnvironmentObject private var service: ServiceController
    let desk: ClaudeDesktopStatus.Account
    let login: () -> Void
    let turnOn: () -> Void

    private var feature: ClaudeDesktopStatus.TokenQuotaStatus? { service.directory?.claudeDesktop?.tokenQuota }

    var body: some View {
        if let quota = desk.tokenQuota, !quota.primary.isEmpty {
            figures(quota)
        } else {
            placeholder
        }
    }

    // Quota read through Claude 桌面's sign-in: the same figures an AuthBaton login gets, and where they came from.
    private func figures(_ quota: ClaudeDesktopStatus.DeskQuota) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            EqualColumns(spacing: 20) {
                ForEach(Array(quota.primary.enumerated()), id: \.offset) { _, window in
                    VStack(alignment: .leading, spacing: 0) {
                        HStack(alignment: .firstTextBaseline, spacing: 7) {
                            Figure(value: window.remaining, size: 36)
                            ViewThatFits(in: .horizontal) {
                                Text(window.remainingCaption)
                                Text(window.shortLabel.trimmingCharacters(in: .whitespaces))
                            }
                            .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                        }
                        QuotaBar(remaining: window.remaining).padding(.top, 10)
                        Text(window.refill()).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1).padding(.top, 7)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            HStack(spacing: 6) {
                Icon(.desktop, size: 11, color: Palette.ink3)
                Text("用 Claude 桌面的登录查" + (quota.checkedAt.map { " · \(When.text($0))" } ?? ""))
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                    .help(quota.message)
                Spacer(minLength: 6)
                Button("补登", action: login).buttonStyle(.pill(.soft))
                    .help("用这个账号登录 AuthBaton，之后还能用重置卡、预热和自动换号")
            }
        }
    }

    private var placeholder: some View {
        let enabled = feature?.enabled == true
        let message = desk.tokenQuota?.message ?? (enabled ? feature?.message : nil)
        let stuck = feature?.state == "key_denied" || ["unauthorized", "error"].contains(desk.tokenQuota?.state ?? "")
        return HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(message ?? seen ?? "只在桌面登录过，读不到额度").font(.system(size: 12, weight: .medium)).foregroundStyle(Palette.ink2)
                    .fixedSize(horizontal: false, vertical: true)
                Group {
                    if enabled && desk.tokenQuota?.state == "no_token" {
                        // A new account has no Claude Code login in Claude 桌面 until its first Code-mode
                        // conversation; the service reads again every 5 minutes. Too narrow for one line, it
                        // breaks at the comma rather than leaving 刷新 alone.
                        ViewThatFits(in: .horizontal) {
                            Text("去 Claude 桌面用 Code 模式聊一次，5 分钟内自动刷新")
                            Text("去 Claude 桌面用 Code 模式聊一次，\n5 分钟内自动刷新")
                        }
                    } else {
                        Text(enabled ? "补登一次就能用重置卡和预热" : "用桌面登录查，或补登")
                    }
                }
                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if !enabled {
                Button("用桌面登录查", action: turnOn).buttonStyle(.pill(.secondary))
                    .help("用 Claude 桌面的登录查这个账号的额度；打开前会说明做什么")
            } else if stuck {
                Button("重试") { Task { await service.claudeDesktop("token_quota_retry") } }.buttonStyle(.pill(.secondary))
            }
            Button("补登", action: login).buttonStyle(.pill(.soft))
                .help("用这个账号登录 AuthBaton，之后就能按账号查额度、用重置卡和预热")
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .frame(minHeight: 68)
        .loopPanel(radius: 11)
    }

    // What Claude 桌面 itself last read, when it could tell this account's usage apart.
    private var seen: String? {
        let parts = [desk.fiveHour.map { "5 小时 \($0.remaining)%" }, desk.weekly.map { "每周 \($0.remaining)%" }].compactMap { $0 }
        return parts.isEmpty ? nil : "Claude 桌面上次读到：" + parts.joined(separator: " · ")
    }
}

// Turning on quota through Claude 桌面's sign-in: what AuthBaton will read and send, and what it never does.
struct TokenQuotaSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void
    @State private var working = false

    var body: some View {
        SheetScaffold(title: "用 Claude 桌面的登录查额度？", lead: "给只在 Claude 桌面登录过的账号查额度，不用补登。", width: 442) {
            VStack(alignment: .leading, spacing: 7) {
                CheckItem(kind: .you, text: "macOS 会问能不能让 AuthBaton 使用「Claude Safe Storage」，点「允许」。AuthBaton 重启或更新后会再问一次")
                CheckItem(kind: .system, text: "用这把钥匙读出 Claude 桌面登录里的令牌，每 5 分钟查一次额度；钥匙只在内存里，不存盘")
                CheckItem(kind: .info, text: "只查额度：不续期、不用重置卡、不发模型请求，也不改 Claude 的任何文件")
                CheckItem(kind: .info, text: "已经补登过的账号照旧用自己的登录；随时可以在 Claude 页底部关掉")
            }
            if let error = service.accountError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
            }
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button(working ? "正在打开…" : "打开") {
                working = true
                Task {
                    await service.claudeDesktop("token_quota", enabled: true, confirm: true)
                    working = false
                    if service.accountError == nil { close() }
                }
            }
            .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            .disabled(working || service.accountBusy)
        }
        .onAppear { service.clearAccountError() }
    }
}

// Turning on Claude Desktop Code tab seamless same-Team switching: clear explanation of admin prompt,
// normal managed-policy notice, native direct connection with short-lived access token lease, and Fail-Open.
struct CodeRelaySheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void
    @State private var working = false

    private var relay: ClaudeDesktopStatus.CodeRelayStatus? { service.directory?.claudeDesktop?.codeRelay }
    private var needsAdmin: Bool { relay?.installState != "installed" }

    var body: some View {
        SheetScaffold(title: "开启 Claude 桌面 Code 无感换号？",
                      lead: "在同一个 Team 的账号之间切换，不用重启 Claude 桌面，正在进行的 Code 对话也不会中断。",
                      width: 468) {
            VStack(alignment: .leading, spacing: 8) {
                CheckItem(kind: .you, text: "首次开启会弹出一次 macOS 管理员密码框，用来在系统目录登记 AuthBaton 插件（只作用于 Claude 桌面 Code 标签）")
                CheckItem(kind: .info, text: "开启后 Claude 桌面可能提示「已加载托管策略/本机托管配置」，这是系统级插件生效后的正常提示，不影响日常使用，也不会覆盖你的其他设置")
                CheckItem(kind: .system, text: "插件通过本机私密通道获取当前同 Team 账号的短期访问令牌（不触碰刷新令牌、不写盘、不改环境变量），仍由 Claude 桌面内置引擎原生直连官方接口；切账号后下一轮立即生效，遇到限流还可自动无感切到下一个同 Team 账号继续回复")
                CheckItem(kind: .info, text: "桌面壳只需登录 1 个该 Team 账号；其他同 Team 账号登录到 AuthBaton 即可在桌面 Code 模式无感切换，日后使用终端 CLI 也可直接复用。跨 Team 或 AuthBaton 退出时自动回退桌面默认登录；随时可以关掉开关，或在 Claude 页底部卸载插件")
            }
            if let error = service.accountError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
            }
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button(working ? "正在开启…" : needsAdmin ? "授权并开启" : "开启") {
                working = true
                Task {
                    await service.claudeDesktop("code_relay", enabled: true, confirm: true)
                    working = false
                    if service.accountError == nil { close() }
                }
            }
            .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            .disabled(working || service.accountBusy)
        }
        .onAppear { service.clearAccountError() }
    }
}

struct CodeRelayUninstallSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void
    @State private var working = false

    var body: some View {
        SheetScaffold(title: "卸载 Code 无感换号插件？",
                      lead: "会关闭无感换号，并清除 AuthBaton 写入系统目录的插件与登记项。",
                      width: 442) {
            VStack(alignment: .leading, spacing: 7) {
                CheckItem(kind: .you, text: "会弹出一次 macOS 管理员密码框，用来清理 /Library/Application Support/ClaudeCode/ 里的 AuthBaton 插件")
                CheckItem(kind: .info, text: "managed-settings.json 里你自己的其他设置会原样保留；若没有其他设置，文件会被一并清理，Claude 桌面的托管配置提示也会消失")
                CheckItem(kind: .info, text: "卸载后 Claude 桌面立即恢复默认直连，不影响已有的聊天记录和账号登录")
            }
            if let error = service.accountError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
            }
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button(working ? "正在卸载…" : "授权并卸载") {
                working = true
                Task {
                    await service.claudeDesktop("code_relay_uninstall", confirm: true)
                    working = false
                    if service.accountError == nil { close() }
                }
            }
            .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            .disabled(working || service.accountBusy)
        }
        .onAppear { service.clearAccountError() }
    }
}

// MARK: 换上

// 换上 on a card. One client in use: a plain button. Both: the left half switches with the remembered
// choice, ▾ picks 一起换 / 只换 Code / 只换桌面 (and remembers it).
struct ClaudeSwitchControl: View {
    @EnvironmentObject private var service: ServiceController
    let entry: ClaudeEntry
    let use: ClaudeUse
    let directory: AccountDirectory
    let switchTo: (SwitchScope?) -> Void
    let login: () -> Void

    var body: some View {
        let plan = ClaudeSwitchPlan(entry, use: use, directory: directory, service: service)
        let codeRelayOn = directory.claudeDesktop?.codeRelay?.enabled == true
        if use == .desktop && !codeRelayOn && entry.desk == nil {
            Button("去桌面登录", action: login).buttonStyle(.pill(.soft))
                .help("Claude 桌面会退出并打开登录页，用这个账号登录一次")
                .disabled(service.accountBusy || directory.claudeDesktop?.busy == true)
        } else if (use == .both || codeRelayOn) && plan.choice {
            HStack(spacing: 0) {
                Button { switchTo(nil) } label: { Text(title(plan.targets(remembered), codeRelayOn: codeRelayOn)).padding(.leading, 11).padding(.trailing, 9).frame(height: 24).contentShape(Rectangle()) }
                    .buttonStyle(.plain)
                    .help(help(plan, codeRelayOn: codeRelayOn))
                Rectangle().fill(Palette.edge).frame(width: 1, height: 24)
                Menu {
                    ForEach(SwitchScope.allCases, id: \.self) { option in
                        Button(menuTitle(option, codeRelayOn: codeRelayOn)) { switchTo(option) }
                    }
                } label: {
                    Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4).frame(width: 24, height: 24).contentShape(Rectangle())
                }
                .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
                .accessibilityLabel("换到哪里")
            }
            .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink)
            .background(Capsule().fill(Palette.card))
            .overlay(Capsule().strokeBorder(Palette.edge, lineWidth: 1))
        } else if plan.any {
            let targets = plan.targets(.both)
            Button(title(targets, codeRelayOn: codeRelayOn)) { switchTo(nil) }
                .buttonStyle(.pill(.secondary))
                .help(targets.code && targets.desk ? "Claude Code 和 Claude 桌面一起换" : targets.code ? (codeRelayOn ? "无感换给桌面 Code 与 Claude Code：不重启桌面，下一轮立即生效" : "换给 Claude Code：不用退出，约 30 秒生效") : "换给 Claude 桌面：会退出再打开")
        }
    }

    private var remembered: SwitchScope { SwitchScope.remembered(UserDefaults.standard.string(forKey: ClaudeUse.scopeKey), directory) }

    // Where a click goes, said on the button (11-22): 换到 Code (the desktop's Code tab follows), 换到 CLI,
    // 换到桌面… or 一起换…. With one client on this Mac there is nowhere else to say, so it stays 换上.
    private func title(_ targets: (code: Bool, desk: Bool), codeRelayOn: Bool) -> String {
        guard use.desk && (use.code || codeRelayOn) else { return targets.desk ? "换上…" : "换上" }
        if targets.code && targets.desk { return "一起换…" }
        if targets.desk { return "换到桌面…" }
        return codeRelayOn ? "换到 Code" : "换到 CLI"
    }

    private func menuTitle(_ scope: SwitchScope, codeRelayOn: Bool) -> String {
        switch scope {
        case .both: "一起换…"
        case .code: codeRelayOn ? "只换 Code（无感）" : "只换 Claude Code"
        case .desktop: "只换 Claude 桌面…"
        }
    }

    private func help(_ plan: ClaudeSwitchPlan, codeRelayOn: Bool) -> String {
        let targets = plan.targets(remembered)
        return targets.code && targets.desk ? "按上次的选择：Code 和桌面一起换" : targets.code ? (codeRelayOn ? "按上次的选择：只换 Code（无感，不重启桌面）" : "按上次的选择：只换 Claude Code") : "按上次的选择：只换 Claude 桌面"
    }
}

// The card's … menu: the account's own actions, then the desktop's.
struct ClaudeEntryMenu: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    let entry: ClaudeEntry
    let use: ClaudeUse
    let directory: AccountDirectory
    let open: (ClaudeSheet) -> Void

    var body: some View {
        let status = directory.claudeDesktop
        let count = status?.known.count ?? 0
        Menu {
            Button("账号详情") { nav.claudeDetail = entry.id }
            if let cli = entry.cli {
                let querying = service.quotaRequests.contains(cli.id) || cli.usage?.refreshing == true
                Button(querying ? "正在查询额度…" : "查询额度") { Task { await service.checkQuota(id: cli.id) } }.disabled(querying)
                if cli.cloudRenewed {
                    Button("云端状态检查") { Task { await service.cloudAction("refresh") } }.disabled(directory.cloud?.busy == true)
                }
                let joined = directory.rule(for: cli.id)?.participate == true
                Button(joined ? "移出自动接力" : "加入自动接力") {
                    if let config = directory.relayConfig(cli.id, participate: !joined) { Task { await RelayRules.save(config, service) } }
                }
                .disabled(directory.strategy == nil)
            }
            if use.desk, let desk = entry.desk {
                Divider()
                Button(desk.participates ? "不参与「下一个」" : "参与「下一个」") {
                    Task { await service.claudeDesktop(desk.participates ? "disable" : "enable", account: desk.handle) }
                }
                if count > 1 {
                    Menu("移到第几位") {
                        ForEach(1...count, id: \.self) { number in
                            Button("第 \(number) 位") { Task { await service.claudeDesktop("move", account: desk.handle, number: number) } }
                                .disabled(number == desk.number)
                        }
                    }
                }
            }
            Divider()
            if let cli = entry.cli {
                // For an account the centre renews, logging in again updates the centre's login.
                if cli.cloudRenewed {
                    Button("更新云端登录…") { open(.account(.login(provider: "claude", accountID: cli.id))) }
                    Button("转回本机续期…") { open(.account(.toLocal(cli.id))) }
                } else {
                    Button("重新登录…") { open(.account(.login(provider: "claude", accountID: cli.id))) }
                    Button("转到云端续期…") { open(.account(.toCloud(cli.id))) }
                }
                Button("重命名…") { open(.account(.rename(cli.id))) }
                Button("执行记录") { open(.account(.history(cli.id))) }
            } else if let desk = entry.desk {
                // Claude 桌面's own sign-in files never leave this Mac.
                Button {} label: {
                    Text("转到云端续期…")
                    Text("仅在 Claude 桌面登录，无法交由云端续期")
                }
                .disabled(true)
                Button("重命名…") { open(.desktop(.rename(desk.handle))) }
            }
            Divider()
            if let cli = entry.cli {
                if cli.cloudRenewed {
                    Button("从认证中心移除…", role: .destructive) { open(.account(.cloudRemove(cli.id))) }
                        .disabled(cli.removeBlocker(directory) != nil)
                } else {
                    Button("从 AuthBaton 移除…", role: .destructive) { open(.account(.remove(cli.id))) }
                        .disabled(cli.removeBlocker(directory) != nil)
                }
            }
            if use.desk, let desk = entry.desk {
                Button("移除桌面登录…", role: .destructive) { open(.desktop(.remove(desk.handle))) }.disabled(desk.inUse)
            }
        } label: {
            MoreDots(color: Palette.ink3).frame(width: 26, height: 26).contentShape(Circle())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        .disabled(service.accountBusy || directory.busy || directory.recoveryNeeded || status?.busy == true || entry.cli?.transferRunning == true)
        .accessibilityLabel("\(entry.name) 的更多操作")
    }
}

// MARK: 换号确认

// Claude 桌面 is involved, so the switch is confirmed. When both sides can move the sheet offers the
// choice and remembers it; otherwise it says which side catches up.
struct ClaudeSwitchSheet: View {
    @EnvironmentObject private var service: ServiceController
    @AppStorage(ClaudeUse.scopeKey) private var scopeSetting = ""
    let request: ClaudeSwitchRequest
    let close: () -> Void
    let login: () -> Void
    @State private var picked: SwitchScope?
    @State private var working = false
    @State private var started = false

    private var directory: AccountDirectory? { service.directory }
    private var entry: ClaudeEntry? {
        guard let directory else { return nil }
        return ClaudeEntry.find(request.entryID, in: ClaudeEntry.all(directory, use: .both))
    }
    private var scope: SwitchScope { picked ?? SwitchScope.remembered(scopeSetting, directory) }
    private var moves: (code: Bool, desk: Bool) {
        guard request.choice else { return (request.code, request.desk) }
        return (scope != .desktop, scope != .code)
    }

    var body: some View {
        if started {
            DesktopSheetView(item: .progress, close: close)
        } else {
            confirm
        }
    }

    private var confirm: some View {
        let name = entry?.name ?? "这个账号"
        let status = directory?.claudeDesktop
        let codeFrom = directory?.current("claude")?.displayName
        let deskFrom = status?.current?.display
        return SheetScaffold(title: "换上 \(name)", lead: entry?.cli?.subtitle ?? entry?.desk?.email) {
            if request.choice {
                VStack(alignment: .leading, spacing: 6) {
                    PillSegments(segments: SwitchScope.allCases.map { .init(value: $0, title: $0.title) },
                                 selection: scope, select: { picked = $0 }, fill: true)
                    Text("下次点「换上」默认按这次的选择来").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            } else if request.desk && !request.code, entry?.inCode == true {
                Text("Claude Code 已在用 \(name)，这次把桌面也换过来。").font(.system(size: 12)).foregroundStyle(Palette.ink2)
            }
            VStack(alignment: .leading, spacing: 10) {
                if moves.code {
                    let note = status?.codeRelay?.enabled == true && !moves.desk
                        ? "不用退出客户端：同 Team 的 Claude 桌面 Code 对话下一轮立即生效，终端 CLI 约 30 秒生效。"
                        : "不用退出 CLI，约 30 秒生效；被额度打断的会话按一次回车继续。"
                    impact(.terminal, "Claude Code", from: codeFrom, to: name, note: note)
                }
                if moves.code && moves.desk { Rectangle().fill(Palette.divider).frame(height: 1) }
                if moves.desk {
                    impact(.desktop, "Claude 桌面", from: deskFrom, to: name, note: "会退出再打开 Claude 桌面；有任务在跑时 Claude 会先问你。现在的登录会原样存好，随时能换回来。")
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .loopPanel()
            if moves.desk, let status, status.turnsOnBypass(entry?.desk) {
                Banner(glyph: .info, text: "换过去会顺带给这个账号打开 Bypass 权限模式。")
            }
            if moves.desk, status?.codeRelay?.enabled == true {
                if entry?.cli == nil {
                    // Only signed in to Claude 桌面: 无感换号 has no Code login to lend, so this switch restarts.
                    Banner(glyph: .relay, text: "\(name) 还没登录 Code 账号，这次只能重启 Claude 桌面。") {
                        Button("登录 Code 账号", action: login)
                            .buttonStyle(.pill(.onAttention, height: 20))
                            .disabled(working || service.accountBusy || directory?.busy == true)
                    }
                } else if entry?.inCode != true {
                    Banner(glyph: .relay, text: "你已开启 Code 无感换号：若与桌面在同一 Team，只换 Claude Code 即可让桌面 Code 对话立即切换，无需重启桌面。")
                }
            }
            if let error = service.accountError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
            }
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button(working ? "正在换…" : moves.desk ? "换上" : "立即换上") { go() }
                .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
                .disabled(working || service.accountBusy || entry == nil || !(moves.code || moves.desk))
        }
        .onAppear { service.clearAccountError() }
    }

    private func impact(_ glyph: Glyph, _ title: String, from: String?, to: String, note: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Icon(glyph, size: 14, color: Palette.ink2).padding(.top, 1.5)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 5) {
                    Text(title).font(.system(size: 12, weight: .semibold))
                    if let from, from != to { Text("\(from) → \(to)").font(.system(size: 11)).foregroundStyle(Palette.ink3) }
                }
                Text(note).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func go() {
        guard let entry else { return }
        let moves = self.moves
        if request.choice { scopeSetting = scope.rawValue }
        working = true
        Task {
            // Claude Code first: it takes a second and needs nothing from Claude 桌面.
            if moves.code, let cli = entry.cli {
                await service.accountOperation("accounts.activate", params: LoginParams(id: cli.id, confirm: true))
                guard service.accountError == nil else { working = false; return }
            }
            if moves.desk, let desk = entry.desk {
                await service.claudeDesktop("switch", account: desk.handle, confirm: true)
                working = false
                if service.accountError == nil { started = true }
            } else {
                working = false
                if service.accountError == nil { close() }
            }
        }
    }
}
