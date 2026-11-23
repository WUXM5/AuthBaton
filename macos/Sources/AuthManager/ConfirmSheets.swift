import SwiftUI

// Routes an AccountSheet to its view. Each sheet clears stale errors on open, so an old
// failure never shows up inside an unrelated dialog.
struct AccountSheetView: View {
    @EnvironmentObject private var service: ServiceController
    let item: AccountSheet
    let close: () -> Void

    var body: some View {
        Group {
            switch item {
            case .login(let provider, let id):
                LoginView(provider: provider, account: account(id))
            case .switchTo(let id):
                if let account = account(id) { SwitchSheet(account: account, close: close) }
            case .reset(let id, let credit, let request, let expires, let retry):
                if let account = account(id) {
                    ResetSheet(account: account, creditID: credit, requestID: request, expiresAt: expires, retry: retry, close: close)
                }
            case .remove(let id):
                if let account = account(id) { RemoveSheet(account: account, close: close) }
            case .recover(let id):
                if let account = account(id) { RecoverSaveSheet(account: account, close: close) }
            case .rename(let id):
                if let account = account(id) { RenameSheet(account: account, close: close) }
            case .history(let id):
                ResetHistoryView(accountID: id)
            case .strategyStop:
                StrategyStopSheet(close: close)
            case .desktopConnection:
                CodexConnectSheet(close: close)
            }
        }
        .environmentObject(service)
        .onAppear { if service.login?.isActive != true { service.clearAccountError() } }
    }

    private func account(_ id: String?) -> ManagedAccount? {
        guard let id else { return nil }
        return service.directory?.accounts.first { $0.id == id }
    }
}

// Runs one operation and closes only when the service accepted it.
@MainActor
private func submit(_ service: ServiceController, _ method: String, _ params: LoginParams, close: @escaping () -> Void) async {
    await service.accountOperation(method, params: params)
    if service.accountError == nil { close() }
}

private struct SheetError: View {
    @EnvironmentObject private var service: ServiceController
    var body: some View {
        if let error = service.accountError { Banner(text: error) }
    }
}

// MARK: 换上

struct SwitchSheet: View {
    @EnvironmentObject private var service: ServiceController
    @AppStorage(CodexUse.key) private var useSetting = "auto"
    let account: ManagedAccount
    let close: () -> Void
    @State private var working = false
    @State private var since: Double?
    // In the no-connection sheet: true picks 关闭热切换, false (the default) restarting the desktop.
    @State private var plainChoice = false

    private var directory: AccountDirectory? { service.directory }
    private var route: SwitchRoute { directory?.switchRoute(for: account) ?? .plain }
    private var from: ManagedAccount? { directory?.current(account.provider) }
    private var attempt: DesktopConnection.Action? { since.flatMap { directory?.desktopAttempt(for: account.id, since: $0) } }
    private var use: CodexUse { directory.map { CodexUse.resolve(useSetting, $0) } ?? .both }
    private var piFollows: Bool { service.snapshot?.piFollows == true }

    var body: some View {
        Group {
            if route == .desktopDisconnected { blocked } else { confirm }
        }
        .onChange(of: attempt?.state) { _, state in if state == "complete" { close() } }
    }

    // Without the v0.14 managed CLI, running Claude clients pick the new login up within 30 s.
    private var claudeInPlace: Bool { account.provider == "claude" && route == .plain }
    // Switching to another account while the desktop still runs on its own login takes it over on the way.
    private var takesOver: Bool {
        guard route == .desktop, !account.native, let desktop = directory?.desktop, let from else { return false }
        return desktop.connected && desktop.selected != from.id
    }

    private var title: String {
        if account.native { return "让 Codex 桌面用「\(account.displayName)」？" }
        if takesOver { return "先接管桌面，再换到「\(account.displayName)」" }
        return "把 \(account.providerName) 换到「\(account.displayName)」？"
    }

    private var lead: String {
        if claudeInPlace { return "运行中的会话约 30 秒内改用新账号；在那之前发出的请求仍用原账号完成。" }
        if account.native { return "账号不变，改由 AuthBaton 给桌面提供登录；之后换号不用退出桌面。" }
        if takesOver { return "桌面已连上，但还在用自己的登录。接管后改由 AuthBaton 托管，原账号会保留。" }
        if account.provider == "codex" && route == .plain { return "仅改写 ~/.codex/auth.json，不会自动重启客户端。" }
        return "新会话会用新账号；正在跑的请求可能中断。"
    }

