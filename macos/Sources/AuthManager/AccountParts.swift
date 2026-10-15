import SwiftUI

// Pieces every account surface shares: the numbers, the reset ticket, the "…" menu, the actions,
// and the strips that say what is wrong with this account and what fixes it.

// MARK: 额度数字

// The account's main windows as big numbers. One window gets one figure; two sit side by side.
struct QuotaFigures<Trailing: View>: View {
    let account: ManagedAccount
    var notch: Int? = nil
    var size: CGFloat = 49
    // Codex cards put "7 天额度剩余" after the number instead of under it.
    var inlineCaption = false
    // Detail pages add the clock time to "3 小时后回满".
    var exact = false
    @ViewBuilder var trailing: Trailing

    var body: some View {
        let windows = Array(account.primaryWindows.prefix(2))
        VStack(alignment: .leading, spacing: 10) {
            if windows.isEmpty {
                Figure(value: nil, size: size)
                Text(account.quotaCaption ?? "额度暂不可查").font(.system(size: 12)).foregroundStyle(Palette.ink3)
                trailingRow(text: nil)
            } else if windows.count == 1 {
                column(windows[0])
                trailingRow(text: windows[0].refill(compact: !exact, exact: exact))
            } else {
                HStack(alignment: .top, spacing: 20) {
                    ForEach(Array(windows.enumerated()), id: \.offset) { _, window in
                        VStack(alignment: .leading, spacing: 10) {
                            column(window)
                            // The clock time goes first when the column is too narrow for both.
                            ViewThatFits(in: .horizontal) {
                                Text(window.refill(compact: !exact, exact: exact))
                                Text(window.refill(compact: !exact))
                            }
                            .font(.system(size: 12)).foregroundStyle(Palette.ink2)
                            .help(window.resetDate.map(When.exact) ?? "")
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                if let caption = account.quotaCaption { Text(caption).font(.system(size: 11)).foregroundStyle(Palette.ink3) }
                HStack(spacing: 7) { trailing }
            }
        }
    }

    private func column(_ window: AccountQuota) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if inlineCaption {
                HStack(alignment: .firstTextBaseline, spacing: 7) {
                    Figure(value: window.remaining, size: size)
                    Text(window.remainingCaption).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            } else {
                Figure(value: window.remaining, size: size)
                Text(window.remainingCaption).font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 2)
            }
            QuotaBar(remaining: window.remaining, below: notch).padding(.top, inlineCaption ? 10 : 14)
        }
    }

    private func trailingRow(text: String?) -> some View {
        HStack(spacing: 7) {
            if let text {
                Text(text).font(.system(size: 12)).foregroundStyle(Palette.ink2)
                    .help(account.primaryWindows.first?.resetDate.map(When.exact) ?? "")
            }
            if let caption = account.quotaCaption, !account.primaryWindows.isEmpty {
                Text("· \(caption)").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            Spacer(minLength: 7)
            trailing
        }
    }
}

extension QuotaFigures where Trailing == EmptyView {
    init(account: ManagedAccount, notch: Int? = nil, size: CGFloat = 49, inlineCaption: Bool = false, exact: Bool = false) {
        self.init(account: account, notch: notch, size: size, inlineCaption: inlineCaption, exact: exact) { EmptyView() }
    }
}

// MARK: 重置卡

// The ticket says how many reset cards are left; a click lists their expiry, and lets an amber one be acknowledged.
struct ChancesTicket: View {
    let account: ManagedAccount
    var large = false
    @ObservedObject private var reminders = ReminderModel.shared
    @State private var showing = false

    var body: some View {
        if let chances = account.chances(dismissed: reminders.dismissed) {
            Button { showing = true } label: { Ticket(text: chances.text, soon: chances.soon, large: large) }
                .buttonStyle(.plain)
                .help(chances.help)
                .popover(isPresented: $showing, arrowEdge: .bottom) { detail(chances) }
        }
    }

    private func detail(_ chances: ResetChances) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(chances.total > 0 ? "\(account.displayName) 有 \(chances.total) 张重置卡" : "\(account.displayName) 没有重置卡")
                .font(.system(size: 12, weight: .semibold))
            ForEach(chances.expiries, id: \.self) { Text($0).font(.system(size: 11)).foregroundStyle(Palette.ink2).monospacedDigit() }
            if chances.total > 0 && chances.expiries.isEmpty {
                Text("官方这次没有返回到期时间，下次查询后显示。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            if chances.soon {
                HStack {
                    Text("7 天内到期的会标成橙色。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    Spacer(minLength: 10)
                    Button("知道了") {
                        let ids = ExpiryReminder.all([account]).flatMap(\.itemIDs)
                        reminders.dismiss(ids)
                        showing = false
                    }
                    .buttonStyle(.pill(.secondary, height: 20))
                }
            }
        }
        .padding(12)
        .frame(width: 238, alignment: .leading)
    }
}

// MARK: 更多操作

struct AccountMenu: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void
    var filled = false

    var body: some View {
        let checkDisabled = service.accountBusy || (directory.checksBlocked ?? true) || account.checkStatus != nil
        let querying = service.quotaRequests.contains(account.id) || account.usage?.refreshing == true
        Menu {
            Button(querying ? "正在查询额度…" : "立即查询额度") { Task { await service.checkQuota(id: account.id) } }
                .disabled(querying)
            Button("本地检查") { check(online: false) }.disabled(checkDisabled)
            Button("认证检查") { check(online: true) }.disabled(checkDisabled || account.state == "unsaved")
            Divider()
            Button("重新登录…") { open(.login(provider: account.provider, accountID: account.id)) }
            Button("重命名…") { open(.rename(account.id)) }
            Button("执行记录") { open(.history(account.id)) }
            Divider()
            Button("从 AuthBaton 移除…", role: .destructive) { open(.remove(account.id)) }
                .disabled(account.removeBlocker(directory) != nil)
        } label: {
            MoreDots(color: filled ? Palette.ink2 : Palette.ink3)
                .frame(width: 26, height: 26)
                .background(filled ? Palette.field : .clear, in: Circle())
                .contentShape(Circle())
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .disabled(service.accountBusy || directory.busy || directory.recoveryNeeded)
        .help("立即查询额度、检查、重新登录、重命名、执行记录、移除")
        .accessibilityLabel("\(account.displayName) 的更多操作")
    }

    private func check(online: Bool) {
        Task { await service.accountOperation("accounts.check", params: LoginParams(id: account.id, online: online)) }
    }
}

// MARK: 换上 / 重置

// The buttons on a card's footer: switch to it, or spend one reset chance.
struct AccountActions: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void
    var showSwitch = true

    var body: some View {
        HStack(spacing: 5) {
            if account.provider == "codex", !account.codexCredits.isEmpty, let credit = account.codexCredits.first {
                Button("用 1 张重置卡") {
                    open(.reset(accountID: account.id, creditID: credit.id, requestID: UUID().uuidString, expiresAt: credit.expiresAt, retry: false))
                }
                .buttonStyle(.pill(.secondary))
                .disabled(directory.resetBlocked(account, busy: service.accountBusy))
                .help("用最早到期的那张：\(QuotaResetCredit.expiryLabel(credit.expiresAt)) 到期")
            }
            if showSwitch, directory.primaryAction(for: account) == .switchTo {
                Button(account.native ? "接管" : "换上") { open(.switchTo(account.id)) }
                    .buttonStyle(.pill(.secondary))
                    .disabled(service.accountBusy || directory.switchDisabled(for: account))
            }
        }
    }
}

// MARK: 就地说问题

// An interrupted plain switch blocks every switch, so each page that offers one says why and how to finish it.
// Only a plain switch interrupted while writing the default login leaves this; desktop switches keep no record.
struct RecoveryBanner: View {
    @EnvironmentObject private var service: ServiceController

