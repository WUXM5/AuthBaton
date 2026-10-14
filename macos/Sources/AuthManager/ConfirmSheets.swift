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
                ResetHistoryView(accountID: id, accountLabel: account(id)?.displayName)
            case .strategyStop:
                StrategyStopSheet(close: close)
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
        if let error = service.accountError {
            Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: error)
        }
    }
}

// MARK: 切换

struct SwitchSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void
    @State private var working = false
    @State private var since: Double?

    private var directory: AccountDirectory? { service.directory }
    private var route: SwitchRoute { directory?.switchRoute(for: account) ?? .plain }
    private var from: ManagedAccount? { directory?.current(account.provider) }
    private var attempt: DesktopConnection.Action? { since.flatMap { directory?.desktopAttempt(for: account.id, since: $0) } }

    var body: some View {
        Group {
            if route == .desktopDisconnected { blocked } else { confirm }
        }
        .onChange(of: attempt?.state) { _, state in if state == "complete" { close() } }
    }

    // Without the v0.14 managed CLI, running Claude clients pick the new login up within 30 s.
    private var claudeInPlace: Bool { account.provider == "claude" && route == .plain }

    private var confirm: some View {
        SheetScaffold(title: account.native ? "让 Codex 桌面使用「\(account.displayName)」？" : "把 \(account.providerName) 切换到「\(account.displayName)」？",
                      lead: claudeInPlace ? "运行中的会话约 30 秒内改用新账号；在那之前发出的请求仍用原账号完成。"
                                          : "之后新开的会话会用新账号；正在进行的请求不保证不中断。") {
            handoff(blockedTarget: false)
            VStack(alignment: .leading, spacing: 8) {
                switch route {
                case .desktop:
                    CheckItem(kind: .system, text: "同时更新 Codex 桌面和全局 ~/.codex/auth.json；pi 的新任务跟着换号")
                    CheckItem(kind: .you, text: "先结束桌面里正在运行的任务，并退出独立 Codex CLI；VS Code 的 Codex 插件会随 VS Code 在后台运行，需在 VS Code 里停用它或退出 VS Code。桌面窗口可以保持打开")
                default:
                    if account.provider == "codex" {
                        CheckItem(kind: .system, text: "更新全局 ~/.codex/auth.json；pi 的新任务跟着换号")
                        CheckItem(kind: .you, text: "先结束任务并退出 Codex 桌面和 CLI，停用 VS Code 的 Codex 插件或退出 VS Code，完成后重新打开")
                    } else {
                        CheckItem(kind: .system, text: "更新 Claude Code 的默认登录，不用退出 Claude CLI / Agent SDK")
                        CheckItem(kind: .you, text: "已被额度打断的会话，切换 30 秒后在原会话按一次回车继续")
                    }
                }
            }
            if let attempt {
                if attempt.pending {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text(attempt.message).font(.callout).foregroundStyle(.secondary)
                    }
                } else if attempt.state == "failed" {
                    Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: attempt.message)
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction)
            Button(working || attempt?.pending == true ? "正在切换…" : attempt?.state == "failed" ? "重试" : claudeInPlace ? "切换" : "已结束任务，切换") {
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
            .buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
            .disabled(working || attempt?.pending == true || service.accountBusy || directory?.switchDisabled(for: account) == true)
        }
    }

    // Hot switch is on but the desktop is not reachable: the service will not start a switch.
    private var blocked: some View {
        let desktop = directory?.desktop
        return SheetScaffold(title: "现在还切不到「\(account.displayName)」",
                             lead: "Codex 桌面热切换已开启，所以换号只通过桌面进行；但桌面现在没有连上，这次切换不会开始。\(attempt == nil ? desktop.map { "原因：\($0.message)" } ?? "" : "")") {
            handoff(blockedTarget: true)
            VStack(alignment: .leading, spacing: 10) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("方法一：以调试模式重启桌面并切换（推荐）").font(.callout.weight(.semibold))
                    Text("Codex 里没有任务在运行时，AuthManager 会正常退出桌面、以调试模式重新打开，连上后直接切换。有任务在运行就不会重启。")
                        .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    if let attempt {
                        HStack(spacing: 8) {
                            if attempt.pending { ProgressView().controlSize(.small) }
                            Text(attempt.message).font(.caption).foregroundStyle(attempt.state == "failed" ? Palette.warning : .secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    } else {
                        Text(desktop?.message ?? "").font(.caption).foregroundStyle(Palette.warning).lineLimit(2)
                    }
                }
                Divider()
                VStack(alignment: .leading, spacing: 6) {
                    Text("方法二：关闭热切换，改用普通切换").font(.callout.weight(.semibold))
                    Text("关闭后需要退出 Codex 才能切换，切完再重新打开。之后自动换号也不可用，只会重置。")
                        .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction)
            Button("关闭热切换") {
                Task { await service.accountOperation("accounts.desktop", params: LoginParams(enabled: false)) }
            }
            .disabled(service.accountBusy || desktop?.busy == true)
            Button(working || attempt?.pending == true ? "正在重启…" : attempt?.state == "failed" ? "重试" : "以调试模式重启并切换") {
                working = true
                let started = Date().timeIntervalSince1970.rounded(.down) - 1
                Task {
                    await service.accountOperation("accounts.desktop_relaunch_switch", params: LoginParams(id: account.id))
                    working = false
                    // The restart and the switch report back later; stay open until they do.
                    if service.accountError == nil { since = started }
                }
            }
            .buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
            .disabled(working || attempt?.pending == true || service.accountBusy || desktop?.busy == true)
        }
    }

    private func handoff(blockedTarget: Bool) -> some View {
        HStack(spacing: 8) {
            side("现在", from, highlighted: false, dashed: false)
            Image(systemName: blockedTarget ? "xmark" : "arrow.right").foregroundStyle(blockedTarget ? Palette.warning : Palette.muted)
            side(blockedTarget ? "想切到" : "切换后", account, highlighted: !blockedTarget, dashed: blockedTarget)
        }
    }

    private func side(_ caption: String, _ account: ManagedAccount?, highlighted: Bool, dashed: Bool) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(caption).font(.caption2).foregroundStyle(.secondary)
            Text(account?.displayName ?? "没有当前账号").font(.callout.weight(.semibold)).lineLimit(1)
            Text(account?.lowestPrimary.map { "主额度剩余 \($0)%" } ?? "额度未知").font(.caption).foregroundStyle(.secondary).monospacedDigit()
        }
        .padding(10).frame(maxWidth: .infinity, alignment: .leading)
        .background(highlighted ? Palette.accent.opacity(0.10) : .clear, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
            .strokeBorder(highlighted ? Palette.accent.opacity(0.6) : Palette.hairline, style: StrokeStyle(lineWidth: 1, dash: dashed ? [4, 3] : [])))
    }
}

