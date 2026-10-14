import Foundation

struct ProviderStatus: Decodable, Identifiable, Sendable {
    let id: String
    let name: String
    let state: String
    let authKind: String
    let summary: String
    let source: String
    let expiresAt: Double?
    let details: [String]

    var stateLabel: String {
        switch state {
        case "configured": "已配置"
        case "expired": "已到期"
        case "missing": "未配置"
        case "unavailable": "未安装"
        default: "需检查"
        }
    }
}

struct Snapshot: Decodable, Sendable {
    let checkedAt: Double
    let serviceVersion: String
    let providers: [ProviderStatus]
    let verification: String
}

struct RPCRequest: Encodable {
    let version = 1
    let id: String
    let method: String
    var params: LoginParams? = nil
}

struct LoginParams: Encodable, Sendable {
    var provider: String? = nil
    var confirm_replace: Bool? = nil
    var attempt_id: String? = nil
    var code: String? = nil
    var id: String? = nil
    var label: String? = nil
    var online: Bool? = nil
    var confirm: Bool? = nil
    var automatic: Bool? = nil
    var port: UInt16? = nil
    var enabled: Bool? = nil
    var credit_id: String? = nil
    var reset_id: String? = nil
    var cloud_action: String? = nil
    var mode: String? = nil
    var endpoint: String? = nil
    var cloud_account_id: String? = nil
    var strategy: CodexStrategyInput? = nil
    var before: String? = nil
    var limit: Int? = nil
    var hour: Int? = nil
}

struct LoginResult: Decodable, Sendable { let attempt: LoginAttempt? }

// Who a login was started for: an existing account, a cloud account, or (both nil) a new account.
struct LoginTarget: Equatable, Sendable {
    var accountID: String?
    var cloudAccountID: String?
}

struct LoginAttempt: Decodable, Sendable {
    let attemptId: String
    let provider: String
    let phase: String
    let message: String
    let verificationUrl: String?
    let userCode: String?
    var isActive: Bool { ["starting", "waiting", "cancelling"].contains(phase) }
    var phaseLabel: String {
        switch phase {
        case "starting": "正在准备"
        case "waiting": "等待浏览器授权"
        case "cancelling": "正在停止"
        case "succeeded": "官方登录已完成"
        case "cancelled": "已取消授权等待"
        case "stopped": "登录等待已停止"
        default: "未确认登录成功"
        }
    }
}

struct RPCError: Decodable, Sendable {
    let code: String
    let message: String
}

struct ServiceInfo: Decodable, Sendable {
    let serviceVersion: String
}

struct RPCResponse<Result: Decodable & Sendable>: Decodable, Sendable {
    let version: Int
    let id: String?
    let result: Result?
    let error: RPCError?
}

enum ServiceError: LocalizedError {
    case connection
    case invalidResponse
    case pathTooLong
    case missingExecutable
    case requestFailed(String)

    var errorDescription: String? {
        switch self {
        case .connection: "无法连接本地认证服务。请重新打开应用后重试。"
        case .invalidResponse: "认证服务返回了无法识别的状态。"
        case .pathTooLong: "本地服务目录路径过长。"
        case .missingExecutable: "应用中缺少后台服务，请使用完整的 AuthManager.app。"
        case .requestFailed(let message): message
        }
    }
}

struct AccountDirectory: Decodable, Sendable {
    let accounts: [ManagedAccount]
    let busy: Bool
    let checksBlocked: Bool?
    let recoveryNeeded: Bool
    let notice: String?
    let maintenanceRunning: Bool
    let desktop: DesktopConnection?
    var cloud: CloudStatus? = nil
    var strategy: CodexStrategyStatus? = nil
}

