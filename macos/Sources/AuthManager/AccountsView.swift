import SwiftUI

// Every confirmation or form an account surface can open; the account is looked up live by ID.
enum AccountSheet: Identifiable {
    case login(provider: String, accountID: String?)
    case switchTo(String)
    case reset(accountID: String, creditID: String?, requestID: String, expiresAt: String?, retry: Bool)
    case remove(String)
    // Moving the renewal: to the user's authentication centre (or pairing first), back to this Mac, or out of the centre.
    case toCloud(String)
    case toLocal(String)
    case cloudRemove(String)
    case recover(String)
    case rename(String)
    case history(String?)
    case strategyStart
    case strategyStop
    case desktopConnection

    var id: String {
        switch self {
        case .login(let provider, let id): "login-\(provider)-\(id ?? "")"
        case .switchTo(let id): "switch-\(id)"
        case .reset(let id, let credit, let request, _, let retry): "reset-\(id)-\(credit ?? request)-\(retry)"
        case .remove(let id): "remove-\(id)"
        case .toCloud(let id): "to-cloud-\(id)"
        case .toLocal(let id): "to-local-\(id)"
        case .cloudRemove(let id): "cloud-remove-\(id)"
        case .recover(let id): "recover-\(id)"
        case .rename(let id): "rename-\(id)"
        case .history(let id): "history-\(id ?? "all")"
        case .strategyStart: "strategy-start"
        case .strategyStop: "strategy-stop"
        case .desktopConnection: "desktop-connection"
        }
    }
}