// MARK: 使用重置

struct ResetSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let creditID: String?
    let requestID: String
    let expiresAt: String?
    let retry: Bool
    let close: () -> Void
    @State private var working = false

    var body: some View {
        SheetScaffold(title: retry ? "重试「\(account.displayName)」上次的重置？" : "为「\(account.displayName)」使用 1 次重置？",
                      lead: retry ? "会重试原来的同一次操作，不会换用另一次机会。如果上次没有生效，这次可能用掉 1 次。"
                                  : "成功后主额度恢复，这个账号的重置次数减 1。能否重置以官方返回为准。") {
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
                GridRow { Text("当前主额度").foregroundStyle(.secondary); Text(account.lowestPrimary.map { "剩余 \($0)%" } ?? "未知") }
                GridRow { Text("使用的这一次").foregroundStyle(.secondary); Text("\(QuotaResetCredit.expiryLabel(expiresAt)) 到期") }
                if !retry { GridRow { Text("使用后剩余").foregroundStyle(.secondary); Text("\(max(0, account.codexCredits.count - 1)) 次") } }
            }
            .font(.callout).panel(padding: 12)
            if !retry, let remaining = account.lowestPrimary, remaining > 50 {
                Callout(icon: "exclamationmark.triangle.fill", tone: .warning, title: "额度还剩 \(remaining)%",
                        message: "现在重置会浪费大部分额度。自动规则一般在见底时才使用。")
            }
            SheetError()
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction)
            Button(retry ? "确认重试同一次操作" : "使用 1 次", role: .destructive) {
                working = true
                Task {
                    await submit(service, "accounts.reset_quota",
                                 LoginParams(id: account.id, confirm: true, credit_id: creditID, reset_id: requestID), close: close)
                    working = false
                }
            }
            .buttonStyle(.borderedProminent).tint(Palette.danger)
            .disabled(working || service.accountBusy)
        }
    }
}

