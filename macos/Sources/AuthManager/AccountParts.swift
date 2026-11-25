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
    // Claude cards: the window the account waits on below its 见底线 shows when it is back instead of what is left.
    var refillWhenLow = false
    // Model or feature windows of their own, as thin bars under the last column.
    var extras: [AccountQuota] = []
    @ViewBuilder var trailing: Trailing

    var body: some View {
        let windows = Array(account.primaryWindows.prefix(2))
        let waiting = refillWhenLow ? notch.flatMap { windows.indexRefillingLast(below: $0) } : nil
        VStack(alignment: .leading, spacing: 10) {
            if windows.isEmpty {
                Figure(value: nil, size: size)
                Text(account.quotaCaption ?? "额度暂不可查").font(.system(size: 12)).foregroundStyle(Palette.ink3)
                trailingRow(text: nil)
            } else if refillWhenLow, let week = account.weekSpent {
                // The week used up: the 5-hour window and Fable have nothing to give, so only when the week is back shows (11-22).
                VStack(alignment: .leading, spacing: 0) {
                    if let due = RefillDue(week) {
                        DueFigure(due: due, size: size)
                    } else {
                        Text(week.refill()).font(.system(size: 13, weight: .semibold)).foregroundStyle(Palette.attentionText)
                    }
                    QuotaBar(remaining: 0, below: notch).padding(.top, 10)
                }
                trailingRow(text: "7 天剩 0%" + (notch.map { " · 见底线 \($0)%" } ?? ""))
            } else if windows.count == 1 {
                let due = waiting == 0 ? RefillDue(windows[0]) : nil
                column(windows[0], due: due)
                extraRows
                trailingRow(text: due != nil ? leftText(windows[0]) : windows[0].refill(compact: !exact, exact: exact))
            } else {
                // On a card (caption beside the number) the two windows sit 12 apart, so two 100% fit a 384 card.
                EqualColumns(spacing: inlineCaption ? 12 : 20) {
                    ForEach(Array(windows.enumerated()), id: \.offset) { index, window in
                        let due = index == waiting ? RefillDue(window) : nil
                        VStack(alignment: .leading, spacing: 10) {
                            column(window, due: due)
                            if due != nil {
                                Text(leftText(window)).font(.system(size: 12)).foregroundStyle(Palette.ink3).lineLimit(1)
                                    .help(window.resetDate.map(When.exact) ?? "")
                            } else {
                                Group {
                                    if exact {
                                        // The clock time goes first when the column is too narrow for both.
                                        ViewThatFits(in: .horizontal) {
                                            Text(window.refill(compact: false, exact: true))
                                            Text(window.refill(compact: false))
                                        }
                                    } else {
                                        Text(window.refill())
                                    }
                                }
                                .font(.system(size: 12)).foregroundStyle(Palette.ink2)
                                .help(window.resetDate.map(When.exact) ?? "")
                            }
                            if index == windows.count - 1 { extraRows }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
                if let caption = account.quotaCaption { Text(caption).font(.system(size: 11)).foregroundStyle(Palette.ink3) }
                HStack(spacing: 7) { trailing }
            }
        }
    }

    // "剩 2% · 见底线 4%" under the bar of the window shown as a refill time.
    private func leftText(_ window: AccountQuota) -> String {
        "剩 \(window.remaining)%" + (notch.map { " · 见底线 \($0)%" } ?? "")
    }

    @ViewBuilder private var extraRows: some View {
        ForEach(Array(extras.enumerated()), id: \.offset) { _, window in
            ExtraQuotaRow(window: window, notch: window.isFable ? notch : nil)
        }
    }

    private func column(_ window: AccountQuota, due: RefillDue? = nil) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if let due {
                DueFigure(due: due, size: size)
            } else if inlineCaption {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Figure(value: window.remaining, size: size)
                    // Two windows side by side at 100% leave no room: the caption shortens to the window's name, never wraps.
                    ViewThatFits(in: .horizontal) {
                        Text(window.remainingCaption)
                        Text(window.shortLabel.trimmingCharacters(in: .whitespaces))
                    }
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
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

extension Array where Element == AccountQuota {
    // Of the windows at or under the 见底线, the one the account waits on: it is usable again only once
    // every one of them is back, so the one that refills last (an unknown time counts as last).
    func indexRefillingLast(below notch: Int) -> Int? {
        indices.filter { self[$0].remaining <= notch }
            .max { (self[$0].resetDate ?? .distantFuture) < (self[$1].resetDate ?? .distantFuture) }
    }
}

// When a window is back, split for the big figure: "2" "天" "后回满". Whole days, else hours, else
// minutes, like the compact refill text; nil when the time is unknown or already past.
struct RefillDue {
    let number: Int
    let unit: String
    let caption: String

    init?(_ window: AccountQuota, now: Date = Date()) {
        guard let seconds = window.resetDate?.timeIntervalSince(now), seconds > 0 else { return nil }
        if seconds >= 86400 {
            number = Int(seconds / 86400); unit = "天"
        } else if seconds >= 3600 {
            number = Int(seconds / 3600); unit = "小时"
        } else {
            number = max(1, Int(seconds / 60)); unit = "分钟"
        }
        caption = "后" + (window.expires == true ? "到期" : "回满")
    }
}

// The big figure of the window an account waits on: how long until it is back, not what is left.
// The unit drops to the size of "%" and then the caption goes when the column is too narrow.
struct DueFigure: View {
    let due: RefillDue
    var size: CGFloat = 36

    var body: some View {
        ViewThatFits(in: .horizontal) {
            line(unit: size, caption: true)
            line(unit: size * 0.42, caption: true)
            line(unit: size * 0.42, caption: false)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(due.number) \(due.unit)\(due.caption)")
    }

    private func line(unit: CGFloat, caption: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: unit == size ? size * 0.2 : 2) {
                Text("\(due.number)").font(.system(size: size, weight: .semibold, design: .rounded)).monospacedDigit()
                    .kerning(-size * 0.02)
                Text(due.unit).font(.system(size: unit, weight: .semibold, design: .rounded))
            }
            .foregroundStyle(Palette.ink)
            if caption {
                Text(due.caption).font(.system(size: 13, weight: .semibold)).foregroundStyle(Palette.attentionText)
            }
        }
        .lineLimit(1)
        .fixedSize()
    }
}

// A model or feature window of its own as a thin bar. Only Fable carries the 见底线: while Claude Code
// uses Fable its week counts against the line (v0.26); other windows never decide a switch.
struct ExtraQuotaRow: View {
    let window: AccountQuota
    var notch: Int? = nil

    var body: some View {
        HStack(spacing: 7) {
            Text(window.scope ?? window.shortLabel).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
            QuotaBar(remaining: window.remaining, below: notch, height: 4)
            Text("\(window.remaining)%").font(.system(size: 11, weight: .semibold, design: .rounded)).monospacedDigit()
                .lineLimit(1).frame(minWidth: 30, alignment: .trailing)
        }
        .help(window.isFable
              ? "Fable 每周额度，\(window.refill())；用 Fable 时也按见底线换号"
              : "\(window.title)：模型或功能单独的额度，\(window.refill())，不参与自动换号判断")
    }
}

extension AccountQuota {
    // Claude's Fable week, as the service picks it for the Fable line.
    var isFable: Bool { kind == "model" && scope?.caseInsensitiveCompare("fable") == .orderedSame }
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
        // Two sets: an account this Mac renews, and one the authentication centre renews.
        let cloud = account.cloudRenewed
        Menu {
            Button(querying ? "正在查询额度…" : "查询额度") { Task { await service.checkQuota(id: account.id) } }
                .disabled(querying)
            if cloud {
                Button("云端状态检查") { Task { await service.cloudAction("refresh") } }.disabled(directory.cloud?.busy == true)
                Button("认证检查") { Task { await service.cloudAction("check", id: account.id, confirm: true) } }.disabled(directory.cloud?.busy == true)
            } else {
                Button("本地检查") { check(online: false) }.disabled(checkDisabled)
                Button("认证检查") { check(online: true) }.disabled(checkDisabled || account.state == "unsaved")
            }
            Divider()
            if cloud {
                Button("更新云端登录…") { open(.login(provider: account.provider, accountID: account.id)) }
                Button("转回本机续期…") { open(.toLocal(account.id)) }
            } else {
                Button("重新登录…") { open(.login(provider: account.provider, accountID: account.id)) }
                Button("转到云端续期…") { open(.toCloud(account.id)) }
            }
            Button("重命名…") { open(.rename(account.id)) }
            Button("执行记录") { open(.history(account.id)) }
            Divider()
            if cloud {
                Button("从认证中心移除…", role: .destructive) { open(.cloudRemove(account.id)) }
                    .disabled(account.removeBlocker(directory) != nil)
            } else {
                Button("从 AuthBaton 移除…", role: .destructive) { open(.remove(account.id)) }
                    .disabled(account.removeBlocker(directory) != nil)
            }
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
        // A move to the centre or back holds the account until it finishes.
        .disabled(service.accountBusy || directory.busy || directory.recoveryNeeded || account.transferRunning)
        .help(cloud ? "查询额度、检查、更新云端登录、转回本机、重命名、执行记录、移除" : "查询额度、检查、重新登录、转到云端、重命名、执行记录、移除")
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
    // A Claude account whose week is used up: the card is the way back, so it is the main button (11-22).
    var prominent = false

    var body: some View {
        HStack(spacing: 5) {
            if account.transferRunning {
                // The account waits for its move; the footer says so instead of offering anything.
                Text(account.health.title).font(.system(size: 12)).foregroundStyle(Palette.ink2).lineLimit(1)
            } else if let next = account.nextReset {
                Button("用 1 张重置卡") {
                    open(.reset(accountID: account.id, creditID: next.creditID, requestID: UUID().uuidString, expiresAt: next.expiresAt, retry: false))
                }
                .buttonStyle(.pill(prominent ? .attention : .secondary))
                .disabled(directory.resetBlocked(account, busy: service.accountBusy))
                .help(help(next.expiresAt))
            }
            if showSwitch, directory.primaryAction(for: account) == .switchTo {
                Button(account.native ? "接管" : "换上") { open(.switchTo(account.id)) }
                    .buttonStyle(.pill(.secondary))
                    .disabled(service.accountBusy || directory.switchDisabled(for: account))
            }
        }
    }

    private func help(_ expiresAt: String?) -> String {
        // Without a date the label is already a whole phrase ("未提供到期时间").
        let expiry = QuotaResetCredit.expiryLabel(expiresAt) + (QuotaResetCredit.date(expiresAt) == nil ? "" : " 到期")
        guard account.provider == "claude" else { return "用最早到期的那张：\(expiry)" }
        let offers = account.claudeOffers
        if offers.count > 1 { return "有 \(offers.count) 类重置卡，确认时再选" }
        return "\(offers.first.map { ClaudeResetKind.name($0.kind) } ?? "重置卡")：\(expiry)"
    }
}

// MARK: 就地说问题

// An interrupted plain switch blocks every switch, so each page that offers one says why and how to finish it.
// Only a plain switch interrupted while writing the default login leaves this; desktop switches keep no record.
struct RecoveryBanner: View {
    @EnvironmentObject private var service: ServiceController

    var body: some View {
        Banner(glyph: .sync, title: "上次切换没确认", text: "写入默认登录时中断。结束相关任务后重试，AuthBaton 会按实际登录状态收尾。") {
            Button("重试切换") { Task { await service.accountOperation("accounts.recover") } }
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
    // What the in-use Codex account says about the desktop. The accounts page has no other place for it; the
    // Codex page says it once in its own band, so its cards pass false.
    var desktop = false

    private var locked: Bool { service.accountBusy || directory.busy || directory.recoveryNeeded }

    var body: some View {
        VStack(spacing: 0) {
            if desktop, account.provider == "codex", account.native, let connection = directory.desktop, connection.enabled {
                if !connection.connected {
                    IssueStrip(glyph: .desktop,
                               // This strip sits on the account in use: what its own rule still does.
                               text: directory.strategy?.config.enabled == true
                                   && directory.rule(for: account.id).map { $0.participate && $0.order.resets } == true
                                   ? "Codex 桌面没连上，现在换不了号，见底时只会重置。" : "Codex 桌面没连上，现在换不了号。") {
                        Button("去连接") { AppNavigation.shared.showDesktopConnection() }.buttonStyle(.pill(.link))
                    }
                } else if connection.selected != account.id {
                    IssueStrip(glyph: .desktop, text: "Codex 桌面还在用自己的登录；交给 AuthBaton 后才能热切换。") {
                        Button("接管") { open(.switchTo(account.id)) }.buttonStyle(.pill(.primary, height: 22))
                            .disabled(service.accountBusy || directory.switchDisabled(for: account))
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
        case .capture:
            IssueStrip(glyph: .alert, text: text) {
                Button("本地检查") {
                    Task { await service.accountOperation("accounts.check", params: LoginParams(id: account.id, online: false)) }
                }
                .buttonStyle(.pill(.primary, height: 22)).disabled(locked)
            }
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
