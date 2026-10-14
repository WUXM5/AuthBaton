import SwiftUI

extension ManagedAccount {
    var needsAction: Bool {
        switch state {
        case "reauth", "attention", "unsaved", "backup_pending", "cloud_transfer": true
        // The cloud renews it; the local login dates behind the notice no longer apply.
        case "cloud_managed": false
        default: loginNotice?.needsAttention == true
        }
    }
}

// Every confirmation or form the accounts page can open; the account is looked up live by ID.
enum AccountSheet: Identifiable {
    case login(provider: String, accountID: String?)
    case switchTo(String)
    case reset(accountID: String, creditID: String?, requestID: String, expiresAt: String?, retry: Bool)
    case remove(String)
    case recover(String)
    case rename(String)
    case history(String?)
    case strategyStop

    var id: String {
        switch self {
        case .login(let provider, let id): "login-\(provider)-\(id ?? "")"
        case .switchTo(let id): "switch-\(id)"
        case .reset(let id, let credit, let request, _, let retry): "reset-\(id)-\(credit ?? request)-\(retry)"
        case .remove(let id): "remove-\(id)"
        case .recover(let id): "recover-\(id)"
        case .rename(let id): "rename-\(id)"
        case .history(let id): "history-\(id ?? "all")"
        case .strategyStop: "strategy-stop"
        }
    }
}

