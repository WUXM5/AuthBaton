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

    var health: Health {
        if let label = checkLabel { return Health(tone: .accent, title: label, detail: nil) }
        switch state {
        case "reauth": return Health(tone: .danger, title: "需重新登录", detail: message)
        case "unsaved":
            return provider == "claude" && native
                ? Health(tone: .warning, title: "续期备份未同步", detail: "Claude CLI 可以照常使用；AuthManager 会自动重试同步，不会重复续期。")
                : Health(tone: .warning, title: "续期结果待保存", detail: message)
        case "uncertain": return Health(tone: .warning, title: "等待重新确认", detail: message)
        case "attention": return Health(tone: .danger, title: "需要处理", detail: message)
        case "backup_pending": return Health(tone: .warning, title: "备份待同步", detail: message)
        // Handed to the cloud: this Mac no longer renews it, and its local login dates are stale.
        case "cloud_transfer": return Health(tone: .warning, title: "云端交接未完成", detail: message)
        case "cloud_managed": return Health(tone: .neutral, title: "由云端维护", detail: message)
        default: break
        }
        if let notice = loginNotice, notice.needsAttention {
            return Health(tone: .warning, title: notice.state == "expired" ? "登录已到期" : "登录即将到期", detail: notice.message)
        }
        switch state {
        case "retry": return Health(tone: .warning, title: "等待重试", detail: message)
        case "limited": return Health(tone: .warning, title: "在线查询受限", detail: message)
        case "pending": return Health(tone: .accent, title: "等待登录", detail: message)
        case "checking": return Health(tone: .accent, title: "正在检查", detail: message)
        // "当前" is already a tag next to the name, so health only says whether the login is usable.
        case "delegated": return Health(tone: .success, title: "可用", detail: "由官方客户端续期；额度独立查询，无需退出客户端。")
        case "saved": return Health(tone: .neutral, title: "已保存", detail: "已保存在本机，尚未在线验证。")
        case "healthy": return Health(tone: .success, title: "可用", detail: nil)
        // A state this build does not know says nothing about whether the login works.
        default: return Health(tone: .warning, title: "状态未识别", detail: message)
        }
    }

    // Short status for the quota cell; stale or unknown quota is muted, not alarming.
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
}

