import AppKit
import SwiftUI

// The everyday surface: a ring per tool, what needs a hand, and the accounts that can take over.
struct MenuBarPanel: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var reminders = ReminderModel.shared
    let openWindow: () -> Void
    @State private var expanded: String?
    @State private var working = false
    @State private var attemptSince: Double?

    private var directory: AccountDirectory? { service.directory }
    private var local: Bool { directory?.cloud?.mode != "cloud" }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if let directory, local {
                rings(directory).padding(.top, 9)
                issues(directory)
                ForEach(["codex", "claude"], id: \.self) { provider in
                    switchList(provider, directory: directory)
                }
            } else if let cloud = directory?.cloud, !local {
                cloudSummary(cloud)
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
            Text("BatonPass").font(.system(size: 12, weight: .semibold))
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
            .help("打开 BatonPass")
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
            Text("还没有账号；打开 BatonPass 添加。").font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(7)
        } else {
            HStack(spacing: 9) {
                ForEach(providers, id: \.self) { provider in ringCard(provider, current: directory.current(provider)) }
            }
        }
    }

    private func ringCard(_ provider: String, current: ManagedAccount?) -> some View {
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
                }
                Ring(values: windows.map(\.remaining), size: 65).padding(.top, 7)
                HStack(spacing: 5) {
                    if let current { Seal(current.displayName, style: .inUse, size: 15) }
                    Text(current?.displayName ?? "没有在用的账号").font(.system(size: 11, weight: .semibold)).lineLimit(1)
                }
                .padding(.top, 7)
                Text(caption(current, windows)).font(.system(size: 10)).foregroundStyle(Palette.ink3).monospacedDigit().lineLimit(1)
                    .padding(.top, 2)
            }
            .padding(10)
            .frame(maxWidth: .infinity)
            .loopCard(radius: 14)
            .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        }
        .buttonStyle(.plain)
        .help("在 BatonPass 里查看 \(GlyphTile.name(provider))")
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
        if !others.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                Text("换 \(GlyphTile.name(provider)) 账号").font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.ink3)
                    .padding(.horizontal, 7).padding(.top, 12).padding(.bottom, 3.5)
                ForEach(others) { account in
                    SwitchRow(account: account, open: expanded == account.id) {
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
        case .desktopDisconnected: "现在换不过去：热切换开着，Codex 只能经过桌面换号，但桌面没连上。桌面里没有任务在运行时，可以让 BatonPass 以调试模式重启桌面并直接换上；或者到窗口里关闭热切换。"
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
                        AppNavigation.shared.show(.desktop)
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
        case .recover: "\(account.health.title)：现在换过去会写入旧的登录。先在窗口里\(account.recoverTitle)，再换上。"
        case .relogin: "\(account.health.title)：先在窗口里重新登录，再换上。"
        default: "\(account.health.title)，现在不能换上。" + (account.health.detail ?? account.message)
        }
        let fix: String? = switch action {
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

    private func cloudSummary(_ cloud: CloudStatus) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("云端模式 · \(cloud.paired ? "设备已配对" : "还没配对")").font(.system(size: 12, weight: .semibold))
            ForEach(cloud.accounts.filter { cloud.selected.values.contains($0.id) }) { account in
                Text("\(account.label) · \(account.stateLabel)").font(.system(size: 11)).foregroundStyle(Palette.ink2)
            }
            Text("\(cloud.accounts.count) 个云端账号").font(.system(size: 11)).foregroundStyle(Palette.ink3)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopCard(radius: 12)
        .padding(.top, 9)
    }

    private var footer: some View {
        VStack(spacing: 0) {
            Rectangle().fill(Palette.edge.opacity(0.7)).frame(height: 1).padding(.horizontal, 7).padding(.top, 9).padding(.bottom, 5)
            MenuRow(title: "打开 BatonPass", action: openWindow)
            MenuRow(title: "退出", shortcut: "⌘Q", key: "q") { NSApplication.shared.terminate(nil) }
        }
    }
}

// An account that can take over: its seal, name and lowest remaining; a click opens the confirmation below it.
private struct SwitchRow: View {
    let account: ManagedAccount
    let open: Bool
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Seal(account.displayName, style: .standby, size: 19)
                Text(account.displayName).font(.system(size: 12)).lineLimit(1)
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
        .accessibilityLabel("换到 \(account.displayName)" + (account.lowestPrimary.map { "，剩余 \($0)%" } ?? ""))
    }
}

private struct MenuRow: View {
    let title: String
    var shortcut: String? = nil
    var key: KeyEquivalent? = nil
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack {
                Text(title).font(.system(size: 12))
                Spacer()
                if let shortcut { Text(shortcut).font(.system(size: 12)).foregroundStyle(Palette.ink3) }
            }
            .padding(.horizontal, 7)
            .frame(height: 26)
            .background(hovering ? Palette.rail : .clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .keyboardShortcut(key.map { KeyboardShortcut($0) })
        .onHover { hovering = $0 }
    }
}

// Menu bar label: the brand icon, Codex's lowest primary quota, and a dot when something needs attention.
struct MenuBarLabel: View {
    @EnvironmentObject private var service: ServiceController

    var body: some View {
        let directory = service.directory
        let local = directory?.cloud?.mode != "cloud"
        let lowest = local ? directory?.current("codex")?.lowestPrimary : nil
        let urgent = local && ((directory?.attentionCount ?? 0) > 0)
        let reminded = local && !ExpiryReminder.all(directory?.accounts ?? [], dismissed: ReminderStore.dismissed()).isEmpty
        let offline = !service.isConnected && !service.isConnecting
        HStack(spacing: 3) {
            Image(nsImage: BrandArtwork.menuIcon(badge: offline || urgent ? .systemRed : reminded ? .systemOrange : nil))
            if let lowest { Text("\(lowest)%").monospacedDigit() }
        }
        .accessibilityLabel(offline ? "BatonPass，后台服务未连接" : urgent ? "BatonPass，有需要处理的事项" : "BatonPass")
    }
}
