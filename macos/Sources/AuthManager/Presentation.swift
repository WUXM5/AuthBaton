import Foundation

// Display-only derivations of service state. Nothing here performs an operation.

struct Health {
    let tone: Tone
    let title: String
    let detail: String?
}

extension ManagedAccount {
    var providerName: String { provider == "codex" ? "Codex" : provider == "claude" ? "Claude Code" : "pi" }

    // Accounts are grouped by provider, so a leading "Codex · " in the stored label is noise.
    var displayName: String {
        for prefix in [providerName, provider, "Claude", "Codex"] {
            for separator in [" · ", "·", " - ", "：", ": "] where label.hasPrefix(prefix + separator) {
                let rest = label.dropFirst(prefix.count + separator.count).trimmingCharacters(in: .whitespaces)
                if !rest.isEmpty { return rest }
            }
        }
        return label
    }

    // The identity line is redundant when the name already is the login email.
    var identityLine: String? {
        guard let identity else { return nil }
        return identity == displayName ? nil : identity
    }

    // Email and plan under the name.
    var subtitle: String {
        [identityLine, plan?.uppercased()].compactMap { $0 }.joined(separator: " · ")
    }

    var needsAction: Bool {
        if transferFailed != nil || cloudTrouble != nil { return true }
        switch ownState {
        case "reauth", "attention", "unsaved", "backup_pending", "cloud_transfer": return true
        // The cloud renews it; the local login dates behind the notice no longer apply.
        case "cloud_managed": return false
        default: return !cloudRenewed && loginNotice?.needsAttention == true
        }
    }

    // Renewed by the authentication centre: this Mac only takes short-lived logins for it.
    var cloudRenewed: Bool { cloud?.maintainer == "cloud" }
    // A transfer to the centre or back is under way: the account's operations wait for it.
    var transferRunning: Bool { cloud?.transfer?.running == true }
    // A transfer that stopped with an error; the account stays with whoever renewed it before.
    var transferFailed: AccountCloud.Transfer? {
        guard let transfer = cloud?.transfer, !transfer.running, transfer.error != nil else { return nil }
        return transfer
    }
    // The centre's login was refused: only a new login brings it back. Its other states settle on their own.
    var cloudTrouble: String? {
        guard cloudRenewed, cloud?.state == "reauthorization_required" else { return nil }
        return cloud?.error ?? "续期被拒（刷新凭据已失效），需重新登录以恢复云端续期。"
    }
    // About how long a client keeps working on a short-lived login once AuthBaton stops: Claude's access
    // lasts about 8 hours, Codex's about 10 days.
    var cloudLapse: String { provider == "claude" ? "8 小时" : "10 天" }
    // The service still marks a row the centre keeps, or one on its way, with the old cloud mode's states;
    // next to `cloud` they say nothing about the login itself.
    private var ownState: String { cloud != nil && ["cloud_managed", "cloud_transfer"].contains(state) ? "healthy" : state }

    // Claude's coordinated states, or any unsaved result. An unanswered renewal needs no hand:
    // the service reconciles it on its own.
    var recoverable: Bool {
        (provider == "claude" && native && ["unsaved", "backup_pending", "attention"].contains(state)) || state == "unsaved"
    }
    var recoverTitle: String { provider == "claude" && native ? "恢复同步" : "重试保存" }

    var primaryWindows: [AccountQuota] { usage?.windows.filter { $0.kind == nil } ?? [] }
    var extraWindows: [AccountQuota] { usage?.windows.filter { $0.kind != nil } ?? [] }
    var lowestPrimary: Int? { primaryWindows.map(\.remaining).min() }
    var quotaReady: Bool { usage?.state == "ready" }