extension AccountQuota {
    var title: String {
        if let scope { return "\(scope) · \(label)" }
        return label.replacingOccurrences(of: "窗口", with: "")
    }
    var shortLabel: String {
        label.replacingOccurrences(of: "窗口", with: "").replacingOccurrences(of: "额度", with: "")
    }
    var resetCaption: String {
        guard let date = resetDate else { return "官方未提供重置时间" }
        let verb = expires == true ? "到期" : "重置"
        if let countdown = When.countdown(to: date) { return "\(countdown)\(verb) · \(When.text(date))" }
        return "\(verb)时间已过，等待下次查询"
    }
    // Whole days only, so it fits table cells and the menu bar; the inspector shows the exact time.
    var shortReset: String {
        guard let date = resetDate else { return "—" }
        let seconds = date.timeIntervalSinceNow
        if seconds >= 86400 { return "\(Int(seconds / 86400)) 天后" }
        return When.countdown(to: date) ?? "已过"
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

    var title: String { "\(accountName) 有 \(count) 次重置 7 天内到期" }
    var detail: String {
        let when = When.countdown(to: earliest).map { "（\($0)）" } ?? ""
        return "\(count > 1 ? "最早 " : "")\(When.text(earliest)) 到期\(when)"
            + (provider == "claude" ? "，过期后作废；可在「换号与重置」里设为见底时自动使用。" : "，过期后作废。")
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

extension AccountDirectory {
    // A desktop switch finishes after the request returns; this is its report for an attempt
    // started at or after `since` (service time, whole seconds).
    func desktopAttempt(for id: String, since: Double) -> DesktopConnection.Action? {
        guard let action = desktop?.action, action.id == id, (action.at ?? 0) >= since else { return nil }
        return action
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

    // The one thing an account's main button offers. The inspector and the menu bar share it,
    // so neither offers a switch to an account whose saved login is broken or unconfirmed.
    func primaryAction(for account: ManagedAccount) -> PrimaryAction {
        if ["cloud_transfer", "cloud_managed"].contains(account.state) { return .wait }
        if account.recoverable { return .recover }
        if account.state == "reauth" || account.loginNotice?.needsAttention == true { return .relogin }
        guard canSwitch(to: account) else { return .current }
        // An unanswered renewal may already have replaced the saved refresh token.
        return account.state == "uncertain" ? .wait : .switchTo
    }

    var attentionCount: Int { accounts.filter(\.needsAction).count + (recoveryNeeded ? 1 : 0) }
}

enum PrimaryAction {
    case recover    // 恢复同步 / 重试保存
    case relogin    // 重新登录
    case switchTo   // 设为当前，或由 AuthManager 接管当前登录
    case wait       // nothing to do yet; health says why
    case current    // already the current default
}

// Plain-language preview of what the strategy would do for the current Codex account right now.
// Mirrors the service: at the account's own line, the first doable action in its order runs;
// a later doable action is only a fallback; a switch ends the episode.
struct StrategyPreview {
    struct Step: Identifiable {
        let id = UUID()
        let title: String
        let detail: String
        let active: Bool
    }
    let below: Int?
    let steps: [Step]
    let others: [String]

    init(config: CodexStrategyConfig, accounts: [ManagedAccount], desktop: DesktopConnection?) {
        let codex = accounts.filter { $0.provider == "codex" }
        let name = { (id: String) in codex.first { $0.id == id }?.displayName ?? "账号" }
        // Claude needs no desktop: the current one switches in place, every one can reset on its own.
        others = config.accounts.compactMap { r in
            guard r.participate, let account = accounts.first(where: { $0.id == r.id }) else { return nil }
            let count = account.usableResets
            let reset = "自动重置" + (count == 0 ? "（现在没有次数）" : "（\(count) 次可用）")
            if account.provider == "claude" {
                let change = "换到下一个 Claude 账号（不暂停会话）"
                let action: String? = switch (account.native, r.order) {
                case (true, .resetFirst): "先\(reset)，仍见底再\(change)"
                case (true, .switchFirst): "先\(change)，换不了再\(reset)"
                case (true, .switchOnly): change
                case (_, .resetOnly), (false, .resetFirst), (false, .switchFirst): reset
                case (false, .switchOnly): nil
                }
                return action.map { "\(account.displayName)（Claude）：剩余 ≤ \(r.below)% 时\($0)" }
            }
            guard r.order.resets, account.provider == "codex", !account.native else { return nil }
            return "\(account.displayName)：剩余 ≤ \(r.below)% 时\(reset)"
        }
        guard let current = codex.first(where: \.native),
              let rule = config.accounts.first(where: { $0.id == current.id }) else {
            below = nil
            steps = [Step(title: "没有当前 Codex 账号", detail: "设定当前账号后才会自动处理。", active: false)]
            return
        }
        below = rule.below
        guard rule.participate else {
            steps = [Step(title: "\(current.displayName) 未参与自动处理", detail: "在下表打开「参与」后才会生效。", active: false)]
            return
        }
        let switchAvailable = desktop?.enabled == true && desktop?.connected == true
        let candidate = config.accounts.first { r in
            r.participate && r.id != current.id && (codex.first { $0.id == r.id }.flatMap(\.lowestPrimary) ?? 0) > r.below
        }
        let credits = current.codexCredits
        let reset: (title: String, detail: String?, skip: String) = (
            "用 1 次重置",
            credits.first.map { "用最早到期的那次（\(QuotaResetCredit.expiryLabel($0.expiresAt))），还剩 \(credits.count) 次。" },
            "跳过：现在没有可用的重置次数。")
        let change: (title: String, detail: String?, skip: String) = {
            guard let candidate else { return ("换号", nil, "跳过：没有额度高于各自见底线的候选账号。") }
            return ("换到 \(name(candidate.id))", switchAvailable ? "暂停运行中的桌面会话，换号确认后逐个接续；这一轮就此结束。" : nil,
                    "跳过：Codex 桌面未连接，没法暂停和接续会话。")
        }()
        let chain = switch rule.order {
        case .resetFirst: [reset, change]
        case .switchFirst: [change, reset]
        case .resetOnly: [reset]
        case .switchOnly: [change]
        }
        var result: [Step] = []
        var decided = false
        for action in chain {
            guard let detail = action.detail else {
                result.append(Step(title: action.title, detail: action.skip, active: false))
                continue
            }
            // Only the first doable action runs; after a switch nothing else happens this round.
            let fallback = rule.order == .resetFirst ? "后备，重置后仍见底时：" : "后备，换不了号时："
            result.append(Step(title: action.title, detail: decided ? fallback + detail : detail, active: !decided))
            decided = true
        }
        result.append(Step(title: "都做不了时停下", detail: "在账号页和菜单栏提醒；额度回到 \(rule.below)% 以上后重新开始。", active: false))
        steps = result
    }
}