    var body: some View {
        Banner(glyph: .sync, title: "上次切换没确认", text: "上次切换在写入默认登录时中断。结束相关任务后重试，AuthBaton 会按实际登录状态收尾。") {
            Button("重试上次切换") { Task { await service.accountOperation("accounts.recover") } }
                .buttonStyle(.pill(.onAttention, height: 20))
        }
    }
}

// What is wrong with this account right now, each with the one thing that fixes it.
struct AccountStrips: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void
    // What the in-use Codex account says about the desktop: the accounts page has no other place for it,
    // the Codex page has its own banner for a lost connection and the take-over button on the card.
    var desktop: DesktopNotice = .none

    enum DesktopNotice { case none, takeOver, all }

    private var locked: Bool { service.accountBusy || directory.busy || directory.recoveryNeeded }

    var body: some View {
        VStack(spacing: 0) {
            if desktop != .none, account.provider == "codex", account.native, let connection = directory.desktop, connection.enabled {
                if !connection.connected {
                    if desktop == .all {
                        IssueStrip(glyph: .desktop,
                                   text: directory.strategy?.config.enabled == true
                                       ? "Codex 桌面没连上，现在换不了号，见底时只会重置。" : "Codex 桌面没连上，现在换不了号。") {
                            Button("去连接") { AppNavigation.shared.show(.desktop) }.buttonStyle(.pill(.link))
                        }
                    }
                } else if connection.selected != account.id {
                    IssueStrip(glyph: .desktop, text: "Codex 桌面还在用自己的登录；交给 AuthBaton 后才能热切换。") {
                        if desktop == .all {
                            Button("接管") { open(.switchTo(account.id)) }.buttonStyle(.pill(.primary, height: 22))
                                .disabled(service.accountBusy || directory.switchDisabled(for: account))
                        }
                    }
                }
            }
            health
            if let action = directory.recentDesktopAction(for: account.id) {
                // A finished attempt carries its time, so it reads as history rather than current state.
                IssueStrip(glyph: .desktop, busy: action.pending,
                           text: (action.pending ? "" : action.at.map { "\(When.text($0)) · " } ?? "") + action.message,
                           tone: action.state == "failed" ? .attention : .neutral)
            }
            if let reset = account.reset { resetStrip(reset) }
            if let error = service.quotaErrors[account.id] {
                IssueStrip(text: error)
            }
            if service.accountErrorID == account.id, let error = service.accountError {
                IssueStrip(text: error) {
                    Button("知道了") { service.clearAccountError() }.buttonStyle(.pill(.secondary, height: 22))
                }
            }
        }
    }