struct CodexStrategyInput: Encodable, Sendable {
    let enabled: Bool
    let accounts: [Rule]
    struct Rule: Encodable, Sendable { let id: String; let order: CodexStrategyOrder; let participate: Bool; let below: Int }
}
// What an account does once its remaining primary quota reaches its own line.
enum CodexStrategyOrder: String, Codable, CaseIterable, Identifiable, Sendable {
    case resetFirst = "reset_first"
    case switchFirst = "switch_first"
    case resetOnly = "reset_only"
    case switchOnly = "switch_only"
    var id: Self { self }
    var label: String {
        switch self {
        case .resetFirst: "先重置"
        case .switchFirst: "先换号"
        case .resetOnly: "只重置"
        case .switchOnly: "只换号"
        }
    }
    var resets: Bool { self != .switchOnly }
    var switches: Bool { self != .resetOnly }
    var explanation: String {
        switch self {
        case .resetFirst: "见底时先用 1 次重置；没有次数，或重置后仍见底，再换号。"
        case .switchFirst: "见底时先换号；换不了（不是当前账号、没有候选，或 Codex 桌面未连接），再用 1 次重置。"
        case .resetOnly: "见底时只用重置次数，不会从这个账号换走。"
        case .switchOnly: "见底时只换号，不消耗重置次数。"
        }
    }
}
struct CodexStrategyConfig: Decodable, Equatable, Sendable {
    var enabled: Bool = false
    var accounts: [Rule] = []
    struct Rule: Decodable, Identifiable, Equatable, Sendable {
        let id: String
        // New accounts never spend credits until the user picks an order that resets.
        var order: CodexStrategyOrder = .switchOnly
        var participate: Bool
        var below: Int = 2
    }
    func rank(of id: String) -> Int { accounts.firstIndex { $0.id == id }! + 1 }
    mutating func move(id: String, by offset: Int) {
        let index = rank(of: id) - 1
        let destination = index + offset
        if accounts.indices.contains(destination) { accounts.swapAt(index, destination) }
    }
    var input: CodexStrategyInput {
        CodexStrategyInput(enabled: enabled,
            accounts: accounts.map { .init(id: $0.id, order: $0.order, participate: $0.participate, below: $0.below) })
    }
}
struct CodexStrategyStatus: Decodable, Sendable {
    let config: CodexStrategyConfig
    let phase: String?
    let busy: Bool
    let message: String
    let lastAt: Double?
    let run: Run?
    var accountStates: [AccountState]? = nil
    struct AccountState: Decodable, Identifiable, Sendable { let id: String; let message: String }
    struct Run: Decodable, Sendable {
        let id: String; let source: String; let target: String; let createdAt: Double
        let tasks: [TaskState]
    }
    struct TaskState: Decodable, Sendable {
        let threadId: String; let state: String
        var label: String {
            switch state {
            case "planned": "等待暂停"
            case "interrupt_sent": "正在确认暂停"
            case "paused": "已暂停"
            case "resume_sent": "正在确认接续"
            case "resumed": "已接续"
            case "skipped": "状态已改变，已跳过"
            default: "待核对"
            }
        }
    }
}