struct AccountsView: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @State private var sheet: AccountSheet?
    @State private var showAdd = false
    @State private var query = ""
    @State private var dismissed = ReminderStore.dismissed()
    private var locked: Bool { service.accountBusy || service.directory?.busy == true }

    var body: some View {
        VStack(spacing: 0) {
            if let directory = service.directory {
                HStack(spacing: 0) {
                    ScrollView {
                        list(directory)
                            .padding(.horizontal, 20).padding(.top, 16).padding(.bottom, 28)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(minWidth: 520)
                    if !directory.accounts.isEmpty {
                        Divider()
                        AccountInspector(account: selected(directory), directory: directory, open: { sheet = $0 })
                            .frame(width: 340)
                    }
                }
            } else if !service.isConnected && !service.isConnecting {
                // The window's banner says why and offers to reconnect.
                Spacer()
            } else if let error = service.directoryError {
                Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: error).padding(20)
                Spacer()
            } else {
                ProgressView("正在载入账号列表…").frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task { await service.refreshQuotaIfNeeded() }
        .searchable(text: $query, placement: .toolbar, prompt: "搜索账号或邮箱")
        .pageBar("账号", subtitle: subtitle) {
            Button("刷新", systemImage: "arrow.clockwise") {
                Task {
                    await service.refresh()
                    await service.refreshQuotaIfNeeded()
                }
            }
            .help("刷新认证状态和额度").disabled(service.isRefreshing)
            Button { showAdd = true } label: { Label("添加账号", systemImage: "plus").labelStyle(.titleAndIcon) }
                .help("登录新账号或导入这台 Mac 已有的登录")
                .disabled(locked || service.directory?.recoveryNeeded == true)
                .popover(isPresented: $showAdd, arrowEdge: .bottom) {
                    AddAccountChooser(login: { provider in showAdd = false; sheet = .login(provider: provider, accountID: nil) },
                                      importCurrent: { provider in showAdd = false; importAccount(provider) })
                        .disabled(locked || service.directory?.recoveryNeeded == true)
                }
        }
        .sheet(item: $sheet) { item in
            AccountSheetView(item: item, close: { sheet = nil }).environmentObject(service)
        }
    }

    private var subtitle: String {
        guard let directory = service.directory else { return "" }
        let attention = directory.attentionCount
        return "\(directory.accounts.count) 个账号" + (attention > 0 ? " · \(attention) 个需要处理" : "")
    }

    private func selected(_ directory: AccountDirectory) -> ManagedAccount? {
        if let id = nav.selection, let account = directory.accounts.first(where: { $0.id == id }) { return account }
        return directory.accounts.first(where: \.needsAction) ?? directory.current("codex") ?? directory.accounts.first
    }

    @ViewBuilder private func list(_ directory: AccountDirectory) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            if let error = service.accountError, service.accountErrorID == nil || !directory.accounts.contains(where: { $0.id == service.accountErrorID }) {
                Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: error) {
                    Button("知道了") { service.clearAccountError() }.controlSize(.small)
                }
            }
            if let error = service.directoryError {
                Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: error)
            }
            if !directory.accounts.isEmpty { nowStrip(directory) }
            alerts(directory)
            if directory.accounts.isEmpty {
                emptyState
            }
            let shown = directory.accounts.filter { account in
                (nav.filter == nil || account.provider == nav.filter)
                    && (query.isEmpty || "\(account.label) \(account.identity ?? "")".localizedCaseInsensitiveContains(query))
            }
            ForEach(["codex", "claude"], id: \.self) { provider in
                let rows = shown.filter { $0.provider == provider }
                if !rows.isEmpty {
                    groupHeader(provider, count: rows.count, directory: directory)
                    AccountTable(accounts: rows, selected: selected(directory)?.id, reminders: Set(ExpiryReminder.all(rows).flatMap(\.itemIDs))) { id in
                        nav.selection = id
                    }
                }
            }
            if !directory.accounts.isEmpty && shown.isEmpty {
                Text("没有匹配「\(query)」的账号").foregroundStyle(.secondary).frame(maxWidth: .infinity).padding(.vertical, 24)
            }
            HStack(spacing: 6) {
                Image(systemName: "clock")
                Text("额度每 5 分钟自动查询；橙色圆点是 7 天内到期的重置次数。关闭窗口后继续在菜单栏运行。")
            }
            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: 正在使用

    private func nowStrip(_ directory: AccountDirectory) -> some View {
        let pi = service.snapshot?.providers.first { $0.id == "pi" }
        return HStack(spacing: 10) {
            ForEach(["codex", "claude"], id: \.self) { provider in
                if let current = directory.current(provider) {
                    nowCard(glyph: provider, title: "\(ProviderGlyph.name(provider)) 正在使用", name: current.displayName,
                            detail: nowDetail(current), tone: current.lowestPrimary.map { Meter.tone(Double($0)) } ?? .neutral) {
                        nav.selection = current.id
                    }
                } else if directory.accounts.contains(where: { $0.provider == provider }) {
                    nowCard(glyph: provider, title: "\(ProviderGlyph.name(provider))", name: "没有当前账号", detail: "在列表里选择一个设为当前", tone: .neutral) {}
                }
            }
            if let pi {
                nowCard(glyph: "pi", title: "pi 取用", name: pi.state == "configured" ? "跟随 Codex 当前" : pi.stateLabel,
                        detail: directory.current("codex").map { "新任务用 \($0.displayName)" } ?? pi.summary, tone: .neutral) {
                    nav.show(.clients)
                }
            }
        }
    }

    private func nowDetail(_ account: ManagedAccount) -> String {
        guard let window = account.primaryWindows.min(by: { $0.remaining < $1.remaining }) else {
            return account.quotaCaption ?? account.health.title
        }
        return "剩余 \(window.remaining)% · \(window.shortReset)重置"
    }

    private func nowCard(glyph: String, title: String, name: String, detail: String, tone: Tone, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                ProviderGlyph(provider: glyph, size: 28)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).font(.caption2).foregroundStyle(.secondary)
                    Text(name).font(.callout.weight(.semibold)).lineLimit(1)
                    Text(detail).font(.caption).monospacedDigit().lineLimit(1)
                        .foregroundStyle(tone == .warning || tone == .danger ? tone.color : Palette.muted)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.hairline))
        }
        .buttonStyle(.plain)
    }

    // MARK: 提醒

    @ViewBuilder private func alerts(_ directory: AccountDirectory) -> some View {
        if directory.recoveryNeeded {
            // Only a plain switch interrupted while writing the default login; desktop switches keep no record.
            Callout(icon: "arrow.uturn.backward.circle.fill", tone: .danger, title: "上次切换尚未确认",
                    message: "上次切换在写入默认登录时中断。结束相关任务后重试，AuthManager 会按实际登录状态收尾。") {
                Button("重试上次切换（结束任务后）") { Task { await service.accountOperation("accounts.recover") } }
                    .buttonStyle(.borderedProminent).controlSize(.small).padding(.top, 2)
            }
        }
        if let notice = directory.notice {
            Callout(icon: "info.circle.fill", tone: .warning, message: notice)
        }
        if let login = service.login {
            Callout(icon: login.isActive ? "person.badge.clock.fill" : "person.crop.circle.badge.checkmark", tone: .accent,
                    message: "\(ProviderGlyph.name(login.provider)) · \(login.phaseLabel)") {
                Button(login.isActive ? "查看登录进度" : "查看最近登录结果") {
                    sheet = .login(provider: login.provider, accountID: service.loginTarget?.accountID)
                }
                    .controlSize(.small)
            }
        }
        let urgent = directory.accounts.filter(\.needsAction)
        let reminders = ExpiryReminder.all(directory.accounts, dismissed: dismissed)
        let desktopBlocked = directory.desktop?.enabled == true && directory.desktop?.connected == false
        if !urgent.isEmpty || !reminders.isEmpty || desktopBlocked {
            VStack(spacing: 0) {
                ForEach(urgent) { account in
                    alertRow(tone: account.health.tone, icon: "exclamationmark.triangle.fill",
                             title: "\(account.displayName)：\(account.health.title)",
                             detail: account.health.detail ?? account.message) {
                        Button("查看") { nav.selection = account.id }
                        if let fix = fixAction(account, directory) {
                            Button(fix.0) { sheet = fix.1 }.buttonStyle(.borderedProminent).disabled(locked)
                        }
                    }
                }
                if desktopBlocked {
                    alertRow(tone: .warning, icon: "macwindow",
                             title: "Codex 桌面热切换已开启，但桌面没有连上",
                             detail: directory.strategy?.config.enabled == true
                                ? "现在手动和自动都换不了 Codex 账号；额度见底时只会重置。以调试模式启动桌面后恢复。"
                                : "现在换不了 Codex 账号。以调试模式启动桌面，或关闭热切换改用普通切换。") {
                        Button("去连接") { nav.show(.desktop) }
                    }
                }
                ForEach(reminders) { reminder in
                    alertRow(tone: .warning, icon: "ticket", title: reminder.title, detail: reminder.detail) {
                        Button("查看") { nav.selection = reminder.accountID }
                        Button("知道了") { ReminderStore.dismiss(reminder.itemIDs); dismissed = ReminderStore.dismissed() }
                    }
                }
            }
            .background(Palette.raised, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.hairline))
        }
    }

    private func fixAction(_ account: ManagedAccount, _ directory: AccountDirectory) -> (String, AccountSheet)? {
        switch directory.primaryAction(for: account) {
        case .recover: (account.recoverTitle, .recover(account.id))
        case .relogin: ("重新登录", .login(provider: account.provider, accountID: account.id))
        default: nil
        }
    }

    private func alertRow<Actions: View>(tone: Tone, icon: String, title: String, detail: String, @ViewBuilder actions: () -> Actions) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon).foregroundStyle(tone.color).frame(width: 18).padding(.top, 1)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.callout.weight(.semibold))
                Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            HStack(spacing: 6) { actions() }.controlSize(.small)
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .overlay(alignment: .bottom) { Rectangle().fill(Palette.hairline).frame(height: 1) }
    }

    // MARK: 分组

    private func groupHeader(_ provider: String, count: Int, directory: AccountDirectory) -> some View {
        HStack(spacing: 8) {
            ProviderGlyph(provider: provider, size: 20)
            Text(ProviderGlyph.name(provider)).font(.callout.weight(.semibold))
            Text("\(count)").font(.callout).foregroundStyle(.tertiary).monospacedDigit()
            Spacer()
            if provider == "codex", let strategy = directory.strategy {
                Button { nav.show(.automation) } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "bolt")
                        if strategy.config.enabled {
                            Text("主额度见底时自动处理")
                            if directory.desktop?.connected != true {
                                Text("· 桌面未连接，只会重置").foregroundStyle(Palette.warning)
                            }
                        } else {
                            Text("自动换号与重置已关闭")
                        }
                    }
                    .font(.caption)
                }
                .buttonStyle(.plain).foregroundStyle(.secondary)
            }
        }
        .padding(.top, 4)
    }

    private var emptyState: some View {
        VStack(spacing: 14) {
            Image(systemName: "person.crop.circle.badge.plus")
                .font(.system(size: 40, weight: .light)).foregroundStyle(Palette.accent)
            Text("把常用账号放到一个列表").font(.title3.weight(.semibold))
            Text("可以登录新账号独立保存，也可以导入这台 Mac 上官方客户端已有的登录；当前账号不会因此被替换。")
                .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center).frame(maxWidth: 440)
            HStack(spacing: 10) {
                Button("登录 Codex 账号") { sheet = .login(provider: "codex", accountID: nil) }
                Button("登录 Claude Code 账号") { sheet = .login(provider: "claude", accountID: nil) }
            }
            .disabled(locked || service.directory?.recoveryNeeded == true)
        }
        .frame(maxWidth: .infinity).padding(.vertical, 36).card()
    }

    private func importAccount(_ provider: String) {
        // A blank name lets the service name the account after its login email.
        Task { await service.accountOperation("accounts.import", params: LoginParams(provider: provider)) }
    }
}