    // Only states that need a hand are amber; "可用" is the quiet default.
    var health: Health {
        if let label = checkLabel { return Health(tone: .active, title: label, detail: nil) }
        if let transfer = cloud?.transfer {
            let back = transfer.direction == "to_local"
            if transfer.running { return Health(tone: .active, title: back ? "正在转回本机…" : "正在转到云端…", detail: nil) }
            if let error = transfer.error { return Health(tone: .attention, title: back ? "转回本机失败" : "转到云端失败", detail: error) }
        }
        if let trouble = cloudTrouble { return Health(tone: .attention, title: "云端续期出错", detail: trouble) }
        switch ownState {
        case "reauth": return Health(tone: .attention, title: "需重新登录", detail: message)
        case "unsaved":
            return provider == "claude" && native
                ? Health(tone: .attention, title: "续期备份没同步", detail: "会自动重试，不会重复续期；CLI 要求重新登录时点「恢复同步」。")
                : Health(tone: .attention, title: "续期结果待保存", detail: message)
        case "uncertain": return Health(tone: .neutral, title: "等待重新确认", detail: message)
        case "attention": return Health(tone: .attention, title: "需要处理", detail: message)
        case "backup_pending": return Health(tone: .attention, title: "备份待同步", detail: message)
        // Handed to the cloud: this Mac no longer renews it, and its local login dates are stale.
        case "cloud_transfer": return Health(tone: .attention, title: "云端交接未完成", detail: message)
        case "cloud_managed": return Health(tone: .neutral, title: "由云端维护", detail: message)
        default: break
        }
        if !cloudRenewed, let notice = loginNotice, notice.needsAttention {
            return Health(tone: .attention, title: notice.state == "expired" ? "登录已到期" : "登录即将到期", detail: notice.message)
        }
        if cloudRenewed, ["uncertain", "verification_pending"].contains(cloud?.state ?? "") {
            return Health(tone: .neutral, title: "云端续期待确认", detail: cloud?.error)
        }
        if cloudRenewed, ["delegated", "healthy", "saved"].contains(ownState) {
            return Health(tone: .neutral, title: "可用", detail: "由认证中心续期，这台 Mac 领取短期凭据。")
        }
        switch ownState {
        case "retry": return Health(tone: .neutral, title: "等待重试", detail: message)
        case "limited": return Health(tone: .neutral, title: "在线查询受限", detail: message)
        case "pending": return Health(tone: .active, title: "等待登录", detail: message)
        case "checking": return Health(tone: .active, title: "正在检查", detail: message)
        case "delegated": return Health(tone: .neutral, title: "可用", detail: "由官方客户端续期，不用退出也能独立查额度。")
        case "saved": return Health(tone: .neutral, title: "已保存", detail: "已保存在本机，还没在线验证。")
        case "healthy": return Health(tone: .neutral, title: "可用", detail: nil)
        // A state this build does not know says nothing about whether the login works.
        default: return Health(tone: .attention, title: "状态未识别", detail: message)
        }
    }

    // A state worth a line on the account even when nothing is broken: waiting, checking, unknown.
    // A transfer says so on the card itself (its footer, or the box with 放弃 / 重试).
    var quietState: Bool { (["delegated", "healthy", "saved"].contains(ownState) && checkLabel == nil && !needsAction) || cloud?.transfer != nil }

    // Short status for the quota line; stale or unknown quota is muted, not alarming.
    var quotaCaption: String? {
        guard let usage else { return "还没查额度" }
        switch usage.state {
        case "ready": return nil
        case "pending": return "正在查额度"
        case "expired": return "等续期后再查"
        default: return usage.windows.isEmpty ? "额度暂不可查" : "显示上次结果"
        }
    }