struct CloudStatus: Decodable, Sendable {
    let mode: String
    let endpoint: String
    let label: String
    let paired: Bool
    let pair: CloudPair?
    let accounts: [CloudAccount]
    let devices: [CloudDevice]
    let selected: [String: String]
    let busy: Bool
    let error: String?
    let checkedAt: Double?
    let pendingActivation: String?
    let transfers: [CloudTransfer]
}
struct CloudPair: Decodable, Sendable { let id: String; let code: String; let url: String; let expiresAt: Double }
struct CloudDevice: Decodable, Identifiable, Sendable { let id: String; let label: String; let lastSeen: Double? }
struct CloudTransfer: Decodable, Sendable { let localId: String; let cloudId: String?; let phase: String }
struct CloudAccount: Decodable, Identifiable, Sendable {
    let id: String
    let provider: String
    let label: String
    let identity: String
    let plan: String
    let expiresAt: Double
    let refreshExpiresAt: Double?
    let version: Int
    let state: String
    let automatic: Bool
    let lastRefresh: Double?
    let error: String?
    let usage: CloudUsage?
    let usageError: String?
    var stateLabel: String {
        switch state { case "ready": "云端维护中"; case "awaiting_handoff": "等待交接"; case "verification_pending": "续期已保存，待验证"; case "refreshing": "正在续期"; case "uncertain": "等待重新确认"; case "reauthorization_required": "需要重新授权"; default: "需要处理" }
    }
}
struct CloudUsage: Decodable, Sendable { let windows: [CloudWindow]; let checkedAt: Double }
struct CloudWindow: Decodable, Sendable { let label: String; let usedPercent: Double; let resetsAt: Double? }
struct LoginNotice: Decodable, Sendable {
    let state: String
    let expiresAt: Double?
    let message: String
    let source: String?
    var needsAttention: Bool { ["soon", "expired"].contains(state) }
}
struct DesktopConnection: Decodable, Sendable {
    let enabled: Bool
    let port: UInt16
    let selected: String?
    let connected: Bool
    // AuthManager supplies the desktop's login. Connected alone only means its debug port answers.
    let managed: Bool
    let busy: Bool
    let message: String
    let action: Action?
    struct Action: Decodable, Sendable, Equatable {
        let id: String
        let state: String
        let message: String
        let at: Double?
        var pending: Bool { ["relaunching", "checking", "switching"].contains(state) }
    }
}
struct ManagedAccount: Decodable, Identifiable, Sendable {
    let id: String
    let provider: String
    let label: String
    let identity: String?
    let plan: String?
    let expiresAt: Double?
    let refreshExpiresAt: Double?
    let native: Bool
    let automatic: Bool
    let state: String
    let message: String
    let lastLocal: Double?
    let lastOnline: Double?
    let lastRefresh: Double?
    let nextCheck: Double
    let quota: [AccountQuota]
    let usage: QuotaStatus?
    let reset: QuotaResetAttempt?
    let renewalMode: String?
    let loginNotice: LoginNotice?
    let checkStatus: String?
    let checkOnline: Bool?
    let warmup: ClaudeWarmup?
    var checkLabel: String? {
        guard let checkStatus else { return nil }
        let kind = checkOnline == true ? "认证检查" : "本地检查"
        return checkStatus == "queued" ? "\(kind)排队中" : "正在\(kind)"
    }
    var renewalLabel: String? {
        switch renewalMode {
        case "claude_coordinated": automatic ? "续期方式：与 Claude 官方客户端协同" : "自动维护已关闭；手动检查仍使用协调锁"
        default: nil
        }
    }
    var stateLabel: String {
        switch state {
        case "healthy": "在线检查通过"
        case "saved": "本地已保存"
        case "pending": "等待登录"
        case "checking": "正在检查"
        case "delegated": "由官方客户端维护"
        case "retry": "等待重试"
        case "limited": "在线查询受限"
        case "reauth": "需重新登录"
        case "unsaved": "续期待保存"
        case "backup_pending": "原生凭据待同步备份"
        case "uncertain": "等待重新确认"
        default: "需要处理"
        }
    }
}
struct AccountQuota: Decodable, Sendable {
    let label: String
    let usedPercent: Double
    let resetsAt: Double?
    let resetsAtIso: String?
    let scope: String?
    let kind: String?
    let expires: Bool?
    var resetDate: Date? { resetsAt.map(Date.init(timeIntervalSince1970:)) ?? QuotaResetCredit.date(resetsAtIso) }
    var remaining: Int { Int(max(0, 100 - usedPercent).rounded(.down)) }
}
struct ClaudeWarmup: Decodable, Sendable {
    struct Record: Decodable, Sendable {
        let at: Double
        let ok: Bool
        let message: String
    }
    let enabled: Bool
    let hour: Int
    let running: Bool
    let last: Record?
}