// 账号: the account in use for each tool as a big number, the bench below in relay order.
// Problems sit on the account they belong to.
struct AccountsView: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var reminders = ReminderModel.shared
    @State private var sheet: AccountSheet?
    private var locked: Bool { service.accountBusy || service.directory?.busy == true }

    var body: some View {
        Group {
            if let directory = service.directory {
                PageScaffold(title: "账号", subtitle: subtitle(directory), spacing: 24) {
                    RefreshPill(checkedAt: latestCheck(directory), busy: service.isRefreshing) {
                        Task {
                            await service.refresh()
                            await service.refreshQuotaIfNeeded()
                        }
                    }
                    Button { sheet = .login(provider: "codex", accountID: nil) } label: {
                        HStack(spacing: 5) {
                            Icon(.plus, size: 12, color: .white, weight: 2)
                            Text("添加账号")
                        }
                    }
                    .buttonStyle(.pill(.primary, height: 27))
                    .help("登录新账号，或导入这台 Mac 上已有的登录")
                    .disabled(locked || directory.recoveryNeeded)
                } content: {
                    notices(directory)
                    if directory.accounts.isEmpty {
                        emptyState
                    } else {
                        HStack(alignment: .top, spacing: 14) {
                            ForEach(["codex", "claude"], id: \.self) { provider in
                                HeroCard(provider: provider, directory: directory) { sheet = $0 }
                            }
                        }
                        .fixedSize(horizontal: false, vertical: true)
                        Bench(directory: directory) { sheet = $0 }
                    }
                }
            } else if !service.isConnected && !service.isConnecting {
                // The window's banner says why and offers to reconnect.
                Color.clear
            } else if let error = service.directoryError {
                PageScaffold(title: "账号") { Banner(text: error) }
            } else {
                ProgressView("正在载入账号列表…").frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task { await service.refreshQuotaIfNeeded() }
        .sheet(item: $sheet) { item in
            AccountSheetView(item: item, close: { sheet = nil }).environmentObject(service)
        }
    }

    private func subtitle(_ directory: AccountDirectory) -> Text {
        let codex = directory.current("codex") != nil, claude = directory.current("claude") != nil
        let lead = directory.accounts.isEmpty ? "还没有账号。"
            : codex && claude ? "Codex 和 Claude Code 都有账号在用。"
            : codex ? "Codex 有账号在用，Claude Code 还没有。"
            : claude ? "Claude Code 有账号在用，Codex 还没有。" : "还没有在用的账号。"
        let issues = directory.issues(dismissed: reminders.dismissed)
        guard !issues.isEmpty else { return Text(lead) }
        // Some issues belong to no account (an unconfirmed switch, a stopped relay) and show as banners instead.
        return Text(lead) + Text("\(issues.count) 件事要处理").foregroundStyle(Palette.attentionText).fontWeight(.semibold)
            + Text(issues.allSatisfy { $0.account != nil } ? "，标在对应的账号上。" : "。")
    }

    private func latestCheck(_ directory: AccountDirectory) -> Double? {
        directory.accounts.compactMap { $0.usage?.checkedAt }.max()
    }

    // What belongs to no single account: an interrupted switch, a service notice, a login, a stalled rule.
    @ViewBuilder private func notices(_ directory: AccountDirectory) -> some View {
        if let error = service.accountError, service.accountErrorID == nil || !directory.accounts.contains(where: { $0.id == service.accountErrorID }) {
            Banner(text: error) {
                Button("知道了") { service.clearAccountError() }.buttonStyle(.pill(.onAttention, height: 20))
            }
        }
        if let error = service.directoryError { Banner(text: error) }
        if directory.recoveryNeeded { RecoveryBanner() }
        if let notice = directory.notice { Banner(glyph: .info, text: notice) }
        if let strategy = directory.strategy, strategy.phase == "attention" {
            // The relay lives on each tool's page; the run, when there is one, names the tool.
            let target = strategy.run.flatMap { run in directory.accounts.first { $0.id == run.target } }
            Banner(glyph: .relay, title: "自动接力停下了", text: strategy.message) {
                Button("去看看") {
                    if let target { AppNavigation.shared.show(account: target) } else { AppNavigation.shared.show(directory.relayRules("codex").isEmpty ? .claude : .codex) }
                }
                .buttonStyle(.pill(.onAttention, height: 20))
            }
        }
        if let login = service.login {
            HStack(spacing: 9) {
                if login.isActive { ProgressView().controlSize(.small) } else { Icon(login.phase == "succeeded" ? .check : .info, size: 14, color: Palette.ink3) }
                Text("\(GlyphTile.name(login.provider)) · \(login.phaseLabel)").font(.system(size: 12))
                Spacer()
                Button(login.isActive ? "查看登录进度" : "查看最近登录结果") {
                    sheet = .login(provider: login.provider, accountID: service.loginTarget?.accountID)
                }
                .buttonStyle(.pill(.secondary))
            }
            .padding(.horizontal, 14).padding(.vertical, 9)
            .loopCard(radius: 12)
        }
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            BrandTile(size: 44)
            Text("把常用账号放在一起").font(.system(size: 14, weight: .bold))
            Text("登录新账号单独保存，或导入这台 Mac 上官方客户端已有的登录；不会换掉现在的默认登录。")
                .font(.system(size: 12)).foregroundStyle(Palette.ink2).multilineTextAlignment(.center).frame(maxWidth: 374)
            HStack(spacing: 9) {
                Button("添加 Codex 账号") { sheet = .login(provider: "codex", accountID: nil) }.buttonStyle(.pill(.primary, height: 27))
                Button("添加 Claude 账号") { sheet = .login(provider: "claude", accountID: nil) }.buttonStyle(.pill(.secondary, height: 27))
            }
            .disabled(locked || service.directory?.recoveryNeeded == true)
        }
        .frame(maxWidth: .infinity).padding(.vertical, 34)
        .loopCard(radius: 19, raised: true)
    }
}

// MARK: 在用

private struct HeroCard: View {
    let provider: String
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void

