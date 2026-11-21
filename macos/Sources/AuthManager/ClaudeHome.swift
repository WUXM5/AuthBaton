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

    var id: String {
        switch self {
        case .account(let sheet): "account-\(sheet.id)"
        case .desktop(let sheet): "desktop-\(sheet.id)"
        case .switching(let request): "switch-\(request.id)"
        case .tokenQuota: "token-quota"
        case .codeRelay: "code-relay"
        case .codeRelayUninstall: "code-relay-uninstall"
        }
    }
}

// MARK: 页面

struct ClaudeHomePage: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @AppStorage(ClaudeUse.key) private var useSetting = "auto"
    @AppStorage(ClaudeUse.scopeKey) private var scopeSetting = SwitchScope.both.rawValue
    @AppStorage(ClaudeUse.driftKey) private var driftDismissed = ""
    @AppStorage(Onboarding.claudeKey) private var onboarding = ""
    @State private var sheet: ClaudeSheet?

    var body: some View {
        Group {
            if let directory = service.directory {
                let use = ClaudeUse.resolve(useSetting, directory)
                let entries = ClaudeEntry.all(directory, use: use)
                if let detail = nav.claudeDetail, let entry = ClaudeEntry.find(detail, in: entries) {
                    ClaudeAccountPage(entry: entry, entries: entries, use: use, directory: directory, open: { sheet = $0 },
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
                }
            }
            .environmentObject(service)
        }
    }

    private var scope: SwitchScope { SwitchScope(rawValue: scopeSetting) ?? .both }

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
        return PageScaffold(title: "Claude", subtitle: Text(starting ? "还没有账号" : subtitle(use, entries: entries, code: codeNow, desk: deskNow, codeRelayOn: codeRelayOn)), provider: "claude", spacing: 14) {
            if !starting {
                modeMenu(use)
                RefreshPill(checkedAt: entries.compactMap { $0.cli?.usage?.checkedAt }.max(), busy: service.isRefreshing) {
                    Task {
                        await service.refresh()
                        await service.refreshQuotaIfNeeded()
                    }
                }
                .fixedSize()
                if use.desk, let status { nextButton(status, entries: entries).fixedSize() }
            }
            addButton(use, directory: directory).fixedSize()
        } content: {
            if use.code && directory.recoveryNeeded { RecoveryBanner() }
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
            if use.desk, let status, !starting {
                DesktopNotices(status: status, locked: deskLocked(status)) { sheet = .desktop($0) }
                SyncStrip(status: status, locked: deskLocked(status)) { sheet = .desktop($0) }
                if let relay = status.codeRelay, status.installed {
                    CodeRelayStrip(relay: relay, locked: deskLocked(status) || relay.busy) { sheet = $0 }
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
                CardGrid {
                    ForEach(entries) { entry in
                        ClaudeEntryCard(entry: entry, use: use, directory: directory, open: { sheet = $0 },
                                        switchTo: { perform(entry, $0, directory: directory, use: use) })
                            .frame(maxHeight: .infinity, alignment: .top)
                    }
                }
            }
            if !starting {
                hints(use, directory: directory)
                footer(use, status: status)
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

    // Round the numbered list, as the menu bar's 下一个 does; the menu offers the other two ways.
    private func nextButton(_ status: ClaudeDesktopStatus, entries: [ClaudeEntry]) -> some View {
        let picks = status.picks
        let ask: (String?) -> Void = { handle in
            guard let handle, let entry = ClaudeEntry.find(handle, in: entries) else { return }
            sheet = .switching(ClaudeSwitchRequest(entryID: entry.id, code: false, desk: true, choice: false))
        }
        let disabled = deskLocked(status) || !status.switchSupported
        return HStack(spacing: 0) {
            Button("下一个账号") { ask(picks?.next) }
                .buttonStyle(.pill(.secondary, height: 27))
                .help("Claude 桌面按编号换到下一个保存了登录、参与「下一个」的账号")
                .disabled(picks?.next == nil || disabled)
            Menu {
                Button("下一个还有余量的") { ask(picks?.nextAvailable) }.disabled(picks?.nextAvailable == nil)
                Button("剩余最多的") { ask(picks?.best) }.disabled(picks?.best == nil)
            } label: {
                Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4).frame(width: 22, height: 27).contentShape(Rectangle())
            }
            .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
            .accessibilityLabel("换号方式")
            .disabled(disabled)
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

    private func footer(_ use: ClaudeUse, status: ClaudeDesktopStatus?) -> some View {
        let showCode = use.code || status?.codeRelay?.enabled == true
        return VStack(alignment: .leading, spacing: 5) {
            if showCode {
                HStack(spacing: 0) {
                    Text("点卡片看 5 小时窗口预热与续期；改自动换号顺序和见底线去")
                    Button("换号与重置") { nav.show(.automation) }.buttonStyle(.pill(.link))
                    Text("。")
                }
            } else {
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
    let open: (DesktopSheet) -> Void

    var body: some View {
        if let operation = status.operation, ["switch", "add"].contains(operation.kind), operation.running {
            Banner(glyph: .sync, text: operation.kind == "add" ? "正在添加 Claude 桌面账号…" : "Claude 桌面正在换上 \(operation.target ?? "账号")…") {
                Button("查看") { open(.progress) }.buttonStyle(.pill(.onAttention))
            }
        } else if let operation = status.operation, !operation.running, operation.ok == false, let message = operation.message {
            Banner(text: message) {
                Button("知道了") { Task { await service.claudeDesktop("dismiss") } }.buttonStyle(.pill(.onAttention))
            }
        }
        if !status.installed {
            Banner(glyph: .info, text: "这台 Mac 上没找到 Claude 桌面。装好并登录以后，这里会显示会话同步。")
        } else if !status.switchSupported {
            Banner(glyph: .info, text: "这一版 Claude 桌面不能在 AuthBaton 里换号；会话同步照常。")
        }
        if let retention = status.retention, retention.atRisk {
            Banner(glyph: .clock, text: retention.days.map { "Claude Code 会在 \($0) 天后清掉桌面会话的对话原文，清掉的会话打开是空的。" }
                   ?? "Claude Code 会定期清掉桌面会话的对话原文，清掉的会话打开是空的。") {
                if retention.canKeep {
                    Button("一直保留") { Task { await service.claudeDesktop("retention_keep") } }
                        .buttonStyle(.pill(.onAttention)).disabled(locked)
                }
            }
        }
        if status.symlinks > 0 {
            Banner(text: "有 \(status.symlinks) 个会话文件夹是软链，Claude 桌面会存不住会话。") {
                Button("改回真文件夹") { Task { await service.claudeDesktop("fix_symlinks", confirm: true) } }
                    .buttonStyle(.pill(.onAttention)).disabled(locked)
            }
        }
    }
}

// The session sync, folded to one quiet line: how many sessions, whether every account matches, and its controls.
private struct SyncStrip: View {
    @EnvironmentObject private var service: ServiceController
    let status: ClaudeDesktopStatus
    let locked: Bool
    let open: (DesktopSheet) -> Void

    var body: some View {
        HStack(spacing: 12) {
            ZStack {
                Circle().strokeBorder(Palette.soft, lineWidth: 2.4).frame(width: 18, height: 18).offset(x: -3)
                Circle().strokeBorder(Palette.brand, lineWidth: 2.4).frame(width: 18, height: 18).offset(x: 3)
            }
            .frame(width: 28, height: 28)
            .accessibilityHidden(true)
            HStack(alignment: .firstTextBaseline, spacing: 3) {
                Text("\(status.sessions)").font(.system(size: 15, weight: .bold, design: .rounded)).monospacedDigit()
                Text("个桌面会话").font(.system(size: 13, weight: .semibold))
            }
            .fixedSize()
            stateLine.frame(maxWidth: .infinity, alignment: .leading)
            Text("后台同步").font(.system(size: 12)).foregroundStyle(Palette.ink2)
            Toggle("后台同步", isOn: Binding(get: { status.background }, set: { value in
                Task { await service.claudeDesktop("background", enabled: value) }
            }))
            .labelsHidden().toggleStyle(.switch).controlSize(.small)
            .disabled(service.accountBusy || !status.installed)
            .help("AuthBaton 开着时，每个账号的桌面会话列表一直保持一样")
            Button(status.syncing ? "同步中" : "立即同步") { Task { await service.claudeDesktop("sync") } }
                .buttonStyle(.pill(.secondary))
                .disabled(locked || status.syncing)
            Button("快照与恢复 ›") { open(.snapshots) }.buttonStyle(.pill(.link))
        }
        .padding(.horizontal, 16).frame(minHeight: 52)
        .loopCard()
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
private struct CodeRelayStrip: View {
    @EnvironmentObject private var service: ServiceController
    let relay: ClaudeDesktopStatus.CodeRelayStatus
    let locked: Bool
    let open: (ClaudeSheet) -> Void

    var body: some View {
        HStack(spacing: 12) {
            ZStack {
                Circle()
                    .fill(relay.state == "active" ? Palette.wash : Palette.chip)
                    .frame(width: 28, height: 28)
                Icon(.relay, size: 14, color: relay.state == "active" ? Palette.brand : Palette.ink3, weight: 2)
            }
            .accessibilityHidden(true)
            Text("Code 无感换号")
                .font(.system(size: 13, weight: .semibold))
                .fixedSize()
            stateLine.frame(maxWidth: .infinity, alignment: .leading)
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
        .padding(.horizontal, 16).frame(minHeight: 52)
        .loopCard()
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
                    Text("正在卸载系统插件…")
                }.foregroundStyle(Palette.ink2)
            } else if relay.state == "active" {
                let countNote = relay.relayedCount > 0 ? " · 已接力 \(relay.relayedCount) 次" : ""
                Text("当前账号 \(relay.activeLabel ?? "—")\(countNote)")
                    .foregroundStyle(Palette.ink2)
                    .help("在上方切 Claude Code 账号时，桌面 Code 对话会立即切过去，无需重启桌面")
            } else if ["team_mismatch", "blocked_remote", "error"].contains(relay.state) {
                Text(relay.message).foregroundStyle(Palette.attentionText).help(relay.message)
            } else {
                Text(relay.message).foregroundStyle(Palette.ink3).help(relay.message)
            }
        }
        .font(.system(size: 12)).lineLimit(1)
    }
}

// MARK: 账号卡

private struct ClaudeEntryCard: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    let entry: ClaudeEntry
    let use: ClaudeUse
    let directory: AccountDirectory
    let open: (ClaudeSheet) -> Void
    let switchTo: (SwitchScope?) -> Void

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
                                    Text(entry.name).font(.system(size: 13, weight: .semibold)).lineLimit(1)
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
                    .help("点开看账号详情")
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
                            if codeRelayOn {
                                Text("Code 无感可用").font(.system(size: 11)).foregroundStyle(Palette.deep).lineLimit(1)
                                    .help("已开启 Code 无感换号：同 Team 账号无需在桌面重复登录，点「换上」即可在桌面 Code 对话里直接切换")
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
            if hasFooter(rule, codeRelayOn: codeRelayOn) { footer(rule, showCode: showCode) }
            if let cli = entry.cli, showCode {
                AccountStrips(account: cli, directory: directory, open: { open(.account($0)) }).padding(.horizontal, 14)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopCard(radius: 17, raised: entry.inUse)
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
                        Pill("第 \(rank) 位", style: rule.participate ? .neutral : .dashed)
                            .help("换号与重置里的顺序：≤ \(rule.below)% · \(rule.order.label)" + (rule.participate ? "" : " · 不参与"))
                    }
                }
                Spacer(minLength: 7)
                if let cli = entry.cli { AccountActions(account: cli, directory: directory, open: { open(.account($0)) }, showSwitch: false) }
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
                if dual && desk { Icon(.desktop, size: 10, color: Palette.deep, weight: 2) }
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
                .padding(.horizontal, 7).frame(height: 17)
                .background(Palette.attentionFill, in: Capsule())
                .help(help(windows.filter { $0.remaining <= notch }, waited: windows[last], notch: notch))
        }
    }

    private func help(_ low: [AccountQuota], waited: AccountQuota, notch: Int) -> String {
        let name = { (window: AccountQuota) in window.shortLabel.trimmingCharacters(in: .whitespaces) }
        if low.count > 1 {
            return low.map { "\(name($0))剩 \($0.remaining)%" }.joined(separator: "、")
                + "，都到了见底线 \(notch)%：自动换号不会换到它；两个都回满才能再用，最晚的是\(name(waited))额度，\(waited.refill())。"
        }
        return "\(name(waited))剩 \(waited.remaining)%，到了见底线 \(notch)%：自动换号不会换到它，\(waited.refill())。"
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
                CheckItem(kind: .info, text: "已经补登过的账号照旧用自己的登录；随时可以在页面底部关掉")
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
                CheckItem(kind: .info, text: "桌面壳只需登录 1 个该 Team 账号；其他同 Team 账号登录到 AuthBaton 即可在桌面 Code 模式无感切换，日后使用终端 CLI 也可直接复用。跨 Team 或 AuthBaton 退出时自动回退桌面默认登录，随时可在页面底部一键关闭或完整卸载")
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
                Button { switchTo(nil) } label: { Text("换上").padding(.leading, 11).padding(.trailing, 9).frame(height: 24).contentShape(Rectangle()) }
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
            Button(targets.desk ? "换上…" : "换上") { switchTo(nil) }
                .buttonStyle(.pill(.secondary))
                .help(targets.code && targets.desk ? "Claude Code 和 Claude 桌面一起换" : targets.code ? (codeRelayOn ? "无感换给桌面 Code 与 Claude Code：不重启桌面，下一轮立即生效" : "换给 Claude Code：不用退出，约 30 秒生效") : "换给 Claude 桌面：会退出再打开")
        }
    }

    private func menuTitle(_ scope: SwitchScope, codeRelayOn: Bool) -> String {
        switch scope {
        case .both: "一起换…"
        case .code: codeRelayOn ? "只换 Code（无感）" : "只换 Claude Code"
        case .desktop: "只换 Claude 桌面…"
        }
    }

    private func help(_ plan: ClaudeSwitchPlan, codeRelayOn: Bool) -> String {
        let scope = SwitchScope(rawValue: UserDefaults.standard.string(forKey: ClaudeUse.scopeKey) ?? "") ?? .both
        let targets = plan.targets(scope)
        return targets.code && targets.desk ? "按上次的选择：Code 和桌面一起换" : targets.code ? (codeRelayOn ? "按上次的选择：只换 Code（无感，不重启桌面）" : "按上次的选择：只换 Claude Code") : "按上次的选择：只换 Claude 桌面"
    }
}

// The card's … menu: the account's own actions, then the desktop's.
private struct ClaudeEntryMenu: View {
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
                Button(querying ? "正在查询额度…" : "立即查询额度") { Task { await service.checkQuota(id: cli.id) } }.disabled(querying)
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
                Button("重新登录…") { open(.account(.login(provider: "claude", accountID: cli.id))) }
                Button("重命名…") { open(.account(.rename(cli.id))) }
                Button("执行记录") { open(.account(.history(cli.id))) }
            } else if let desk = entry.desk {
                Button("重命名…") { open(.desktop(.rename(desk.handle))) }
            }
            Divider()
            if let cli = entry.cli {
                Button("从 AuthBaton 移除…", role: .destructive) { open(.account(.remove(cli.id))) }
                    .disabled(cli.removeBlocker(directory) != nil)
            }
            if use.desk, let desk = entry.desk {
                Button("移除桌面登录…", role: .destructive) { open(.desktop(.remove(desk.handle))) }.disabled(desk.inUse)
            }
        } label: {
            MoreDots(color: Palette.ink3).frame(width: 26, height: 26).contentShape(Circle())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        .disabled(service.accountBusy || directory.busy || directory.recoveryNeeded || status?.busy == true)
        .accessibilityLabel("\(entry.name) 的更多操作")
    }
}

// MARK: 换号确认

// Claude 桌面 is involved, so the switch is confirmed. When both sides can move the sheet offers the
// choice and remembers it; otherwise it says which side catches up.
struct ClaudeSwitchSheet: View {
    @EnvironmentObject private var service: ServiceController
    @AppStorage(ClaudeUse.scopeKey) private var scopeSetting = SwitchScope.both.rawValue
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
    private var scope: SwitchScope { picked ?? (SwitchScope(rawValue: scopeSetting) ?? .both) }
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
                    let note = status?.codeRelay?.enabled == true
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
                } else {
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