    @ViewBuilder private var health: some View {
        let health = account.health
        let detail = health.detail ?? (account.message.isEmpty ? nil : account.message)
        let text = health.title + (detail.map { "。\($0)" } ?? "")
        switch directory.primaryAction(for: account) {
        case .recover:
            IssueStrip(glyph: .sync, text: text) {
                Button(account.recoverTitle) { open(.recover(account.id)) }.buttonStyle(.pill(.primary, height: 22)).disabled(locked)
            }
        case .relogin:
            IssueStrip(glyph: .alert, text: text) {
                Button("重新登录") { open(.login(provider: account.provider, accountID: account.id)) }
                    .buttonStyle(.pill(.primary, height: 22)).disabled(locked)
            }
        default:
            if account.checkLabel != nil {
                IssueStrip(busy: true, text: health.title, tone: .neutral)
            } else if !account.quietState {
                IssueStrip(glyph: health.tone == .attention ? .alert : .info, text: text, tone: health.tone == .attention ? .attention : .neutral)
            }
        }
    }

    // The last reset is a result, not an instruction; an unconfirmed one asks to be retried as the same operation.
    @ViewBuilder private func resetStrip(_ reset: QuotaResetAttempt) -> some View {
        if reset.unresolved {
            IssueStrip(glyph: .alert, text: reset.message) {
                if reset.outcome != "pending" {
                    Button("重试上次重置…") {
                        open(.reset(accountID: account.id, creditID: reset.creditId, requestID: reset.requestId, expiresAt: reset.expiresAt, retry: true))
                    }
                    .buttonStyle(.pill(.secondary, height: 22))
                    .disabled(locked || account.usage?.refreshing == true)
                }
            }
        } else if let when = reset.finishedAt ?? reset.startedAt, Date().timeIntervalSince1970 - when < 600 {
            IssueStrip(glyph: .check, text: reset.message, tone: .neutral)
        }
    }
}