    var body: some View {
        let current = directory.current(provider)
        let any = directory.accounts.contains { $0.provider == provider }
        if !any {
            AddCard(provider: provider, open: open, lead: "还没有 \(GlyphTile.name(provider)) 账号", detail: "登录一个，或导入这台 Mac 上已有的登录。")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            VStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: 9) {
                        GlyphTile(provider: provider, size: 26)
                        Button { AppNavigation.shared.show(provider == "codex" ? .codex : .claude) } label: {
                            Text(GlyphTile.name(provider)).font(.system(size: 13, weight: .semibold)).foregroundStyle(Palette.ink)
                        }
                        .buttonStyle(.plain)
                        .help("查看全部 \(GlyphTile.name(provider)) 账号")
                        Spacer(minLength: 7)
                        AccountSwitcher(provider: provider, current: current, directory: directory, open: open)
                    }
                    if let current {
                        QuotaFigures(account: current, notch: directory.notch(for: current), size: 49) {
                            ChancesTicket(account: current)
                            if let warmup = current.warmup, warmup.enabled {
                                HStack(spacing: 5) {
                                    Icon(.sun, size: 11, color: Palette.ink2)
                                    Text(String(format: "每天 %02d:00 起预热", warmup.hour))
                                }
                                .font(.system(size: 11)).foregroundStyle(Palette.ink2)
                                .padding(.horizontal, 9).frame(height: 20)
                                .background(Palette.chip, in: Capsule())
                                .help("每个 5 小时窗口结束后发一条短消息，接上下一个窗口" + (warmup.last.map { "；\(When.text($0.at)) \($0.message)" } ?? ""))
                            }
                        }
                        .padding(.top, 19)
                    } else {
                        VStack(alignment: .leading, spacing: 3.5) {
                            Text("没有在用的账号").font(.system(size: 19, weight: .semibold)).foregroundStyle(Palette.ink3)
                            Text("在右上角选一个换上。").font(.system(size: 12)).foregroundStyle(Palette.ink3)
                        }
                        .padding(.top, 19)
                    }
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 19).padding(.top, 17).padding(.bottom, 15)
                if let current {
                    AccountStrips(account: current, directory: directory, open: open, desktop: true).padding(.horizontal, 19)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .loopCard(radius: 19, raised: true)
        }
    }
}

// The in-use account as a button: its menu lists who can take over.
private struct AccountSwitcher: View {
    @EnvironmentObject private var service: ServiceController
    let provider: String
    let current: ManagedAccount?
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void

    var body: some View {
        let others = directory.relay(provider).filter { !$0.native }
        Menu {
            ForEach(others) { account in
                let action = directory.primaryAction(for: account)
                Button(action == .switchTo ? "换上 \(account.displayName)" + (account.lowestPrimary.map { " · 剩余 \($0)%" } ?? "")
                                           : "\(account.displayName) · \(account.health.title)") {
                    open(.switchTo(account.id))
                }
                .disabled(action != .switchTo || service.accountBusy || directory.switchDisabled(for: account))
            }
            if !others.isEmpty { Divider() }
            Button("添加 \(GlyphTile.name(provider)) 账号…") { open(.login(provider: provider, accountID: nil)) }
        } label: {
            HStack(spacing: 7) {
                if let current { Seal(current.displayName, style: .inUse, size: 22) }
                Text(accountName: current?.displayName ?? "选一个账号").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink).lineLimit(1)
                Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4)
            }
            .padding(.leading, current == nil ? 10 : 3.5).padding(.trailing, 9)
            .frame(height: 27)
            .background(Palette.ground, in: Capsule())
            .contentShape(Capsule())
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .accessibilityLabel(current.map { "\(GlyphTile.name(provider)) 在用 \($0.displayName)，换一个账号" } ?? "选一个 \(GlyphTile.name(provider)) 账号")
    }
}

// MARK: 备用

private struct Bench: View {
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void

    var body: some View {
        let groups = ["codex", "claude"].map { provider in (provider, directory.relay(provider).filter { !$0.native }) }.filter { !$0.1.isEmpty }
        let singles = ["codex", "claude"].filter { provider in directory.accounts.filter { $0.provider == provider }.count == 1 }
        if !groups.isEmpty || !singles.isEmpty {
            VStack(alignment: .leading, spacing: 10) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text("备用").font(.system(size: 14, weight: .bold))
                    if !groups.isEmpty {
                        Text(directory.strategy?.config.enabled == true ? "见底时按这个顺序换上" : "随时可以换上")
                            .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                    Spacer()
                }
                if !groups.isEmpty {
                    VStack(spacing: 0) {
                        ForEach(groups, id: \.0) { provider, rows in
                            HStack(spacing: 7) {
                                Icon(.provider(provider), size: 13)
                                Text(GlyphTile.name(provider)).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                                Spacer()
                            }
                            .padding(.horizontal, 15).frame(height: 34)
                            .overlay(alignment: .top) { if provider != groups.first?.0 { Rectangle().fill(Palette.divider).frame(height: 1) } }
                            ForEach(rows) { account in
                                BenchRow(account: account, directory: directory, open: open)
                                    .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
                            }
                        }
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 15, style: .continuous))
                    .loopCard()
                }
                ForEach(singles, id: \.self) { provider in
                    AddCard(provider: provider, open: open, lead: "\(GlyphTile.name(provider)) 只有这一个账号", detail: "再加一个，见底时就能自动换号。")
                }
            }
        }
    }
}

