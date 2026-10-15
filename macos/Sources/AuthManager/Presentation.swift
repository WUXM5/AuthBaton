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
        switch state {
        case "reauth", "attention", "unsaved", "backup_pending", "cloud_transfer": true
        // The cloud renews it; the local login dates behind the notice no longer apply.
        case "cloud_managed": false
        default: loginNotice?.needsAttention == true
        }
    }

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
        switch state {
        case "reauth": return Health(tone: .attention, title: "需重新登录", detail: message)
        case "unsaved":
            return provider == "claude" && native
                ? Health(tone: .attention, title: "续期备份没同步", detail: "CLI 照常可用，AuthBaton 会自动重试，不会重复续期。")
                : Health(tone: .attention, title: "续期结果待保存", detail: message)
        case "uncertain": return Health(tone: .neutral, title: "等待重新确认", detail: message)
        case "attention": return Health(tone: .attention, title: "需要处理", detail: message)
        case "backup_pending": return Health(tone: .attention, title: "备份待同步", detail: message)
        // Handed to the cloud: this Mac no longer renews it, and its local login dates are stale.
        case "cloud_transfer": return Health(tone: .attention, title: "云端交接未完成", detail: message)
        case "cloud_managed": return Health(tone: .neutral, title: "由云端维护", detail: message)
        default: break
        }
        if let notice = loginNotice, notice.needsAttention {
            return Health(tone: .attention, title: notice.state == "expired" ? "登录已到期" : "登录即将到期", detail: notice.message)
        }
        switch state {
        case "retry": return Health(tone: .neutral, title: "等待重试", detail: message)
        case "limited": return Health(tone: .neutral, title: "在线查询受限", detail: message)
        case "pending": return Health(tone: .active, title: "等待登录", detail: message)
        case "checking": return Health(tone: .active, title: "正在检查", detail: message)
        case "delegated": return Health(tone: .neutral, title: "可用", detail: "由官方客户端续期；额度独立查询，无需退出客户端。")
        case "saved": return Health(tone: .neutral, title: "已保存", detail: "已保存在本机，尚未在线验证。")
        case "healthy": return Health(tone: .neutral, title: "可用", detail: nil)
        // A state this build does not know says nothing about whether the login works.
        default: return Health(tone: .attention, title: "状态未识别", detail: message)
        }
    }

    // A state worth a line on the account even when nothing is broken: waiting, checking, unknown.
    var quietState: Bool { ["delegated", "healthy", "saved"].contains(state) && checkLabel == nil && !needsAction }

    // Short status for the quota line; stale or unknown quota is muted, not alarming.
    var quotaCaption: String? {
        guard let usage else { return "尚未查询额度" }
        switch usage.state {
        case "ready": return nil
        case "pending": return "正在查询额度"
        case "expired": return "等待续期后重新查询"
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
        if reset?.unresolved == true { return "先确认上次重置的结果。" }
        if usage?.refreshing == true { return "额度查询进行中，稍后再移除。" }
        if warmup?.running == true { return "正在发送预热，稍后再移除。" }
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

    // When the window fills back up. Compact keeps whole days only, for cards and rows.
    func refill(compact: Bool = true, exact: Bool = false) -> String {
        guard let date = resetDate else { return "官方没给回满时间" }
        let verb = expires == true ? "到期" : "回满"
        guard let countdown = When.countdown(to: date) else { return "\(verb)时间已过，等下次查询" }
        let seconds = date.timeIntervalSinceNow
        let short = compact && seconds >= 86400 ? "\(Int(seconds / 86400)) 天后" : countdown
        return short + verb + (exact ? " · \(When.text(date))" : "")
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

    var title: String { "\(accountName) 有 \(count) 张重置卡 7 天内到期" }
    var detail: String {
        let when = When.countdown(to: earliest).map { "（\($0)）" } ?? ""
        return "\(count > 1 ? "最早 " : "")\(When.text(earliest)) 到期\(when)"
            + (provider == "claude" ? "，过期作废；可以在「换号与重置」里设成见底时自动用。" : "，过期作废。")
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

    // The service refuses every local operation naming an account handed to the cloud, strategy rules included.
    func cloudBound(_ id: String) -> Bool { cloud?.transfers.contains { $0.localId == id } == true }

    // The one thing an account's main button offers. Every page and the menu bar share it,
    // so none offers a switch to an account whose saved login is broken or unconfirmed.
    func primaryAction(for account: ManagedAccount) -> PrimaryAction {
        if ["cloud_transfer", "cloud_managed"].contains(account.state) { return .wait }
        if account.recoverable { return .recover }
        if account.state == "reauth" || account.loginNotice?.needsAttention == true { return .relogin }
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
    // and the service refuses any local operation on an account handed to the cloud.
    func resetBlocked(_ account: ManagedAccount, busy accountBusy: Bool) -> Bool {
        accountBusy || busy || recoveryNeeded || maintenanceRunning != true || account.usage?.refreshing == true || cloudBound(account.id)
            || account.usage?.state != "ready" || account.reset?.unresolved == true || account.codexCredits.isEmpty
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
    case recover    // 恢复同步 / 重试保存
    case relogin    // 重新登录
    case switchTo   // 换上，或由 AuthManager 接管当前登录
    case wait       // nothing to do yet; health says why
    case current    // already the current default
}