    var codexCredits: [QuotaResetCredit] {
        (usage?.resetCredits?.details ?? []).filter(\.available)
            .sorted { ($0.expiryDate ?? .distantFuture) < ($1.expiryDate ?? .distantFuture) }
    }
    var creditCount: Int? {
        if provider == "claude" { return usage?.claudeResets?.grants?.remaining }
        return usage?.resetCredits?.availableCount
    }
    // Resets the strategy can spend: listed Codex credits, or Claude's official grants (unknown counts as none).
    var usableResets: Int { provider == "claude" ? creditCount ?? 0 : codexCredits.count }
    // The card a new reset spends: Codex's earliest-expiring listed credit, or the Claude card chosen in the
    // confirmation (by default `claudeDefaultOffer`), which the service names itself. None until the renewal
    // is saved and confirmed: the service refuses then.
    var nextReset: (creditID: String?, expiresAt: String?)? {
        if ["unsaved", "uncertain"].contains(state) { return nil }
        if provider == "claude" { return claudeDefaultOffer.map { (nil, $0.endsAt) } }
        return codexCredits.first.map { ($0.id, $0.expiresAt) }
    }
    // The Claude kinds a person can choose from, one card each, in the official order.
    var claudeOffers: [ClaudeResetOffer] { usage?.claudeResets?.offers ?? [] }
    // Preselected: a card that lapses within 30 minutes (the user, 11-22), else the kind of the official next
    // card, which the rules spend; otherwise the first kind.
    var claudeDefaultOffer: ClaudeResetOffer? {
        let lapsing = claudeOffers.compactMap { offer in QuotaResetCredit.date(offer.endsAt).map { (offer, $0.timeIntervalSinceNow) } }
            .filter { $0.1 > 0 && $0.1 <= 1800 }
            .min { $0.1 < $1.1 }?.0
        let next = usage?.claudeResets?.next?.kind
        return lapsing ?? claudeOffers.first { $0.kind == next } ?? claudeOffers.first
    }

    // The reset chances as one ticket: how many, and how many run out within a week.
    func chances(dismissed: Set<String>, now: Date = Date()) -> ResetChances? {
        var dates: [(id: String, count: Int, date: Date?)] = []
        let total: Int
        if provider == "codex" {
            guard let credits = usage?.resetCredits else { return nil }
            let listed = codexCredits
            dates = listed.map { ($0.id, 1, $0.expiryDate) }
            // The details request failed but the summary still counts them; unknown expiry is not zero.
            total = listed.isEmpty ? credits.availableCount : listed.count
        } else {
            guard let remaining = usage?.claudeResets?.grants?.remaining else { return nil }
            total = remaining
            dates = (usage?.claudeResets?.grants?.items ?? []).filter { $0.remaining > 0 }
                .map { ("\(id):\($0.endsAt ?? "")", $0.remaining, QuotaResetCredit.date($0.endsAt)) }
        }
        let soon = dates.filter { item in
            guard let date = item.date else { return false }
            return date > now && date.timeIntervalSince(now) <= ExpiryReminder.window && !dismissed.contains(item.id)
        }
        let expiries = dates.compactMap { item in item.date.map { "\(When.exact($0)) 到期" + (item.count > 1 ? "（\(item.count) 张）" : "") } }
        return ResetChances(total: total, soonCount: soon.reduce(0) { $0 + $1.count },
                            earliestSoon: soon.compactMap(\.date).min(), expiries: expiries)
    }

    // Why this account cannot be removed right now, if it cannot.
    func removeBlocker(_ directory: AccountDirectory) -> String? {
        if native || directory.desktop?.selected == id { return "在用的账号不能移除，先换上别的账号。" }
        if transferRunning { return "正在转换续期方，稍后再移除。" }
        if reset?.unresolved == true { return "先确认上次重置的结果。" }
        if usage?.refreshing == true { return "正在查额度，稍后再移除。" }
        if warmup?.running == true { return "正在预热，稍后再移除。" }
        return nil
    }
}

struct ResetChances {
    let total: Int
    let soonCount: Int
    let earliestSoon: Date?
    let expiries: [String]

    var soon: Bool { soonCount > 0 }
    var text: String {
        guard total > 0 else { return "没有重置卡" }
        guard let earliest = earliestSoon, soonCount > 0 else { return "\(total) 张重置卡" }
        return "\(total) 张重置卡 · \(soonCount) 张 \(When.day(earliest))\(soonCount > 1 ? "起" : "")到期"
    }
    var help: String { expiries.isEmpty ? text : expiries.joined(separator: "\n") }
}