private struct BenchRow: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var reminders = ReminderModel.shared
    let account: ManagedAccount
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void
    @State private var hovering = false

    var body: some View {
        let action = directory.primaryAction(for: account)
        let rule = directory.rule(for: account.id)
        HStack(spacing: 12) {
            Text(directory.rank(of: account).map(String.init) ?? "").font(.system(size: 11)).foregroundStyle(Palette.ink3).monospacedDigit()
                .frame(width: 12, alignment: .leading)
            Seal(account.displayName, style: rule?.participate == false ? .out : .standby, size: 26)
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 4) {
                    Text(accountName: account.displayName).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                    CloudBadge(account: account, size: 11)
                }
                detail
            }
            .frame(minWidth: 102, maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 9) {
                if let lowest = account.lowestPrimary {
                    QuotaBar(remaining: lowest, height: 4.5)
                    Text("\(lowest)%").font(.system(size: 13, weight: .semibold, design: .rounded)).monospacedDigit()
                        .lineLimit(1).frame(width: 40, alignment: .trailing)
                } else {
                    Text(account.quotaCaption ?? "—").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
            }
            .frame(minWidth: 77, maxWidth: 170)
            let lowest = account.primaryWindows.min { $0.remaining < $1.remaining }
            ViewThatFits(in: .horizontal) {
                Text(lowest?.refill() ?? "")
                Text(lowest?.refillBrief() ?? "")
            }
            .font(.system(size: 11)).monospacedDigit().foregroundStyle(Palette.ink3).lineLimit(1)
            .frame(width: 96, alignment: .leading)
            .help(lowest?.refillHelp() ?? "")
            let locked = service.accountBusy || directory.busy || directory.recoveryNeeded
            HStack(spacing: 5) {
                switch action {
                case .capture:
                    Button("本地检查") {
                        Task { await service.accountOperation("accounts.check", params: LoginParams(id: account.id, online: false)) }
                    }
                    .buttonStyle(.pill(.primary)).disabled(locked)
                case .recover:
                    Button(account.recoverTitle) { open(.recover(account.id)) }.buttonStyle(.pill(.primary)).disabled(locked)
                case .relogin:
                    Button("重新登录") { open(.login(provider: account.provider, accountID: account.id)) }.buttonStyle(.pill(.primary)).disabled(locked)
                default:
                    if hovering { AccountActions(account: account, directory: directory, open: open) }
                }
            }
            .frame(width: 170, alignment: .trailing)
            AccountMenu(account: account, directory: directory, open: open, filled: hovering)
        }
        .padding(.horizontal, 15).padding(.vertical, 9)
        .frame(minHeight: 51)
        .background(hovering ? Palette.hover : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
    }

    // Under the name: what is wrong if anything, otherwise the reset cards.
    @ViewBuilder private var detail: some View {
        let health = account.health
        if account.needsAction || !account.quietState {
            Text(health.title).font(.system(size: 11)).foregroundStyle(health.tone.color).lineLimit(1)
                .help(health.detail ?? account.message)
        } else if let chances = account.chances(dismissed: reminders.dismissed) {
            Text(chances.text).font(.system(size: 11)).foregroundStyle(chances.soon ? Palette.attentionText : Palette.ink3).lineLimit(1)
                .help(chances.help)
        } else {
            Text(account.subtitle.isEmpty ? " " : account.subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
        }
    }
}

// A dashed invitation to add an account.
struct AddCard: View {
    let provider: String
    let open: (AccountSheet) -> Void
    let lead: String
    let detail: String

    var body: some View {
        HStack(spacing: 12) {
            Icon(.provider(provider), size: 15)
            VStack(alignment: .leading, spacing: 0) {
                Text(lead).font(.system(size: 12, weight: .semibold))
                Text(detail).font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            Spacer(minLength: 10)
            Button("添加 \(provider == "codex" ? "Codex" : "Claude") 账号") { open(.login(provider: provider, accountID: nil)) }
                .buttonStyle(.pill(.secondary, height: 26))
        }
        .padding(.horizontal, 15).padding(.vertical, 14)
        .overlay(RoundedRectangle(cornerRadius: 15, style: .continuous).strokeBorder(Palette.dash, style: StrokeStyle(lineWidth: 1.5, dash: [5, 4])))
    }
}
