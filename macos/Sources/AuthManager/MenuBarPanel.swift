import AppKit
import SwiftUI

// The everyday surface: a ring per tool, what needs a hand, and the accounts that can take over.
struct MenuBarPanel: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var reminders = ReminderModel.shared
    @ObservedObject private var bar = MenuBarController.shared
    @AppStorage(MenuBarShows.key) private var shows = ""
    @AppStorage(MenuBarShows.lastKey) private var lastPage = ""
    let openWindow: () -> Void
    @State private var expanded: String?
    @State private var working = false
    @State private var attemptSince: Double?
    // 退出 asked first: accounts in use here that the centre renews stop getting fresh logins once AuthBaton quits.
    @State private var quitting = MenuBarPanel.previewQuitting
    // Preview renders only: draws the panel with its quit confirmation open.
    @MainActor static var previewQuitting = false

    private var directory: AccountDirectory? { service.directory }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if let directory {
                rings(directory).padding(.top, 9)
                issues(directory)
                ForEach(["codex", "claude"], id: \.self) { provider in
                    switchList(provider, directory: directory)
                }
                if let desktop = directory.claudeDesktop, desktop.installed { claudeDesktop(desktop) }
                if let network = directory.network, network.available { networkRow(network) }
            } else if service.isConnected {
                Text("正在载入账号…").font(.system(size: 12)).foregroundStyle(Palette.ink3).padding(7)
            } else {
                disconnected
            }
            footer
        }
        .padding(10)
        .frame(width: 306)
        .background(Palette.panel)
        .foregroundStyle(Palette.ink)
        .tint(Palette.solid)
        // The network row says whether a switch would cut a reply; the service reads traffic only while asked.
        .task {
            while !Task.isCancelled {
                await service.watchNetwork()
                try? await Task.sleep(for: .seconds(5))
            }
        }
        .onDisappear { quitting = false }
        .onChange(of: bar.quitAsked) { _, asked in
            if asked { quitting = true }
            bar.quitAsked = false
        }
        .onChange(of: service.directory?.desktop?.action) { _, action in
            if let action, let since = attemptSince, action.state == "complete", (action.at ?? 0) >= since {
                expanded = nil
                attemptSince = nil
            }
        }
    }

    private var header: some View {
        let checked = directory?.accounts.compactMap { $0.usage?.checkedAt }.max()
        return HStack(spacing: 3.5) {
            Text("AuthBaton").font(.system(size: 12, weight: .semibold))
            if !service.isConnected {
                Circle().fill(Palette.attentionDot).frame(width: 5, height: 5).padding(.leading, 3.5)
            }
            Spacer()
            Button { Task { await service.refresh(); await service.refreshQuotaIfNeeded() } } label: {
                Icon(.refresh, size: 14, color: Palette.ink2).frame(width: 24, height: 24).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(checked.map { "\(When.text($0))查过 · 点一下立即刷新" } ?? "刷新")
            .disabled(service.isRefreshing)
            .accessibilityLabel("刷新")
            Button(action: openWindow) {
                Icon(.window, size: 14, color: Palette.ink2).frame(width: 24, height: 24).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("打开 AuthBaton")
            .accessibilityLabel("打开主窗口")
        }
        .padding(.leading, 3.5)
        .frame(height: 26)
    }

    // Nothing from a lost service is shown as current; say so and offer the way back.
    private var disconnected: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(service.isConnecting ? "正在连接后台服务…" : "后台服务没连上").font(.system(size: 12, weight: .semibold))
            if let error = service.errorText {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
            }
            ReconnectButton(kind: .primary)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopCard(radius: 12)
        .padding(.top, 9)
    }

    // MARK: 在用

    @ViewBuilder private func rings(_ directory: AccountDirectory) -> some View {
        let providers = ["codex", "claude"].filter { provider in directory.accounts.contains { $0.provider == provider } }
        if providers.isEmpty {
            Text("还没有账号；打开 AuthBaton 添加。").font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(7)
        } else {
            HStack(spacing: 9) {
                ForEach(providers, id: \.self) { provider in
                    ringCard(provider, current: directory.current(provider), shown: providers.count > 1 && provider == MenuBarShows.provider(shows, last: lastPage))
                }
            }
        }
    }

    // shown: the tool whose number the menu bar carries, ringed and tagged.
    private func ringCard(_ provider: String, current: ManagedAccount?, shown: Bool) -> some View {
        let windows = Array((current?.primaryWindows ?? []).prefix(2))
        return Button {
            AppNavigation.shared.show(provider == "codex" ? .codex : .claude)
            openWindow()
        } label: {
            VStack(spacing: 0) {
                HStack(spacing: 5) {
                    Icon(.provider(provider), size: 12)
                    Text(GlyphTile.name(provider)).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                    Spacer(minLength: 0)
                    if shown { Pill("菜单栏", style: .brand, small: true).fixedSize().help("菜单栏显示的是它；右键菜单栏图标可以改") }
                }
                Ring(values: windows.map(\.remaining), size: 65).padding(.top, 7)
                HStack(spacing: 5) {
                    if let current { Seal(current.displayName, style: .inUse, size: 15) }
                    Text(accountName: current?.displayName ?? "没有在用的账号").font(.system(size: 11, weight: .semibold)).lineLimit(1)
                    if let current { CloudBadge(account: current, size: 10) }
                }
                .padding(.top, 7)
                Text(caption(current, windows)).font(.system(size: 10)).foregroundStyle(Palette.ink3).monospacedDigit().lineLimit(1)
                    .padding(.top, 2)
            }
            .padding(10)
            .frame(maxWidth: .infinity)
            .loopCard(radius: 14)
            .overlay { if shown { RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Palette.brand.opacity(0.55), lineWidth: 1.2) } }
            .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        }
        .buttonStyle(.plain)
        .help("在 AuthBaton 里查看 \(GlyphTile.name(provider))")
    }

    private func caption(_ account: ManagedAccount?, _ windows: [AccountQuota]) -> String {
        guard let account else { return " " }
        if windows.count >= 2 { return windows.map { "\($0.shortLabel) \($0.remaining)%" }.joined(separator: " · ") }
        return windows.first?.refill() ?? account.quotaCaption ?? " "
    }

    // MARK: 要处理

    @ViewBuilder private func issues(_ directory: AccountDirectory) -> some View {
        let issues = directory.issues(dismissed: reminders.dismissed)
        if !issues.isEmpty {
            Button {
                if issues.count == 1, let id = issues[0].account, let account = directory.accounts.first(where: { $0.id == id }) {
                    AppNavigation.shared.show(account: account)
                } else {
                    AppNavigation.shared.show(.accounts)
                }
                openWindow()
            } label: {
                HStack(spacing: 9) {
                    Icon(.alert, size: 15, color: Palette.attention)
                    VStack(alignment: .leading, spacing: 0) {
                        Text("\(issues.count) 件事要处理").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.attentionText)
                        Text(issues.map(\.summary).joined(separator: " · ")).font(.system(size: 10)).foregroundStyle(Palette.attentionInk).lineLimit(2)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 3.5)
                    Icon(.chevronRight, size: 10, color: Palette.attentionText, weight: 2.2)
                }
                .padding(.horizontal, 10).padding(.vertical, 9)
                .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            }
            .buttonStyle(.plain)
            .padding(.top, 9)
        }
    }

    // MARK: 换账号

    @ViewBuilder private func switchList(_ provider: String, directory: AccountDirectory) -> some View {
        let others = directory.relay(provider).filter { !$0.native }
        // Claude's list moves Claude Code; with 无感换号 the desktop's Code tab follows it (11-22: the window's Code slot).
        let relayOn = directory.claudeDesktop?.codeRelay?.enabled == true
        let title = provider == "claude" ? (relayOn ? "换 Code 账号（无感）" : "换 Claude Code 账号") : "换 \(GlyphTile.name(provider)) 账号"
        let next = directory.relayNext(provider)
        if !others.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                Text(title).font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.ink3)
                    .padding(.horizontal, 7).padding(.top, 12).padding(.bottom, 3.5)
                ForEach(others) { account in
                    SwitchRow(account: account, open: expanded == account.id, next: account.id == next) {
                        service.clearAccountError()
                        attemptSince = nil
                        expanded = expanded == account.id ? nil : account.id
                    }
                    if expanded == account.id {
                        let action = directory.primaryAction(for: account)
                        if action == .switchTo { confirm(account, directory: directory) } else { unavailable(account, action) }
                    }
                }
            }
        }
    }

    // Same routes and the same service call as the window's switch sheet.
    private func confirm(_ account: ManagedAccount, directory: AccountDirectory) -> some View {
        let route = directory.switchRoute(for: account)
        let blocked = route == .desktopDisconnected
        let text: String = switch route {
        case .desktopDisconnected: "现在换不过去：热切换开着，Codex 只能通过桌面换号，但桌面没连上。桌面没有任务在运行时，可以让 AuthBaton 以调试模式重启桌面并直接换上；或者到窗口里关闭热切换。"
        case .desktop: "同时更新 Codex 桌面和默认文件。先结束桌面任务，退出独立的 Codex CLI；VS Code 的 Codex 插件在后台运行，要停用它或退出 VS Code。桌面窗口可以开着。"
        case .plain: account.provider == "codex"
            ? "更新 ~/.codex/auth.json。先结束任务，退出 Codex 桌面和 CLI，停用 VS Code 的 Codex 插件或退出 VS Code，换完再打开。"
            : "更新 Claude Code 的默认登录，不用退出：运行中的会话约 30 秒内改用新账号；被额度打断的会话，换上 30 秒后在原会话按回车继续。"
        }
        let disabled = working || service.accountBusy || directory.switchDisabled(for: account)
        let current = attempt(account, directory)
        return VStack(alignment: .leading, spacing: 7) {
            Text(text).font(.system(size: 11)).foregroundStyle(blocked ? Palette.attentionInk : Palette.ink2).fixedSize(horizontal: false, vertical: true)
            if let error = service.accountError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
            }
            if let current {
                HStack(alignment: .top, spacing: 5) {
                    if current.pending { ProgressView().controlSize(.mini) }
                    Text(current.message).font(.system(size: 11)).foregroundStyle(current.state == "failed" ? Palette.attentionText : Palette.ink3)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else if blocked, let message = directory.desktop?.message, !message.isEmpty {
                Text(message).font(.system(size: 10)).foregroundStyle(Palette.attentionText)
            }
            HStack(spacing: 5) {
                Spacer()
                Button("取消") { expanded = nil }.buttonStyle(.pill(.secondary, height: 20))
                if blocked {
                    Button("打开设置") {
                        AppNavigation.shared.showDesktopConnection()
                        openWindow()
                    }
                    .buttonStyle(.pill(.secondary, height: 20))
                    Button(current?.state == "failed" ? "重试" : "重启并换上") {
                        switchTo(account, method: "accounts.desktop_relaunch_switch")
                    }
                    .buttonStyle(.pill(.primary, height: 20)).disabled(disabled || current?.pending == true)
                } else {
                    Button(current?.state == "failed" ? "重试" : "换上 \(account.displayName)") {
                        switchTo(account, method: "accounts.activate", desktop: route == .desktop)
                    }
                    .buttonStyle(.pill(.primary, height: 20)).disabled(disabled || current?.pending == true)
                }
            }
        }
        .padding(10)
        .background(blocked ? Palette.attentionWash : Palette.wash, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(.horizontal, 3.5).padding(.bottom, 3.5)
    }

    // Same way out as the window's account cards: their fix, one click away in the window.
    private func unavailable(_ account: ManagedAccount, _ action: PrimaryAction) -> some View {
        let text: String = switch action {
        case .capture: "\(account.health.title)：登录还没保存。先在窗口里点「本地检查」保存，再换上。"
        case .recover: "\(account.health.title)：现在换过去会写入旧的登录。先在窗口里\(account.recoverTitle)，再换上。"
        case .relogin: "\(account.health.title)：先在窗口里重新登录，再换上。"
        default: "\(account.health.title)，现在不能换上。" + (account.health.detail ?? account.message)
        }
        let fix: String? = switch action {
        case .capture: "本地检查…"
        case .recover: account.recoverTitle + "…"
        case .relogin: "重新登录…"
        default: nil
        }
        return VStack(alignment: .leading, spacing: 7) {
            Text(text).font(.system(size: 11)).foregroundStyle(Palette.attentionInk).fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 5) {
                Spacer()
                Button("取消") { expanded = nil }.buttonStyle(.pill(.secondary, height: 20))
                if let fix {
                    Button(fix) {
                        AppNavigation.shared.show(account: account)
                        openWindow()
                    }
                    .buttonStyle(.pill(.primary, height: 20))
                }
            }
        }
        .padding(10)
        .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .padding(.horizontal, 3.5).padding(.bottom, 3.5)
    }

    private func attempt(_ account: ManagedAccount, _ directory: AccountDirectory) -> DesktopConnection.Action? {
        attemptSince.flatMap { directory.desktopAttempt(for: account.id, since: $0) }
    }

    // A desktop switch reports back later; keep the row open until it does.
    private func switchTo(_ account: ManagedAccount, method: String, desktop: Bool = true) {
        working = true
        let started = Date().timeIntervalSince1970.rounded(.down) - 1
        Task {
            await service.accountOperation(method, params: LoginParams(id: account.id, confirm: true))
            working = false
            guard service.accountError == nil else { return }
            if desktop { attemptSince = started } else { expanded = nil }
        }
    }

    // Network: is it safe to switch now, and 切网, which opens the picker in the main window.
    private func networkRow(_ network: NetworkStatus) -> some View {
        let op = network.operation?.running == true ? network.operation : nil
        let status: String = if let op {
            "Claude Code 已暂停 · \(String(format: "%d:%02d", (op.remaining ?? 0) / 60, (op.remaining ?? 0) % 60)) 后不切了"
        } else if network.watching {
            network.safe ? "可以切网" : "\(network.live.count) 个回复在传输"
        } else {
            "正在读取…"
        }
        return VStack(alignment: .leading, spacing: 0) {
            Text("网络").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink3)
                .padding(.horizontal, 7).padding(.top, 9).padding(.bottom, 3.5)
            HStack(spacing: 9) {
                if let op {
                    ZStack {
                        Circle().stroke(Palette.soft, lineWidth: 2.6)
                        Circle().trim(from: 0, to: Double(op.remaining ?? 0) / 300)
                            .stroke(Palette.brand, style: StrokeStyle(lineWidth: 2.6, lineCap: .round)).rotationEffect(.degrees(-90))
                    }
                    .frame(width: 17, height: 17).frame(width: 19, height: 19)
                } else {
                    Icon(network.current.kind == "hotspot" ? .hotspot : .wifi, size: 15, color: Palette.ink2).frame(width: 19, height: 19)
                }
                VStack(alignment: .leading, spacing: 0) {
                    Text(op.map { "等着切到 \($0.target)" } ?? network.current.title).font(.system(size: 12)).lineLimit(1)
                    HStack(spacing: 5) {
                        if op == nil && network.watching && !network.safe {
                            Circle().fill(Palette.brand).frame(width: 6, height: 6)
                        }
                        Text(status).font(.system(size: 10)).foregroundStyle(Palette.ink3).lineLimit(1)
                    }
                }
                Spacer(minLength: 3.5)
                if op != nil {
                    Button { Task { await service.network("cancel") } } label: { Text("×").font(.system(size: 12, weight: .semibold)) }
                        .buttonStyle(.pill(.secondary, height: 20))
                        .help("取消切换")
                        .accessibilityLabel("取消切换")
                } else {
                    Button("切网") {
                        AppNavigation.shared.show(.network)
                        AppNavigation.shared.networkSheet = true
                        openWindow()
                    }
                    .buttonStyle(.pill(.secondary, height: 20))
                }
            }
            .padding(.horizontal, 7).frame(minHeight: 32)
            .contentShape(Rectangle())
            .onTapGesture {
                AppNavigation.shared.show(.network)
                openWindow()
            }
        }
    }

    // Claude desktop: who it is signed in as, whether every account's sessions match, and ▶| to the next one.
    private func claudeDesktop(_ desktop: ClaudeDesktopStatus) -> some View {
        let current = desktop.current
        let next = desktop.picks?.next.flatMap { handle in desktop.accounts.first { $0.handle == handle } }
        let confirming = expanded == "claude_desktop"
        let state: String = desktop.busy ? (desktop.operation?.kind == "add" ? "正在添加账号…" : "正在换号…")
            : desktop.restartNeeded ? "要重启一次 Claude" : desktop.pending > 0 ? "\(desktop.pending) 处待同步"
            : desktop.background ? "会话一致" : "后台同步关着"
        // Named as the window's desktop slot: with 无感换号 on, this is the desktop's chat / work only (Claude Chat).
        let relayOn = desktop.codeRelay?.enabled == true
        let locked = desktop.busy || service.accountBusy
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 4) {
                Icon(.chat, size: 10, color: Palette.ink3)
                Text(relayOn ? "Claude Chat" : "Claude 桌面").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink3)
            }
            .padding(.horizontal, 7).padding(.top, 9).padding(.bottom, 3.5)
            HStack(spacing: 9) {
                Seal(current?.display ?? "?", style: current == nil ? .out : .inUse, size: 19)
                VStack(alignment: .leading, spacing: 0) {
                    Text(current.map { "在用 \($0.title)" } ?? "没认出在用的账号")
                        .font(.system(size: 12)).lineLimit(1)
                    Text(state).font(.system(size: 10)).lineLimit(1)
                        .foregroundStyle(desktop.restartNeeded || desktop.attention != nil ? Palette.attentionText : Palette.ink3)
                }
                Spacer(minLength: 3.5)
                if let next, desktop.switchSupported {
                    // ▶| as on 提前换, with the seal of the account it switches to (11-26); what it does is in the help.
                    let who = relayOn ? "Chat" : "Claude 桌面"
                    Button { expanded = confirming ? nil : "claude_desktop" } label: {
                        HStack(spacing: 4) {
                            SkipAheadMark()
                            Seal(next.display, style: .standby, size: 16).opacity(locked ? 0.45 : 1)
                        }
                    }
                    .buttonStyle(.pill(.soft, height: 22))
                    .disabled(locked)
                    .help("现在把 \(who) 换到 \(next.title)\nClaude 桌面会退出并重开，点了先确认")
                    .accessibilityLabel("现在把 \(who) 换到 \(next.title)")
                }
            }
            .padding(.horizontal, 7).frame(minHeight: 32)
            .contentShape(Rectangle())
            .onTapGesture {
                AppNavigation.shared.show(.claude)
                openWindow()
            }
            if confirming, let next {
                VStack(alignment: .leading, spacing: 7) {
                    Text("换上 \(next.title)？Claude 桌面会退出再打开；还有任务在跑的话，Claude 会先问你。"
                         + (desktop.turnsOnBypass(next) ? "这个账号还没开 Bypass 权限模式，换上时会顺带打开。" : ""))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 5) {
                        Spacer()
                        Button("取消") { expanded = nil }.buttonStyle(.pill(.secondary, height: 20))
                        Button("换上") {
                            Task {
                                await service.claudeDesktop("switch", account: next.handle, confirm: true)
                                if service.accountError == nil { expanded = nil }
                            }
                        }
                        .buttonStyle(.pill(.primary, height: 20)).disabled(locked)
                    }
                }
                .padding(10)
                .background(Palette.wash, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .padding(.horizontal, 3.5).padding(.bottom, 3.5)
            }
        }
    }

    private var footer: some View {
        let inUse = directory?.cloudInUse ?? []
        return VStack(spacing: 0) {
            Rectangle().fill(Palette.edge.opacity(0.7)).frame(height: 1).padding(.horizontal, 7).padding(.top, 9).padding(.bottom, 5)
            MenuRow(title: "打开 AuthBaton", action: openWindow)
            if quitting && !inUse.isEmpty {
                quitConfirm(inUse)
            } else {
                // What quitting costs is said where it is clicked, and nowhere else.
                MenuRow(title: "退出", shortcut: "⌘Q", key: "q",
                        detail: inUse.isEmpty ? nil : "在用云端号 \(inUse.map(\.displayName).joined(separator: "、")) · 退出约 \(Self.lapse(inUse))掉登录") {
                    if inUse.isEmpty { NSApplication.shared.terminate(nil) } else { quitting = true }
                }
            }
        }
    }

    // How long until the first client in use loses its login once nothing renews it: what its short-lived
    // login has left, or the usual lifetime when that is not known.
    static func lapse(_ accounts: [ManagedAccount]) -> String {
        if let soonest = accounts.compactMap({ $0.cloud?.accessExpiresAt }).min(),
           let countdown = When.countdown(to: Date(timeIntervalSince1970: soonest)) { return countdown }
        return (accounts.contains { $0.provider == "claude" } ? "8 小时" : "10 天") + "后"
    }

    private func quitConfirm(_ accounts: [ManagedAccount]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 10) {
                Icon(.alert, size: 15, color: Palette.attention)
                    .frame(width: 30, height: 30)
                    .background(Palette.attentionFill, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                VStack(alignment: .leading, spacing: 3) {
                    Text("现在退出 AuthBaton？").font(.system(size: 13, weight: .bold))
                    Text("退出后将停止为在用云端账号续签短期凭据，客户端到期后会掉登录。")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                }
            }
            VStack(spacing: 4) {
                ForEach(accounts) { account in
                    HStack(spacing: 5) {
                        Icon(.provider(account.provider), size: 11)
                        Text("\(GlyphTile.name(account.provider)) · \(account.displayName)").font(.system(size: 11, weight: .medium)).lineLimit(1)
                        CloudShape().fill(Palette.ink3).frame(width: 10, height: 10)
                        Spacer(minLength: 4)
                        Text("约 \(Self.lapse([account]))失效").font(.system(size: 11)).foregroundStyle(Palette.attentionText).lineLimit(1).fixedSize()
                    }
                    .padding(.horizontal, 8).frame(height: 28)
                    .background(Palette.field, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                }
            }
            HStack(spacing: 7) {
                Spacer()
                Button("仍然退出") { NSApplication.shared.terminate(nil) }.buttonStyle(.pill(.secondary, height: 26))
                Button("保持运行") { quitting = false }.buttonStyle(.pill(.primary, height: 26)).keyboardShortcut(.defaultAction)
            }
        }
        .padding(12)
        .background(Palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
        .padding(.top, 5)
    }
}