extension AccountQuota {
    var title: String {
        if let scope { return "\(scope) · \(label)" }
        return label.replacingOccurrences(of: "窗口", with: "")
    }
    var shortLabel: String {
        label.replacingOccurrences(of: "窗口", with: "").replacingOccurrences(of: "额度", with: "")
    }
    // "7 天额度剩余" under the big number.
    var remainingCaption: String { "\(shortLabel)额度剩余" }

    var verb: String { expires == true ? "到期" : "回满" }

    // When the window is back (11-26, Gemini 稿): "14:47 回满", "明天 16:00 回满", "52 分钟后回满". Detail pages add
    // how long that is, or the clock beside minutes to go.
    func refill(exact: Bool = false, now: Date = Date()) -> String {
        guard let date = resetDate else { return "官方没给回满时间" }
        guard let due = When.due(date, now: now) else { return "\(verb)时间已过，等下次查询" }
        guard exact else { return due.text(verb) }
        return due.text(verb) + " · " + (due.minutes ? due.soonClock : "还剩 " + When.span(to: date, now: now))
    }

    // Where even that is too wide (11-26, Gemini 稿): 回满 goes, 到期 stays. "下周二 17:00", "52 分钟后".
    func refillBrief(now: Date = Date()) -> String {
        guard verb == "回满", let date = resetDate, let due = When.due(date, now: now) else { return refill(now: now) }
        return due.minutes ? "\(due.time) 分钟后" : [due.day, due.time].compactMap { $0 }.joined(separator: " ")
    }

    // The tooltip says what the text leaves out: how long to go and the full day beside a clock time, the clock
    // beside minutes to go.
    func refillHelp(now: Date = Date()) -> String {
        guard let date = resetDate, let due = When.due(date, now: now) else { return "" }
        let name = shortLabel.trimmingCharacters(in: .whitespaces) + "额度"
        if due.minutes { return "\(name) · \(due.soonClock) \(verb) · 还剩 \(due.time) 分钟" }
        return "\(name) · \(When.span(to: date, now: now))后\(verb) · \(When.dayClock(due.at))"
    }
}

extension QuotaResetCredit {
    var expiryDate: Date? { Self.date(expiresAt) }
}

// Reset chances that expire within a week, grouped per account for the reminder the user asked for.
struct ExpiryReminder: Identifiable {
    let accountID: String
    let accountName: String
    let provider: String
    let count: Int
    let earliest: Date
    // One per expiring credit or grant; dismissal is stored per item so a new one reminds again.
    let itemIDs: [String]
    var id: String { accountID }

    static let window: TimeInterval = 7 * 86400

    var title: String { "\(accountName) 有 \(count) 张重置卡，7 天内到期" }
    var detail: String {
        let when = When.countdown(to: earliest).map { "（\($0)）" } ?? ""
        return "\(count > 1 ? "最早 " : "")\(When.clock(earliest)) 到期\(when)"
            + (provider == "claude" ? "，过期作废；可以在自动接力里设成见底时自动用。" : "，过期作废。")
    }

    static func all(_ accounts: [ManagedAccount], dismissed: Set<String> = [], now: Date = Date()) -> [ExpiryReminder] {
        func soon(_ date: Date?) -> Date? {
            guard let date, date > now, date.timeIntervalSince(now) <= window else { return nil }
            return date
        }
        var result: [ExpiryReminder] = []
        for account in accounts {
            var items: [(id: String, count: Int, date: Date)] = []
            if account.provider == "codex" {
                for credit in account.codexCredits { if let date = soon(credit.expiryDate) { items.append((credit.id, 1, date)) } }
            } else {
                for grant in account.usage?.claudeResets?.grants?.items ?? [] where grant.remaining > 0 {
                    if let date = soon(QuotaResetCredit.date(grant.endsAt)) { items.append(("\(account.id):\(grant.endsAt ?? "")", grant.remaining, date)) }
                }
            }
            guard let earliest = items.map(\.date).min(), items.contains(where: { !dismissed.contains($0.id) }) else { continue }
            result.append(.init(accountID: account.id, accountName: account.displayName, provider: account.provider,
                                count: items.reduce(0) { $0 + $1.count }, earliest: earliest, itemIDs: items.map(\.id)))
        }
        return result.sorted { $0.earliest < $1.earliest }
    }
}

