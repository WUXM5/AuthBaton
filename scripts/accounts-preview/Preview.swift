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
    func clearAccountError() { accountError = nil; accountErrorID = nil }
    func refresh() async {}
    func restart() async { calls.append("restart {}") }
    func checkQuota(id: String? = nil) async { calls.append("accounts.quota \(id ?? "all")") }
    func refreshQuotaIfNeeded() async {}
    func resetHistory(accountID: String?, before: String? = nil) async throws -> ResetHistoryPage {
        try Fixture.decoder.decode(ResetHistoryPage.self, from: Data(#"{"entries":[{"id":"s-1:0","account_id":"synthetic-p","account_label":"Codex · 个人","source":"automatic","started_at":1790179200,"finished_at":1790179202,"outcome":"reset","message":"已用 1 张重置卡；用量和剩余张数将重新查询","trigger":{"remaining_percent":1.2,"threshold_percent":2},"retry":false},{"id":"s-2:0","account_id":"synthetic-p","account_label":"Codex · 个人","source":"legacy","started_at":1790089000,"finished_at":null,"outcome":"reset","message":"旧版本的记录没保存来源和结果时间","trigger":null,"retry":false}],"next_cursor":null,"total":2}"#.utf8))
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
             "warmup": ["enabled": true, "hour": 6, "running": false, "last": ["at": now - 3.2 * 3600, "ok": true, "message": "已发送，5 小时窗口开始计时"]],
             "login_notice": ["state": "normal", "expires_at": now + 6.7 * 86400, "message": "当前不在重新登录提醒窗口内", "source": "本机登录记录"],
             "usage": ["state": "ready", "message": "额度已检查", "checked_at": now - 200, "attempted_at": now - 201, "next_update": now + 100, "refreshing": false,
                       "windows": [["label": "5 小时窗口", "used_percent": 14, "resets_at_iso": iso(3.7 * 3600)],
                                   ["label": "独立窗口", "used_percent": 0, "scope": "feature_x", "kind": "feature"],
                                   ["label": "7 天窗口", "used_percent": 70, "resets_at_iso": iso(26.4 * 3600)],
                                   ["label": "7 天窗口", "used_percent": 0, "scope": "ModelY", "kind": "model", "resets_at_iso": iso(26.4 * 3600)]],
                       "claude_resets": ["grants": ["eligible": true, "message": "官方提供的重置卡", "remaining": 1, "weekly_resets_at": iso(26.4 * 3600),
                                                    "items": [["label": "合成赠送：一次额度重置", "remaining": 1, "total": 1, "starts_at": iso(-7 * 86400), "ends_at": iso(24 * 86400),
                                                               "limits": ["5 小时额度", "每周额度"], "paused": false, "usable_now": true, "requires_limit": false]]]]]],
        ]
        accounts[1]["reset"] = ["request_id": "synthetic-req", "credit_id": "synthetic-used", "expires_at": iso(20 * 86400), "outcome": "reset",
                                "message": "已用 1 张重置卡；用量和剩余张数将重新查询", "unresolved": false, "started_at": now - 5 * 86400]
        var data: [String: Any] = [
            "accounts": accounts, "busy": false, "checks_blocked": false, "recovery_needed": false, "maintenance_running": true, "notice": NSNull(),
            "desktop": ["enabled": true, "port": 9522, "connected": false, "managed": false, "busy": false, "message": "调试端口未监听；请使用调试模式启动 Codex 桌面"],
            "cloud": ["mode": "local", "endpoint": "", "label": "我的 Mac", "paired": false, "accounts": [], "devices": [], "selected": [:], "busy": false, "transfers": []],
            "strategy": ["config": ["enabled": true, "accounts": [
                ["id": "synthetic-orbit", "order": "switch_first", "participate": true, "below": 3],
                ["id": "synthetic-p", "order": "reset_first", "participate": true, "below": 2],
                ["id": "synthetic-team", "order": "switch_only", "participate": true, "below": 3],
                ["id": "synthetic-nova", "order": "switch_only", "participate": true, "below": 3]]],
                         "busy": false, "message": "正在独立监测 4 个账号；重置无需桌面连接", "phase": NSNull(), "run": NSNull(), "last_at": NSNull(),
                         "account_states": [["id": "synthetic-orbit", "message": "额度充足，高于见底线 3%"],
                                            ["id": "synthetic-p", "message": "额度充足，高于见底线 2%"]]],
        ]
        switch scenario {
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
        case "empty":
            data["accounts"] = []
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
    static let snapshot = #"{"checked_at":1790649075,"service_version":"0.15.0","verification":"仅检查本地凭据和 Claude CLI 登录状态，未请求模型，也未验证刷新能力。","providers":[{"id":"codex","name":"Codex","state":"configured","auth_kind":"ChatGPT OAuth","summary":"本地 access token 尚未到期","source":"~/.codex/auth.json","expires_at":1790778068,"details":[]},{"id":"claude","name":"Claude Code","state":"configured","auth_kind":"Claude 订阅","summary":"Claude CLI 报告已登录","source":"/opt/homebrew/bin/claude","expires_at":null,"details":[]},{"id":"pi","name":"pi · OpenAI Codex","state":"configured","auth_kind":"BatonPass 本机服务","summary":"已安装统一账号插件","source":"~/.pi/agent/auth.json","expires_at":null,"details":[]}]}"#

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
        for (from, to) in [("～", "~"), ("：", ":"), ("，", ","), ("（", "("), ("）", ")"), ("…", "..."), ("「", ""), ("」", ""),
                           ("·", "."), ("・", "."), ("•", "."), ("；", ";"), ("？", "?"), ("！", "!")] {
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
        ReminderModel.shared.reload()
        AppNavigation.shared.selection = nil
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
        Driver.press("以调试模式重启并换上", .exact)
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
                     && Driver.lastMenu.contains { $0.title == "立即查询额度" } && Driver.lastMenu.contains { $0.title == "执行记录" },
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
        select(.claude)
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
        check.expect(Driver.visible("从左往右接力") && Driver.visible("只在 Claude 账号之间换号"), "Codex 和 Claude 分成两条接力线")
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
    section(7, "Codex 桌面页：启动与关闭热切换；已连接但没接管时写明并给出接管") {
        Fixture.load("unmanaged", into: state)
        select(.desktop)
        check.expect(Driver.visible("已连接 · 未接管"), "已连接但没接管时写明「已连接 · 未接管」")
        select(.codex)
        check.expect(Driver.visible("交给 BatonPass"), "Codex 页在用的卡片说明要接管")
        check.expect(Driver.near("orbit", "接管") != nil, "在用的卡片给出「接管」")
        select(.accounts)
        check.expect(Driver.visible("交给 BatonPass") && !Driver.hits("接管", .exact).isEmpty, "账号页在用的卡片说明并给出「接管」")
        Fixture.load("connected", into: state)
        select(.desktop)
        check.expect(Driver.visible("已连接 · 已接管"), "接管后写明「已连接 · 已接管」")
        Fixture.load("default", into: state)
        select(.desktop)
        state.calls.removeAll()
        Driver.press("以调试模式重新打开")
        check.expect(check.lastCall("accounts.desktop_launch") != nil, "「以调试模式重新打开」调用 accounts.desktop_launch")
        if let toggle = Driver.views(NSSwitch.self).first {
            Driver.click(toggle)
            check.expect(check.lastCall("accounts.desktop")?.contains("\"enabled\":false") == true, "关闭热切换开关发送 enabled:false")
        } else {
            check.expect(false, "找到热切换开关")
        }
    }
    section(8, "个人云端：切换模式需要确认") {
        select(.cloud)
        state.calls.removeAll()
        Driver.press("云端", .exact)
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
    section(11, "Claude 换号：不用退出会话，确认后调用 accounts.activate") {
        Fixture.load("claude2", into: state)
        select(.claude, "synthetic-claude-b")
        Driver.press("换上", .exact)
        check.expect(Driver.visible("不用退出") && Driver.visible("按一次回车继续"), "换号框写明不用退出、被额度打断的会话回车继续")
        check.expect(!Driver.visible("已结束任务"), "Claude 换号不要求先结束任务")
        state.calls.removeAll()
        Driver.press("换上", .exact)
        let activate = check.lastCall("accounts.activate") ?? ""
        check.expect(activate.contains("synthetic-claude-b") && activate.contains("\"confirm\":true"), "确认后调用 accounts.activate(id, confirm:true)")
        check.expect(main.attachedSheet == nil, "Claude 换号完成后框关闭")
    }
    section(12, "Claude 续期没收到回应：自动重新确认，不要求人处理") {
        Fixture.load("uncertain", into: state)
        select(.claude)
        check.expect(Driver.visible("等待重新确认"), "Claude 页写明「等待重新确认」")
        check.expect(Driver.visible("自动用同一凭据重新确认"), "说明会自动重新确认")
        check.expect(!state.directory!.issues(dismissed: []).contains { $0.account == "synthetic-claude" }, "不计入要处理的事")
        check.expect(Driver.hits("恢复同步").isEmpty, "不给恢复同步按钮")
        more("Claude 当前账号", choose: nil)
        check.expect(Driver.lastMenu.contains { $0.title.contains("认证检查") && $0.enabled }, "可手动「认证检查」立即重新确认")
        if Driver.lastMenu.isEmpty { print("    菜单：\(Driver.lastMenu)") }
    }
    section(13, "Claude 5 小时窗口预热：开关沿用关闭前的时间，已开的显示时间和上次结果") {
        select(.claude)
        check.visible("06:00", "已开启的账号显示预热时间")
        check.visible("5小时窗口开始计时", "显示上次预热结果")
        Fixture.load("claude2", into: state)
        select(.claude, "synthetic-claude-b")
        check.expect(!Driver.visible("09:00", wait: 0.3), "关闭时不显示预热时间")
        state.calls.removeAll()
        if let label = Driver.hits("到点发一条短消息").first,
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
        check.expect(Driver.visible("等待重新确认"), "写明在等什么")
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
        select(.automation)
        state.calls.removeAll()
        // The editor's arrows sit right of its title; orbit is first, so only "往后移一位" is enabled.
        if let (window, point) = Driver.controls(beside: "orbit", size: 22).last {
            Driver.click(window, point)
            check.visible("未保存", "移动后出现保存条")
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
    main.orderOut(nil)
    UserDefaults.standard.removeObject(forKey: "dismissedExpiryReminders")
    print(check.failures.isEmpty ? "全部通过" : "失败 \(check.failures.count) 项：\(check.failures)")
    return check.failures.count
}

@main struct Preview {
    static func page(_ name: String) -> Page {
        ["accounts": .accounts, "codex": .codex, "claude": .claude, "automation": .automation, "desktop": .desktop,
         "clients": .clients, "cloud": .cloud][name]!
    }
    @MainActor static func main() {
        let args = CommandLine.arguments
        let app = ActiveApp.shared
        NSWindow.pretendKey()
        if args.count >= 2 && args[1] == "interact" {
            exit(Int32(interact()))
        }
        // dump [scenario] [page] [selection]: OCR lines with their window positions, for writing locators.
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
            for (name, page, selection) in [("accounts", Page.accounts, nil), ("codex", .codex, nil), ("claude", .claude, claude), ("automation", .automation, nil), ("desktop", .desktop, nil)] {
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
                ("claude", "default", .claude, nil), ("claude2", "claude2", .claude, "synthetic-claude-b"), ("claude-switchblock", "switchblock", .claude, "synthetic-claude-b"),
                ("automation", "default", .automation, nil), ("automation-running", "running", .automation, nil), ("automation-hotoff", "hotoff", .automation, nil),
                ("automation-claude2", "claude2", .automation, "synthetic-claude"), ("automation-many", "many", .automation, nil),
                ("desktop", "default", .desktop, nil), ("desktop-connected", "connected", .desktop, nil), ("desktop-off", "hotoff", .desktop, nil),
                ("clients", "default", .clients, nil), ("cloud", "default", .cloud, nil),
            ]
            for (name, scenario, page, selection) in pages {
                Fixture.load(scenario, into: state)
                AppNavigation.shared.show(page, select: selection)
                if selection == nil { AppNavigation.shared.selection = nil }
                let size = render(DashboardView(), size: window, appearance: appearance, to: output.appendingPathComponent("\(appearance)-\(name).png"))
                print("\(appearance)/\(name): \(size.width) × \(size.height)")
            }
            Fixture.load("default", into: state)
            let claude = state.directory!.accounts.first { $0.provider == "claude" }!
            let nova = state.directory!.accounts.first { $0.id == "synthetic-nova" }!
            let sheets: [(String, AnyView)] = [
                ("sheet-login", AnyView(LoginView(provider: "codex"))),
                ("sheet-switch-blocked", AnyView(SwitchSheet(account: nova, close: {}))),
                ("sheet-recover", AnyView(RecoverSaveSheet(account: claude, close: {}))),
                ("sheet-reset", AnyView(ResetSheet(account: nova, creditID: "credit-l1", requestID: "r", expiresAt: Fixture.iso(5 * 86400), retry: false, close: {}))),
                ("sheet-remove", AnyView(RemoveSheet(account: nova, close: {}))),
                ("sheet-rename", AnyView(RenameSheet(account: nova, close: {}))),
                ("history", AnyView(ResetHistoryView(accountID: nil))),
                ("menubar", AnyView(MenuBarPanel(openWindow: {}))),
            ]
            for (name, view) in sheets {
                let size = render(view, size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-\(name).png"))
                print("\(appearance)/\(name): \(size.width) × \(size.height)")
            }
            Fixture.load("connected", into: state)
            let switchSize = render(SwitchSheet(account: state.directory!.accounts.first { $0.id == "synthetic-nova" }!, close: {}).background(Palette.card),
                                    size: nil, appearance: appearance, to: output.appendingPathComponent("\(appearance)-sheet-switch-desktop.png"))
            print("\(appearance)/sheet-switch-desktop: \(switchSize.width) × \(switchSize.height)")
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
