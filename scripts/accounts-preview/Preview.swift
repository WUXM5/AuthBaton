// Compiles the production views against a stub service. The stub has no socket, process,
// network or credential access; every account, email and ID below is synthetic.
import AppKit
import SwiftUI
import Vision

@MainActor final class ServiceController: ObservableObject {
    static let shared = ServiceController()
    @Published var directory: AccountDirectory?
    @Published var snapshot: Snapshot?
    @Published var accountBusy = false
    @Published var quotaRequests: Set<String> = []
    @Published var quotaErrors: [String: String] = [:]
    @Published var accountError: String?
    @Published var accountErrorID: String?
    @Published var errorText: String?
    @Published var login: LoginAttempt?
    @Published var loginTarget: LoginTarget?
    @Published var isConnected = true
    @Published var isConnecting = false
    let directoryError: String? = nil
    let loginBusy = false
    let loginError: String? = nil
    let isRefreshing = false
    var canRestart: Bool { !isConnecting }
    let socketPath = NSHomeDirectory() + "/Library/Application Support/AuthManager/service.sock"
    // Recorded instead of sent: "method {json params}".
    var calls: [String] = []
    var failNext: String?

    func accountOperation(_ method: String, params: LoginParams = LoginParams()) async {
        let json = String(data: try! JSONEncoder().encode(params), encoding: .utf8)!
        calls.append("\(method) \(json)")
        if let failure = failNext {
            failNext = nil
            accountError = failure
            accountErrorID = params.id
        } else {
            accountError = nil
            accountErrorID = nil
        }
    }
    func cloudAction(_ action: String, id: String? = nil, mode: String? = nil, endpoint: String? = nil, label: String? = nil, confirm: Bool? = nil, automatic: Bool? = nil) async {
        await accountOperation("accounts.cloud", params: LoginParams(id: id, label: label, confirm: confirm, automatic: automatic, cloud_action: action, mode: mode, endpoint: endpoint))
    }
    func claudeDesktop(_ action: String, account: String? = nil, label: String? = nil, enabled: Bool? = nil, number: Int? = nil,
                       snapshot: String? = nil, confirm: Bool? = nil) async {
        await accountOperation("accounts.claude_desktop", params: LoginParams(
            label: label, confirm: confirm, enabled: enabled, desktop_action: action, account: account, number: number,
            snapshot: snapshot))
    }
    func network(_ action: String, phone: String? = nil, ssid: String? = nil, password: String? = nil) async {
        await accountOperation("accounts.network", params: LoginParams(network_action: action, phone: phone, ssid: ssid, password: password))
    }
    // Passive: keeps the service reading traffic. Not a request the checks count.
    func watchNetwork() async {}
    func clearAccountError() { accountError = nil; accountErrorID = nil }
    func refresh() async {}
    func restart() async { calls.append("restart {}") }
    func checkQuota(id: String? = nil) async { calls.append("accounts.quota \(id ?? "all")") }
    func refreshQuotaIfNeeded() async {}
    func resetHistory(accountID: String?, before: String? = nil) async throws -> ResetHistoryPage {
        try Fixture.decoder.decode(ResetHistoryPage.self, from: Data(#"{"entries":[{"id":"s-3:0","account_id":"synthetic-p","account_label":"Codex · 个人","source":"automatic","started_at":1790265600,"finished_at":1790265602,"outcome":"reset","message":"已用 1 张重置卡，将重新查询用量和剩余张数","trigger":{"remaining_percent":63.0,"threshold_percent":2,"expiring":true},"retry":false},{"id":"s-1:0","account_id":"synthetic-p","account_label":"Codex · 个人","source":"automatic","started_at":1790179200,"finished_at":1790179202,"outcome":"reset","message":"已用 1 张重置卡，将重新查询用量和剩余张数","trigger":{"remaining_percent":1.2,"threshold_percent":2},"retry":false},{"id":"s-2:0","account_id":"synthetic-p","account_label":"Codex · 个人","source":"legacy","started_at":1790089000,"finished_at":null,"outcome":"reset","message":"旧版本的记录没保存来源和结果时间","trigger":null,"retry":false}],"next_cursor":null,"total":3}"#.utf8))
    }
    func clearLogin() {}
    func cancelLogin() async {}
    func startLogin(provider: String, label: String, accountID: String? = nil, cloudAccountID: String? = nil) async {}
    func submitLoginCode(_ code: String) async {}
}

@MainActor enum BrandArtwork {
    static func menuIcon(badge: NSColor?) -> NSImage { NSImage(systemSymbolName: "key", accessibilityDescription: nil)! }
}

// MARK: Synthetic state shaped like the 2026-09-29 live directory

enum Fixture {
    static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()
    static let now = Date().timeIntervalSince1970
    static func iso(_ offset: Double) -> String { ISO8601DateFormatter().string(from: Date(timeIntervalSince1970: now + offset)) }
    static func credit(_ id: String, _ days: Double) -> [String: Any] {
        ["id": id, "reset_type": "codex_rate_limits", "status": "available", "expires_at": iso(days * 86400)]
    }
    // Claude desktop with three signed-in accounts whose session lists match, one older folder.
    static func claudeDesktop(_ now: Double) -> [String: Any] {
        func usage(_ remaining: Int, _ age: Double) -> [String: Any] { ["remaining": remaining, "at": now - age, "stale": false] }
        func account(_ handle: String, _ number: Int?, _ display: String, _ email: String?, alias: String?, inUse: Bool, participates: Bool = true,
                     five: [String: Any]?, weekly: [String: Any]?, linked: String? = nil, known: Bool = true, sessions: Int = 25, bypass: Bool = false) -> [String: Any] {
            var row: [String: Any] = ["handle": handle, "display": display, "in_use": inUse, "login_saved": known, "participates": participates,
                                      "sessions": sessions, "behind": 0, "known": known, "bypass": bypass]
            if let number { row["number"] = number }
            if let email { row["email"] = email }
            if let alias { row["alias"] = alias }
            if let five { row["five_hour"] = five }
            if let weekly { row["weekly"] = weekly }
            if let linked { row["linked_account"] = linked }
            return row
        }
        return [
            "installed": true, "running": true, "version": "4.4212.0", "switch_supported": true, "background": true, "syncing": false,
            "headline": "3 个账号的会话都一样 · 在用 #2 work", "sessions": 25, "pending": 0, "restart_needed": false,
            "last_sync": ["at": now - 140, "ok": true, "message": "没有要改的"],
            "accounts": [
                account("h-nova", 1, "nova@example.invalid", "nova@example.invalid", alias: nil, inUse: false,
                        five: usage(88, 600), weekly: usage(54, 600), linked: "synthetic-claude"),
                account("h-orbit", 2, "work", "orbit@example.invalid", alias: "work", inUse: true, five: usage(38, 120), weekly: usage(71, 120), bypass: true),
                account("h-lab", 3, "lab", "lab@example.invalid", alias: "lab", inUse: false, participates: false, five: nil, weekly: usage(12, 5 * 3600), bypass: true),
                account("h-old", nil, "c***@example.invalid", "c***@example.invalid", alias: nil, inUse: false, five: nil, weekly: nil, known: false, sessions: 6),
            ],
            "picks": ["next": "h-nova", "next_available": "h-nova", "best": "h-nova"],
            "snapshots": [["id": "20261102-210000-watch", "at": now - 2400, "kind": "hourly", "folders": 3],
                          ["id": "20261101-090000-baseline", "at": now - 36 * 3600, "kind": "baseline", "folders": 2]],
            "retention": ["at_risk": false, "kept": true, "can_undo": true, "can_keep": true],
            "code_relay": ["enabled": false, "install_state": "not_installed", "state": "off", "message": "同 Team 账号不重启桌面、对话不中断", "busy": false, "relayed_count": 0],
            "symlinks": 0,
        ]
    }
    static func codex(_ id: String, _ label: String, _ email: String, plan: String, native: Bool, state: String, used: Double, resetDays: Double, credits: [[String: Any]]?, extra: Bool = false) -> [String: Any] {
        var windows: [[String: Any]] = [["label": "7 天窗口", "used_percent": used, "resets_at": now + resetDays * 86400, "expires": false]]
        if extra { windows.append(["label": "7 天窗口", "used_percent": 0, "resets_at": now + 7 * 86400, "scope": "gpt-reserve · model-x", "kind": "model"]) }
        return ["id": id, "provider": "codex", "label": label, "identity": email, "plan": plan, "native": native, "automatic": true,
                "state": state, "message": native ? "当前默认账号由官方客户端续期；额度可独立查询，无需退出客户端" : "Codex 在线检查成功",
                "last_local": now - 480, "last_online": native ? now - 6 * 86400 : now - 480, "last_refresh": native ? NSNull() : now - 3 * 86400,
                "expires_at": now + (native ? 36 : 170) * 3600, "next_check": now + 1800, "quota": [],
                "usage": ["state": "ready", "message": "额度已更新", "windows": windows, "checked_at": now - 240, "attempted_at": now - 242,
                          "next_update": now + 60, "refreshing": false,
                          "reset_credits": credits.map { ["available_count": $0.count, "details": $0] } ?? NSNull()]]
    }
    // Safe network switching: synthetic networks, an iPhone, sessions and history.
    static func network(_ now: Double, _ state: String) -> [String: Any] {
        let live: [String: Any] = ["safe": ["notes": "idle", "rho": "idle", "codex": "idle"], "busy": ["notes": "live", "rho": "idle", "codex": "live"],
                                   "waiting": ["notes": "held", "rho": "held", "codex": "live"]][state == "wrong" ? "safe" : state] as! [String: String]
        let running = state == "waiting"
        var operation: Any = NSNull()
        if running {
            operation = ["kind": "phone", "target": "我的 iPhone", "from": "Studio-5G", "step": "waiting", "running": true, "started_at": now - 48, "remaining": 252]
        } else if state == "wrong" {
            operation = ["kind": "wifi", "target": "Cafe Guest", "from": "Studio-5G", "step": "wrong_password", "running": false, "ok": false,
                         "message": "密码不对，已连回原来的网络", "started_at": now - 120, "waited": 12]
        }
        return [
            "available": true, "current": ["kind": "wifi", "name": "Studio-5G"], "safe": state == "safe" || state == "wrong", "watching": true, "monitor": "mihomo",
            "sources": [["name": "Claude Code · notes", "state": live["notes"]!, "gated": true], ["name": "Claude Code · rho", "state": live["rho"]!, "gated": true],
                        ["name": "Codex 桌面", "state": live["codex"]!, "gated": false], ["name": "Claude 桌面", "state": "idle", "gated": false]],
            "phones": [["name": "我的 iPhone", "battery": 62, "network": "5G", "signal": 3]],
            "networks": [["ssid": "Studio-5G", "remembered": true, "current": true], ["ssid": "Home-2.4G", "remembered": true, "current": false],
                         ["ssid": "Cafe Guest", "remembered": false, "current": false], ["ssid": "Office-Lab", "remembered": true, "current": false]],
            "operation": operation,
            "history": [["at": now - 3 * 3600, "from": "Home-2.4G", "to": "我的 iPhone", "result": "done", "waited": 55],
                        ["at": now - 4 * 3600, "from": "Studio-5G", "to": "Cafe Guest", "result": "wrong_password", "waited": 12, "message": "密码不对，已连回原来的网络"],
                        ["at": now - 28 * 3600, "from": "Studio-5G", "to": "我的 iPhone", "result": "timeout", "message": "5 分钟没等到空闲，网络没动"]],
        ]
    }
    static func directory(_ scenario: String) -> [String: Any] {
        var accounts: [[String: Any]] = [
            codex("synthetic-nova", "Codex · nova", "o***@example.invalid", plan: "pro", native: false, state: "healthy", used: 0, resetDays: 7,
                  credits: [credit("credit-l1", 5), credit("credit-l2", 6), credit("credit-l3", 24)]),
            codex("synthetic-p", "Codex · 12345678（个人）", "1***@example.invalid", plan: "pro", native: false, state: "healthy", used: 0, resetDays: 5, credits: [], extra: true),
            codex("synthetic-orbit", "Codex · orbit", "m***@example.invalid", plan: "pro", native: true, state: "delegated", used: 1, resetDays: 4.6,
                  credits: [credit("credit-r1", 6), credit("credit-r2", 24)]),
            codex("synthetic-team", "lab team", "a***@example.invalid", plan: "team", native: false, state: "healthy", used: 0, resetDays: 4.2, credits: [credit("credit-t1", 24)]),
            ["id": "synthetic-claude", "provider": "claude", "label": "Claude 当前账号", "identity": "a***@example.invalid", "plan": "team", "native": true,
             "automatic": true, "state": "unsaved", "message": "此 Claude CLI 版本尚未验证，请使用已核验的 2.1.278",
             "last_local": now - 6.4 * 86400, "last_online": now - 6.4 * 86400, "last_refresh": now - 6.6 * 86400,
             "expires_at": now - 6.3 * 86400, "refresh_expires_at": now + 6.7 * 86400, "next_check": now - 6.4 * 86400, "quota": [],
             "renewal_mode": "claude_coordinated",
             "warmup": ["enabled": true, "hour": 6, "running": false, "last": ["at": now - 3.2 * 3600, "ok": true, "message": "已发送，5 小时窗口开始计时"], "next": now + 1.8 * 3600],
             "login_notice": ["state": "normal", "expires_at": now + 6.7 * 86400, "message": "当前不在重新登录提醒时段内", "source": "本机登录记录"],
             "usage": ["state": "ready", "message": "额度已检查", "checked_at": now - 200, "attempted_at": now - 201, "next_update": now + 100, "refreshing": false,
                       "windows": [["label": "5 小时窗口", "used_percent": 14, "resets_at_iso": iso(3.7 * 3600)],
                                   ["label": "独立窗口", "used_percent": 0, "scope": "feature_x", "kind": "feature"],
                                   ["label": "7 天窗口", "used_percent": 70, "resets_at_iso": iso(26.4 * 3600)],
                                   ["label": "7 天窗口", "used_percent": 0, "scope": "ModelY", "kind": "model", "resets_at_iso": iso(26.4 * 3600)]],
                       "claude_resets": ["grants": ["eligible": true, "message": "官方提供的重置卡", "remaining": 2, "weekly_resets_at": iso(26.4 * 3600),
                                                    "items": [["label": "合成赠送：一次额度重置", "kind": "full", "remaining": 1, "total": 1, "starts_at": iso(-7 * 86400), "ends_at": iso(24 * 86400),
                                                               "limits": ["5 小时额度", "每周额度"], "paused": false, "usable_now": true, "requires_limit": false],
                                                              ["label": "合成赠送：一次 Fable 重置", "kind": "fable", "remaining": 1, "total": 1, "starts_at": iso(-3 * 86400), "ends_at": iso(117 * 86400),
                                                               "limits": ["本周 Fable 用量", "5 小时额度"], "paused": false, "usable_now": true, "requires_limit": false]]],
                                         "next": ["kind": "full", "limits": ["5 小时额度", "每周额度"], "remaining": 1, "ends_at": iso(24 * 86400)],
                                         "offers": [["kind": "fable", "limits": ["本周 Fable 用量", "5 小时额度"], "remaining": 1, "ends_at": iso(117 * 86400)],
                                                    ["kind": "full", "limits": ["5 小时额度", "每周额度"], "remaining": 1, "ends_at": iso(24 * 86400)]]]]],
        ]
        accounts[1]["reset"] = ["request_id": "synthetic-req", "credit_id": "synthetic-used", "expires_at": iso(20 * 86400), "outcome": "reset",
                                "message": "已用 1 张重置卡，将重新查询用量和剩余张数", "unresolved": false, "started_at": now - 5 * 86400]
        var data: [String: Any] = [
            "accounts": accounts, "busy": false, "checks_blocked": false, "recovery_needed": false, "maintenance_running": true, "notice": NSNull(),
            "desktop": ["enabled": true, "port": 9522, "connected": false, "managed": false, "busy": false, "message": "调试端口未监听；请使用调试模式启动 Codex 桌面"],
            "cloud": ["mode": "local", "endpoint": "", "label": "我的 Mac", "paired": false, "accounts": [], "devices": [], "selected": [:], "busy": false, "transfers": []],
            "strategy": ["config": ["enabled": true, "accounts": [
                ["id": "synthetic-orbit", "order": "switch_first", "participate": true, "below": 3],
                ["id": "synthetic-p", "order": "reset_first", "participate": true, "below": 2],
                ["id": "synthetic-team", "order": "switch_only", "participate": true, "below": 3],
                ["id": "synthetic-nova", "order": "switch_only", "participate": true, "below": 3]]],
                         "busy": false, "message": "", "phase": NSNull(), "run": NSNull(), "last_at": NSNull(),
                         "account_states": [["id": "synthetic-orbit", "message": "额度充足，高于见底线 3%"],
                                            ["id": "synthetic-p", "message": "额度充足，高于见底线 2%"]]],
            "claude_desktop": claudeDesktop(now),
            "network": network(now, "busy"),
        ]
        switch scenario {
        case "netsafe": data["network"] = network(now, "safe")
        case "netwaiting": data["network"] = network(now, "waiting")
        case "netwrong": data["network"] = network(now, "wrong")
        case "desktopbehind":
            var desktop = claudeDesktop(now)
            var list = desktop["accounts"] as! [[String: Any]]
            list[2]["behind"] = 4
            desktop["accounts"] = list
            desktop["pending"] = 4
            desktop["restart_needed"] = true
            desktop["attention"] = "要重启一次 Claude"
            desktop["headline"] = "#3 lab 差 4 个会话；重启一次 Claude 就能看到"
            desktop["retention"] = ["at_risk": true, "days": 12, "kept": false, "can_undo": false, "can_keep": true]
            desktop["symlinks"] = 1
            data["claude_desktop"] = desktop
        case "desktopswitching":
            var desktop = claudeDesktop(now)
            desktop["operation"] = ["kind": "switch", "target": "#2 work", "step": "asking", "running": true]
            data["claude_desktop"] = desktop
        case "desktopmissing":
            data["claude_desktop"] = ["installed": false, "running": false, "switch_supported": false, "background": true, "syncing": false,
                                      "headline": "这台 Mac 上没找到 Claude 桌面", "sessions": 0, "pending": 0, "restart_needed": false,
                                      "accounts": [], "snapshots": [], "symlinks": 0]
        case "connected":
            data["desktop"] = ["enabled": true, "port": 9522, "connected": true, "managed": true, "busy": false, "message": "已连接主窗口", "selected": "synthetic-orbit"]
        case "connectedbusy":
            // A scheduled check is running; the service holds a desktop switch until it finishes.
            data["desktop"] = ["enabled": true, "port": 9522, "connected": true, "managed": true, "busy": true, "message": "已连接主窗口", "selected": "synthetic-orbit"]
            data["busy"] = true
        case "plainbusy":
            data["desktop"] = ["enabled": false, "port": 9522, "connected": false, "managed": false, "busy": false, "message": ""]
            data["strategy"] = NSNull()
            data["busy"] = true
        case "unmanaged":
            data["desktop"] = ["enabled": true, "port": 9522, "connected": true, "managed": false, "busy": false, "message": "桌面连接就绪；选择账号后将更新全局默认"]
        case "plain":
            data["desktop"] = ["enabled": false, "port": 9522, "connected": false, "managed": false, "busy": false, "message": ""]
            data["strategy"] = NSNull()
        case "hotoff":
            // Hot switch turned off by the user; the strategy stays on and can only reset Codex.
            data["desktop"] = ["enabled": false, "port": 9522, "connected": false, "managed": false, "busy": false, "message": ""]
        case "off":
            // Rules saved, strategy switched off.
            var strategy = data["strategy"] as! [String: Any]
            var config = strategy["config"] as! [String: Any]
            config["enabled"] = false
            strategy["config"] = config
            strategy["message"] = ""
            data["strategy"] = strategy
        case "switchonly":
            // Strategy off, and every rule only switches: nothing here may promise a reset.
            var strategy = data["strategy"] as! [String: Any]
            var config = strategy["config"] as! [String: Any]
            config["enabled"] = false
            config["accounts"] = (config["accounts"] as! [[String: Any]]).map { rule -> [String: Any] in
                var rule = rule
                rule["order"] = "switch_only"
                return rule
            }
            strategy["config"] = config
            strategy["message"] = ""
            data["strategy"] = strategy
        case "empty":
            data["accounts"] = []
        case "fresh":
            // A first run: no AuthBaton account, and Claude 桌面 installed but not yet seen signed in.
            data["accounts"] = []
            var desktop = data["claude_desktop"] as! [String: Any]
            desktop["accounts"] = []
            desktop["snapshots"] = []
            desktop["sessions"] = 0
            data["claude_desktop"] = desktop
        case "codexone":
            // Just after the first Codex account, the desktop already connected (a lost one's band would take the strip's place).
            let list = data["accounts"] as! [[String: Any]]
            data["accounts"] = list.filter { $0["provider"] as? String != "codex" || $0["id"] as? String == "synthetic-orbit" }
            data["desktop"] = ["enabled": true, "port": 9522, "connected": true, "managed": true, "busy": false, "message": "已连接主窗口", "selected": "synthetic-orbit"]
        case "offempty":
            // A new install: no accounts yet and the strategy never turned on.
            data["accounts"] = []
            data["strategy"] = ["config": ["enabled": false, "accounts": []], "busy": false, "message": "", "phase": NSNull(), "run": NSNull(),
                                "last_at": NSNull(), "account_states": []]
        case "attention":
            var list = data["accounts"] as! [[String: Any]]
            list[0]["state"] = "reauth"; list[0]["message"] = "合成：刷新令牌已失效，需要重新登录"
            list[3]["check_status"] = "running"; list[3]["check_online"] = true
            data["accounts"] = list
            data["recovery_needed"] = true
            data["notice"] = "合成：调试端口未监听；切换未开始"
        case "running":
            var strategy = data["strategy"] as! [String: Any]
            strategy["phase"] = "attention"
            strategy["message"] = "接续结果尚未确认；不会重复发送继续指令，请先核对桌面会话"
            strategy["run"] = ["id": "synthetic-batch", "source": "synthetic-orbit", "target": "synthetic-team", "created_at": now - 60,
                               "tasks": [["thread_id": "synthetic-session-1", "state": "resumed"], ["thread_id": "synthetic-session-2", "state": "resume_sent"]]]
            data["strategy"] = strategy
            data["busy"] = true
        case "uncertain":
            // A renewal whose reply was lost; the service reconciles it on its own.
            var list = data["accounts"] as! [[String: Any]]
            let index = list.firstIndex { $0["id"] as? String == "synthetic-claude" }!
            list[index]["state"] = "uncertain"
            list[index]["message"] = "续期请求没有收到回应；稍后自动用同一凭据重新确认，无需操作"
            data["accounts"] = list
        case "claude2":
            // A second, non-current Claude account to switch to in place.
            var list = data["accounts"] as! [[String: Any]]
            var second = list.first { $0["provider"] as? String == "claude" }!
            second["id"] = "synthetic-claude-b"; second["label"] = "Claude 备用"; second["identity"] = "b***@example.invalid"
            second["native"] = false; second["state"] = "healthy"; second["message"] = ""; second["expires_at"] = now + 3600
            // Switched off earlier at 9:00; switching on again keeps that hour.
            second["warmup"] = ["enabled": false, "hour": 9, "running": false]
            list.append(second)
            data["accounts"] = list
        case "deskquota":
            // Quota through Claude 桌面's own sign-in is on: work reads its quota that way, lab has no usable token.
            var desktop = claudeDesktop(now)
            desktop["token_quota"] = ["enabled": true, "state": "ready", "message": "已读到 Claude 桌面的登录"]
            var rows = desktop["accounts"] as! [[String: Any]]
            rows[1]["token_quota"] = ["state": "ready", "message": "额度已更新", "checked_at": now - 90,
                                      "windows": [["label": "5 小时窗口", "used_percent": 41, "resets_at_iso": iso(2.2 * 3600)],
                                                  ["label": "7 天窗口", "used_percent": 63, "resets_at_iso": iso(3.4 * 86400)]]]
            rows[2]["token_quota"] = ["state": "no_token", "message": "这个账号还没在 Claude 桌面用过 Code 模式", "checked_at": NSNull(), "windows": []]
            desktop["accounts"] = rows
            data["claude_desktop"] = desktop
        case "coderelayon":
            // Code 无感换号 installed and active on a same-Team account; 2 accounts keep the footer visible.
            var desktop = claudeDesktop(now)
            let rows = (desktop["accounts"] as! [[String: Any]]).prefix(2)
            desktop["accounts"] = Array(rows)
            desktop["code_relay"] = [
                "enabled": true, "install_state": "installed", "state": "active",
                "message": "已开启 · 当前账号 Claude 当前账号",
                "active_label": "Claude 当前账号", "same_team": true,
                "busy": false, "relayed_count": 3,
            ]
            data["claude_desktop"] = desktop
        case "coderelaydesk":
            // Pure-Desktop mode with Code 无感换号 on: 1 desktop shell login + 1 AuthBaton-only same-Team Code account.
            var list = data["accounts"] as! [[String: Any]]
            var second = list.first { $0["provider"] as? String == "claude" }!
            second["id"] = "synthetic-claude-b"; second["label"] = "Claude 备用"; second["identity"] = "b***@example.invalid"
            second["native"] = false; second["state"] = "healthy"; second["message"] = ""; second["expires_at"] = now + 3600
            second["warmup"] = ["enabled": false, "hour": 9, "running": false]
            list.append(second)
            data["accounts"] = list
            var desktop = claudeDesktop(now)
            var rows = Array((desktop["accounts"] as! [[String: Any]]).prefix(1))
            rows[0]["in_use"] = true
            desktop["accounts"] = rows
            desktop["code_relay"] = [
                "enabled": true, "install_state": "installed", "state": "active",
                "message": "已开启 · 当前账号 Claude 当前账号",
                "active_label": "Claude 当前账号", "same_team": true,
                "busy": false, "relayed_count": 1,
            ]
            data["claude_desktop"] = desktop
        case "coderelaydeskonly":
            // Code 无感换号 on, but every account is only signed in to Claude 桌面: no Code account in AuthBaton yet.
            data["accounts"] = (data["accounts"] as! [[String: Any]]).filter { $0["provider"] as? String != "claude" }
            var desktop = claudeDesktop(now)
            var rows = Array((desktop["accounts"] as! [[String: Any]]).prefix(2))
            rows[0].removeValue(forKey: "linked_account")
            desktop["accounts"] = rows
            desktop["code_relay"] = [
                "enabled": true, "install_state": "installed", "state": "waiting_account",
                "message": "请先登录至少一个同 Team 的 Code 账号，即可无感换号",
                "busy": false, "relayed_count": 0,
            ]
            data["claude_desktop"] = desktop
        case "claudeboth":
            // Claude 备用 also has nova's desktop login: its 换上 can move Claude Code, Claude 桌面 or both.
            var list = data["accounts"] as! [[String: Any]]
            var second = list.first { $0["provider"] as? String == "claude" }!
            second["id"] = "synthetic-claude-b"; second["label"] = "Claude 备用"; second["identity"] = "b***@example.invalid"
            second["native"] = false; second["state"] = "healthy"; second["message"] = ""; second["expires_at"] = now + 3600
            second["warmup"] = ["enabled": false, "hour": 9, "running": false]
            list.append(second)
            data["accounts"] = list
            var desktop = claudeDesktop(now)
            var rows = desktop["accounts"] as! [[String: Any]]
            rows[0]["linked_account"] = "synthetic-claude-b"
            desktop["accounts"] = rows
            data["claude_desktop"] = desktop
        case "full":
            // Every window at 100% and a second Claude account: the widest an account card gets.
            var list = data["accounts"] as! [[String: Any]]
            var second = list.first { $0["provider"] as? String == "claude" }!
            second["id"] = "synthetic-claude-b"; second["label"] = "Claude 备用"; second["identity"] = "b***@example.invalid"
            second["native"] = false; second["state"] = "healthy"; second["message"] = ""; second["expires_at"] = now + 3600
            second["warmup"] = ["enabled": false, "hour": 9, "running": false]
            list.append(second)
            data["accounts"] = list.map { account -> [String: Any] in
                var account = account
                if var usage = account["usage"] as? [String: Any], let windows = usage["windows"] as? [[String: Any]] {
                    usage["windows"] = windows.map { window -> [String: Any] in var window = window; window["used_percent"] = 0; return window }
                    account["usage"] = usage
                }
                return account
            }
        case "claudelow", "claudelow5", "claudelowboth":
            // Two Claude accounts under rules with a 4% line, each with a Fable week. Claude 备用 is at the line
            // on its 7-day window (claudelow), its 5-hour window (claudelow5) or both (claudelowboth);
            // in claudelow the current account's Fable week is at the line too.
            func windows(five: Double, fiveIn: Double, week: Double, weekIn: Double, fable: Double) -> [[String: Any]] {
                [["label": "5 小时窗口", "used_percent": five, "resets_at_iso": iso(fiveIn)],
                 ["label": "7 天窗口", "used_percent": week, "resets_at_iso": iso(weekIn)],
                 ["label": "7 天窗口", "used_percent": fable, "scope": "Fable", "kind": "model", "resets_at_iso": iso(weekIn)]]
            }
            var list = data["accounts"] as! [[String: Any]]
            let index = list.firstIndex { $0["id"] as? String == "synthetic-claude" }!
            var first = list[index]
            var usage = first["usage"] as! [String: Any]
            usage["windows"] = windows(five: 38, fiveIn: 2.1 * 3600, week: 60, weekIn: 3.2 * 86400, fable: scenario == "claudelow" ? 97 : 41)
            first["usage"] = usage
            list[index] = first
            var second = first
            second["id"] = "synthetic-claude-b"; second["label"] = "Claude 备用"; second["identity"] = "b***@example.invalid"
            second["native"] = false; second["state"] = "healthy"; second["message"] = ""; second["expires_at"] = now + 3600
            second["warmup"] = ["enabled": false, "hour": 9, "running": false]
            usage["windows"] = scenario == "claudelow" ? windows(five: 10, fiveIn: 4.4 * 3600, week: 98, weekIn: 2.2 * 86400, fable: 12)
                : scenario == "claudelow5" ? windows(five: 100, fiveIn: 2.3 * 3600, week: 39, weekIn: 4.1 * 86400, fable: 12)
                : windows(five: 100, fiveIn: 2.3 * 3600, week: 99, weekIn: 2.2 * 86400, fable: 12)
            second["usage"] = usage
            list.append(second)
            data["accounts"] = list
            var strategy = data["strategy"] as! [String: Any]
            var config = strategy["config"] as! [String: Any]
            config["accounts"] = (config["accounts"] as! [[String: Any]])
                + [["id": "synthetic-claude", "order": "switch_first", "participate": true, "below": 4],
                   ["id": "synthetic-claude-b", "order": "switch_first", "participate": true, "below": 4]]
            strategy["config"] = config
            data["strategy"] = strategy
        case "switchblock":
            // Saved logins a switch must not use yet: one needs a new login, one an unsaved renewal,
            // one a renewal whose reply was lost.
            var list = data["accounts"] as! [[String: Any]]
            list[0]["state"] = "reauth"; list[0]["message"] = "合成：刷新令牌已失效，需要重新登录"
            list[3]["state"] = "unsaved"; list[3]["message"] = "合成：续期结果尚未写入"
            var second = list.first { $0["provider"] as? String == "claude" }!
            second["id"] = "synthetic-claude-b"; second["label"] = "Claude 备用"; second["identity"] = "b***@example.invalid"
            second["native"] = false; second["state"] = "uncertain"; second["message"] = "续期请求没有收到回应；稍后自动用同一凭据重新确认，无需操作"
            list.append(second)
            data["accounts"] = list
        case "cloud":
            // Back in local mode after handing two accounts to the cloud; one state is newer than this build.
            var list = data["accounts"] as! [[String: Any]]
            list[0]["state"] = "cloud_transfer"; list[0]["message"] = "云端交接尚未完成，请在云端模式恢复"; list[0]["automatic"] = false
            list[3]["state"] = "cloud_managed"; list[3]["message"] = "凭据由个人云端维护，请在云端模式查看"; list[3]["automatic"] = false
            list[1]["state"] = "synthetic_future"; list[1]["message"] = "合成：这个版本不认识的状态"
            data["accounts"] = list
            var cloud = data["cloud"] as! [String: Any]
            cloud["transfers"] = [["local_id": "synthetic-nova", "cloud_id": NSNull(), "phase": "prepared"],
                                  ["local_id": "synthetic-team", "cloud_id": "synthetic-cloud-team", "phase": "cloud_managed"]]
            data["cloud"] = cloud
        case "many":
            // More accounts than the strategy takes rules for (32).
            var list = data["accounts"] as! [[String: Any]]
            for n in 1...31 {
                list.append(codex(String(format: "synthetic-extra-%02d", n), String(format: "Codex · extra %02d", n), "x\(n)***@example.invalid",
                                  plan: "plus", native: false, state: "healthy", used: 10, resetDays: 3, credits: []))
            }
            data["accounts"] = list
        default: break
        }
        return data
    }
    static let snapshot = #"{"checked_at":1790649075,"service_version":"0.15.0","verification":"仅检查本地凭据和 Claude CLI 登录状态，未请求模型，也未验证刷新能力。","providers":[{"id":"codex","name":"Codex","state":"configured","auth_kind":"ChatGPT OAuth","summary":"本地 access token 尚未到期","source":"~/.codex/auth.json","expires_at":1790778068,"details":[]},{"id":"claude","name":"Claude Code","state":"configured","auth_kind":"Claude 订阅","summary":"Claude CLI 显示已登录","source":"/opt/homebrew/bin/claude","expires_at":null,"details":[]},{"id":"pi","name":"pi · OpenAI Codex","state":"configured","auth_kind":"AuthBaton 本机服务","summary":"已安装统一账号插件","source":"~/.pi/agent/auth.json","expires_at":null,"details":[]}]}"#

    // `action` is the desktop's report on the latest switch attempt, as the service publishes it.
    @MainActor static func load(_ scenario: String, into state: ServiceController, action: [String: Any]? = nil) {
        var data = directory(scenario)
        if let action, var desktop = data["desktop"] as? [String: Any] {
            desktop["action"] = action
            data["desktop"] = desktop
        }
        state.directory = try! decoder.decode(AccountDirectory.self, from: JSONSerialization.data(withJSONObject: data))
        state.snapshot = try! decoder.decode(Snapshot.self, from: Data(snapshot.utf8))
    }
}

// MARK: Rendering

@MainActor func makeWindow<V: View>(_ view: V, size: NSSize, appearance: String = "light", chrome: Bool = true) -> (NSWindow, NSView) {
    let root = view.environmentObject(ServiceController.shared).tint(Palette.solid)
        .preferredColorScheme(appearance == "dark" ? .dark : .light)
    let host = NSHostingView(rootView: root)
    // Bridge .toolbar/.navigationTitle into the window like the real Window scene does.
    host.sceneBridgingOptions = .all
    host.appearance = NSAppearance(named: appearance == "dark" ? .darkAqua : .aqua)
    let window = KeyWindow(contentRect: NSRect(origin: NSPoint(x: 40, y: 40), size: size),
                          styleMask: chrome ? [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView] : [.borderless],
                          backing: .buffered, defer: false)
    window.appearance = host.appearance
    // The app's window hides its title bar and lets the sidebar run up under the traffic lights.
    if chrome {
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
    }
    window.isReleasedWhenClosed = false
    window.contentView = host
    window.setContentSize(size)
    return (window, host)
}

extension NSView {
    var subviewsDeep: [NSView] { subviews + subviews.flatMap(\.subviewsDeep) }
}

@MainActor func capture(_ window: NSWindow) -> NSBitmapImageRep {
    // The frame view includes the toolbar and title; the hosting view alone misses them.
    let frame = window.contentView!.superview ?? window.contentView!
    // macOS 26 glass (the sidebar) caches as solid black: capture the page without it,
    // then paint each glass view's content back on top over the window colour.
    func glass(_ view: NSView) -> [NSGlassEffectView] {
        ((view as? NSGlassEffectView).map { [$0] } ?? []) + view.subviews.flatMap(glass)
    }
    let panes = glass(frame).filter { !$0.isHidden }
    panes.forEach { $0.isHidden = true }
    let base = frame.bitmapImageRepForCachingDisplay(in: frame.bounds)!
    frame.cacheDisplay(in: frame.bounds, to: base)
    panes.forEach { $0.isHidden = false }
    guard !panes.isEmpty else { return base }
    let image = NSImage(size: frame.bounds.size)
    image.addRepresentation(base)
    let output = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: base.pixelsWide, pixelsHigh: base.pixelsHigh, bitsPerSample: 8,
                                  samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    output.size = frame.bounds.size
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: output)
    window.effectiveAppearance.performAsCurrentDrawingAppearance {
        NSColor.windowBackgroundColor.setFill()
        for pane in panes { pane.convert(pane.bounds, to: frame).fill() }
    }
    base.draw(in: frame.bounds, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    // Table rows inside the glass do not cache through their scroll view; draw each row cell directly.
    func rows(_ view: NSView) -> [NSView] {
        let name = String(describing: Swift.type(of: view))
        if name.hasPrefix("CellHostingView") || view is NSHostingView<AnyView> || view.superview.map({ String(describing: Swift.type(of: $0)).hasPrefix("ListTableHeaderView") }) == true {
            return [view]
        }
        return view.subviews.flatMap(rows)
    }
    for pane in panes {
        for case let row as NSTableRowView in pane.subviewsDeep where row.isSelected {
            NSColor(Palette.solid).withAlphaComponent(0.14).setFill()
            NSBezierPath(roundedRect: row.convert(row.bounds, to: frame).insetBy(dx: 10, dy: 0), xRadius: 8, yRadius: 8).fill()
        }
        for cell in rows(pane) where !cell.isHidden && cell.bounds.height > 0 {
            let bitmap = cell.bitmapImageRepForCachingDisplay(in: cell.bounds)!
            cell.cacheDisplay(in: cell.bounds, to: bitmap)
            bitmap.draw(in: cell.convert(cell.bounds, to: frame), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        }
    }
    NSGraphicsContext.restoreGraphicsState()
    return output
}

// A background process cannot become frontmost, so emulate it: inactive windows draw grey accents,
// and destructive buttons ignore clicks in them. Our own windows need the subclass to draw as key;
// the swizzle covers the sheets SwiftUI creates.
final class ActiveApp: NSApplication {
    override var isActive: Bool { true }
}

final class KeyWindow: NSWindow {
    override var isKeyWindow: Bool { true }
    override var canBecomeKey: Bool { true }
    // Wide renders (1600) are larger than a laptop screen; keep the size asked for.
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect { frameRect }
}

extension NSWindow {
    @objc var previewIsKeyWindow: Bool { true }
    static func pretendKey() {
        method_exchangeImplementations(class_getInstanceMethod(NSWindow.self, #selector(getter: NSWindow.isKeyWindow))!,
                                       class_getInstanceMethod(NSWindow.self, #selector(getter: NSWindow.previewIsKeyWindow))!)
    }
}

@MainActor func render<V: View>(_ view: V, size: NSSize?, appearance: String, to url: URL) -> NSSize {
    let fitting = size ?? NSHostingView(rootView: view.environmentObject(ServiceController.shared)).fittingSize
    let (window, host) = makeWindow(view, size: fitting, appearance: appearance, chrome: size != nil)
    host.layoutSubtreeIfNeeded()
    // Off-screen windows skip list rows and keep the inactive look; show it briefly instead.
    window.makeKeyAndOrderFront(nil)
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.8))
    try! capture(window).representation(using: .png, properties: [:])!.write(to: url)
    window.orderOut(nil)
    return fitting
}

// MARK: In-process interaction
// SwiftUI elements are not in the accessibility tree without TCC trust, so the driver reads the
// rendered windows with Vision OCR and clicks with synthetic mouse events sent to our own windows.

@MainActor enum Driver {
    struct Hit {
        let window: NSWindow
        let text: String
        let rect: CGRect   // window coordinates
        var center: NSPoint { NSPoint(x: rect.midX, y: rect.midY) }
    }

    static var menuChoice: String?
    static var lastMenu: [(title: String, enabled: Bool)] = []

    static func settle(_ seconds: Double = 0.5) { RunLoop.current.run(until: Date(timeIntervalSinceNow: seconds)) }

    // Front-most first: sheets and popovers before the window they belong to.
    static func windows() -> [NSWindow] {
        func rank(_ window: NSWindow) -> Int {
            if window.sheetParent != nil { return 2 }
            return String(describing: Swift.type(of: window)).contains("Popover") ? 1 : 0
        }
        return NSApp.windows.filter { $0.isVisible && $0.frame.width > 40 && $0.contentView != nil }.sorted { rank($0) > rank($1) }
    }

    static func normalize(_ text: String) -> String {
        // SF draws a capital I and a small l alike, and OCR mixes them up ("Iab team"); fold both sides.
        var result = text.replacingOccurrences(of: "I", with: "l").lowercased().replacingOccurrences(of: " ", with: "")
        for (from, to) in [("～", "~"), ("：", ":"), ("，", ","), ("（", "("), ("）", ")"), ("…", "..."), ("⋯", "..."), ("「", ""), ("」", ""),
                           ("·", "."), ("・", "."), ("•", "."), ("；", ";"), ("？", "?"), ("！", "!"),
                           // OCR sometimes reads 换 as its traditional form, and in small text 底 as 厎, 未 as 末, 撤 as 撒,
                           // … as ⋯ (the app never writes 厎, 末, 撒 or ⋯).
                           ("換", "换"), ("厎", "底"), ("末", "未"), ("撒", "撤")] {
            result = result.replacingOccurrences(of: from, with: to)
        }
        return result
    }

    static func read(_ window: NSWindow) -> [Hit] {
        guard let image = capture(window).cgImage else { return [] }
        let bounds = CGRect(x: 0, y: 0, width: image.width, height: image.height)
        // Vision finds nothing on transparent pixels; flatten onto an opaque page first.
        let context = CGContext(data: nil, width: image.width, height: image.height, bitsPerComponent: 8, bytesPerRow: 0,
                                space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
        context.setFillColor(CGColor(gray: 1, alpha: 1))
        context.fill(bounds)
        context.draw(image, in: bounds)
        let request = VNRecognizeTextRequest()
        request.recognitionLanguages = ["zh-Hans", "en-US"]
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = false
        try? VNImageRequestHandler(cgImage: context.makeImage()!).perform([request])
        let size = (window.contentView!.superview ?? window.contentView!).bounds.size
        func place(_ box: CGRect) -> CGRect {
            CGRect(x: box.minX * size.width, y: box.minY * size.height, width: box.width * size.width, height: box.height * size.height)
        }
        return (request.results ?? []).flatMap { observation -> [Hit] in
            guard let candidate = observation.topCandidates(1).first else { return [] }
            return [Hit(window: window, text: candidate.string, rect: place(observation.boundingBox))]
        }
    }

    // `suffix` picks a button whose OCR line carries a leading glyph over prose that quotes its label.
    enum Match { case exact, prefix, suffix, contains, within(String) }

    // Exact lines first, then lines containing the text; top-to-bottom, left-to-right.
    static func hits(_ wanted: String, _ match: Match = .contains) -> [Hit] {
        let key = normalize(wanted)
        for window in windows() {
            let lines = read(window)
            var found: [Hit]
            switch match {
            case .exact: found = lines.filter { normalize($0.text) == key }
            case .prefix: found = lines.filter { normalize($0.text).hasPrefix(key) }.compactMap { narrow($0, to: key) }
            case .suffix: found = lines.filter { normalize($0.text).hasSuffix(key) }.compactMap { narrow($0, to: key) }
            case .contains:
                found = lines.filter { normalize($0.text) == key }
                if found.isEmpty { found = lines.compactMap { narrow($0, to: key) } }
            case .within(let line): found = lines.filter { normalize($0.text).contains(normalize(line)) }.compactMap { narrow($0, to: key) }
            }
            if !found.isEmpty { return found.sorted { ($0.rect.maxY, -$0.rect.minX) > ($1.rect.maxY, -$1.rect.minX) } }
        }
        return []
    }

    // OCR often merges neighbouring labels into one line; aim at the wanted part by character position.
    static func narrow(_ hit: Hit, to key: String) -> Hit? {
        let line = normalize(hit.text)
        guard let range = line.range(of: key), !line.isEmpty else { return nil }
        let count = CGFloat(line.count)
        let start = CGFloat(line.distance(from: line.startIndex, to: range.lowerBound)) / count
        let width = CGFloat(key.count) / count
        return Hit(window: hit.window, text: hit.text,
                   rect: CGRect(x: hit.rect.minX + hit.rect.width * start, y: hit.rect.minY, width: hit.rect.width * width, height: hit.rect.height))
    }

    // Polls briefly so transitions can finish; a negative check simply waits the full second.
    static func visible(_ wanted: String, wait: Double = 1) -> Bool {
        let key = normalize(wanted)
        let deadline = Date(timeIntervalSinceNow: wait)
        repeat {
            if windows().contains(where: { window in read(window).contains { normalize($0.text).contains(key) } }) { return true }
            settle(0.25)
        } while Date() < deadline
        return false
    }

    static func text() -> [String] { windows().flatMap { read($0).map(\.text) } }

    static func click(_ window: NSWindow, _ point: NSPoint) {
        func event(_ type: NSEvent.EventType) -> NSEvent {
            NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                               windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: 1)!
        }
        if ProcessInfo.processInfo.environment["DRIVER_DEBUG"] != nil, let frame = window.contentView?.superview {
            print("    click \(Int(point.x)),\(Int(point.y)) in \(Swift.type(of: window)) key=\(window.isKeyWindow) → \(frame.hitTest(point).map { String(describing: Swift.type(of: $0)).prefix(60) } ?? "nil")")
        }
        // AppKit controls track the mouse with nextEvent, so the release must already be queued.
        NSApp.postEvent(event(.leftMouseUp), atStart: false)
        window.sendEvent(event(.leftMouseDown))
        if let up = NSApp.nextEvent(matching: .leftMouseUp, until: Date(), inMode: .default, dequeue: true) { window.sendEvent(up) }
        settle()
    }

    @discardableResult static func press(_ wanted: String, _ match: Match = .contains, nth: Int = 0) -> Bool {
        // Wait until the target exists and has stopped moving: sheets, alerts and save bars animate in.
        var found = hits(wanted, match)
        let deadline = Date(timeIntervalSinceNow: 2)
        while Date() < deadline {
            settle(0.2)
            let again = hits(wanted, match)
            if found.count > nth, again.count > nth, found[nth].window == again[nth].window,
               abs(found[nth].rect.midX - again[nth].rect.midX) < 2, abs(found[nth].rect.midY - again[nth].rect.midY) < 2 { break }
            found = again
        }
        guard found.count > nth else {
            print("  ✗ 找不到「\(wanted)」；最前窗口可见：\(windows().first.map { read($0).map(\.text).joined(separator: " | ") } ?? "")")
            return false
        }
        click(found[nth].window, found[nth].center)
        return true
    }

    // SwiftUI menus run AppKit's modal tracking loop; pick the item from inside that loop.
    static func installMenuHook() {
        NotificationCenter.default.addObserver(forName: NSMenu.didBeginTrackingNotification, object: nil, queue: nil) { note in
            guard let menu = note.object as? NSMenu else { return }
            RunLoop.main.perform(inModes: [.eventTracking, .default, .modalPanel]) {
                MainActor.assumeIsolated {
                    lastMenu = menu.items.filter { !$0.isSeparatorItem }.map { ($0.title, $0.isEnabled) }
                    let index = menuChoice.flatMap { choice in menu.items.firstIndex { $0.title == choice && $0.isEnabled } }
                    menuChoice = nil
                    menu.cancelTracking()
                    if let index { menu.performActionForItem(at: index) }
                }
            }
        }
    }

    // Focusable SwiftUI controls get an invisible AppKit stand-in with the control's frame. Icon-only
    // controls (the "…" menu, the move arrows) have no text for OCR, so they are found by size, right of a
    // label on its line; the nearest comes first.
    static func controls(beside anchor: String, size: CGFloat) -> [(window: NSWindow, point: NSPoint)] {
        // The stand-ins are made when AppKit builds the key view loop, which a page switch does not redo.
        windows().forEach { $0.recalculateKeyViewLoop() }
        settle(0.2)
        let anchors = hits(anchor)
        var found: [(window: NSWindow, point: NSPoint, distance: CGFloat)] = []
        for window in windows() {
            for view in (window.contentView!.superview ?? window.contentView!).subviewsDeep
            where String(describing: Swift.type(of: view)) == "KeyViewProxy" && abs(view.bounds.width - size) < 1.5 && abs(view.bounds.height - size) < 1.5 {
                let rect = view.convert(view.bounds, to: nil)
                for hit in anchors where hit.window == window && rect.midX > hit.rect.maxX && abs(rect.midY - hit.rect.midY) < 34 {
                    found.append((window, NSPoint(x: rect.midX, y: rect.midY), rect.midX - hit.rect.maxX))
                }
            }
        }
        if found.isEmpty, ProcessInfo.processInfo.environment["DRIVER_DEBUG"] != nil {
            for window in windows() {
                let views = (window.contentView!.superview ?? window.contentView!).subviewsDeep
                let names = Dictionary(grouping: views, by: { String(describing: Swift.type(of: $0)).prefix(40) }).mapValues(\.count)
                print("    \(Swift.type(of: window)) anchors=\(anchors.filter { $0.window == window }.map { "\($0.text)@\(Int($0.rect.midX)),\(Int($0.rect.midY))" })")
                print("    views: \(names.sorted { $0.value > $1.value }.prefix(12).map { "\($0.key)×\($0.value)" })")
                for view in views where String(describing: Swift.type(of: view)).contains("Proxy") {
                    print("      \(Swift.type(of: view)) \(view.convert(view.bounds, to: nil).integral)")
                }
            }
        }
        return found.sorted { $0.distance < $1.distance }.map { ($0.window, $0.point) }
    }

    // The "…" menu next to an account's name.
    static func moreMenu(near name: String) -> (NSWindow, NSPoint)? { controls(beside: name, size: 26).first.map { ($0.window, $0.point) } }

    // A button on the same card as a label: below it, in its column, above the next card.
    static func near(_ anchor: String, _ wanted: String) -> Hit? {
        let targets = hits(wanted)
        for hit in hits(anchor) {
            let below = targets.filter { target in
                let drop = hit.rect.midY - target.rect.midY
                return target.window == hit.window && target.rect.midX > hit.rect.minX - 68 && target.rect.midX < hit.rect.minX + 340 && drop > 0 && drop < 187
            }
            if let nearest = below.max(by: { $0.rect.midY < $1.rect.midY }) { return nearest }
        }
        return nil
    }

    @discardableResult static func pressNear(_ anchor: String, _ wanted: String) -> Bool {
        guard let hit = near(anchor, wanted) else { return false }
        click(hit.window, hit.center)
        return true
    }

    // A label's control on its line, right of it: the 见底时 menu reads as its current choice.
    @discardableResult static func pressBeside(_ anchor: String, _ wanted: String) -> Bool {
        let targets = hits(wanted)
        for hit in hits(anchor) {
            let line = targets.filter { $0.window == hit.window && abs($0.rect.midY - hit.rect.midY) < 12 && $0.rect.minX >= hit.rect.maxX - 2 }
            if let target = line.min(by: { $0.rect.minX < $1.rect.minX }) {
                click(target.window, target.center)
                return true
            }
        }
        return false
    }

    static func views<T: NSView>(_ type: T.Type) -> [T] {
        func walk(_ view: NSView) -> [T] { ((view as? T).map { [$0] } ?? []) + view.subviews.flatMap(walk) }
        return windows().flatMap { window in walk(window.contentView!.superview ?? window.contentView!) }
    }

    static func click(_ view: NSView) {
        click(view.window!, view.convert(NSPoint(x: view.bounds.midX, y: view.bounds.midY), to: nil))
    }

    static func escape(_ window: NSWindow) {
        let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
                                     windowNumber: window.windowNumber, context: nil, characters: "\u{1b}", charactersIgnoringModifiers: "\u{1b}",
                                     isARepeat: false, keyCode: 53)!
        window.sendEvent(event)
        settle(0.6)
    }

    static func enter(_ text: String) {
        guard let window = windows().first, let field = views(NSTextField.self).first(where: { $0.isEditable && $0.window == window }) else {
            print("  ✗ 找不到输入框")
            return
        }
        window.makeFirstResponder(field)
        guard let editor = field.currentEditor() as? NSTextView else { return }
        editor.selectAll(nil)
        editor.insertText(text, replacementRange: editor.selectedRange())
        settle()
    }
}

@MainActor final class Checks {
    var failures: [String] = []
    // Polls briefly: service calls run in a Task and SwiftUI updates land a frame later.
    func expect(_ condition: @autoclosure () -> Bool, _ what: String) {
        var ok = condition()
        let deadline = Date(timeIntervalSinceNow: 1.5)
        while !ok && Date() < deadline {
            Driver.settle(0.2)
            ok = condition()
        }
        print(ok ? "  ✓ \(what)" : "  ✗ \(what)")
        if !ok { failures.append(what) }
    }
    func visible(_ text: String, _ what: String) {
        let ok = Driver.visible(text)
        expect(ok, what)
        if !ok { print("    可见：\(Driver.text().joined(separator: " | ").suffix(900))") }
    }
    func lastCall(_ method: String) -> String? { ServiceController.shared.calls.last { $0.hasPrefix(method + " ") } }
}

@MainActor func interact() -> Int {
    let state = ServiceController.shared
    let check = Checks()
    NSApp.setActivationPolicy(.accessory)
    Driver.installMenuHook()
    UserDefaults.standard.removeObject(forKey: "dismissedExpiryReminders")
    func open<V: View>(_ view: V, size: NSSize) -> NSWindow {
        let (window, _) = makeWindow(view, size: size)
        window.makeKeyAndOrderFront(nil)
        Driver.settle(1)
        return window
    }
    func select(_ page: Page, _ id: String? = nil) {
        AppNavigation.shared.show(page, select: id)
        if id == nil { AppNavigation.shared.selection = nil }
        Driver.settle(0.8)
    }
    // A switch starts from the in-use account's menu on the accounts page: "换上 nova · 剩余 100%".
    func openSwitch(_ id: String) {
        let account = state.directory!.accounts.first { $0.id == id }!
        let current = state.directory!.current(account.provider)!
        select(.accounts)
        Driver.menuChoice = "换上 \(account.displayName)" + (account.lowestPrimary.map { " · 剩余 \($0)%" } ?? "")
        Driver.press(current.displayName)
        Driver.settle()
    }
    // The "…" menu next to an account's name, with `choice` picked from it (nil only reads the menu).
    func more(_ name: String, choose choice: String?) {
        Driver.menuChoice = choice
        Driver.lastMenu = []
        if let (window, point) = Driver.moreMenu(near: name) {
            Driver.click(window, point)
            Driver.settle()
        } else {
            check.expect(false, "找到「\(name)」的更多操作菜单")
        }
    }
    // Saving with the strategy on asks first; the confirmation names what it saves.
    func saveRules() {
        Driver.press("保存规则")
        Driver.press("确认保存", .exact)
    }
    // The picked station's order, changed through the 见底时 menu in its rule panel.
    func setOrder(from current: String, to order: String) {
        Driver.menuChoice = order
        if !Driver.pressBeside("见底时", current) { check.expect(false, "找到规则面板的「见底时」菜单") }
        Driver.settle()
    }

    let only = Set((ProcessInfo.processInfo.environment["PREVIEW_ONLY"] ?? "").split(separator: ",").compactMap { Int($0) })
    func section(_ number: Int, _ title: String, _ run: () -> Void) {
        guard only.isEmpty || only.contains(number) else { return }
        print("\(number) \(title)")
        // Every section starts from the default synthetic state so any subset can run alone.
        Fixture.load("default", into: state)
        state.calls.removeAll()
        state.failNext = nil
        state.clearAccountError()
        state.isConnected = true
        state.errorText = nil
        state.login = nil
        state.loginTarget = nil
        UserDefaults.standard.removeObject(forKey: "dismissedExpiryReminders")
        for key in [ClaudeUse.key, ClaudeUse.scopeKey, ClaudeUse.driftKey, CodexUse.key, CodexUse.hotKey, Onboarding.codexKey, Onboarding.claudeKey] {
            UserDefaults.standard.removeObject(forKey: key)
        }
        // Sections that are not about the first-time relay card start past it.
        UserDefaults.standard.set(true, forKey: AutomationPage.onboardedKey)
        UserDefaults.standard.removeObject(forKey: AutomationPage.hintKey)
        ReminderModel.shared.reload()
        AppNavigation.shared.selection = nil
        AppNavigation.shared.desktopSheet = false
        AppNavigation.shared.claudeDetail = nil
        Driver.settle(0.5)
        run()
        // Let a closing sheet finish; one still open is closed with Escape so SwiftUI clears its state.
        // Ending it from AppKit instead leaves SwiftUI thinking it is up, and the page opens no more sheets.
        Driver.settle(0.6)
        for _ in 0..<3 where main.attachedSheet != nil { Driver.escape(main.attachedSheet!) }
        main.attachedSheet.map { main.endSheet($0) }
        Driver.settle(0.3)
    }
    let main = open(DashboardView(), size: NSSize(width: 1020, height: 765))

    section(1, "Claude 续期备份没同步 → 恢复同步（确认后才调用 retry_save）") {
        select(.accounts)
        check.expect(Driver.visible("续期备份没同步"), "Claude 卡片底部写明「续期备份没同步」")
        check.expect(!Driver.visible("2.1.278"), "过时的版本文案不再作为主要说明出现")
        let count = state.directory!.issues(dismissed: []).count
        check.expect(Driver.visible("\(count)件事要处理"), "页头写明 \(count) 件事要处理")
        state.calls.removeAll()
        Driver.press("恢复同步")
        check.expect(state.calls.isEmpty, "点按钮只打开确认，不直接发请求")
        check.expect(Driver.visible("这一步不联网"), "确认框说明这一步不联网")
        check.expect(Driver.visible("1 分钟"), "确认框说明约 1 分钟后恢复协调续期")
        Driver.press("恢复同步", .exact)
        check.expect(check.lastCall("accounts.retry_save")?.contains("synthetic-claude") == true, "确认后调用 accounts.retry_save(id)")
        check.expect(main.attachedSheet == nil, "成功后确认框关闭")
    }
    section(2, "热切换开着但没连上 → 以调试模式重启并换上；连上后同一个框接着换上") {
        openSwitch("synthetic-nova")
        state.calls.removeAll()
        check.expect(Driver.visible("现在还换不到"), "出现「现在还换不到」说明")
        check.expect(Driver.visible("关闭热切换"), "给出第二条出路：关闭热切换")
        Driver.press("重启桌面并换上", .exact)
        check.expect(check.lastCall("accounts.desktop_relaunch_switch")?.contains("synthetic-nova") == true, "按钮调用 accounts.desktop_relaunch_switch(id)")
        check.expect(check.lastCall("accounts.activate") == nil, "没连上时不直接调用 accounts.activate")
        let restart = Date().timeIntervalSince1970
        Fixture.load("default", into: state, action: ["id": "synthetic-nova", "state": "relaunching", "message": "正在以调试模式重启 Codex 桌面；没有任务在运行", "at": restart])
        check.visible("正在以调试模式重启", "重启中在框内显示进度")
        check.expect(main.attachedSheet != nil, "重启期间框不关")
        print("  → 模拟桌面连上、服务接着换号，同一个框显示进度并在完成后关闭")
        Fixture.load("connected", into: state, action: ["id": "synthetic-nova", "state": "checking", "message": "正在检查桌面连接与任务状态", "at": restart])
        Driver.settle(0.8)
        check.expect(Driver.visible("同时更新 Codex 桌面"), "连上后自动改为桌面热切换确认")
        check.visible("正在检查桌面连接", "服务接着换号时框内显示进度")
        Fixture.load("connected", into: state, action: ["id": "synthetic-nova", "state": "complete", "message": "已切换到 nova", "at": restart + 1])
        Driver.settle(0.8)
        check.expect(main.attachedSheet == nil, "重启并换上完成后框自动关闭")
        print("  → 已连接时手动确认：检查中 → 被拒 → 重试 → 完成")
        Fixture.load("connected", into: state)
        openSwitch("synthetic-nova")
        Driver.press("已结束任务，换上")
        let activate = check.lastCall("accounts.activate") ?? ""
        check.expect(activate.contains("synthetic-nova") && activate.contains("\"confirm\":true"), "确认后调用 accounts.activate(id, confirm:true)")
        check.expect(main.attachedSheet != nil, "发出换号后框不关，等桌面回报")
        let now = Date().timeIntervalSince1970
        Fixture.load("connected", into: state, action: ["id": "synthetic-nova", "state": "checking", "message": "正在检查桌面任务", "at": now])
        check.visible("正在检查桌面任务", "检查中在框内显示进度")
        check.expect(Driver.visible("正在换"), "检查中按钮变为「正在换…」")
        let refusal = "Codex 桌面仍有运行或等待中的任务，请结束后切换：「合成任务」等待审批；切换未开始"
        Fixture.load("connected", into: state, action: ["id": "synthetic-nova", "state": "failed", "message": refusal, "at": now])
        check.visible("切换未开始", "被拒时框内显示原因")
        // The full refusal wraps, so the thread's name may be split across two lines.
        let refused = main.attachedSheet.map { Driver.read($0).map(\.text).joined() } ?? ""
        check.expect(refused.contains("合成任务"), "被拒时框内点名是哪个任务")
        state.calls.removeAll()
        Driver.press("重试", .exact)
        check.expect(check.lastCall("accounts.activate")?.contains("synthetic-nova") == true, "「重试」再次调用 accounts.activate")
        Fixture.load("connected", into: state, action: ["id": "synthetic-nova", "state": "complete", "message": "已切换到 nova", "at": Date().timeIntervalSince1970 + 1])
        Driver.settle(0.8)
        check.expect(main.attachedSheet == nil, "桌面回报完成后框自动关闭")
        print("  → 之前的失败记录不应出现在新打开的框里")
        Fixture.load("connected", into: state, action: ["id": "synthetic-nova", "state": "failed", "message": refusal, "at": now - 600])
        select(.codex)
        check.visible("10分钟前", "Codex 卡片把旧的失败记为历史，带时间")
        openSwitch("synthetic-nova")
        let fresh = main.attachedSheet.map { Driver.read($0).map(\.text).joined(separator: " ") } ?? ""
        check.expect(!fresh.contains("合成任务") && fresh.contains("已结束任务"), "新框不显示旧的失败记录")
        Driver.press("取消", .exact)
    }
    section(3, "用重置卡：额度还多时额外提醒，确认后才用最早到期的一张") {
        select(.codex)
        state.calls.removeAll()
        if !Driver.pressNear("nova", "用 1 张重置卡") { check.expect(false, "找到 nova 卡片上的「用 1 张重置卡」") }
        check.expect(Driver.visible("现在用会浪费"), "剩余 100% 时出现浪费提醒")
        check.expect(Driver.visible("用后还剩"), "确认框写明用后还剩几张")
        check.expect(state.calls.isEmpty, "打开确认不发请求")
        Driver.press("用 1 张", .exact)
        let reset = check.lastCall("accounts.reset_quota") ?? ""
        check.expect(reset.contains("credit-l1") && reset.contains("\"confirm\":true"), "确认后用最早到期的卡调用 reset_quota")
    }
    section(4, "更多菜单：重命名失败时错误留在框内，再开别的框不串错；续期待保存时不能认证检查") {
        select(.codex)
        more("lab team", choose: "重命名…")
        check.expect(Driver.lastMenu.contains { $0.title.contains("移除") } && Driver.lastMenu.contains { $0.title.contains("本地检查") }
                     && Driver.lastMenu.contains { $0.title == "查询额度" } && Driver.lastMenu.contains { $0.title == "执行记录" },
                     "更多菜单列出查询额度、检查、重命名、执行记录、移除")
        check.expect(Driver.visible("当名称"), "重命名框提供「用登录邮箱当名称」")
        Driver.press("当名称")
        state.failNext = "合成失败：名称被拒绝"
        Driver.press("保存", .exact)
        check.expect(check.lastCall("accounts.update")?.contains("example.invalid") == true, "保存发送邮箱作为名称")
        check.expect(Driver.visible("合成失败"), "失败错误显示在重命名框内，框不关闭")
        Driver.press("取消", .exact)
        if !Driver.pressNear("lab team", "换上") { check.expect(false, "找到 lab team 卡片上的「换上」") }
        check.expect(Driver.visible("现在还换不到"), "打开了换号框")
        check.expect(!Driver.visible("合成失败"), "新打开的换号框不带上一个错误")
        Driver.press("取消", .exact)
        select(.claude, "synthetic-claude")
        more("Claude 当前账号", choose: nil)
        check.expect(Driver.lastMenu.contains { $0.title.contains("认证检查") && !$0.enabled }, "续期未保存时「认证检查」不可用")
        if Driver.lastMenu.isEmpty || Driver.lastMenu.contains(where: { $0.title.contains("认证检查") && $0.enabled }) { print("    菜单：\(Driver.lastMenu)") }
    }
    section(5, "重置卡到期提醒：同一账号的多张合在一张票上，知道了之后不再标橙") {
        select(.codex)
        check.visible("3张重置卡.2张", "nova 的票写明 3 张重置卡、其中 2 张快到期")
        Driver.press("3张重置卡.2张")
        check.visible("nova 有 3 张重置卡", "点票列出 nova 每张卡的到期时间")
        Driver.press("知道了", .exact)
        Driver.settle(0.5)
        check.expect(!Driver.visible("3张重置卡.2张") && Driver.visible("3张重置卡"), "知道了之后 nova 的票只写张数")
        check.expect(Driver.visible("2张重置卡.1张"), "orbit 的提醒还在")
        check.expect(ReminderModel.shared.dismissed.isSuperset(of: ["credit-l1", "credit-l2"]), "按卡记下已知道，新卡会再提醒")
    }
    section(6, "换号与重置：改规则后保存需确认，发送完整规则") {
        select(.automation)
        check.visible("桌面没连上：现在只会重置，不会换号", "桌面没连上时写明 Codex 只会重置")
        check.expect(Driver.visible("从左往右接力") && Driver.visible("只换 Claude Code"), "Codex 和 Claude 分成两条接力线")
        check.expect(Driver.visible("见底线") && Driver.visible("见底时") && Driver.visible("参与"), "规则面板有见底线、见底时、参与")
        state.calls.removeAll()
        setOrder(from: "先换号", to: "先重置")
        check.visible("未保存", "修改后出现保存条「有未保存的修改」")
        check.expect(state.calls.isEmpty, "修改不立即发送")
        Driver.press("保存规则")
        check.expect(Driver.visible("按改过的规则自动执行"), "开启状态下保存先弹确认")
        Driver.press("确认保存", .exact)
        let strategy = check.lastCall("accounts.strategy") ?? ""
        check.expect(strategy.contains("\"confirm\":true") && strategy.contains("\"below\":3") && !strategy.contains("switch_below")
                     && strategy.contains("\"order\":\"reset_first\""), "确认后发送 accounts.strategy 完整规则（逐账号见底线）")
        check.expect(strategy.contains("synthetic-claude") && strategy.contains("\"order\":\"reset_only\"") && strategy.contains("\"participate\":false"),
                     "新 Claude 账号随规则发送：只重置，默认不参与")
        print("    \(strategy.prefix(400))")
    }
    section(7, "Codex 桌面并进 Codex 页：页头胶囊写连接状态，连接详情里重开、端口和热切换；没接管时页面上方写明并给出接管") {
        Fixture.load("unmanaged", into: state)
        select(.codex)
        check.visible("接管后才能热切换", "已连接但没接管时页面上方写明")
        check.expect(Driver.visible("未接管") && Driver.visible("接管桌面"), "页头胶囊写「未接管」，横幅给「接管桌面」")
        check.expect(Driver.near("orbit", "接管") == nil, "在用的卡片不再重复「接管」")
        Driver.press("连接详情")
        check.visible("已连接 · 未接管", "连接详情写明「已连接 · 未接管」")
        if let sheet = main.attachedSheet { Driver.escape(sheet) }
        Driver.settle(0.5)
        select(.accounts)
        check.expect(Driver.visible("交给 AuthBaton") && !Driver.hits("接管", .exact).isEmpty, "账号页在用的卡片说明并给出「接管」")
        Fixture.load("connected", into: state)
        select(.codex)
        check.expect(!Driver.visible("还在用自己的登录", wait: 0.3) && !Driver.visible("Codex 桌面没连上", wait: 0.3), "连上并接管后页面上没有横幅")
        // The capsule's monitor glyph reads as a character in front of its label.
        Driver.press("热切换", .suffix)
        check.visible("已连接 · 已接管", "连接详情写明「已连接 · 已接管」")
        if let sheet = main.attachedSheet { Driver.escape(sheet) }
        Driver.settle(0.5)
        Fixture.load("default", into: state)
        select(.codex)
        check.visible("Codex 桌面没连上", "没连上时页面上方写明")
        state.calls.removeAll()
        Driver.press("以调试模式重新打开")
        check.expect(check.lastCall("accounts.desktop_launch") != nil, "横幅「以调试模式重新打开」调用 accounts.desktop_launch")
        Driver.press("没连上", .suffix)
        check.visible("用调试模式重开，分三步", "没连上时连接详情列出三步")
        if let toggle = Driver.views(NSSwitch.self).first(where: { $0.window?.sheetParent != nil }) {
            Driver.click(toggle)
            check.expect(check.lastCall("accounts.desktop")?.contains("\"enabled\":false") == true, "关闭热切换开关发送 enabled:false")
        } else {
            check.expect(false, "找到连接详情里的热切换开关")
        }
    }
    section(8, "个人云端：切换模式需要确认") {
        select(.cloud)
        state.calls.removeAll()
        // The 本地 / 云端 segments sometimes read as one line; the lone 云端 further down is a heading.
        if Driver.hits("本地 云端").isEmpty { Driver.press("云端", .exact) } else { Driver.press("云端", .within("本地 云端")) }
        check.expect(state.calls.isEmpty, "点击「云端」不会直接切换")
        check.visible("包括自动重", "确认框写明自动重置也暂停")
        Driver.press("切换", .exact)
        check.expect(check.lastCall("accounts.cloud")?.contains("\"mode\":\"cloud\"") == true, "确认后才发送 configure mode=cloud")
    }
    section(9, "添加账号：导入不带名称，由服务按登录邮箱命名") {
        select(.accounts)
        state.calls.removeAll()
        Driver.press("添加账号", .suffix)
        check.expect(Driver.visible("登录新账号") && Driver.visible("导入这台 Mac"), "添加账号框给出登录新账号和导入两种方式")
        Driver.press("导入这台 Mac")
        check.expect(!Driver.visible("账号名称", wait: 0.3), "选导入后不再要名称")
        Driver.press("导入", .exact)
        let imported = check.lastCall("accounts.import") ?? ""
        check.expect(imported.contains("codex") && !imported.contains("label"), "导入请求不带名称")
        check.expect(main.attachedSheet == nil, "导入成功后框关闭")
    }
    section(10, "菜单栏面板：点账号先展开说明，没连上时给重启并换上") {
        main.orderOut(nil)
        let panel = open(MenuBarPanel(openWindow: {}), size: NSSize(width: 306, height: 527))
        check.expect(Driver.visible("件事要处理") && Driver.visible("续期备份没同步"), "面板顶部汇总要处理的事")
        check.expect(Driver.visible("3张重置卡快") && Driver.visible("桌面没连上"), "汇总合计快到期的重置卡和桌面状态")
        Driver.press("nova")
        check.expect(Driver.visible("现在换不过去"), "展开后说明没连上换不过去")
        check.expect(Driver.hits("换上 nova", .exact).isEmpty, "没连上时不提供直接换上")
        Driver.press("重启并换上")
        check.expect(check.lastCall("accounts.desktop_relaunch_switch")?.contains("synthetic-nova") == true, "面板里点「重启并换上」调用 accounts.desktop_relaunch_switch")
        Fixture.load("connected", into: state)
        Driver.settle(0.8)
        Driver.press("换上 nova")
        check.expect(check.lastCall("accounts.activate")?.contains("synthetic-nova") == true, "连上后点「换上 nova」调用 accounts.activate")
        Fixture.load("connected", into: state, action: ["id": "synthetic-nova", "state": "failed", "at": Date().timeIntervalSince1970,
                                                        "message": "Codex 桌面仍有运行或等待中的任务，请结束后切换：「合成任务」运行中；切换未开始"])
        check.visible("合成任务", "面板展开行内显示被拒原因")
        check.expect(Driver.visible("重试"), "被拒后按钮变为「重试」")
        Fixture.load("connected", into: state, action: ["id": "synthetic-nova", "state": "complete", "message": "已切换到 nova", "at": Date().timeIntervalSince1970 + 1])
        Driver.settle(0.8)
        check.expect(!Driver.visible("重试") && !Driver.visible("合成任务"), "完成后展开行收起")
        panel.orderOut(nil)
        main.orderFrontRegardless()
    }
    section(11, "Claude 换号：只换 Claude Code 时点了就换，不弹框，调用 accounts.activate") {
        Fixture.load("claude2", into: state)
        select(.claude, "synthetic-claude-b")
        state.calls.removeAll()
        Driver.press("换上", .exact)
        let activate = check.lastCall("accounts.activate") ?? ""
        check.expect(activate.contains("synthetic-claude-b") && activate.contains("\"confirm\":true"), "点「换上」直接调用 accounts.activate(id, confirm:true)")
        check.expect(main.attachedSheet == nil, "只换 Claude Code 不弹确认框")
        check.expect(!state.calls.contains { $0.contains("claude_desktop") }, "没有桌面登录的账号不碰 Claude 桌面")
    }
    section(12, "Claude 续期没收到回应：自动重新确认，不要求人处理") {
        Fixture.load("uncertain", into: state)
        select(.claude, "synthetic-claude")
        // The strip's ⓘ merges into the first characters and its small text is often misread: match the steadier parts.
        check.visible("没有收到回应", "Claude 页写明续期没收到回应、在等重新确认")
        check.visible("稍后自动用", "说明会自动重新确认")
        check.expect(!state.directory!.issues(dismissed: []).contains { $0.account == "synthetic-claude" }, "不计入要处理的事")
        check.expect(Driver.hits("恢复同步").isEmpty, "不给恢复同步按钮")
        check.expect(Driver.hits("用 1 张重置卡").isEmpty, "续期没确认时不给用重置卡（服务会拒绝）")
        more("Claude 当前账号", choose: nil)
        check.expect(Driver.lastMenu.contains { $0.title.contains("认证检查") && $0.enabled }, "可手动「认证检查」立即重新确认")
        if Driver.lastMenu.isEmpty { print("    菜单：\(Driver.lastMenu)") }
    }
    section(13, "Claude 5 小时窗口预热：开关沿用关闭前的时间，已开的显示起点、上次结果和下次时间；立即预热点一次发一条") {
        select(.claude, "synthetic-claude")
        check.visible("06:00", "已开启的账号显示每天的起点")
        check.visible("5小时窗口开始计时", "显示上次预热结果")
        check.visible("下次约", "显示下次预热时间")
        state.calls.removeAll()
        Driver.press("立即预热")
        let now = check.lastCall("accounts.warmup_now") ?? ""
        check.expect(now.contains("\"synthetic-claude\""), "「立即预热」调用 accounts.warmup_now(id)")
        if now.isEmpty { print("    调用：\(state.calls)") }
        Fixture.load("claude2", into: state)
        select(.claude, "synthetic-claude-b")
        check.expect(!Driver.visible("09:00", wait: 0.3), "关闭时不显示预热时间")
        check.expect(!Driver.hits("立即预热").isEmpty, "关闭自动预热时仍可立即预热")
        state.calls.removeAll()
        if let label = Driver.hits("发一条短消息").first,
           let toggle = Driver.views(NSSwitch.self).min(by: {
               abs($0.convert($0.bounds, to: nil).midY - label.rect.midY) < abs($1.convert($1.bounds, to: nil).midY - label.rect.midY)
           }) {
            Driver.click(toggle)
            let call = check.lastCall("accounts.warmup") ?? ""
            check.expect(call.contains("synthetic-claude-b") && call.contains("\"enabled\":true") && call.contains("\"hour\":9"),
                         "重新打开开关调用 accounts.warmup(id, enabled:true, hour:9)，沿用关闭前的时间")
            if call.isEmpty { print("    调用：\(state.calls)") }
        } else {
            check.expect(false, "找到预热开关")
        }
    }
    section(14, "菜单栏换号：登录失效、续期待保存、续期待确认的账号不提供换上，给出窗口里同样的出路") {
        Fixture.load("switchblock", into: state)
        main.orderOut(nil)
        let panel = open(MenuBarPanel(openWindow: {}), size: NSSize(width: 306, height: 765))
        Driver.press("nova")
        check.expect(Driver.visible("先在窗口里重新登录"), "需重新登录的账号说明先重新登录")
        check.expect(Driver.hits("换上 nova").isEmpty, "需重新登录的账号不提供「换上」或「重启并换上」")
        AppNavigation.shared.selection = nil
        // OCR reads the trailing ellipsis inconsistently; only the button line starts with the action.
        Driver.press("重新登录", .prefix)
        check.expect(AppNavigation.shared.page == .codex && AppNavigation.shared.selection == "synthetic-nova", "「重新登录…」打开 Codex 页并指向这个账号")
        check.expect(state.calls.isEmpty, "菜单栏不直接发请求")
        Driver.press("lab team")
        // OCR drops the 的 at this size.
        check.expect(Driver.visible("写入旧"), "续期待保存的账号说明现在换过去会写入旧的登录")
        check.expect(Driver.hits("换上 lab team").isEmpty && !Driver.hits("重试保存", .prefix).isEmpty, "续期待保存的账号只给「重试保存…」")
        Driver.press("Claude 备用")
        check.expect(Driver.visible("现在不能换上"), "续期待确认的账号说明现在不能换上")
        check.expect(Driver.hits("换上 Claude 备用").isEmpty, "续期待确认的账号不提供换上")
        panel.orderOut(nil)
        main.orderFrontRegardless()
        print("  → 窗口里同样不给换上")
        select(.claude, "synthetic-claude-b")
        check.expect(Driver.hits("换上", .exact).isEmpty, "Claude 页对续期待确认的账号不给「换上」")
        // OCR can merge the ⓘ before it into the first character (read as 「①警待」).
        check.expect(Driver.visible("待重新确认"), "写明在等什么")
    }
    section(15, "云端交接和云端维护的账号：不显示成「可用」，交接未完成算要处理；不认识的状态不算正常") {
        Fixture.load("cloud", into: state)
        select(.codex)
        check.visible("云端交接未完成", "交接中的账号显示「云端交接未完成」")
        check.visible("由云端维护", "云端维护的账号显示「由云端维护」")
        check.visible("状态未识别", "不认识的状态显示「状态未识别」")
        check.expect(Driver.near("nova", "换上") == nil && Driver.near("lab team", "换上") == nil, "交接中和云端维护的账号不给「换上」")
        let issues = state.directory!.issues(dismissed: [])
        check.expect(issues.contains { $0.account == "synthetic-nova" } && !issues.contains { $0.account == "synthetic-team" }, "交接未完成算要处理，云端维护不算")
        select(.accounts)
        check.expect(Driver.visible("\(issues.count)件事要处理"), "账号页页头写明 \(issues.count) 件事要处理")
    }
    section(16, "换号与重置：规则不含云端账号（含已保存的），超过 32 个账号时说明并只发 32 条") {
        Fixture.load("cloud", into: state)
        select(.automation)
        check.expect(!Driver.visible("lab team") && !Driver.visible("nova"), "接力线不列云端交接或云端维护的账号")
        setOrder(from: "先换号", to: "先重置")
        saveRules()
        var strategy = check.lastCall("accounts.strategy") ?? ""
        check.expect(!strategy.isEmpty && !strategy.contains("synthetic-team") && !strategy.contains("synthetic-nova"),
                     "保存的规则不含云端账号，已保存配置里的残留也去掉")
        print("  → 编辑中账号转交云端：发送时也去掉")
        Fixture.load("default", into: state)
        select(.accounts)
        select(.automation)
        state.calls.removeAll()
        setOrder(from: "先换号", to: "先重置")
        Fixture.load("cloud", into: state)
        Driver.settle(0.5)
        saveRules()
        strategy = check.lastCall("accounts.strategy") ?? ""
        check.expect(!strategy.isEmpty && !strategy.contains("synthetic-team") && !strategy.contains("synthetic-nova"), "发送前去掉已转交云端的账号")
        print("  → 36 个账号")
        Fixture.load("many", into: state)
        select(.accounts)
        select(.automation)
        check.visible("规则最多32个账号", "超过上限时说明规则最多 32 个账号")
        check.visible("extra 31", "说明里点名没加入的账号")
        state.calls.removeAll()
        setOrder(from: "先换号", to: "先重置")
        saveRules()
        strategy = check.lastCall("accounts.strategy") ?? ""
        check.expect(strategy.components(separatedBy: "\"id\":\"").count - 1 == 32 && !strategy.contains("synthetic-extra-31"), "只发送 32 条规则")
    }
    section(17, "后台服务断开：窗口和菜单栏都写明没连上，不再显示旧账号，可直接重新连接") {
        state.isConnected = false
        state.directory = nil
        state.errorText = "合成：后台服务已退出，账号和额度不再更新。"
        select(.accounts)
        check.visible("合成：后台服务已退出", "窗口顶部写明服务已退出")
        check.expect(!Driver.visible("正在载入账号列表"), "不显示正在载入")
        Driver.press("重新连接后台服务")
        check.expect(state.calls.contains("restart {}"), "窗口顶部的「重新连接后台服务」重新连接")
        main.orderOut(nil)
        state.calls.removeAll()
        let panel = open(MenuBarPanel(openWindow: {}), size: NSSize(width: 306, height: 340))
        check.expect(Driver.visible("后台服务没连上") && Driver.visible("合成：后台服务已退出"), "菜单栏写明没连上和原因")
        check.expect(!Driver.visible("换 Codex 账号"), "菜单栏不显示旧账号")
        Driver.press("重新连接后台服务")
        check.expect(state.calls.contains("restart {}"), "菜单栏的「重新连接后台服务」重新连接")
        panel.orderOut(nil)
        main.orderFrontRegardless()
    }
    section(18, "重新登录：只显示为这个账号发起的登录结果") {
        state.login = LoginAttempt(attemptId: "synthetic-attempt", provider: "codex", phase: "succeeded",
                                   message: "合成：nova 的官方登录已完成", verificationUrl: nil, userCode: nil)
        state.loginTarget = LoginTarget(accountID: "synthetic-nova")
        select(.codex)
        more("lab team", choose: "重新登录…")
        check.expect(Driver.visible("开始官方登录") && !Driver.visible("nova 的官方登录已完成"), "给 lab team 重新登录时不显示 nova 的结果")
        Driver.press("取消", .exact)
        more("nova", choose: "重新登录…")
        check.visible("nova 的官方登录已完成", "给 nova 重新登录时显示它自己的结果")
    }
    section(19, "后台检查进行中：桌面热切换照样能点（服务等检查结束再换），普通切换仍要等") {
        Fixture.load("connectedbusy", into: state)
        state.calls.removeAll()
        openSwitch("synthetic-nova")
        check.expect(Driver.visible("同时更新 Codex 桌面"), "检查进行中仍能打开桌面换号确认")
        Driver.press("已结束任务，换上")
        check.expect(check.lastCall("accounts.activate")?.contains("synthetic-nova") == true, "检查进行中也发出换号，由服务排队")
        Driver.press("取消", .exact)
        Driver.settle(0.3)
        Fixture.load("plainbusy", into: state)
        state.calls.removeAll()
        openSwitch("synthetic-nova")
        check.expect(Driver.lastMenu.contains { $0.title.hasPrefix("换上 nova") && !$0.enabled }, "普通切换在检查进行中不可点")
        check.expect(main.attachedSheet == nil && check.lastCall("accounts.activate") == nil, "没有打开换号框，也没发请求")
    }
    section(20, "换号与重置总开关：关闭和打开都先确认") {
        select(.automation)
        state.calls.removeAll()
        if let master = Driver.views(NSSwitch.self).max(by: { $0.convert($0.bounds, to: nil).midY < $1.convert($1.bounds, to: nil).midY }) {
            Driver.click(master)
            check.expect(Driver.visible("关闭自动换号与重置"), "关闭前先弹确认")
            check.expect(state.calls.isEmpty, "确认前不发请求")
            Driver.press("关闭", .exact)
            let stop = check.lastCall("accounts.strategy_stop") ?? ""
            check.expect(stop.contains("\"confirm\":true"), "确认后调用 accounts.strategy_stop(confirm:true)")
        } else {
            check.expect(false, "找到总开关")
        }
        Fixture.load("off", into: state)
        select(.automation)
        state.calls.removeAll()
        if let master = Driver.views(NSSwitch.self).max(by: { $0.convert($0.bounds, to: nil).midY < $1.convert($1.bounds, to: nil).midY }) {
            Driver.click(master)
            check.expect(Driver.visible("允许按这些规则自动执行"), "打开前先弹确认")
            Driver.press("确认开启", .exact)
            let strategy = check.lastCall("accounts.strategy") ?? ""
            check.expect(strategy.contains("\"enabled\":true") && strategy.contains("\"confirm\":true"), "确认后发送 enabled:true 的完整规则")
        } else {
            check.expect(false, "找到总开关")
        }
    }
    section(21, "换号与重置：往后移一位改接力顺序，保存后按新顺序发送") {
        select(.accounts)
        select(.automation)
        state.calls.removeAll()
        // The editor's arrows sit right of its title; orbit is first, so only "往后移一位" is enabled.
        if let (window, point) = Driver.controls(beside: "orbit", size: 22).last {
            Driver.click(window, point)
            check.visible("顺位已改", "移动后这条轨的标题行写「顺位已改」")
            check.expect(!Driver.visible("未保存", wait: 0.3), "只改了顺序时不再出现底部保存条")
            saveRules()
            let strategy = check.lastCall("accounts.strategy") ?? ""
            let orbit = strategy.range(of: "synthetic-orbit")?.lowerBound, personal = strategy.range(of: "synthetic-p\"")?.lowerBound
            check.expect(orbit != nil && personal != nil && personal! < orbit!, "orbit 排到 12345678（个人）后面")
        } else {
            check.expect(false, "找到规则面板的「往后移一位」")
        }
    }
    section(22, "执行记录：按账号筛选，名字用账号现在的名称") {
        select(.codex)
        more("12345678（个人）", choose: "执行记录")
        check.expect(Driver.visible("执行记录") && Driver.visible("12345678"), "从账号打开的执行记录筛选到这个账号")
        check.expect(Driver.visible("已用 1 张重置卡"), "记录里写结果")
        check.expect(Driver.visible("旧记录"), "旧记录标明来源未知")
        Driver.press("完成", .exact)
    }
    section(23, "Claude 用重置卡：确认后才用；有多类卡时可以选，默认官方下一张那一类，请求只带类别不带卡号") {
        Fixture.load("claude2", into: state)
        select(.claude, "synthetic-claude-b")
        check.visible("1张完整重置卡", "Claude 页按类别列出完整重置卡")
        check.visible("1张Fable重置卡", "Claude 页按类别列出 Fable 重置卡")
        state.calls.removeAll()
        if !Driver.press("用 1 张重置卡", .exact) { check.expect(false, "找到 Claude 备用卡片上的「用 1 张重置卡」") }
        check.expect(Driver.visible("用后还剩"), "确认框写明用后还剩几张")
        check.expect(Driver.visible("用这张完整重置卡"), "默认选官方下一张那一类")
        check.expect(state.calls.isEmpty, "打开确认不发请求")
        Driver.press("用 1 张", .exact)
        let reset = check.lastCall("accounts.reset_quota") ?? ""
        check.expect(reset.contains("synthetic-claude-b") && reset.contains("\"next_grant\":true") && reset.contains("\"confirm\":true")
                     && reset.contains("\"reset_kind\":\"full\"") && !reset.contains("credit_id"), "确认后请服务用完整重置卡这一类，请求里没有卡号")
        state.calls.removeAll()
        if !Driver.press("用 1 张重置卡", .exact) { check.expect(false, "再次找到「用 1 张重置卡」") }
        Driver.press("Fable 重置卡", .exact)
        check.expect(Driver.visible("用这张Fable重置卡") && Driver.visible("本周Fable用量"), "选了 Fable 重置卡后票上换成它和它适用的额度")
        Driver.press("用 1 张", .exact)
        let fable = check.lastCall("accounts.reset_quota") ?? ""
        check.expect(fable.contains("\"reset_kind\":\"fable\"") && !fable.contains("credit_id"), "确认后请服务用 Fable 重置卡这一类")
    }
    section(24, "Claude 登录：不自动开浏览器，链接可以打开或复制，手动授权收在后备里") {
        Fixture.load("default", into: state)
        ClaudeLogin.waiting(state)
        UserDefaults.standard.set(ClaudeUse.code.rawValue, forKey: ClaudeUse.key)
        select(.claude)
        Driver.press("添加账号", .suffix)
        check.expect(Driver.visible("打开浏览器") && Driver.visible("复制链接"), "授权中给出「打开浏览器」和「复制链接」")
        check.expect(!Driver.visible("提交", wait: 0.3), "手动授权默认收起")
        // The real pasteboard: keep whatever the person had on it and put it back afterwards.
        let board = NSPasteboard.general
        let saved = (board.pasteboardItems ?? []).map { item in
            let copy = NSPasteboardItem()
            for type in item.types { if let data = item.data(forType: type) { copy.setData(data, forType: type) } }
            return copy
        }
        Driver.press("复制链接")
        check.expect(board.string(forType: .string) == ClaudeLogin.link, "「复制链接」复制的是回到本机的登录链接")
        check.visible("已复制", "复制后按钮变成「已复制」")
        board.clearContents()
        if !saved.isEmpty { board.writeObjects(saved) }
        Driver.press("用手动授权")
        check.expect(Driver.visible("打开手动授权页面") && Driver.visible("提交"), "展开后有手动授权页面和代码输入")
        Driver.press("在后台等待", .exact)
        state.login = nil
        state.loginTarget = nil
    }
    section(25, "Claude 页（两边都用）：同步条、桌面换号要确认并按 handle 发送，Code 在用的账号只把桌面换过来") {
        // With the Code relay strip on the page, the third card's buttons sit below a 765-high window.
        main.setContentSize(NSSize(width: 1020, height: 900))
        defer { main.setContentSize(NSSize(width: 1020, height: 765)) }
        Fixture.load("default", into: state)
        select(.claude)
        check.expect(Driver.visible("Code与桌面") && Driver.visible("个桌面会话") && Driver.visible("各账号一样"), "两边都用：页头写使用方式，同步条写会话数和各账号一样")
        check.expect(Driver.visible("或补登") && !Driver.hits("补登", .exact).isEmpty && !Driver.hits("用桌面登录查", .exact).isEmpty, "只在桌面登录过的账号给出「用桌面登录查」和「补登」")
        check.expect(Driver.visible("Claude桌面还在用work"), "Code 和桌面不在同一个账号时提示，可以把桌面也换上")
        state.calls.removeAll()
        if let toggle = Driver.views(NSSwitch.self).first {
            Driver.click(toggle)
            let call = check.lastCall("accounts.claude_desktop") ?? ""
            check.expect(call.contains("\"desktop_action\":\"background\"") && call.contains("\"enabled\":false"), "关掉后台同步发送 background enabled:false")
        } else {
            check.expect(false, "同步条有后台同步开关")
        }
        Driver.press("也换上")
        check.visible("换上 Claude 当前账号", "「桌面也换上」先弹确认")
        check.visible("这次把桌面也换过来", "Code 已在用这个账号：确认里只说把桌面换过来")
        check.visible("打开Bypass权限模式", "在用的账号开着 Bypass、要换上的没开：确认里写明会替它打开")
        check.expect(Driver.visible("退出登录", wait: 0.3) == false, "换号确认不混入添加账号的提醒")
        Driver.press("换上", .exact)
        let call = check.lastCall("accounts.claude_desktop") ?? ""
        check.expect(call.contains("\"desktop_action\":\"switch\"") && call.contains("\"account\":\"h-nova\"") && call.contains("\"confirm\":true"),
                     "确认后发送 switch，账号用 handle、带 confirm")
        check.expect(check.lastCall("accounts.activate") == nil, "Code 已在用，不再换 Claude Code")
        check.visible("让 Claude 退出", "确认后同一个框接着显示步骤")
        Driver.press("好", .exact)
        check.expect(check.lastCall("accounts.claude_desktop")?.contains("\"desktop_action\":\"dismiss\"") == true, "结束后点「好」清掉这次操作")
        Driver.press("下一个账号", .exact)
        check.visible("换上 Claude 当前账号", "「下一个账号」按编号挑到 #1，同样先确认")
        Driver.press("取消", .exact)
        if !Driver.pressNear("lab", "换上") { check.expect(false, "找到 lab 卡片上的「换上」") }
        check.visible("换上 lab", "只有桌面登录的账号也能换上桌面")
        check.expect(Driver.visible("打开Bypass权限模式", wait: 0.3) == false, "要换上的账号已经开了 Bypass，确认里不提")
        Driver.press("取消", .exact)
        Driver.press("不用", .exact)
        check.expect(Driver.visible("Claude桌面还在用work", wait: 0.3) == false, "点「不用」后不再提示这一对")
        Driver.menuChoice = "在 Claude 桌面登录新账号…"
        Driver.press("添加账号", .suffix)
        check.visible("退出登录", "两边都用时添加账号可以选在 Claude 桌面登录，提醒不要退出登录")
        Driver.press("取消", .exact)
    }
    section(26, "Claude 页的桌面部分：换号进行中可以不等，落后、要重启、保留期和软链都写明并给出处理") {
        Fixture.load("desktopswitching", into: state)
        select(.claude)
        check.visible("Claude桌面正在换上#2work", "换号进行中页面顶部写明")
        Driver.press("查看", .exact)
        check.expect(Driver.visible("Claude 在问要不要退出") && Driver.visible("不等了"), "Claude 在问时说明去 Claude 里回答，可以不等")
        state.calls.removeAll()
        Driver.press("不等了", .exact)
        check.expect(check.lastCall("accounts.claude_desktop")?.contains("\"desktop_action\":\"stop\"") == true, "「不等了」发送 stop")
        Driver.press("在后台继续", .exact)
        Fixture.load("desktopbehind", into: state)
        select(.claude)
        check.expect(Driver.visible("要重启一次Claude才看得到新会话"), "同步条写明要重启一次 Claude")
        check.expect(Driver.visible("12 天后") && Driver.visible("改回真文件夹"), "保留期和软链提醒都在")
        state.calls.removeAll()
        Driver.press("一直保留", .exact)
        check.expect(check.lastCall("accounts.claude_desktop")?.contains("\"desktop_action\":\"retention_keep\"") == true, "「一直保留」发送 retention_keep")
        Driver.press("改回真文件夹", .exact)
        let fix = check.lastCall("accounts.claude_desktop") ?? ""
        check.expect(fix.contains("\"desktop_action\":\"fix_symlinks\"") && fix.contains("\"confirm\":true"), "「改回真文件夹」发送 fix_symlinks")
        // Left to itself the page would drop Claude 桌面 once it is gone; someone who said they use both still sees why.
        UserDefaults.standard.set(ClaudeUse.both.rawValue, forKey: ClaudeUse.key)
        Fixture.load("desktopmissing", into: state)
        select(.claude)
        check.visible("这台Mac上没找到Claude桌面", "没装 Claude 桌面时写明")
        main.orderOut(nil)
        Fixture.load("default", into: state)
        let panel = open(MenuBarPanel(openWindow: {}), size: NSSize(width: 306, height: 720))
        check.expect(Driver.visible("在用 #2 work") && Driver.visible("会话一致"), "菜单栏写桌面在用的账号和同步状态")
        Driver.press("下一个", .exact)
        check.visible("换上 #1 nova@example", "菜单栏「下一个」先在行内确认")
        check.visible("换上时会顺带打开", "菜单栏的确认同样写明会顺带打开 Bypass")
        state.calls.removeAll()
        Driver.press("换上", .exact)
        check.expect(check.lastCall("accounts.claude_desktop")?.contains("\"account\":\"h-nova\"") == true, "菜单栏确认后按 handle 换上")
        panel.orderOut(nil)
        main.orderFrontRegardless()
    }
    section(27, "Claude 页：两边都能换时「换上」分两半，▾ 选只换一边并记住；只用 Code / 只用桌面时页面只放那一边") {
        // With the Code relay strip on the page, the third card's buttons sit below a 765-high window.
        main.setContentSize(NSSize(width: 1020, height: 900))
        defer { main.setContentSize(NSSize(width: 1020, height: 765)) }
        Fixture.load("claudeboth", into: state)
        select(.claude)
        check.expect(Driver.visible("只有会话、没保存登录") && Driver.visible("c***@example.invalid"), "只有会话目录的旧账号单独写一句")
        state.calls.removeAll()
        // Only Claude 备用 can move both sides, so its split button is the one plain 「换上」 on the page.
        if !Driver.press("换上", .exact) { check.expect(false, "找到 Claude 备用卡片上的「换上」") }
        check.expect(Driver.visible("一起换") && Driver.visible("只换Code") && Driver.visible("只换桌面"), "两边都能换：确认框给一起换 / 只换 Code / 只换桌面")
        check.visible("下次点「换上」默认按这次的选择来", "写明会记住这次的选择")
        check.expect(Driver.visible("不用退出CLI") && Driver.visible("会退出再打开Claude桌面"), "默认一起换：两边各写一句会怎样")
        Driver.press("只换Code", .exact)
        check.expect(Driver.visible("会退出再打开Claude桌面", wait: 0.3) == false, "选只换 Code 后不再提 Claude 桌面")
        Driver.press("立即换上", .exact)
        let activate = check.lastCall("accounts.activate") ?? ""
        check.expect(activate.contains("synthetic-claude-b") && activate.contains("\"confirm\":true"), "只换 Code：调用 accounts.activate")
        check.expect(!state.calls.contains { $0.contains("\"desktop_action\":\"switch\"") }, "只换 Code 不碰 Claude 桌面")
        check.expect(UserDefaults.standard.string(forKey: ClaudeUse.scopeKey) == SwitchScope.code.rawValue, "记住了「只换 Code」")
        Driver.settle(0.5)
        state.calls.removeAll()
        if !Driver.press("换上", .exact) { check.expect(false, "再次找到「换上」") }
        check.expect(main.attachedSheet == nil && check.lastCall("accounts.activate") != nil, "下次按记住的「只换 Code」直接换，不弹框")
        print("  → 只用 Code")
        UserDefaults.standard.set(ClaudeUse.code.rawValue, forKey: ClaudeUse.key)
        Fixture.load("default", into: state)
        select(.claude)
        check.expect(Driver.visible("只用Code") && !Driver.visible("个桌面会话", wait: 0.3), "只用 Code：没有桌面会话同步条")
        check.expect(Driver.hits("下一个账号", .exact).isEmpty, "只用 Code：没有「下一个账号」")
        check.visible("只存了桌面登录", "只在桌面登录过的账号收成一行提示")
        print("  → 只用桌面")
        UserDefaults.standard.set(ClaudeUse.desktop.rawValue, forKey: ClaudeUse.key)
        Fixture.load("claude2", into: state)
        select(.claude)
        check.expect(Driver.visible("只用桌面") && Driver.visible("个桌面账号") && Driver.visible("个桌面会话"), "只用桌面：页头写桌面账号数，同步条在")
        check.expect(Driver.visible("去桌面登录"), "没在桌面登录过的账号给「去桌面登录」")
        check.expect(Driver.visible("第1位", wait: 0.3) == false, "只用桌面：卡片不写 Claude Code 的顺位")
    }
    section(28, "只在桌面登录过的账号：打开「用桌面的登录查额度」要先说明并确认，打开后卡片显示额度和来源") {
        Fixture.load("default", into: state)
        select(.claude)
        state.calls.removeAll()
        if !Driver.press("用桌面登录查", .exact) { check.expect(false, "找到「用桌面登录查」") }
        check.visible("Claude Safe Storage", "确认框写明系统会问 Claude Safe Storage")
        check.expect(Driver.visible("不续期") && Driver.visible("不用重置卡"), "确认框写明只查额度、不续期、不用重置卡")
        check.expect(check.lastCall("accounts.claude_desktop") == nil, "打开确认框不发请求")
        Driver.press("打开", .exact)
        let call = check.lastCall("accounts.claude_desktop") ?? ""
        check.expect(call.contains("\"desktop_action\":\"token_quota\"") && call.contains("\"enabled\":true") && call.contains("\"confirm\":true"),
                     "确认后发送 token_quota enabled:true confirm:true")
        Fixture.load("deskquota", into: state)
        select(.claude)
        check.visible("用Claude桌面的登录查", "work 的卡片显示用桌面登录查到的额度和来源")
        check.visible("还没在Claude桌面用过Code模式", "lab 没有 Claude Code 登录时写明原因")
        check.visible("用Code模式聊一次", "并提示去 Code 模式聊一次，之后自动刷新")
        check.expect(Driver.hits("用桌面登录查", .exact).isEmpty, "打开以后不再给「用桌面登录查」")
    }
    section(32, "Codex 使用方式：只用 CLI 关掉热切换、页面不再出现桌面；换回用桌面恢复热切换；没连上的换号框选「关闭热切换」后变成普通切换") {
        select(.codex)
        check.expect(Driver.visible("CLI 与桌面"), "开着热切换时默认按「CLI 与桌面」")
        state.calls.removeAll()
        Driver.menuChoice = "只用 Codex CLI"
        // The menu's chevron reads as a character after its label.
        Driver.press("CLI 与桌面", .prefix)
        Driver.settle()
        check.expect(check.lastCall("accounts.desktop")?.contains("\"enabled\":false") == true, "选「只用 CLI」发送热切换 enabled:false")
        check.expect(UserDefaults.standard.string(forKey: CodexUse.key) == CodexUse.cli.rawValue, "记住了「只用 CLI」")
        Fixture.load("hotoff", into: state)
        Driver.settle()
        check.expect(Driver.visible("只用 CLI") && !Driver.visible("普通切换", wait: 0.3) && !Driver.visible("Codex 桌面没连上", wait: 0.3),
                     "只用 CLI：页头没有桌面胶囊，也没有桌面横幅")
        check.visible("只用 CLI 时见底只会重置", "页脚写明只用 CLI 时见底只会重置")
        openSwitch("synthetic-nova")
        check.expect(Driver.visible("仅改写") && Driver.visible("请先退出正在运行的 Codex CLI"), "只用 CLI 的换号框：普通切换，提醒先退出 CLI")
        Driver.press("取消", .exact)
        Driver.settle()
        select(.codex)
        state.calls.removeAll()
        Driver.menuChoice = "CLI 与桌面都用"
        Driver.press("只用 CLI", .prefix)
        Driver.settle()
        check.expect(check.lastCall("accounts.desktop")?.contains("\"enabled\":true") == true, "换回用桌面时恢复热切换 enabled:true")
        print("  → 没连上的换号框：选「关闭热切换」，主按钮跟着变")
        UserDefaults.standard.removeObject(forKey: CodexUse.key)
        Fixture.load("default", into: state)
        openSwitch("synthetic-nova")
        check.expect(Driver.visible("重启桌面并换上"), "默认选「重启桌面并换上」")
        Driver.press("关闭热切换，改用普通切换")
        check.visible("关闭热切换并继续", "选第二种后主按钮变「关闭热切换并继续」")
        state.calls.removeAll()
        Driver.press("关闭热切换并继续", .exact)
        check.expect(check.lastCall("accounts.desktop")?.contains("\"enabled\":false") == true, "确认后发送热切换 enabled:false")
        UserDefaults.standard.set(CodexUse.both.rawValue, forKey: CodexUse.key)
        Fixture.load("hotoff", into: state)
        Driver.settle(0.8)
        check.visible("已退出，换上", "关掉热切换后同一个框变成普通切换确认")
        check.visible("退出 Codex 桌面和 CLI", "用桌面时普通切换提醒先退出桌面")
        Driver.press("取消", .exact)
    }
    section(33, "首次使用引导：没有账号时 Codex / Claude 页换成起手卡，能导入本机登录；先不用后不再出现；有了第一个账号后只剩一行提示；老用户看不到") {
        select(.codex)
        check.expect(!Driver.visible("开始管理 Codex 账号", wait: 0.3) && !Driver.visible("再加一个备用号", wait: 0.3), "已有账号时不出现引导")
        check.expect(UserDefaults.standard.string(forKey: Onboarding.codexKey) == Onboarding.done, "已有账号的第一次打开直接记为结束")
        UserDefaults.standard.removeObject(forKey: Onboarding.codexKey)
        UserDefaults.standard.removeObject(forKey: Onboarding.claudeKey)
        Fixture.load("fresh", into: state)
        select(.codex)
        check.visible("开始管理 Codex 账号", "Codex 没有账号：起手卡")
        check.expect(Driver.visible("平时怎么用 Codex") && Driver.visible("本机已有 Codex 登录"), "第一步选使用方式，第二步给出本机登录")
        // One 「CLI 与桌面」 (the card's segment); the header menu would be a second. OCR sometimes misreads 桌.
        check.expect(Driver.hits("CLI 与", .prefix).count == 1, "起手卡时页头没有使用方式菜单，只在卡里选")
        state.calls.removeAll()
        Driver.press("导入本机登录", .exact)
        check.expect(check.lastCall("accounts.import")?.contains("codex") == true, "「导入本机登录」调用 accounts.import(codex)")
        Driver.press("先不用", .exact)
        Driver.settle()
        check.expect(UserDefaults.standard.string(forKey: Onboarding.codexKey) == Onboarding.done, "先不用后记下引导已结束")
        check.expect(!Driver.visible("开始管理 Codex 账号", wait: 0.3) && Driver.visible("还没有 Codex 账号"), "先不用后回到普通空页面")
        select(.claude)
        check.visible("开始管理 Claude 账号", "Claude 没有账号：起手卡")
        check.expect(Driver.visible("本机 Claude Code 登录") && Driver.visible("在 Claude 桌面登录"), "列出本机 Claude Code 登录和 Claude 桌面")
        print("  → 有了第一个账号：一行提示")
        UserDefaults.standard.set(Onboarding.started, forKey: Onboarding.codexKey)
        Fixture.load("codexone", into: state)
        select(.codex)
        check.visible("再加一个备用号", "第一个账号后提示加备用号")
        Driver.press("添加备用号", .exact)
        check.expect(main.attachedSheet != nil, "「添加备用号」打开添加账号")
        Driver.press("取消", .exact)
        Driver.settle()
    }
    section(29, "Claude 卡见底：见底那栏的大数字换成多久后回满，条下写剩余和见底线，名字旁「已见底」；两个都见底只放大最晚回满的；独立额度是小进度条") {
        Fixture.load("claudelow", into: state)
        select(.claude)
        check.visible("已见底", "7 天见底的账号名字旁写「已见底」")
        check.visible("后回满", "见底那栏的大数字换成多久后回满")
        check.visible("剩2%·见底线4%", "条下面写剩余和见底线")
        // The amber tick at the start of a low Fable bar sometimes reads as a letter after the label.
        check.expect(Driver.hits("Fable", .prefix).count == 2 && Driver.visible("88%"), "两张卡的 Fable 都是一行小进度条，带百分比")
        check.expect(!Driver.visible("独立额度"), "卡片上不再写「独立额度：…」")
        Fixture.load("claudelow5", into: state)
        select(.claude)
        check.visible("剩0%·见底线4%", "只有 5 小时见底时换的是 5 小时那栏")
        check.visible("4天后回满", "7 天那栏照常写回满时间")
        Fixture.load("claudelowboth", into: state)
        select(.claude)
        check.visible("剩1%·见底线4%", "两个都见底：放大的是回满最晚的 7 天")
        check.visible("2小时后回满", "两个都见底：5 小时那栏照常小字写回满时间")
        Fixture.load("default", into: state)
        select(.claude)
        check.expect(Driver.hits("已见底").isEmpty, "没有规则盯着的账号不写「已见底」")
    }
    section(30, "换号与重置第一次：从没开过时总开关卡换成两步起手卡，开启照样先确认，开启后提示一次；先不用后不再出现；没账号时先去添加") {
        UserDefaults.standard.removeObject(forKey: AutomationPage.onboardedKey)
        Fixture.load("off", into: state)
        // A fresh page: a draft left by another section would show as unsaved edits.
        select(.accounts)
        select(.automation)
        check.visible("开启自动接力前，先核对这两件事", "从没开过时出现起手卡")
        check.visible("Codex：桌面没连上，目前只会重置", "桌面没连上时写明 Codex 只会重置")
        check.visible("默认未参与，暂无法接力", "写明 Claude 账号默认不参与")
        check.expect(!Driver.visible("已关闭 · 打开后按下面的规则自动处理", wait: 0.3), "起手卡替换原来的总开关卡")
        state.calls.removeAll()
        Driver.press("开启自动接力", .exact)
        check.expect(Driver.visible("允许按这些规则自动执行"), "开启照样先弹确认")
        check.expect(state.calls.isEmpty, "确认前不发请求")
        Driver.press("确认开启", .exact)
        let strategy = check.lastCall("accounts.strategy") ?? ""
        check.expect(strategy.contains("\"enabled\":true") && strategy.contains("\"confirm\":true"), "确认后发送 enabled:true 的完整规则")
        // The service now reports the strategy on.
        Fixture.load("default", into: state)
        Driver.settle(0.6)
        check.visible("自动规则已开始运行", "由起手卡开启后提示一次")
        check.expect(UserDefaults.standard.bool(forKey: AutomationPage.onboardedKey), "开过一次就记下，不再出现起手卡")
        if let (window, point) = Driver.controls(beside: "改规则保存时会再请你确认", size: 22).first {
            Driver.click(window, point)
            check.expect(!Driver.visible("自动规则已开始运行", wait: 0.5), "点 × 关掉提示")
            check.expect(UserDefaults.standard.string(forKey: AutomationPage.hintKey) == "done", "关掉后不再出现")
        } else {
            check.expect(false, "找到提示的 ×")
        }
        print("  → 先不用")
        UserDefaults.standard.removeObject(forKey: AutomationPage.onboardedKey)
        Fixture.load("off", into: state)
        select(.accounts)
        select(.automation)
        Driver.press("先不用", .exact)
        check.visible("已关闭 · 打开后按下面的规则自动处理", "先不用后回到普通的总开关卡")
        check.expect(UserDefaults.standard.bool(forKey: AutomationPage.onboardedKey), "先不用后记下")
        print("  → 还没有账号")
        UserDefaults.standard.removeObject(forKey: AutomationPage.onboardedKey)
        Fixture.load("offempty", into: state)
        select(.accounts)
        select(.automation)
        check.visible("还没有可接力的账号", "没账号时说先去添加")
        check.expect(!Driver.visible("从左往右接力", wait: 0.3), "没账号时不画接力轨")
        Driver.press("去添加账号", .exact)
        check.expect(AppNavigation.shared.page == .accounts, "「去添加账号」打开账号页")
        print("  → 已经开着的老用户")
        UserDefaults.standard.removeObject(forKey: AutomationPage.onboardedKey)
        UserDefaults.standard.removeObject(forKey: AutomationPage.hintKey)
        Fixture.load("default", into: state)
        select(.automation)
        check.expect(!Driver.visible("开启自动接力前", wait: 0.3) && !Driver.visible("自动规则已开始运行", wait: 0.3), "已经开着时不出现起手卡和提示")
        check.expect(UserDefaults.standard.bool(forKey: AutomationPage.onboardedKey), "已经开着的记为看过")
    }
    section(31, "换号与重置排序：顺位已改写在这条轨的标题行，撤销只恢复这条轨的顺序，保存规则只有一个，保存发送新顺序") {
        select(.accounts)
        select(.automation)
        state.calls.removeAll()
        // The editor's arrow moves through the same reorder a drop does.
        @MainActor func moveOrbitRight() -> Bool {
            guard let (window, point) = Driver.controls(beside: "orbit", size: 22).last else { return false }
            Driver.click(window, point)
            return true
        }
        check.expect(moveOrbitRight(), "找到规则面板的「往后移一位」")
        check.visible("顺位已改", "顺序变了，标题行写「顺位已改」")
        check.expect(Driver.hits("保存规则").count == 1, "只改顺序：只有标题行一个「保存规则」")
        setOrder(from: "先换号", to: "先重置")
        check.visible("未保存", "顺序以外也改了：出现底部保存条")
        check.expect(Driver.hits("保存规则").count == 1, "两样都改了：仍只有一个「保存规则」")
        Driver.press("撤销", .exact)
        check.expect(!Driver.visible("顺位已改", wait: 0.5), "撤销后这条轨回到原来的顺序")
        check.visible("未保存", "撤销只动顺序，规则的改动还在")
        Driver.press("还原", .exact)
        check.expect(!Driver.visible("未保存", wait: 0.5), "还原后没有改动")
        check.expect(moveOrbitRight(), "再往后移一位")
        saveRules()
        let strategy = check.lastCall("accounts.strategy") ?? ""
        let orbit = strategy.range(of: "synthetic-orbit")?.lowerBound, personal = strategy.range(of: "synthetic-p\"")?.lowerBound
        check.expect(orbit != nil && personal != nil && personal! < orbit!, "保存后 orbit 排到 12345678（个人）后面")
        check.expect(strategy.contains("\"order\":\"switch_first\""), "只改了顺序，规则本身照旧")
    }
    section(34, "Claude 桌面 Code 无感换号：开启安装确认与托管提示说明、开启态展示、底部卸载插件闭环") {
        Fixture.load("default", into: state)
        select(.claude)
        state.calls.removeAll()
        check.visible("Code无感换号", "Claude 页出现 Code 无感换号条")
        if !Driver.press("开启…", .exact) { check.expect(false, "找到「开启…」按钮") }
        check.visible("管理员密码框", "安装确认框说明首次开启会弹管理员密码框")
        check.expect(Driver.visible("托管策略") || Driver.visible("托管配置"), "安装确认框主动解释 Claude 桌面的托管配置提示属正常现象")
        check.expect(Driver.visible("短期访问令牌") && Driver.visible("原生直连"), "安装确认框写明只领短期访问令牌且由桌面内置引擎原生直连")
        check.expect(check.lastCall("accounts.claude_desktop") == nil, "仅打开安装确认框不发请求")
        Driver.press("授权并开启", .exact)
        let enableCall = check.lastCall("accounts.claude_desktop") ?? ""
        check.expect(enableCall.contains("\"desktop_action\":\"code_relay\"")
                     && enableCall.contains("\"enabled\":true")
                     && enableCall.contains("\"confirm\":true"),
                     "确认后发送 code_relay enabled:true confirm:true")
        Fixture.load("coderelayon", into: state)
        select(.claude)
        state.calls.removeAll()
        // The strip's small grey line reads unevenly (当前 as 当刖, 接力 as 接刀); match the parts OCR keeps.
        check.visible("账号Claude", "开启后状态条展示当前接力账号")
        check.visible("3次", "开启后展示已接力次数")
        check.expect(!Driver.visible("桌面也换上", wait: 0.3), "开启无感换号时不弹账号不一致横幅")
        if let hit = Driver.hits("卸载插件").first {
            // Vision OCR merges the footer text and trailing link into one line; click the trailing link.
            Driver.click(hit.window, NSPoint(x: hit.rect.maxX - 18, y: hit.rect.midY))
            check.expect(true, "底部找到「卸载插件…」入口")
        } else {
            check.expect(false, "底部找到「卸载插件…」入口")
        }
        check.visible("托管配置提示也会消失", "卸载确认框写明卸载后托管配置提示消失")
        check.expect(check.lastCall("accounts.claude_desktop") == nil, "仅打开卸载确认框不发请求")
        Driver.press("授权并卸载", .exact)
        let uninstallCall = check.lastCall("accounts.claude_desktop") ?? ""
        check.expect(uninstallCall.contains("\"desktop_action\":\"code_relay_uninstall\"")
                     && uninstallCall.contains("\"confirm\":true"),
                     "确认后发送 code_relay_uninstall confirm:true")
        print("  → 纯桌面模式开启 Code 无感换号：Code 账号免桌面登录直接无感换上")
        UserDefaults.standard.set(ClaudeUse.desktop.rawValue, forKey: ClaudeUse.key)
        defer { UserDefaults.standard.removeObject(forKey: ClaudeUse.key) }
        Fixture.load("coderelaydesk", into: state)
        select(.claude)
        state.calls.removeAll()
        check.visible("Code无感可用", "只用桌面且开启无感换号时，仅登在 AuthBaton 的账号标注「Code 无感可用」")
        check.expect(!Driver.visible("去桌面登录", wait: 0.3), "不再要求去桌面重复登录")
        if !Driver.press("换上", .exact) { check.expect(false, "找到 Claude 备用卡片上的「换上」") }
        let directActivate = check.lastCall("accounts.activate") ?? ""
        check.expect(main.attachedSheet == nil && directActivate.contains("synthetic-claude-b"),
                     "只用桌面开启无感换号时，点「换上」直接无感切 Code，不弹桌面重启框")
        Driver.menuChoice = "添加 Code 模式账号（无感换号 / CLI 共用）…"
        Driver.press("添加账号", .suffix)
        check.expect(main.attachedSheet != nil, "只用桌面开启无感换号时，页头「添加账号」可直接添加 Code 模式账号")
        Driver.press("取消", .exact)
        print("  → 开着无感换号但只在桌面登录过：确认框说明这次要重启桌面，并给「登录 Code 账号」")
        Fixture.load("coderelaydeskonly", into: state)
        select(.claude)
        state.calls.removeAll()
        if !Driver.press("换上…", .exact) { check.expect(false, "找到只在桌面登录过的账号的「换上…」") }
        // The long synthetic email wraps the banner after 「Code 账」; match the line OCR keeps whole.
        check.visible("这次只能重启Claude桌面", "确认框写明这个账号还没登录 Code 账号、这次只能重启 Claude 桌面")
        check.expect(!Driver.visible("只换ClaudeCode即可", wait: 0.3), "不再提示这个账号做不到的「只换 Claude Code」")
        Driver.press("登录 Code 账号", .exact)
        check.visible("官方ClaudeCLI登录", "点「登录 Code 账号」打开 Claude 登录框")
        check.expect(check.lastCall("accounts.claude_desktop") == nil, "没有发出桌面换号")
        Driver.press("取消", .exact)
    }
    section(35, "文案跟着规则走：没有规则会重置时哪里都不说会重置；确认框只写这些规则会做的事") {
        UserDefaults.standard.set(true, forKey: AutomationPage.onboardedKey)
        Fixture.load("switchonly", into: state)
        select(.accounts)
        select(.automation)
        check.visible("桌面没连上：现在不会换号", "都只换号、桌面没连上：只写不会换号")
        check.expect(!Driver.visible("只会重置", wait: 0.3), "都只换号时页面上不出现「只会重置」")
        func rule(_ id: String, _ order: CodexStrategyOrder, on: Bool = true) -> CodexStrategyConfig.Rule {
            CodexStrategyConfig.Rule(id: id, order: order, participate: on)
        }
        let tool: (String) -> String? = { $0.hasPrefix("claude") ? "claude" : "codex" }
        // The report this fix came from: one Claude account, 只换号.
        var text = AutomationPage.confirmation([rule("claude-a", .switchOnly)], tool: tool)
        check.expect(text.contains("没有账号会自动用重置卡") && !text.contains("用掉") && !text.contains("官方"),
                     "Claude 账号只换号：确认框只说不会用重置卡，不再说会用掉官方的卡")
        check.expect(text.contains("Claude 直接换默认登录") && !text.contains("Codex"), "只有 Claude 换号时不提 Codex")
        text = AutomationPage.confirmation([rule("claude-a", .resetFirst), rule("codex-a", .switchOnly)], tool: tool)
        check.expect(text.contains("会自动用掉重置卡") && text.contains("官方排的下一张") && !text.contains("没有账号会自动用重置卡"),
                     "有 Claude 账号会重置：写明会用卡、用的是官方排的下一张")
        text = AutomationPage.confirmation([rule("codex-a", .resetOnly), rule("claude-a", .switchOnly, on: false)], tool: tool)
        check.expect(text.contains("会自动用掉重置卡") && !text.contains("官方") && !text.contains("换号按接力顺序") && !text.contains("Fable"),
                     "只有 Codex 只重置：不提换号、Claude 的卡和 Fable")
        text = AutomationPage.confirmation([rule("claude-a", .resetOnly, on: false)], tool: tool)
        check.expect(text.contains("现在没有账号参与") && !text.contains("见底线"), "没有账号参与：只说现在什么都不会自动做")
        // The same answer from the service is not news: the directory is published only when it changes.
        let json = try! JSONSerialization.data(withJSONObject: Fixture.directory("default"))
        var other = Fixture.directory("default")
        other["busy"] = true
        let first = try! Fixture.decoder.decode(AccountDirectory.self, from: json)
        check.expect(first == (try! Fixture.decoder.decode(AccountDirectory.self, from: json)), "同一份目录解出来相等，轮询不会重复刷新界面")
        check.expect(first != (try! Fixture.decoder.decode(AccountDirectory.self, from: JSONSerialization.data(withJSONObject: other))),
                     "目录里有一处变了就不相等")
    }
    main.orderOut(nil)
    UserDefaults.standard.removeObject(forKey: "dismissedExpiryReminders")
    print(check.failures.isEmpty ? "全部通过" : "失败 \(check.failures.count) 项：\(check.failures)")
    return check.failures.count
}

@MainActor enum ClaudeLogin {
    static let link = "https://claude.com/cai/oauth/authorize?code=true&redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback&state=synthetic"
    // Adding a Claude account, waiting for the browser.
    static func waiting(_ state: ServiceController) {
        state.login = LoginAttempt(attemptId: "synthetic-claude-login", provider: "claude", phase: "waiting",
                                   message: "在浏览器里登录这个 Claude 账号，登录完会自动回到这里。",
                                   verificationUrl: "https://claude.com/cai/oauth/authorize?code=true&state=synthetic-manual",
                                   userCode: nil, browserUrl: link)
        state.loginTarget = LoginTarget(accountID: nil, cloudAccountID: nil)
    }
}

// MARK: Main-thread cost
// perf [directory.json] [rounds]: how long the main thread stays busy after each sidebar switch and after
// each service poll, per page. Busy time is what the run loop spends between waking and sleeping again, so
// it covers body evaluation, layout, drawing and the Core Animation commit. With a directory saved from
// the running service the numbers are for that Mac's own accounts; nothing is sent back to the service.
@MainActor enum Perf {
    static var wake = 0.0
    static var busy: [Double] = []

    static func watch() {
        let start = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.entry.rawValue | CFRunLoopActivity.afterWaiting.rawValue, true, -.max) { _, _ in
            MainActor.assumeIsolated { wake = CACurrentMediaTime() }
        }
        let end = CFRunLoopObserverCreateWithHandler(nil, CFRunLoopActivity.beforeWaiting.rawValue | CFRunLoopActivity.exit.rawValue, true, .max) { _, _ in
            MainActor.assumeIsolated {
                if wake > 0 { busy.append((CACurrentMediaTime() - wake) * 1000) }
                wake = 0
            }
        }
        CFRunLoopAddObserver(CFRunLoopGetMain(), start, .commonModes)
        CFRunLoopAddObserver(CFRunLoopGetMain(), end, .commonModes)
    }

    // Runs `change`, then lets the run loop work for `window` seconds: (longest single block, total), in ms.
    static func cost(_ window: Double = 0.35, _ change: () -> Void) -> (longest: Double, total: Double) {
        busy = []
        let began = CACurrentMediaTime()
        change()
        let own = (CACurrentMediaTime() - began) * 1000
        RunLoop.current.run(until: Date(timeIntervalSinceNow: window))
        // The change itself runs before the loop wakes; it belongs to the first block.
        let blocks = busy.isEmpty ? [own] : [busy[0] + own] + busy.dropFirst()
        return (blocks.max() ?? 0, blocks.reduce(0, +))
    }

    static func median(_ values: [Double]) -> Double { values.sorted()[values.count / 2] }

    static func run(_ live: String?, rounds: Int) -> Int {
        let state = ServiceController.shared
        if let live {
            state.directory = try! Fixture.decoder.decode(AccountDirectory.self, from: Data(contentsOf: URL(fileURLWithPath: live)))
            state.snapshot = try! Fixture.decoder.decode(Snapshot.self, from: Data(Fixture.snapshot.utf8))
        } else {
            Fixture.load("full", into: state)
        }
        UserDefaults.standard.set(true, forKey: AutomationPage.onboardedKey)
        var pages: [(String, Page)] = [("账号", .accounts), ("Codex", .codex), ("Claude", .claude), ("换号与重置", .automation),
                                      ("客户端与服务", .clients), ("个人云端", .cloud), ("网络", .network)]
        // PERF_ONLY=Claude,网络 keeps the run to those pages, for sampling one of them.
        if let only = ProcessInfo.processInfo.environment["PERF_ONLY"]?.split(separator: ",").map(String.init) {
            pages = pages.filter { only.contains($0.0) }
        }
        AppNavigation.shared.show(.clients)
        let (window, _) = makeWindow(DashboardView(), size: NSSize(width: 1020, height: 660))
        window.orderFrontRegardless()
        watch()
        Driver.settle(1)
        var first: [String: Double] = [:], again: [String: [Double]] = [:], total: [String: [Double]] = [:], poll: [String: [Double]] = [:]
        for round in 0...rounds {
            for (name, page) in pages {
                let switched = cost { AppNavigation.shared.show(page) }
                if round == 0 { first[name] = switched.longest } else { again[name, default: []].append(switched.longest); total[name, default: []].append(switched.total) }
                // What the 2-second poll does: the same directory, published again.
                let polled = cost(0.15) { state.directory = state.directory }
                if round > 0 { poll[name, default: []].append(polled.total) }
            }
        }
        let idle = (0..<5).map { _ in cost(0.15) {}.total }
        print("页面            首次切入  再次切入(中位/最大)  切入总计  一次轮询")
        for (name, _) in pages {
            let a = again[name]!, t = total[name]!, p = poll[name]!
            print(name.padding(toLength: 10, withPad: "　", startingAt: 0),
                  String(format: "%7.1f  %8.1f /%6.1f  %8.1f  %8.1f", first[name]!, median(a), a.max()!, median(t), median(p)))
        }
        print(String(format: "空转（什么都不做）%.1f ms；单位 ms，%d 轮", median(idle), rounds))
        window.orderOut(nil)
        return 0
    }
}

@main struct Preview {
    static func page(_ name: String) -> Page {
        ["accounts": .accounts, "codex": .codex, "claude": .claude, "automation": .automation,
         "clients": .clients, "cloud": .cloud, "network": .network][name]!
    }
    @MainActor static func main() {
        let args = CommandLine.arguments
        let app = ActiveApp.shared
        NSWindow.pretendKey()
        if args.count >= 2 && args[1] == "interact" {
            exit(Int32(interact()))
        }
        if args.count >= 2 && args[1] == "perf" {
            app.setActivationPolicy(.accessory)
            let live = args.count > 2 && args[2] != "-" ? args[2] : nil
            exit(Int32(Perf.run(live, rounds: args.count > 3 ? Int(args[3])! : 6)))
        }
        // dump [scenario] [page] [selection]: OCR lines with their window positions, for writing locators.
        if args.count >= 4 && args[1] == "same" {
            // Polls saved from a running service: how many decode to the same directory as the one before,
            // which is how many the app would not redraw for.
            let polls = args[2...].map { try! Fixture.decoder.decode(AccountDirectory.self, from: Data(contentsOf: URL(fileURLWithPath: $0))) }
            let same = zip(polls, polls.dropFirst()).filter { $0 == $1 }.count
            print("相邻两次轮询 \(polls.count - 1) 对，解出来相同 \(same) 对")
            exit(0)
        }
        if args.count >= 2 && args[1] == "dump" {
            app.setActivationPolicy(.accessory)
            Fixture.load(args.count > 2 ? args[2] : "default", into: ServiceController.shared)
            AppNavigation.shared.show(page(args.count > 3 ? args[3] : "accounts"), select: args.count > 4 ? args[4] : nil)
            let (window, _) = makeWindow(DashboardView(), size: NSSize(width: 1020, height: 765))
            window.orderFrontRegardless()
            Driver.settle(1)
            for hit in Driver.read(window) { print(String(format: "%5.0f %5.0f  ", hit.rect.minX, hit.rect.minY), hit.text) }
            try! capture(window).representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: NSTemporaryDirectory() + "accounts-preview-dump.png"))
            print("saved", NSTemporaryDirectory() + "accounts-preview-dump.png")
            exit(0)
        }
        if args.count >= 3 && args[1] == "classes" {
            // Lists the AppKit views behind one page, to find controls OCR cannot read.
            app.setActivationPolicy(.accessory)
            Fixture.load(args.count > 3 ? args[3] : "default", into: ServiceController.shared)
            AppNavigation.shared.show(page(args[2]))
            let (window, _) = makeWindow(DashboardView(), size: NSSize(width: 1020, height: 765))
            window.orderFrontRegardless()
            Driver.settle(1)
            var counts: [String: Int] = [:]
            for view in (window.contentView!.superview ?? window.contentView!).subviewsDeep {
                let name = String(describing: Swift.type(of: view))
                counts[name, default: 0] += 1
                if name.contains("Menu") || name.contains("PopUp") || name.contains("Button") || name.contains("KeyViewProxy") {
                    print(name.prefix(110), view.convert(view.bounds, to: nil).integral)
                }
            }
            for (name, count) in counts.sorted(by: { $0.value > $1.value }).prefix(25) { print(count, name.prefix(110)) }
            exit(0)
        }
        if args.count == 3 && args[1] == "imagerender" {
            // Draws a sheet with SwiftUI's own renderer, to tell window-capture artifacts from real ones.
            app.setActivationPolicy(.accessory)
            Fixture.load("default", into: ServiceController.shared)
            let nova = ServiceController.shared.directory!.accounts.first { $0.id == "synthetic-nova" }!
            let renderer = ImageRenderer(content: SwitchSheet(account: nova, close: {}).environmentObject(ServiceController.shared))
            renderer.scale = 2
            let rep = NSBitmapImageRep(cgImage: renderer.cgImage!)
            try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
            exit(0)
        }
        if args.count == 5 && args[1] == "live" {
            // Renders a masked directory saved from the running service; nothing is sent back to it.
            app.setActivationPolicy(.accessory)
            let state = ServiceController.shared
            state.directory = try! Fixture.decoder.decode(AccountDirectory.self, from: Data(contentsOf: URL(fileURLWithPath: args[2])))
            state.snapshot = try! Fixture.decoder.decode(Snapshot.self, from: Data(contentsOf: URL(fileURLWithPath: args[3])))
            let output = URL(fileURLWithPath: args[4], isDirectory: true)
            try! FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
            let claude = state.directory!.accounts.first { $0.provider == "claude" }?.id
            for (name, page, selection) in [("accounts", Page.accounts, nil), ("codex", .codex, nil), ("claude-home", .claude, nil), ("claude", .claude, claude), ("automation", .automation, nil)] {
                AppNavigation.shared.show(page, select: selection)
                _ = render(DashboardView(), size: NSSize(width: 1020, height: 660), appearance: "light", to: output.appendingPathComponent("live-\(name).png"))
            }
            _ = render(MenuBarPanel(openWindow: {}), size: nil, appearance: "light", to: output.appendingPathComponent("live-menubar.png"))
            print("Rendered live masked data to \(output.path)")
            exit(0)
        }
        precondition(args.count >= 2, "output directory required")
        app.setActivationPolicy(.accessory)
        let output = URL(fileURLWithPath: args[1], isDirectory: true)
        try! FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let state = ServiceController.shared
        let window = NSSize(width: 1020, height: 660)
        for appearance in ["light", "dark"] {
            let pages: [(String, String, Page, String?)] = [
                ("accounts", "default", .accounts, nil), ("accounts-attention", "attention", .accounts, nil), ("accounts-empty", "empty", .accounts, nil),
                ("accounts-connected", "connected", .accounts, nil), ("accounts-plain", "plain", .accounts, nil), ("accounts-claude2", "claude2", .accounts, nil),
                ("codex", "default", .codex, nil), ("codex-attention", "attention", .codex, nil), ("codex-cloud", "cloud", .codex, nil),
                ("codex-unmanaged", "unmanaged", .codex, nil), ("codex-selected", "connected", .codex, "synthetic-nova"),
                ("claude", "default", .claude, nil), ("claude-both", "claudeboth", .claude, nil), ("claude-code", "default", .claude, nil),
                ("claude-desktop", "claude2", .claude, nil), ("claude-account", "default", .claude, "synthetic-claude"),
                ("claude2", "claude2", .claude, "synthetic-claude-b"), ("claude-lab", "default", .claude, "h-lab"),
                ("claude-switchblock", "switchblock", .claude, "synthetic-claude-b"),
                ("claude-low", "claudelow", .claude, nil), ("claude-low5", "claudelow5", .claude, nil), ("claude-lowboth", "claudelowboth", .claude, nil),
                ("automation", "default", .automation, nil), ("automation-running", "running", .automation, nil), ("automation-hotoff", "hotoff", .automation, nil),
                ("automation-onboard", "off", .automation, nil), ("automation-onboard-empty", "offempty", .automation, nil),
                ("automation-on-hint", "default", .automation, nil), ("automation-drag", "default", .automation, "synthetic-nova"),
                ("automation-claude2", "claude2", .automation, "synthetic-claude"), ("automation-many", "many", .automation, nil),
                ("codex-connected", "connected", .codex, nil), ("codex-plain", "hotoff", .codex, nil), ("codex-cli", "hotoff", .codex, nil),
                ("codex-onboard", "fresh", .codex, nil), ("claude-onboard", "fresh", .claude, nil), ("codex-next", "codexone", .codex, nil),
                ("claude-desktop-behind", "desktopbehind", .claude, nil), ("claude-deskquota", "deskquota", .claude, nil),
                ("claude-desktop-switching", "desktopswitching", .claude, nil), ("claude-desktop-missing", "desktopmissing", .claude, nil),
                ("clients", "default", .clients, nil), ("cloud", "default", .cloud, nil),
                ("network", "default", .network, nil), ("network-safe", "netsafe", .network, nil),
                ("network-waiting", "netwaiting", .network, nil), ("network-wrong", "netwrong", .network, nil),
                // The page column at other window widths: 1020 is the default, 833 the smallest.
                ("claude-full", "full", .claude, nil), ("claude-full-833", "full", .claude, nil),
                ("claude-full-1300", "full", .claude, nil), ("claude-full-1600", "full", .claude, nil),
                ("codex-1300", "full", .codex, nil), ("automation-1300", "default", .automation, nil),
                ("accounts-1300", "default", .accounts, nil), ("clients-1300", "default", .clients, nil),
            ]
            let sizes: [String: NSSize] = ["claude-full-833": NSSize(width: 833, height: 600),
                                           "claude-full-1300": NSSize(width: 1300, height: 830), "claude-full-1600": NSSize(width: 1600, height: 900),
                                           "codex-1300": NSSize(width: 1300, height: 830), "automation-1300": NSSize(width: 1300, height: 830),
                                           "accounts-1300": NSSize(width: 1300, height: 830), "clients-1300": NSSize(width: 1300, height: 830)]
            // How this Mac uses Claude, for the pages drawn in one mode; the rest decide it themselves.
            let modes: [String: ClaudeUse] = ["claude-code": .code, "claude-desktop": .desktop]
            let codexModes: [String: CodexUse] = ["codex-plain": .both, "codex-cli": .cli]
            for (name, scenario, page, selection) in pages {
                Fixture.load(scenario, into: state)
                if let mode = modes[name] { UserDefaults.standard.set(mode.rawValue, forKey: ClaudeUse.key) } else { UserDefaults.standard.removeObject(forKey: ClaudeUse.key) }
                // Each page decides its own first run and way of use, except where the scenario pins one.
                for key in [CodexUse.key, CodexUse.hotKey, Onboarding.codexKey, Onboarding.claudeKey] { UserDefaults.standard.removeObject(forKey: key) }
                if let use = codexModes[name] { UserDefaults.standard.set(use.rawValue, forKey: CodexUse.key) }
                if name == "codex-next" { UserDefaults.standard.set(Onboarding.started, forKey: Onboarding.codexKey) }
                // The relay page's first-time card and its note after turning on, only where drawn.
                UserDefaults.standard.set(!name.hasPrefix("automation-onboard"), forKey: AutomationPage.onboardedKey)
                if name == "automation-on-hint" { UserDefaults.standard.set("show", forKey: AutomationPage.hintKey) } else { UserDefaults.standard.removeObject(forKey: AutomationPage.hintKey) }
                AutomationPage.previewDragging = name == "automation-drag" ? "synthetic-nova" : nil
                AppNavigation.shared.show(page, select: selection)
                if selection == nil { AppNavigation.shared.selection = nil }
                let size = render(DashboardView(), size: sizes[name] ?? window, appearance: appearance, to: output.appendingPathComponent("\(appearance)-\(name).png"))
                print("\(appearance)/\(name): \(size.width) × \(size.height)")
            }
            Fixture.load("default", into: state)
            let claude = state.directory!.accounts.first { $0.provider == "claude" }!
            let nova = state.directory!.accounts.first { $0.id == "synthetic-nova" }!
            let sheets: [(String, AnyView)] = [
                ("sheet-login", AnyView(LoginView(provider: "codex"))),
                ("sheet-switch-blocked", AnyView(SwitchSheet(account: nova, close: {}))),
                ("sheet-desktop-connection", AnyView(CodexConnectSheet(close: {}))),
                ("sheet-recover", AnyView(RecoverSaveSheet(account: claude, close: {}))),
                ("sheet-reset", AnyView(ResetSheet(account: nova, creditID: "credit-l1", requestID: "r", expiresAt: Fixture.iso(5 * 86400), retry: false, close: {}))),
                ("sheet-remove", AnyView(RemoveSheet(account: nova, close: {}))),
                ("sheet-rename", AnyView(RenameSheet(account: nova, close: {}))),
                ("history", AnyView(ResetHistoryView(accountID: nil))),
                ("menubar", AnyView(MenuBarPanel(openWindow: {}))),
                ("sheet-desktop-switch", AnyView(ClaudeSwitchSheet(request: ClaudeSwitchRequest(entryID: "synthetic-claude", code: false, desk: true, choice: false), close: {}, login: {}))),
                ("sheet-network-new", AnyView(NetworkSwitchSheet(initial: .wifi("Cafe Guest"), started: {}, close: {}))),
                ("sheet-network-phone", AnyView(NetworkSwitchSheet(initial: .phone("我的 iPhone"), started: {}, close: {}))),
            ]
            for (name, view) in sheets {
                let size = render(view, size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-\(name).png"))
                print("\(appearance)/\(name): \(size.width) × \(size.height)")
            }
            UserDefaults.standard.removeObject(forKey: ClaudeUse.key)
            Fixture.load("default", into: state)
            let consentSize = render(TokenQuotaSheet(close: {}), size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-token-quota.png"))
            print("\(appearance)/sheet-token-quota: \(consentSize.width) × \(consentSize.height)")
            let relayConsentSize = render(CodeRelaySheet(close: {}), size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-code-relay.png"))
            print("\(appearance)/sheet-code-relay: \(relayConsentSize.width) × \(relayConsentSize.height)")
            let relayUninstallSize = render(CodeRelayUninstallSheet(close: {}), size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-code-relay-uninstall.png"))
            print("\(appearance)/sheet-code-relay-uninstall: \(relayUninstallSize.width) × \(relayUninstallSize.height)")
            Fixture.load("coderelayon", into: state)
            AppNavigation.shared.show(.claude, select: nil)
            let relayOnSize = render(DashboardView(), size: window, appearance: appearance, to: output.appendingPathComponent("\(appearance)-claude-coderelayon.png"))
            print("\(appearance)/claude-coderelayon: \(relayOnSize.width) × \(relayOnSize.height)")
            Fixture.load("coderelaydeskonly", into: state)
            let noCodeSize = render(ClaudeSwitchSheet(request: ClaudeSwitchRequest(entryID: "desktop:h-nova", code: false, desk: true, choice: false), close: {}, login: {}),
                                    size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-desktop-switch-nocode.png"))
            print("\(appearance)/sheet-desktop-switch-nocode: \(noCodeSize.width) × \(noCodeSize.height)")
            // Both sides can move: the switch sheet offers 一起换 / 只换 Code / 只换桌面.
            Fixture.load("claudeboth", into: state)
            let choice = ClaudeSwitchSheet(request: ClaudeSwitchRequest(entryID: "synthetic-claude-b", code: true, desk: true, choice: true), close: {}, login: {})
            let choiceSize = render(choice, size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-claude-switch.png"))
            print("\(appearance)/sheet-claude-switch: \(choiceSize.width) × \(choiceSize.height)")
            // A saved Claude account with two kinds of reset card to choose from.
            Fixture.load("claude2", into: state)
            let claudeB = state.directory!.accounts.first { $0.id == "synthetic-claude-b" }!
            let resetSize = render(ResetSheet(account: claudeB, creditID: nil, requestID: "r", expiresAt: claudeB.nextReset?.expiresAt, retry: false, close: {}),
                                   size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-reset-claude.png"))
            print("\(appearance)/sheet-reset-claude: \(resetSize.width) × \(resetSize.height)")
            for (name, scenario) in [("sheet-network-waiting", "netwaiting"), ("sheet-network-wrong", "netwrong")] {
                Fixture.load(scenario, into: state)
                let size = render(NetworkProgressSheet(close: {}, retry: { _ in }), size: nil, appearance: appearance,
                                  to: output.appendingPathComponent("\(appearance)-\(name).png"))
                print("\(appearance)/\(name): \(size.width) × \(size.height)")
            }
            Fixture.load("netwaiting", into: state)
            _ = render(MenuBarPanel(openWindow: {}), size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-menubar-network-waiting.png"))
            Fixture.load("default", into: state)
            ClaudeLogin.waiting(state)
            let loginSize = render(LoginView(provider: "claude").background(Palette.card), size: nil, appearance: appearance,
                                   to: output.appendingPathComponent("\(appearance)-sheet-login-claude.png"))
            print("\(appearance)/sheet-login-claude: \(loginSize.width) × \(loginSize.height)")
            state.login = nil
            state.loginTarget = nil
            Fixture.load("connected", into: state)
            let switchSize = render(SwitchSheet(account: state.directory!.accounts.first { $0.id == "synthetic-nova" }!, close: {}).background(Palette.card),
                                    size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-switch-desktop.png"))
            print("\(appearance)/sheet-switch-desktop: \(switchSize.width) × \(switchSize.height)")
            for (name, scenario, use) in [("sheet-desktop-connection-on", "connected", CodexUse.both), ("sheet-desktop-connection-unmanaged", "unmanaged", .both),
                                          ("sheet-switch-takeover", "unmanaged", .both), ("sheet-switch-plain", "hotoff", .both), ("sheet-switch-cli", "hotoff", .cli)] {
                UserDefaults.standard.set(use.rawValue, forKey: CodexUse.key)
                Fixture.load(scenario, into: state)
                let view = name.hasPrefix("sheet-desktop") ? AnyView(CodexConnectSheet(close: {}))
                    : AnyView(SwitchSheet(account: state.directory!.accounts.first { $0.id == "synthetic-nova" }!, close: {}))
                let size = render(view.background(Palette.card), size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-\(name).png"))
                print("\(appearance)/\(name): \(size.width) × \(size.height)")
            }
            UserDefaults.standard.removeObject(forKey: CodexUse.key)
            Fixture.load("claude2", into: state)
            let claudeSize = render(SwitchSheet(account: state.directory!.accounts.first { $0.id == "synthetic-claude-b" }!, close: {}).background(Palette.card),
                                    size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-switch-claude.png"))
            print("\(appearance)/sheet-switch-claude: \(claudeSize.width) × \(claudeSize.height)")
            // The service is gone: nothing it published stays on screen.
            state.isConnected = false
            state.directory = nil
            state.errorText = "后台服务已退出，账号和额度不再更新。重新连接后台服务后恢复。"
            AppNavigation.shared.show(.accounts)
            _ = render(DashboardView(), size: window, appearance: appearance, to: output.appendingPathComponent("\(appearance)-accounts-offline.png"))
            _ = render(MenuBarPanel(openWindow: {}), size: nil, appearance: appearance,
                       to: output.appendingPathComponent("\(appearance)-menubar-offline.png"))
            state.isConnected = true
            state.errorText = nil
            print("\(appearance)/offline")
        }
        print("Rendered with synthetic data only")
    }
}