// Dismissed reminders are a per-viewer convenience only; they never change service state.
enum ReminderStore {
    private static let key = "dismissedExpiryReminders"
    static func dismissed() -> Set<String> { Set(UserDefaults.standard.stringArray(forKey: key) ?? []) }
    static func dismiss(_ ids: [String]) {
        var set = dismissed()
        set.formUnion(ids)
        UserDefaults.standard.set(Array(set), forKey: key)
    }
}

// The window and the menu bar both count reminders, so a dismissal has to reach every view at once.
@MainActor
final class ReminderModel: ObservableObject {
    static let shared = ReminderModel()
    @Published private(set) var dismissed = ReminderStore.dismissed()

    func dismiss(_ ids: [String]) {
        ReminderStore.dismiss(ids)
        dismissed = ReminderStore.dismissed()
    }

    func reload() { dismissed = ReminderStore.dismissed() }
}

extension AccountDirectory {
    // A desktop switch finishes after the request returns; this is its report for an attempt
    // started at or after `since` (service time, whole seconds).
    func desktopAttempt(for id: String, since: Double) -> DesktopConnection.Action? {
        guard let action = desktop?.action, action.id == id, (action.at ?? 0) >= since else { return nil }
        return action
    }

    // The desktop's last word on this account while it still reads as news: a switch in progress,
    // a refusal within the hour, a success within ten minutes.
    func recentDesktopAction(for id: String, now: Double = Date().timeIntervalSince1970) -> DesktopConnection.Action? {
        guard let action = desktop?.action, action.id == id else { return nil }
        if action.pending { return action }
        let age = now - (action.at ?? 0)
        return action.state == "failed" ? (age < 3600 ? action : nil) : (age < 600 ? action : nil)
    }
}

enum SwitchRoute: Equatable {
    case plain                 // native files; Codex clients must be closed, running Claude clients switch in place
    case desktop               // hot switch through the connected desktop
    case desktopDisconnected   // hot switch is on but the desktop is not reachable: switching cannot start
}

extension AccountDirectory {
    func current(_ provider: String) -> ManagedAccount? { accounts.first { $0.provider == provider && $0.native } }

    func switchRoute(for account: ManagedAccount) -> SwitchRoute {
        if account.provider == "codex", let desktop, desktop.enabled {
            return desktop.connected ? .desktop : .desktopDisconnected
        }
        return .plain
    }

    // The service holds a desktop switch until a check or poll in progress finishes, so that work
    // does not disable it; a plain switch needs the service idle.
    func switchDisabled(for account: ManagedAccount) -> Bool {
        recoveryNeeded || (busy && switchRoute(for: account) != .desktop)
    }

    func canSwitch(to account: ManagedAccount) -> Bool {
        !account.native || (account.provider == "codex" && desktop?.enabled == true && desktop?.selected != account.id)
    }

    // This Mac, as the authentication centre knows it.
    var thisMac: String { cloud?.devices.first { $0.this == true }?.label ?? cloud?.label ?? "这台 Mac" }

    // Accounts in use here whose login the centre renews: their clients lapse a while after AuthBaton stops.
    var cloudInUse: [ManagedAccount] { accounts.filter { $0.native && $0.cloudRenewed } }

    // The one thing an account's main button offers. Every page and the menu bar share it,
    // so none offers a switch to an account whose saved login is broken or unconfirmed.
    // A transfer under way holds the account; a failed one leaves it where it was, so it can still be switched to.
    func primaryAction(for account: ManagedAccount) -> PrimaryAction {
        if account.transferRunning || (account.cloud == nil && ["cloud_transfer", "cloud_managed"].contains(account.state)) { return .wait }
        if account.staged == true { return .capture }
        if account.recoverable { return .recover }
        // For an account the centre renews, 重新登录 updates the centre's login.
        if account.cloudTrouble != nil { return .relogin }
        if account.state == "reauth" || (!account.cloudRenewed && account.loginNotice?.needsAttention == true) { return .relogin }
        guard canSwitch(to: account) else { return .current }
        // An unanswered renewal may already have replaced the saved refresh token.
        return account.state == "uncertain" ? .wait : .switchTo
    }