// An account that can take over: its seal, name and lowest remaining; a click opens the confirmation below it.
private struct SwitchRow: View {
    let account: ManagedAccount
    let open: Bool
    // Where the rules take Claude Code next.
    var next = false
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Seal(account.displayName, style: .standby, size: 19)
                Text(accountName: account.displayName).font(.system(size: 12)).lineLimit(1)
                CloudBadge(account: account, size: 11)
                if next { PointerMark().fill(Palette.brand).frame(width: 6, height: 7.5).help(PointerRank.meaning) }
                if account.needsAction { Circle().fill(Palette.attentionDot).frame(width: 5, height: 5) }
                Spacer(minLength: 3.5)
                if let lowest = account.lowestPrimary {
                    QuotaBar(remaining: lowest, height: 3.5).frame(width: 44)
                    Text("\(lowest)%").font(.system(size: 11)).foregroundStyle(Palette.ink2).monospacedDigit()
                        .lineLimit(1).frame(width: 34, alignment: .trailing)
                } else {
                    Text(account.quotaCaption ?? "—").font(.system(size: 10)).foregroundStyle(Palette.ink3).lineLimit(1)
                }
            }
            .padding(.horizontal, 7)
            .frame(height: 31)
            .background(open || hovering ? Palette.rail : .clear, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityLabel("换上 \(account.displayName)" + (account.lowestPrimary.map { "，剩余 \($0)%" } ?? "") + (next ? "，\(PointerRank.meaning)" : ""))
    }
}

private struct MenuRow: View {
    let title: String
    var shortcut: String? = nil
    var key: KeyEquivalent? = nil
    // A second line under the title: what the action costs right now.
    var detail: String? = nil
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(title).font(.system(size: 12))
                    Spacer()
                    if let shortcut { Text(shortcut).font(.system(size: 12)).foregroundStyle(Palette.ink3) }
                }
                if let detail {
                    HStack(spacing: 4) {
                        CloudShape().fill(Palette.ink3).frame(width: 10, height: 10)
                        Text(detail).font(.system(size: 10)).foregroundStyle(Palette.ink3).lineLimit(1)
                    }
                }
            }
            .padding(.horizontal, 7)
            .padding(.vertical, detail == nil ? 0 : 5)
            .frame(minHeight: 26)
            .background(hovering ? Palette.rail : .clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .keyboardShortcut(key.map { KeyboardShortcut($0) })
        .onHover { hovering = $0 }
    }
}