// MARK: 账号表

struct AccountTable: View {
    let accounts: [ManagedAccount]
    let selected: String?
    let reminders: Set<String>
    let select: (String) -> Void

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Text("账号").frame(minWidth: 150, maxWidth: .infinity, alignment: .leading)
                Text("状态").frame(width: 108, alignment: .leading)
                Text("主额度剩余 · 下次重置").frame(minWidth: 210, maxWidth: .infinity, alignment: .leading)
                Text("重置次数").frame(width: 62, alignment: .leading)
            }
            .font(.system(size: 11)).foregroundStyle(.tertiary)
            .padding(.horizontal, 14).frame(height: 28)
            .background(Palette.raised)
            ForEach(accounts) { account in
                Divider()
                AccountRow(account: account, selected: account.id == selected,
                           soonCredits: Set(account.codexCredits.map(\.id)).intersection(reminders)) { select(account.id) }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.hairline))
    }
}

private struct AccountRow: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let selected: Bool
    let soonCredits: Set<String>
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(account.displayName).font(.callout.weight(.semibold)).lineLimit(1)
                        if account.native { Tag(text: "当前", tone: .accent, filled: true) }
                        if service.directory?.desktop?.selected == account.id { Tag(text: "桌面", tone: .accent) }
                    }
                    // The plan rides on the second line so an email-length name keeps the full column.
                    let detail = [account.identityLine, account.plan?.uppercased()].compactMap { $0 }.joined(separator: " · ")
                    Text(detail.isEmpty ? " " : detail).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                .frame(minWidth: 150, maxWidth: .infinity, alignment: .leading)
                HealthLabel(health: account.health).frame(width: 108, alignment: .leading)
                VStack(alignment: .leading, spacing: 5) {
                    if account.primaryWindows.isEmpty {
                        Text(account.quotaCaption ?? "—").font(.caption).foregroundStyle(.secondary)
                    } else {
                        ForEach(Array(account.primaryWindows.prefix(2).enumerated()), id: \.offset) { _, window in
                            QuotaLine(quota: window)
                        }
                        if let caption = account.quotaCaption {
                            Text(caption).font(.caption2).foregroundStyle(.tertiary)
                        }
                    }
                }
                .frame(minWidth: 210, maxWidth: .infinity, alignment: .leading)
                credits.frame(width: 62, alignment: .leading)
            }
            .padding(.horizontal, 14).padding(.vertical, 9)
            .frame(minHeight: 54)
            .background(selected ? Palette.accent.opacity(0.10) : hovering ? Palette.track.opacity(0.6) : .clear)
            .overlay(alignment: .leading) {
                if account.needsAction {
                    UnevenRoundedRectangle(bottomTrailingRadius: 3, topTrailingRadius: 3).fill(Palette.danger).frame(width: 3).padding(.vertical, 8)
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    @ViewBuilder private var credits: some View {
        if account.provider == "claude" {
            VStack(alignment: .leading, spacing: 0) {
                Text(account.creditCount.map { "\($0) 次" } ?? "—").font(.caption).monospacedDigit()
                Text("官方重置").font(.caption2).foregroundStyle(.tertiary)
            }
        } else if account.codexCredits.isEmpty, let count = account.usage?.resetCredits?.availableCount, count > 0 {
            // The details request failed but the usage summary still counts the credits; unknown expiry is not zero.
            HStack(spacing: 3) {
                ForEach(0..<min(count, 5), id: \.self) { _ in Circle().fill(Palette.muted).frame(width: 6, height: 6) }
                Text("\(count)").font(.caption).foregroundStyle(.secondary).monospacedDigit().padding(.leading, 2)
            }
            .help("官方这次没有返回明细，到期时间在下次查询后显示")
        } else if account.codexCredits.isEmpty {
            Text(account.usage?.resetCredits == nil ? "—" : "无").font(.caption).foregroundStyle(.tertiary)
        } else {
            HStack(spacing: 3) {
                ForEach(account.codexCredits.prefix(5)) { credit in
                    Circle().fill(soonCredits.contains(credit.id) ? Palette.warning : Palette.accent).frame(width: 6, height: 6)
                        .help("\(QuotaResetCredit.expiryLabel(credit.expiresAt)) 到期")
                }
                Text("\(account.codexCredits.count)").font(.caption).foregroundStyle(.secondary).monospacedDigit().padding(.leading, 2)
            }
        }
    }
}

// MARK: 添加账号

struct AddAccountChooser: View {
    let login: (String) -> Void
    let importCurrent: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("登录新账号（独立保存，不替换当前默认）").font(.caption.weight(.semibold)).foregroundStyle(.secondary).padding(.horizontal, 8).padding(.top, 4)
            AddOption(glyph: "codex", title: "Codex", detail: "官方设备授权，在浏览器中确认") { login("codex") }
            AddOption(glyph: "claude", title: "Claude Code", detail: "官方 Claude CLI 登录") { login("claude") }
            Divider().padding(.vertical, 6)
            Text("导入这台 Mac 已有的登录").font(.caption.weight(.semibold)).foregroundStyle(.secondary).padding(.horizontal, 8)
            AddOption(glyph: "codex", title: "当前 Codex 登录", detail: "按登录邮箱命名；已在列表中则只更新") { importCurrent("codex") }
            AddOption(glyph: "claude", title: "当前 Claude Code 登录", detail: "按登录邮箱命名；已在列表中则只更新") { importCurrent("claude") }
        }
        .padding(8).frame(width: 340)
    }
}

private struct AddOption: View {
    let glyph: String
    let title: String
    let detail: String
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 10) {
                ProviderGlyph(provider: glyph, size: 26)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.callout.weight(.medium))
                    Text(detail).font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8).padding(.vertical, 6)
            .contentShape(Rectangle())
            .background(hovering ? Palette.accent.opacity(0.1) : .clear, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}