// MARK: 移除

struct RemoveSheet: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let close: () -> Void

    var body: some View {
        SheetScaffold(title: "从 AuthManager 移除「\(account.displayName)」？", lead: "以后要用它需要重新导入或登录。") {
            VStack(alignment: .leading, spacing: 8) {
                CheckItem(kind: .you, text: "删除 AuthManager 保存的记录、凭据和额度缓存，并停止它的后台查询和维护")
                CheckItem(kind: .system, text: "原始认证文件和官方客户端登录保持不变，不会注销订阅账号")
            }
            SheetError()
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction)
            Button("移除账号", role: .destructive) {
                Task { await submit(service, "accounts.remove", LoginParams(id: account.id, confirm: true), close: close) }
            }
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
                      lead: claudeNative ? "重新读取 Claude CLI 在钥匙串里的最新登录，核对是同一个账号后，重写 AuthManager 自己的备份。"
                                         : "按已保存的续期结果重试写回；不会重复发送续期请求。") {
            VStack(alignment: .leading, spacing: 8) {
                if claudeNative {
                    CheckItem(kind: .system, text: "这一步不联网；Claude CLI 可以保持运行")
                    CheckItem(kind: .system, text: "身份对不上就停下并提示，不覆盖任何登录")
                    CheckItem(kind: .info, text: "完成约 1 分钟后，AuthManager 重新和 Claude CLI 协同续期：access 快到期时会联网续期")
                } else {
                    CheckItem(kind: .system, text: "保存成功后恢复自动维护")
                    CheckItem(kind: .you, text: "如果它是当前账号，请先退出对应官方客户端")
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction)
            Button(working ? "正在处理…" : claudeNative ? "恢复同步" : "重试保存") {
                working = true
                Task {
                    await submit(service, "accounts.retry_save", LoginParams(id: account.id), close: close)
                    working = false
                }
            }
            .buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
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
        SheetScaffold(title: "重命名账号", lead: "名称只在本机用来区分账号，不会修改官方账号资料。", width: 400) {
            TextField(account.identity ?? "账号名称", text: $name).textFieldStyle(.roundedBorder)
            if let identity = account.identity, name != identity {
                Button("用登录邮箱作为名称") { name = identity }.buttonStyle(.link).font(.caption)
            }
            SheetError()
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction)
            Button("保存") {
                Task { await submit(service, "accounts.update", LoginParams(id: account.id, label: name), close: close) }
            }
            .buttonStyle(.borderedProminent).keyboardShortcut(.defaultAction)
            .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || service.accountBusy)
        }
        .onAppear { name = account.label }
    }
}

// MARK: 停止自动策略

struct StrategyStopSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void

    var body: some View {
        let running = service.directory?.strategy?.run != nil
        SheetScaffold(title: running ? "停止自动流程并核对？" : "关闭自动换号与重置？",
                      lead: "不再发起新的暂停、换号或接续。") {
            VStack(alignment: .leading, spacing: 8) {
                CheckItem(kind: .system, text: "已经提交的操作会收尾")
                CheckItem(kind: .you, text: "已经暂停的会话可能需要你在 Codex 里手动继续")
                CheckItem(kind: .info, text: "已使用的重置次数不能撤回")
            }
            SheetError()
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction)
            Button(running ? "停止并核对" : "关闭", role: .destructive) {
                Task { await submit(service, "accounts.strategy_stop", LoginParams(confirm: true), close: close) }
            }
            .disabled(service.accountBusy)
        }
    }
}