    var attentionCount: Int { accounts.filter(\.needsAction).count + (recoveryNeeded ? 1 : 0) }

    // Hot switch is on but the desktop does not answer: Codex cannot switch, by hand or by rule.
    var desktopBlocked: Bool { desktop?.enabled == true && desktop?.connected == false }

    // MARK: Strategy rules

    func rule(for id: String) -> CodexStrategyConfig.Rule? { strategy?.config.accounts.first { $0.id == id } }

    // Priority counts only accounts of the same provider.
    func rank(of account: ManagedAccount) -> Int? {
        let rules = (strategy?.config.accounts ?? []).filter { rule in accounts.contains { $0.id == rule.id && $0.provider == account.provider } }
        return rules.firstIndex { $0.id == account.id }.map { $0 + 1 }
    }

    // The 见底线 notch is only drawn where a rule is actually watching the account.
    func notch(for account: ManagedAccount) -> Int? {
        guard strategy?.config.enabled == true, let rule = rule(for: account.id), rule.participate else { return nil }
        return rule.below
    }

    // Whether any rule of this tool that takes part spends reset cards. Where a switch cannot happen, only
    // such accounts still reset at their line; the others do nothing.
    func resets(_ provider: String) -> Bool {
        (strategy?.config.accounts ?? []).contains { rule in
            rule.participate && rule.order.resets && accounts.contains { $0.id == rule.id && $0.provider == provider }
        }
    }

    // Accounts of a provider in relay order; those without a rule come last.
    func relay(_ provider: String) -> [ManagedAccount] {
        let order = strategy?.config.accounts.map(\.id) ?? []
        return accounts.filter { $0.provider == provider }.enumerated().sorted { a, b in
            let x = order.firstIndex(of: a.element.id) ?? order.count + a.offset
            let y = order.firstIndex(of: b.element.id) ?? order.count + b.offset
            return x < y
        }.map(\.element)
    }

    // A reset needs a fresh quota reading and an idle service; an unconfirmed earlier reset comes first,
    // and an account waits while it moves to the centre or back.
    func resetBlocked(_ account: ManagedAccount, busy accountBusy: Bool) -> Bool {
        accountBusy || busy || recoveryNeeded || maintenanceRunning != true || account.usage?.refreshing == true || account.transferRunning
            || account.usage?.state != "ready" || account.reset?.unresolved == true || account.nextReset == nil
    }

    // Everything that needs a hand, once each: the badge, the menu bar and the accounts page count these.
    func issues(dismissed: Set<String>) -> [Issue] {
        var result: [Issue] = []
        if recoveryNeeded { result.append(Issue(id: "recovery", summary: "上次切换没确认", account: nil)) }
        for account in accounts where account.needsAction {
            result.append(Issue(id: "account-\(account.id)", summary: account.health.title, account: account.id))
        }
        // Without a Codex account there is nothing the desktop would switch.
        if desktopBlocked, accounts.contains(where: { $0.provider == "codex" }) {
            result.append(Issue(id: "desktop", summary: "桌面没连上", account: nil))
        }
        if strategy?.phase == "attention" { result.append(Issue(id: "strategy", summary: "自动换号停下了", account: nil)) }
        let reminders = ExpiryReminder.all(accounts, dismissed: dismissed)
        if !reminders.isEmpty {
            result.append(Issue(id: "reminders", summary: "\(reminders.reduce(0) { $0 + $1.count }) 张重置卡快到期", account: reminders.first?.accountID))
        }
        return result
    }
}

struct Issue: Identifiable {
    let id: String
    let summary: String
    let account: String?
}

enum PrimaryAction {
    case capture    // 本地检查：保存已完成、还没存下来的登录
    case recover    // 恢复同步 / 重试保存
    case relogin    // 重新登录
    case switchTo   // 换上，或由 AuthManager 接管当前登录
    case wait       // nothing to do yet; health says why
    case current    // already the current default
}
