import AppKit
import SwiftUI

// The everyday surface: glance at quota, switch an account, see what needs attention.
struct MenuBarPanel: View {
    @EnvironmentObject private var service: ServiceController
    let openWindow: () -> Void
    @State private var expanded: String?
    @State private var working = false
    @State private var attemptSince: Double?

    private var directory: AccountDirectory? { service.directory }
    private var local: Bool { directory?.cloud?.mode != "cloud" }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            header
            if let directory, local {
                attention(directory)
                current(directory)
                ForEach(["codex", "claude"], id: \.self) { provider in
                    switchList(provider, directory: directory)
                }
                automationLine(directory)
            } else if let cloud = directory?.cloud, !local {
                cloudSummary(cloud)
            } else if service.isConnected {
                Text("正在载入账号…").font(.callout).foregroundStyle(.secondary).padding(8)
            } else {
                disconnected
            }
            footer
        }
        .padding(10)
        .frame(width: 340)
        .onChange(of: service.directory?.desktop?.action) { _, action in
            if let action, let since = attemptSince, action.state == "complete", (action.at ?? 0) >= since {
                expanded = nil
                attemptSince = nil
            }
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text("AuthManager").font(.system(size: 13, weight: .semibold))
            HStack(spacing: 5) {
                Dot(tone: service.isConnected ? .success : .warning, size: 6)
                Text(service.isConnected ? (local ? "本地服务正常" : "云端模式") : "后台服务未连接").font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button { Task { await service.refresh(); await service.refreshQuotaIfNeeded() } } label: { Image(systemName: "arrow.clockwise") }
                .buttonStyle(.borderless).help("刷新").disabled(service.isRefreshing)
            Button(action: openWindow) { Image(systemName: "macwindow") }.buttonStyle(.borderless).help("打开 AuthManager")
        }
        .padding(.horizontal, 4)
    }

    // Nothing from a lost service is shown as current; say so and offer the way back.
    private var disconnected: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(service.isConnecting ? "正在连接后台服务…" : "后台服务未连接").font(.callout.weight(.medium))
            if let error = service.errorText {
                Text(error).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            ReconnectButton().controlSize(.small)
        }
        .padding(8)
    }

    @ViewBuilder private func attention(_ directory: AccountDirectory) -> some View {
        let urgent = directory.accounts.filter(\.needsAction)
        let reminders = ExpiryReminder.all(directory.accounts, dismissed: ReminderStore.dismissed())
        if directory.recoveryNeeded {
            alertRow(tone: .danger, text: "上次切换尚未确认，需要核对", target: nil)
        }
        ForEach(urgent) { account in
            alertRow(tone: account.health.tone, text: "\(account.displayName)：\(account.health.title)", target: account.id)
        }
        if let first = reminders.first {
            alertRow(tone: .warning, text: reminders.count == 1 ? first.title
                                                                 : "\(reminders.reduce(0) { $0 + $1.count }) 次重置 7 天内到期，最早 \(first.accountName)",
                     target: first.accountID)
        }
    }

    private func alertRow(tone: Tone, text: String, target: String?) -> some View {
        Button {
            AppNavigation.shared.show(.accounts, select: target)
            openWindow()
        } label: {
            HStack(spacing: 8) {
                Image(systemName: tone == .danger ? "exclamationmark.triangle.fill" : "clock.badge.exclamationmark").foregroundStyle(tone.color)
                Text(text).font(.callout).lineLimit(1)
                Spacer(minLength: 4)
                Text("处理 ›").font(.caption.weight(.semibold)).foregroundStyle(tone.color)
            }
            .padding(.horizontal, 10).padding(.vertical, 8)
            .background(tone.color.opacity(0.12), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func current(_ directory: AccountDirectory) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            sectionTitle("正在使用")
            VStack(spacing: 10) {
                ForEach(Array(["codex", "claude"].compactMap { directory.current($0) }.enumerated()), id: \.element.id) { index, account in
                    if index > 0 { Divider() }
                    HStack(alignment: .top, spacing: 10) {
                        ProviderGlyph(provider: account.provider, size: 24)
                        VStack(alignment: .leading, spacing: 4) {
                            HStack(alignment: .firstTextBaseline) {
                                Text(account.displayName).font(.callout.weight(.semibold)).lineLimit(1)
                                Spacer(minLength: 4)
                                Text(account.providerName).font(.caption2).foregroundStyle(.secondary)
                            }
                            if account.primaryWindows.isEmpty {
                                Text(account.quotaCaption ?? "—").font(.caption).foregroundStyle(.secondary)
                            }
                            ForEach(Array(account.primaryWindows.enumerated()), id: \.offset) { _, window in
                                QuotaLine(quota: window, labelWidth: 40)
                            }
                        }
                    }
                }
                if directory.current("codex") == nil && directory.current("claude") == nil {
                    Text("还没有当前账号").font(.callout).foregroundStyle(.secondary)
                }
            }
            .padding(10)
            .background(Palette.card, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.hairline))
        }
    }

    @ViewBuilder private func switchList(_ provider: String, directory: AccountDirectory) -> some View {
        let others = directory.accounts.filter { $0.provider == provider && !$0.native }
        if !others.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                sectionTitle("换 \(ProviderGlyph.name(provider)) 账号", trailing: "点一下看说明，再确认")
                ForEach(others) { account in
                    Button {
                        service.clearAccountError()
                        attemptSince = nil
                        expanded = expanded == account.id ? nil : account.id
                    } label: {
                        HStack(spacing: 6) {
                            Text(account.displayName).font(.callout.weight(.medium)).lineLimit(1)
                            Text(summary(account)).font(.caption).foregroundStyle(.secondary).monospacedDigit().lineLimit(1)
                            Spacer(minLength: 4)
                            if account.needsAction { Dot(tone: .danger, size: 6) }
                            Image(systemName: expanded == account.id ? "chevron.down" : "chevron.right").font(.caption2).foregroundStyle(.tertiary)
                        }
                        .padding(.horizontal, 8).padding(.vertical, 6)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    if expanded == account.id {
                        let action = directory.primaryAction(for: account)
                        if action == .switchTo { confirm(account, directory: directory) } else { unavailable(account, action) }
                    }
                }
            }
        }
    }

    private func summary(_ account: ManagedAccount) -> String {
        var parts: [String] = []
        if let lowest = account.lowestPrimary { parts.append("剩余 \(lowest)%") }
        if let count = account.creditCount { parts.append(account.provider == "claude" ? "官方重置 \(count) 次" : "\(count) 次重置") }
        return parts.isEmpty ? "" : "· " + parts.joined(separator: " · ")
    }

    // Same routes and the same service call as the window's switch sheet.
    private func confirm(_ account: ManagedAccount, directory: AccountDirectory) -> some View {
        let route = directory.switchRoute(for: account)
        let blocked = route == .desktopDisconnected
        let text: String = switch route {
        case .desktopDisconnected: "现在切不过去：热切换开着，换号只走 Codex 桌面，但桌面没连上。Codex 里没有任务在运行时，可以让 AuthManager 以调试模式重启桌面并直接切换；或到窗口里关闭热切换。"
        case .desktop: "会同时更新 Codex 桌面和默认文件。请先结束桌面任务并退出独立 Codex CLI；VS Code 的 Codex 插件会在后台运行，需停用它或退出 VS Code。桌面窗口可以保持打开。"
        case .plain: account.provider == "codex"
            ? "会更新 ~/.codex/auth.json。请先结束任务并退出 Codex 桌面和 CLI，停用 VS Code 的 Codex 插件或退出 VS Code，完成后重新打开。"
            : "会更新 Claude Code 默认登录，不用退出：运行中的会话约 30 秒内改用新账号；被额度打断的会话，切换 30 秒后在原会话按回车继续。"
        }
        let disabled = working || service.accountBusy || directory.switchDisabled(for: account)
        return VStack(alignment: .leading, spacing: 8) {
            Text(text).font(.caption).foregroundStyle(blocked ? Color.primary : Palette.muted).fixedSize(horizontal: false, vertical: true)
            if let error = service.accountError {
                Text(error).font(.caption).foregroundStyle(Palette.warning).fixedSize(horizontal: false, vertical: true)
            }
            if let attempt = attempt(account, directory) {
                HStack(alignment: .top, spacing: 6) {
                    if attempt.pending { ProgressView().controlSize(.mini) }
                    Text(attempt.message).font(.caption).foregroundStyle(attempt.state == "failed" ? Palette.warning : Palette.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            if blocked, let message = directory.desktop?.message {
                Text(message).font(.caption2).foregroundStyle(.secondary)
            }
            HStack(spacing: 6) {
                Spacer()
                Button("取消") { expanded = nil }
                if blocked {
                    Button("打开设置") {
                        AppNavigation.shared.show(.desktop)
                        openWindow()
                    }
                    Button(attempt(account, directory)?.state == "failed" ? "重试" : "重启并切到 \(account.displayName)") {
                        switchTo(account, method: "accounts.desktop_relaunch_switch")
                    }
                    .buttonStyle(.borderedProminent).disabled(disabled || attempt(account, directory)?.pending == true)
                } else {
                    Button(attempt(account, directory)?.state == "failed" ? "重试" : "切到 \(account.displayName)") {
                        switchTo(account, method: "accounts.activate", desktop: route == .desktop)
                    }
                    .buttonStyle(.borderedProminent).disabled(disabled || attempt(account, directory)?.pending == true)
                }
            }
            .controlSize(.small)
        }
        .padding(10)
        .background((blocked ? Palette.warning : Palette.accent).opacity(0.10), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .padding(.horizontal, 4).padding(.bottom, 4)
    }

    // Same way out as the window's inspector: its main button, one click away in the window.
    private func unavailable(_ account: ManagedAccount, _ action: PrimaryAction) -> some View {
        let text: String = switch action {
        case .recover: "\(account.health.title)：现在切过去会写入旧的登录。先在窗口里\(account.recoverTitle)，再切换。"
        case .relogin: "\(account.health.title)：先在窗口里重新登录，再切换。"
        default: "\(account.health.title)，现在不能切换。" + (account.health.detail ?? account.message)
        }
        let fix: String? = switch action {
        case .recover: account.recoverTitle + "…"
        case .relogin: "重新登录…"
        default: nil
        }
        return VStack(alignment: .leading, spacing: 8) {
            Text(text).font(.caption).foregroundStyle(Palette.muted).fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 6) {
                Spacer()
                Button("取消") { expanded = nil }
                if let fix {
                    Button(fix) {
                        AppNavigation.shared.show(.accounts, select: account.id)
                        openWindow()
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            .controlSize(.small)
        }
        .padding(10)
        .background(Palette.warning.opacity(0.10), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .padding(.horizontal, 4).padding(.bottom, 4)
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

    @ViewBuilder private func automationLine(_ directory: AccountDirectory) -> some View {
        if let strategy = directory.strategy {
            Button {
                AppNavigation.shared.show(.automation)
                openWindow()
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "bolt").foregroundStyle(.secondary).frame(width: 16)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(strategy.config.enabled ? "自动换号与重置：已开启" : "自动换号与重置：已关闭").font(.callout)
                        if strategy.config.enabled, directory.desktop?.connected != true {
                            Text("桌面未连接，现在只会重置").font(.caption).foregroundStyle(Palette.warning)
                        } else if strategy.phase == "attention" {
                            Text(strategy.message).font(.caption).foregroundStyle(Palette.warning).lineLimit(1)
                        }
                    }
                    Spacer()
                    Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
                }
                .padding(.horizontal, 8).padding(.vertical, 7)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }

    private func cloudSummary(_ cloud: CloudStatus) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            sectionTitle("云端模式 · \(cloud.paired ? "设备已配对" : "尚未配对")")
            ForEach(cloud.accounts.filter { cloud.selected.values.contains($0.id) }) { account in
                Text("\(account.label) · \(account.stateLabel)").font(.callout)
            }
            Text("\(cloud.accounts.count) 个云端账号").font(.caption).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 4)
    }

    private var footer: some View {
        HStack(spacing: 6) {
            Button("打开 AuthManager", action: openWindow)
            Spacer()
            Button("退出") { NSApplication.shared.terminate(nil) }.keyboardShortcut("q")
        }
        .controlSize(.small)
        .padding(.top, 6)
        .overlay(alignment: .top) { Divider() }
    }

    private func sectionTitle(_ title: String, trailing: String? = nil) -> some View {
        HStack {
            Text(title).font(.system(size: 11, weight: .semibold)).foregroundStyle(.tertiary)
            Spacer()
            if let trailing { Text(trailing).font(.caption2).foregroundStyle(.tertiary) }
        }
        .padding(.horizontal, 6).padding(.top, 4).padding(.bottom, 4)
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
        .accessibilityLabel(offline ? "AuthManager，后台服务未连接" : urgent ? "AuthManager，有需要处理的事项" : "AuthManager")
    }
}