struct QuotaStatus: Decodable, Sendable {
    let state: String
    let message: String
    let windows: [AccountQuota]
    let checkedAt: Double?
    let attemptedAt: Double?
    let nextUpdate: Double
    let refreshing: Bool
    let resetCredits: QuotaResetCredits?
    let claudeResets: ClaudeResetStatus?
}
struct ClaudeResetStatus: Decodable, Sendable {
    let grants: ClaudeResetGrants?
    let weekly: ClaudeWeeklyReset?
}
struct ClaudeResetGrants: Decodable, Sendable {
    let eligible: Bool
    let message: String
    let remaining: Int?
    let items: [ClaudeResetGrant]
    let cooldownUntil: String?
    let weeklyResetsAt: String?
}
struct ClaudeResetGrant: Decodable, Sendable {
    let label: String
    let remaining: Int
    let total: Int?
    let startsAt: String?
    let endsAt: String?
    let limits: [String]
    let paused: Bool
    let usableNow: Bool?
    let requiresLimit: Bool?
    func availability(eligible: Bool, now: Date = Date()) -> String {
        if remaining == 0 { return "已用完" }
        if let end = QuotaResetCredit.date(endsAt), end <= now { return "已到期" }
        if let start = QuotaResetCredit.date(startsAt), start > now { return "尚未开始" }
        if paused { return "已暂停" }
        if !eligible { return "当前不可用" }
        switch usableNow { case true: return "当前可用"; case false: return "当前不可用"; default: return "可用状态未提供" }
    }
}
struct ClaudeWeeklyReset: Decodable, Sendable {
    let eligible: Bool
    let message: String
    let available: Bool?
    let perWeek: Int?
    let nextAvailableAt: String?
    let weeklyResetsAt: String?
}
struct QuotaResetCredits: Decodable, Sendable {
    let availableCount: Int
    let details: [QuotaResetCredit]?
}
struct QuotaResetCredit: Decodable, Identifiable, Sendable {
    let id: String
    let resetType: String
    let status: String
    let expiresAt: String?

    static func date(_ value: String?) -> Date? {
        guard let value else { return nil }
        let formatter = ISO8601DateFormatter()
        if let date = formatter.date(from: value) { return date }
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.date(from: value)
    }
    static func expiryLabel(_ value: String?) -> String {
        guard let value else { return "未提供到期时间" }
        guard let date = date(value) else { return "到期时间格式不可识别" }
        return date.formatted(date: .abbreviated, time: .shortened)
    }
    var available: Bool {
        status == "available" && resetType == "codex_rate_limits"
            && (expiresAt == nil || Self.date(expiresAt).map { $0 > Date() } == true)
    }
}
struct QuotaResetAttempt: Decodable, Sendable {
    let requestId: String
    let creditId: String?  // nil for Claude: grant IDs stay in the service, a retry names the operation
    let expiresAt: String?
    let outcome: String
    let message: String
    let unresolved: Bool
    var source: String? = nil
    var startedAt: Double? = nil
    var finishedAt: Double? = nil
}
struct ResetHistoryPage: Decodable, Sendable {
    let entries: [ResetHistoryEntry]
    let nextCursor: String?
    let total: Int
}
struct ResetHistoryEntry: Decodable, Identifiable, Sendable {
    let id: String
    let accountId: String
    let accountLabel: String
    let source: String
    let startedAt: Double
    let finishedAt: Double?
    let outcome: String
    let message: String
    let trigger: Trigger?
    let retry: Bool
    struct Trigger: Decodable, Sendable { let remainingPercent: Double; let thresholdPercent: Int }
    var sourceLabel: String {
        switch source { case "automatic": "自动"; case "manual": "手动"; default: "旧记录 · 来源未知" }
    }
    var outcomeLabel: String { Self.outcomeText(outcome) }
    static func outcomeText(_ outcome: String) -> String {
        switch outcome {
        case "reset": "重置成功"
        case "already_redeemed": "已确认重置"
        case "pending": "执行中"
        case "unknown": "结果待确认"
        case "nothing_to_reset": "无需重置"
        case "no_credit": "次数不可用"
        default: "未提交"
        }
    }
    static func time(_ value: Double) -> String {
        Date(timeIntervalSince1970: value).formatted(.dateTime.year().month(.twoDigits).day(.twoDigits).hour(.twoDigits(amPM: .omitted)).minute(.twoDigits).second(.twoDigits))
    }
}
struct AccountResult: Decodable, Sendable {
    let directory: AccountDirectory
    let attempt: LoginAttempt?
}