    private var go: String {
        if working || attempt?.pending == true { return account.native ? "正在接管…" : "正在换…" }
        if attempt?.state == "failed" { return "重试" }
        if claudeInPlace { return "换上" }
        if account.native { return "已结束任务，接管" }
        if takesOver { return "接管并换上" }
        return route == .desktop ? "已结束任务，换上" : "已退出，换上"
    }

    private var confirm: some View {
        SheetScaffold(title: title, lead: lead) {
            // Taking over keeps the account; only who supplies the desktop's login changes.
            if !account.native { Handoff(from: from, to: account, blocked: takesOver ? "尚未接管" : nil) }
            VStack(alignment: .leading, spacing: 9) {
                switch route {
                case .desktop:
                    if takesOver {
                        CheckItem(kind: .system, text: "接管后立刻换到「\(account.displayName)」，之后换号不用退出桌面。")
                    } else if !account.native {
                        // Taking over the account in use changes no account; the lead already says what it does.
                        CheckItem(kind: .system, text: "同时更新 Codex 桌面和 ~/.codex/auth.json" + (piFollows ? "，pi 新任务跟着换。" : "。"))
                    }
                    CheckItem(kind: .you, text: use == .desktop ? "先等桌面里正在跑的任务结束；桌面窗口不用退出。"
                                                              : "先结束桌面里的任务，退出独立的 Codex CLI，停用 VS Code 插件；桌面窗口可以开着。")
                default:
                    if account.provider == "codex" {
                        CheckItem(kind: .system, text: "更新 ~/.codex/auth.json" + (piFollows ? "；pi 的新任务跟着换。" : "。"))
                        CheckItem(kind: .you, text: use.desk ? "请先结束任务并退出 Codex 桌面和 CLI，停用 VS Code 插件，换完再打开。"
                                                             : "请先退出正在运行的 Codex CLI，停用 VS Code 的 Codex 插件，换完再打开。")
                    } else {
                        CheckItem(kind: .system, text: "更新 Claude Code 的默认登录，不用退出 Claude CLI 或 Agent SDK。")
                        CheckItem(kind: .you, text: "已被额度打断的会话，换上 30 秒后在原会话按一次回车继续。")
                    }
                }
            }
            if let attempt {
                if attempt.pending {
                    HStack(spacing: 7) {
                        ProgressView().controlSize(.small)
                        Text(attempt.message).font(.system(size: 11)).foregroundStyle(Palette.ink2)
                    }
                } else if attempt.state == "failed" {
                    Banner(glyph: .desktop, text: attempt.message)
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button(go) {
                working = true
                let desktop = route == .desktop
                // The service stamps attempts in whole seconds.
                let started = Date().timeIntervalSince1970.rounded(.down) - 1
                Task {
                    await service.accountOperation("accounts.activate", params: LoginParams(id: account.id, confirm: true))
                    working = false
                    guard service.accountError == nil else { return }
                    // A desktop switch only starts here; stay open until the desktop reports back.
                    if desktop { since = started } else { close() }
                }
            }
            .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
            .disabled(working || attempt?.pending == true || service.accountBusy || directory?.switchDisabled(for: account) == true)
        }
    }

    // Hot switch is on but the desktop is not reachable: the service will not start a switch. Two ways on, one
    // button that follows the pick; 关闭热切换 turns this sheet into the plain switch's confirmation.
    private var blocked: some View {
        let desktop = directory?.desktop
        let restarting = working || attempt?.pending == true
        return SheetScaffold(title: "现在还换不到「\(account.displayName)」",
                             lead: "热切换需要连上 Codex 桌面；现在没连上，选一种方式继续。") {
            Handoff(from: from, to: account, blocked: attempt?.message ?? desktop?.message ?? "桌面没连上")
            VStack(alignment: .leading, spacing: 9) {
                choice(!plainChoice, title: "以调试模式重启桌面并换上", recommended: true,
                       detail: "桌面没有任务运行时，自动重启并以调试模式连上后直接换号；有任务在跑不会强退。") { plainChoice = false }
                if let attempt, attempt.pending {
                    HStack(spacing: 5) {
                        ProgressView().controlSize(.mini)
                        Text(attempt.message).font(.system(size: 11)).foregroundStyle(Palette.ink2)
                    }
                    .padding(.leading, 14)
                }
                choice(plainChoice, title: "关闭热切换，改用普通切换", recommended: false,
                       detail: "仅改写 auth.json，换号前要自己退出桌面和 CLI。之后 Codex 见底时不再自动换号。") { plainChoice = true }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            if plainChoice {
                Button("关闭热切换并继续") {
                    UserDefaults.standard.set(false, forKey: CodexUse.hotKey)
                    Task { await service.accountOperation("accounts.desktop", params: LoginParams(enabled: false)) }
                }
                .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                .disabled(service.accountBusy || desktop?.busy == true)
            } else {
                Button(restarting ? "正在重启…" : attempt?.state == "failed" ? "重试" : "重启桌面并换上") {
                    working = true
                    let started = Date().timeIntervalSince1970.rounded(.down) - 1
                    Task {
                        await service.accountOperation("accounts.desktop_relaunch_switch", params: LoginParams(id: account.id))
                        working = false
                        // The restart and the switch report back later; stay open until they do.
                        if service.accountError == nil { since = started }
                    }
                }
                .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                .disabled(restarting || service.accountBusy || desktop?.busy == true)
            }
        }
    }

    private func choice(_ on: Bool, title: String, recommended: Bool, detail: String, pick: @escaping () -> Void) -> some View {
        Button(action: pick) {
            HStack(alignment: .top, spacing: 10) {
                VStack(alignment: .leading, spacing: 3.5) {
                    HStack(spacing: 7) {
                        Text(title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
                        if recommended { Pill("推荐", style: .brand, small: true) }
                    }
                    Text(detail).font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                ZStack {
                    Circle().strokeBorder(on ? Palette.brand : Palette.dashStrong, lineWidth: on ? 0 : 1.2)
                    if on {
                        Circle().fill(Palette.brand)
                        Icon(.check, size: 8, color: .white, weight: 2.6)
                    }
                }
                .frame(width: 15, height: 15)
            }
            .padding(.horizontal, 14).padding(.vertical, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(on ? Palette.card : Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(on ? Palette.brand : Color.clear, lineWidth: 1.5))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

// Who is in use now and who takes over: a solid line when the switch can happen, an amber dashed one
// with the reason when it cannot.
private struct Handoff: View {
    let from: ManagedAccount?
    let to: ManagedAccount
    let blocked: String?

    var body: some View {
        HStack(spacing: 0) {
            side(from, caption: blocked == nil ? "现在" : "在用", style: blocked == nil ? .standby : .inUse)
            Group {
                if let blocked {
                    VStack(spacing: 3.5) {
                        RelayLine(style: .blocked, arrow: false)
                        Text(blocked).font(.system(size: 10)).foregroundStyle(Palette.attentionText)
                            .lineLimit(2).multilineTextAlignment(.center).help(blocked)
                    }
                } else {
                    RelayLine(style: .open, thickness: 3)
                }
            }
            .padding(.horizontal, 10)
            .frame(maxWidth: .infinity)
            side(to, caption: blocked == nil ? "换上后" : "想换上", style: blocked == nil ? .inUse : .standby)
        }
        .padding(.horizontal, 14).padding(.vertical, 12)
        .loopPanel()
    }

    private func side(_ account: ManagedAccount?, caption: String, style: Seal.Style) -> some View {
        HStack(spacing: 9) {
            Seal(account?.displayName ?? "?", style: account == nil ? .out : style, size: 29)
            VStack(alignment: .leading, spacing: 0) {
                Text(accountName: account?.displayName ?? "没有在用的账号").font(.system(size: 12, weight: .semibold)).lineLimit(1)
                Text(caption + (account?.lowestPrimary.map { " · 剩余 \($0)%" } ?? "")).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    .monospacedDigit().lineLimit(1)
            }
        }
        .fixedSize()
    }
}

// MARK: 用重置卡

struct ResetSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let creditID: String?
    let requestID: String
    let expiresAt: String?
    let retry: Bool
    let close: () -> Void
    @State private var working = false
    @State private var chosen: String? = nil

    // A new Claude reset spends the kind chosen here, by default the official next card's kind.
    private var offer: ClaudeResetOffer? {
        guard account.provider == "claude", !retry else { return nil }
        return account.claudeOffers.first { $0.kind == chosen } ?? account.claudeDefaultOffer
    }

    var body: some View {
        SheetScaffold(title: retry ? "重试「\(account.displayName)」上次的重置？" : "给「\(account.displayName)」用 1 张重置卡？",
                      lead: retry ? "会重试同一次操作，不会换用另一张卡。如果上次没生效，这次可能用掉 1 张。"
                                  : offer != nil ? "成功后这张卡适用的额度回满，这个账号的重置卡少 1 张。能不能重置以官方返回为准。"
                                  : "成功后额度回满，这个账号的重置卡少 1 张。能不能重置以官方返回为准。") {
            if let offer, account.claudeOffers.count > 1 {
                PillSegments(segments: account.claudeOffers.map { .init(value: $0.kind, title: ClaudeResetKind.name($0.kind)) },
                             selection: offer.kind, select: { chosen = $0 }, fill: true)
            }
            ticket
            if !retry, let remaining = refilled, remaining > 50 {
                HStack(alignment: .top, spacing: 9) {
                    Icon(.alert, size: 14, color: Palette.attention).padding(.top, 1)
                    (Text("\(offer?.kind == "fable" ? "Fable 额度" : "额度")还剩 \(remaining)%。").fontWeight(.semibold).foregroundStyle(Palette.attentionText)
                     + Text(offer?.kind == "fable" ? "现在用会浪费大部分额度。" : "现在用会浪费大部分额度；自动规则一般在见底时才用。").foregroundStyle(Palette.attentionInk))
                        .font(.system(size: 11)).fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, 12).padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button(working ? "正在重置…" : retry ? "确认重试" : "用 1 张") {
                working = true
                Task {
                    await submit(service, "accounts.reset_quota",
                                 LoginParams(id: account.id, confirm: true, credit_id: creditID,
                                             next_grant: !retry && account.provider == "claude" ? true : nil,
                                             reset_kind: offer?.kind, reset_id: requestID), close: close)
                    working = false
                }
            }
            .buttonStyle(.pill(.primary, height: 27))
            .disabled(working || service.accountBusy || (!retry && account.provider == "claude" && offer == nil))
        }
    }

    // What is left of the windows the chosen card refills: the fuller they are, the more a reset wastes.
    // A 5 小时 or 每周 card refills that window, a Fable card the Fable week, any other card every main window.
    private var refilled: Int? {
        func main(_ name: String) -> Int? { account.primaryWindows.first { $0.label.hasPrefix(name) }?.remaining }
        switch offer?.kind {
        case "session": return main("5 小时") ?? account.lowestPrimary
        case "weekly": return main("7 天") ?? account.lowestPrimary
        case "fable": return account.extraWindows.first(where: \.isFable)?.remaining
        default: return account.lowestPrimary
        }
    }

    // The card being spent, drawn as a ticket: which one on the left, how many stay on the right.
    private var ticket: some View {
        // Listed Codex credits, or Claude's official count; nil when Claude's count did not come back.
        let count = account.provider == "claude" ? account.creditCount : account.codexCredits.count
        return HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                Text(retry ? "重试上次操作" : offer.map { ClaudeResetKind.phrase("用这张", $0.kind) } ?? "用这张")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink2)
                Text((offer.map(\.endsAt) ?? expiresAt).map { "\(Self.expiry($0)) 到期" } ?? (account.provider == "claude" ? "Claude 官方重置卡" : "到期时间没返回"))
                    .font(.system(size: 13, weight: .semibold)).monospacedDigit()
                if let offer {
                    Text(offer.limits.isEmpty ? "适用额度没返回" : "适用 \(offer.limits.joined(separator: "、"))")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                } else if !retry, let count {
                    Text("\(account.displayName) 现在有 \(count) 张").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            }
            .padding(.leading, 20).padding(.trailing, 15).padding(.vertical, 14)
            .frame(maxWidth: .infinity, alignment: .leading)
            if !retry, let count {
                Line().stroke(Palette.soft, style: StrokeStyle(lineWidth: 2, dash: [4, 3])).frame(width: 2).padding(.vertical, 10)
                VStack(alignment: .leading, spacing: 0) {
                    Text("用后还剩").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                    HStack(alignment: .firstTextBaseline, spacing: 3.5) {
                        Text("\(max(0, count - 1))").font(.system(size: 26, weight: .semibold, design: .rounded)).monospacedDigit()
                        Text("张").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                    }
                }
                .padding(.leading, 15).padding(.trailing, 20).padding(.vertical, 14)
                .frame(width: 109, alignment: .leading)
            }
        }
        .background(Palette.wash, in: TicketShape(radius: 12, notch: 7.5))
        .overlay(TicketShape(radius: 12, notch: 7.5).stroke(Palette.soft, lineWidth: 1))
    }

    private static func expiry(_ value: String) -> String {
        guard let date = QuotaResetCredit.date(value) else { return QuotaResetCredit.expiryLabel(value) }
        return When.day(date) + String(format: " %02d:%02d", Calendar.current.component(.hour, from: date), Calendar.current.component(.minute, from: date))
    }
}

private struct Line: Shape {
    func path(in rect: CGRect) -> Path {
        Path { p in
            p.move(to: CGPoint(x: rect.midX, y: rect.minY))
            p.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
        }
    }
}

// MARK: 移除

struct RemoveSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void

    var body: some View {
        SheetScaffold(title: "从 AuthBaton 移除「\(account.displayName)」？", lead: "以后再用时要重新导入或登录。") {
            VStack(alignment: .leading, spacing: 9) {
                CheckItem(kind: .you, text: "删除 AuthBaton 保存的记录、凭据和额度缓存，停止后台查询和维护。")
                CheckItem(kind: .system, text: "原来的认证文件和官方客户端登录不变，不会注销订阅账号。")
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button("移除账号", role: .destructive) {
                Task { await submit(service, "accounts.remove", LoginParams(id: account.id, confirm: true), close: close) }
            }
            .buttonStyle(.pill(.destructive, height: 27))
            .disabled(service.accountBusy)
        }
    }
}

// MARK: 恢复同步 / 重试保存

struct RecoverSaveSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void
    @State private var working = false

    private var claudeNative: Bool { account.provider == "claude" && account.native }

    var body: some View {
        SheetScaffold(title: claudeNative ? "恢复「\(account.displayName)」的续期同步？" : "重试保存「\(account.displayName)」的续期结果？",
                      lead: !claudeNative ? "按已保存的续期结果重试写回；不会重复发送续期请求。"
                          // A renewal that was never written back goes into the CLI's keychain first.
                          : account.state == "unsaved" ? "先核对 Claude CLI 在钥匙串里的登录：还是续期前那份，就把已保存的续期结果写回去；再重写 AuthBaton 自己的备份。"
                          : "重新读取 Claude CLI 在钥匙串里的最新登录，确认是同一个账号后，重写 AuthBaton 自己的备份。") {
            VStack(alignment: .leading, spacing: 9) {
                if claudeNative {
                    CheckItem(kind: .system, text: "这一步不联网；Claude CLI 可以开着。")
                    CheckItem(kind: .system, text: "身份对不上就停下并提示，不覆盖任何登录。")
                    if account.automatic {
                        CheckItem(kind: .info, text: "完成约 1 分钟后，AuthBaton 重新和 Claude CLI 协同续期：Access 快到期时会联网续期。")
                    }
                } else {
                    CheckItem(kind: .system, text: "保存成功后恢复自动维护。")
                    if account.native { CheckItem(kind: .you, text: "这是在用账号，先退出对应的官方客户端。") }
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button(working ? "正在处理…" : claudeNative ? "恢复同步" : "重试保存") {
                working = true
                Task {
                    await submit(service, "accounts.retry_save", LoginParams(id: account.id), close: close)
                    working = false
                }
            }
            .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
            .disabled(working || service.accountBusy)
        }
    }
}

// MARK: 重命名

struct RenameSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void
    @State private var name = ""

    var body: some View {
        SheetScaffold(title: "重命名账号", lead: "名称只在本机用来区分账号，不会改官方账号资料。", width: 340) {
            VStack(alignment: .leading, spacing: 7) {
                LoopField(placeholder: account.identity ?? "账号名称", text: $name)
                if let identity = account.identity, name != identity {
                    Button("用登录邮箱当名称") { name = identity }.buttonStyle(.pill(.link))
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button("保存") {
                Task { await submit(service, "accounts.update", LoginParams(id: account.id, label: name), close: close) }
            }
            .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
            .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || service.accountBusy)
        }
        .onAppear { name = account.label }
    }
}

// MARK: 关闭自动换号

struct StrategyStopSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void

    var body: some View {
        let running = service.directory?.strategy?.run != nil
        SheetScaffold(title: running ? "停止这次换号并核对？" : "关闭自动换号与重置？",
                      lead: running ? "不再发起新的暂停、换号或接续，自动换号与重置也会关闭。" : "见底时不再自动换号，也不再自动用重置卡；规则会保留。") {
            VStack(alignment: .leading, spacing: 9) {
                if running {
                    CheckItem(kind: .system, text: "已经提交的操作会收尾。")
                    CheckItem(kind: .you, text: "已经暂停的会话，可能要你在 Codex 里手动继续。")
                }
                CheckItem(kind: .info, text: "已经用掉的重置卡不能撤回。")
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button(running ? "停止并核对" : "关闭", role: .destructive) {
                Task { await submit(service, "accounts.strategy_stop", LoginParams(confirm: true), close: close) }
            }
            .buttonStyle(.pill(.destructive, height: 27))
            .disabled(service.accountBusy)
        }
    }
}
