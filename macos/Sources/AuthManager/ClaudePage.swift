import SwiftUI

// Claude Code: one account at a time in full; with more than one, a row of seals picks which.
struct ClaudePage: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @State private var sheet: AccountSheet?

    var body: some View {
        Group {
            if let directory = service.directory {
                let accounts = directory.relay("claude")
                let account = accounts.first { $0.id == nav.selection } ?? directory.current("claude") ?? accounts.first
                PageScaffold(title: "Claude Code", subtitle: Text(subtitle(directory, accounts)), provider: "claude") {
                    RefreshPill(checkedAt: accounts.compactMap { $0.usage?.checkedAt }.max(), busy: service.isRefreshing) {
                        Task {
                            await service.refresh()
                            await service.refreshQuotaIfNeeded()
                        }
                    }
                    Button { sheet = .login(provider: "claude", accountID: nil) } label: {
                        HStack(spacing: 5) {
                            Icon(.plus, size: 12, color: .white, weight: 2)
                            Text("添加 Claude 账号")
                        }
                    }
                    .buttonStyle(.pill(.primary, height: 27))
                    .disabled(service.accountBusy || directory.busy || directory.recoveryNeeded)
                } content: {
                    if directory.recoveryNeeded { RecoveryBanner() }
                    if accounts.count > 1, let account { picker(accounts, selected: account.id) }
                    if let account {
                        ViewThatFits(in: .horizontal) {
                            HStack(alignment: .top, spacing: 14) {
                                ClaudeMain(account: account, directory: directory) { sheet = $0 }
                                    .frame(minWidth: 323, idealWidth: 323, maxWidth: .infinity)
                                ClaudeSide(account: account, directory: directory) { sheet = $0 }
                                    .frame(minWidth: 255, idealWidth: 255, maxWidth: 306)
                            }
                            VStack(spacing: 14) {
                                ClaudeMain(account: account, directory: directory) { sheet = $0 }
                                ClaudeSide(account: account, directory: directory) { sheet = $0 }
                            }
                        }
                    } else {
                        AddCard(provider: "claude", open: { sheet = $0 }, lead: "还没有 Claude Code 账号", detail: "登录一个，或导入这台 Mac 上已有的登录。")
                    }
                }
            } else {
                Color.clear
            }
        }
        .task { await service.refreshQuotaIfNeeded() }
        .sheet(item: $sheet) { item in
            AccountSheetView(item: item, close: { sheet = nil }).environmentObject(service)
        }
    }

    private func subtitle(_ directory: AccountDirectory, _ accounts: [ManagedAccount]) -> String {
        "\(accounts.count) 个账号" + (directory.current("claude").map { " · 在用 \($0.displayName)" } ?? " · 还没有在用的账号")
    }

    private func picker(_ accounts: [ManagedAccount], selected: String) -> some View {
        HStack(spacing: 3.5) {
            ForEach(accounts) { account in
                let on = account.id == selected
                Button { nav.selection = account.id } label: {
                    HStack(spacing: 7) {
                        Seal(account.displayName, style: account.native ? .inUse : .standby, size: 19)
                        Text(account.displayName).font(.system(size: 12, weight: on ? .semibold : .regular)).lineLimit(1)
                        if account.native { Pill("在用", style: .brand, small: true) }
                        if account.needsAction { Circle().fill(Palette.attentionDot).frame(width: 6, height: 6) }
                    }
                    .padding(.leading, 5).padding(.trailing, 10).frame(height: 27)
                    .background {
                        if on { Capsule().fill(Palette.card).shadow(color: .black.opacity(0.1), radius: 1, y: 1) }
                    }
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(on ? .isSelected : [])
            }
        }
        .padding(3)
        .background(Palette.rail, in: Capsule())
    }
}

// MARK: 左栏

private struct ClaudeMain: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            hero
            if !account.extraWindows.isEmpty { extra }
            resets
        }
    }

    private var hero: some View {
        let rule = directory.rule(for: account.id)
        return VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 10) {
                    Seal(account.displayName, style: account.native ? .inUse : rule?.participate == false ? .out : .standby, size: 34)
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 7) {
                            Text(account.displayName).font(.system(size: 14, weight: .semibold)).lineLimit(1)
                            if account.native { Pill("在用", style: .brand, small: true) }
                        }
                        Text(account.subtitle.isEmpty ? " " : account.subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                            .textSelection(.enabled).lineLimit(1)
                    }
                    Spacer(minLength: 7)
                    AccountMenu(account: account, directory: directory, open: open, filled: true)
                }
                QuotaFigures(account: account, notch: directory.notch(for: account), size: 44, exact: true) {
                    Spacer(minLength: 0)
                    AccountActions(account: account, directory: directory, open: open)
                }
                .padding(.top, 17)
            }
            .padding(.horizontal, 19).padding(.top, 17).padding(.bottom, 15)
            AccountStrips(account: account, directory: directory, open: open).padding(.horizontal, 19)
        }
        .loopCard(radius: 19, raised: true)
    }

    // Model or feature windows of their own; they never decide a switch.
    private var extra: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 5) {
                Text("独立额度").font(.system(size: 13, weight: .semibold))
                Icon(.info, size: 12, color: Palette.ink3).help("模型或功能单独的额度，不参与自动换号判断")
            }
            .padding(.bottom, 3.5)
            ForEach(Array(account.extraWindows.enumerated()), id: \.offset) { index, window in
                HStack(spacing: 12) {
                    (Text(window.scope ?? window.label) + Text(window.scope == nil ? "" : " · \(window.label)").foregroundStyle(Palette.ink3))
                        .font(.system(size: 12)).lineLimit(1)
                        .frame(minWidth: 102, maxWidth: .infinity, alignment: .leading)
                    QuotaBar(remaining: window.remaining, height: 4.5).frame(minWidth: 68, maxWidth: 136)
                    Text("\(window.remaining)%").font(.system(size: 12, weight: .semibold, design: .rounded)).monospacedDigit()
                        .lineLimit(1).frame(width: 40, alignment: .trailing)
                    Text(window.refill(compact: false)).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                        .frame(width: 99, alignment: .leading)
                        .help(window.resetDate.map(When.exact) ?? "")
                }
                .frame(minHeight: 37)
                .overlay(alignment: .top) { if index > 0 { Rectangle().fill(Palette.divider).frame(height: 1) } }
            }
        }
        .padding(.horizontal, 17).padding(.vertical, 15)
        .loopCard()
    }

    // Official reset cards: shown here, spent only by the official client or by a rule.
    private var resets: some View {
        let status = account.usage?.claudeResets
        return VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("重置卡").font(.system(size: 13, weight: .semibold))
                Spacer()
                Button("执行记录 ›") { open(.history(account.id)) }.buttonStyle(.pill(.link))
            }
            if let grants = status?.grants {
                if !grants.eligible { Text(grants.message).font(.system(size: 11)).foregroundStyle(Palette.ink3) }
                if grants.remaining == nil {
                    Text("剩余张数这次没返回，不能按 0 张算。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
                ForEach(Array(grants.items.enumerated()), id: \.offset) { _, grant in
                    let soon = QuotaResetCredit.date(grant.endsAt).map { $0.timeIntervalSinceNow <= ExpiryReminder.window } ?? false
                    HStack(spacing: 12) {
                        Ticket(text: grant.remaining > 0 ? "\(grant.remaining) 张重置卡" : "没有重置卡", soon: soon && grant.remaining > 0, large: true)
                        VStack(alignment: .leading, spacing: 0) {
                            Text("有效期至 \(QuotaResetCredit.expiryLabel(grant.endsAt))").font(.system(size: 12)).foregroundStyle(Palette.ink2)
                            Text(usage(grant, eligible: grants.eligible)).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .help(grant.label)
                }
                if grants.items.isEmpty, let remaining = grants.remaining {
                    Ticket(text: remaining > 0 ? "\(remaining) 张重置卡" : "没有重置卡", large: true)
                }
                if let until = grants.cooldownUntil {
                    Text("冷却到 \(QuotaResetCredit.expiryLabel(until))").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            }
            if let weekly = status?.weekly {
                VStack(alignment: .leading, spacing: 2) {
                    Text("每周会话额度重置").font(.system(size: 12, weight: .semibold))
                    Text([weekly.perWeek.map { "每周 \($0) 次" } ?? "每周次数没返回",
                          weekly.available.map { $0 ? "现在可用" : "现在不可用" } ?? "可用状态没返回"].joined(separator: " · "))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    if let next = weekly.nextAvailableAt {
                        Text("下次可用：\(QuotaResetCredit.expiryLabel(next))").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                }
            }
            if status?.grants == nil && status?.weekly == nil {
                Text("官方还没返回重置信息；可以立即查询后再看。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            if let reset = account.reset, !reset.unresolved, let when = reset.finishedAt ?? reset.startedAt,
               Date().timeIntervalSince1970 - when >= 600 {
                Text("上次重置：\(ResetHistoryEntry.outcomeText(reset.outcome)) · \(When.text(when))")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            Text("手动使用请到 Claude 官方客户端；也可以在换号与重置里设成见底时自动用。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 17).padding(.vertical, 15)
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopCard()
    }

    private func usage(_ grant: ClaudeResetGrant, eligible: Bool) -> String {
        var parts = [grant.limits.isEmpty ? "适用额度没返回" : "适用 \(grant.limits.joined(separator: "、"))"]
        if let requires = grant.requiresLimit { parts.append(requires ? "到上限后才能用" : "可以提前用") }
        parts.append(grant.availability(eligible: eligible))
        return parts.joined(separator: " · ")
    }
}

// MARK: 右栏

private struct ClaudeSide: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void

    private var locked: Bool { service.accountBusy || directory.busy || directory.recoveryNeeded }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            warmup
            maintenance
            rules
            removal
        }
    }

    // MARK: 5 小时窗口预热

    private var warmup: some View {
        let plan = account.warmup
        let save: (Bool, Int) -> Void = { enabled, hour in
            Task { await service.accountOperation("accounts.warmup", params: LoginParams(id: account.id, enabled: enabled, hour: hour)) }
        }
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 9) {
                Icon(.sun, size: 15, color: Palette.ink2)
                Text("5 小时窗口预热").font(.system(size: 13, weight: .semibold))
                    .help("到点用最小的模型发一条短消息，让 5 小时窗口提前开始计时。例：10 点开工就设 6 点。电脑睡眠错过时，在设定时间后 5 小时内醒来会补发，再晚就等第二天。")
                Spacer(minLength: 7)
                Toggle("5 小时窗口预热", isOn: Binding(get: { plan?.enabled == true }, set: { save($0, plan?.hour ?? 6) }))
                    .labelsHidden().toggleStyle(.switch).controlSize(.small)
            }
            if let plan, plan.enabled {
                HStack(spacing: 9) {
                    Text("每天几点").font(.system(size: 12)).foregroundStyle(Palette.ink2)
                    Spacer()
                    Menu {
                        Picker("预热时间", selection: Binding(get: { plan.hour }, set: { save(true, $0) })) {
                            ForEach(0..<24, id: \.self) { Text(String(format: "%02d:00", $0)).tag($0) }
                        }
                        .pickerStyle(.inline).labelsHidden()
                    } label: {
                        HStack(spacing: 7) {
                            Text(String(format: "%02d:00", plan.hour)).font(.system(size: 12, weight: .semibold)).monospacedDigit()
                            Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4)
                        }
                        .padding(.leading, 10).padding(.trailing, 9).frame(height: 26)
                        .background(Palette.field, in: Capsule())
                        .contentShape(Capsule())
                    }
                    .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
                }
                .padding(.top, 10)
                Group {
                    if plan.running {
                        HStack(spacing: 5) {
                            ProgressView().controlSize(.mini)
                            Text("正在发送…")
                        }
                    } else if let last = plan.last {
                        Text("\(When.text(last.at)) · \(last.message)")
                            .foregroundStyle(last.ok ? Palette.ink3 : Palette.attentionText)
                            .help(When.exact(last.at))
                    }
                }
                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 7)
            } else {
                Text("到点发一条短消息，让 5 小时窗口提前开始计时。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    .padding(.top, 5)
            }
        }
        .disabled(service.accountBusy)
        .padding(.horizontal, 17).padding(.vertical, 15)
        .loopCard()
    }

    // MARK: 自动检查与续期

    private var maintenance: some View {
        let checked = [account.lastLocal, account.lastOnline].compactMap { $0 }.max()
        let paused = account.automatic && account.state == "reauth"
        let checkDisabled = service.accountBusy || (directory.checksBlocked ?? true) || account.checkStatus != nil
        var stamps: [(String, Double?, String)] = [
            ("最近检查", checked, [("本地检查", account.lastLocal), ("认证检查", account.lastOnline), ("最近续期", account.lastRefresh)]
                .map { "\($0.0) \($0.1.map(When.exact) ?? "暂无记录")" }.joined(separator: "\n")),
        ]
        if account.automatic && !["reauth", "attention", "pending"].contains(account.state) {
            stamps.append(("下次自动处理", account.nextCheck, When.exact(account.nextCheck)))
        }
        if let expiry = account.expiresAt { stamps.append(("Access 到期", expiry, When.exact(expiry))) }
        if let expiry = account.refreshExpiresAt { stamps.append(("登录期限", expiry, When.exact(expiry))) }
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 9) {
                Icon(.shield, size: 15, color: Palette.ink2)
                Text("自动检查与续期").font(.system(size: 13, weight: .semibold))
                Spacer(minLength: 7)
                Toggle("自动检查与续期", isOn: Binding(get: { account.automatic }, set: { value in
                    Task { await service.accountOperation("accounts.update", params: LoginParams(id: account.id, automatic: value)) }
                }))
                .labelsHidden().toggleStyle(.switch).controlSize(.small)
                .disabled(locked)
            }
            Text(paused ? "开着，但要先处理上面的问题才会继续" : account.renewalLabel ?? "定期联网检查，快到期时续期")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                .padding(.leading, 24).padding(.top, 3.5)
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                ForEach(stamps, id: \.0) { stamp in
                    GridRow {
                        Text(stamp.0).foregroundStyle(Palette.ink3)
                        Text(stamp.1.map { When.text($0) } ?? "暂无记录").monospacedDigit()
                            .foregroundStyle(stamp.1 == nil ? Palette.ink3 : Palette.ink)
                            .frame(maxWidth: .infinity, alignment: .trailing)
                            .help(stamp.2)
                    }
                }
            }
            .font(.system(size: 11))
            .padding(.top, 10)
            HStack(spacing: 7) {
                Button("本地检查") { check(online: false) }.buttonStyle(.pill(.secondary)).help("只读本机凭据，不联网")
                    .disabled(checkDisabled)
                Button("认证检查") { check(online: true) }.buttonStyle(.pill(.secondary)).help("联网验证，必要时和官方客户端协同续期")
                    .disabled(checkDisabled || account.state == "unsaved")
            }
            .padding(.top, 12)
            if let notice = account.loginNotice {
                Text(notice.message).font(.system(size: 11))
                    .foregroundStyle(notice.needsAttention ? Palette.attentionText : Palette.ink3)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 9)
            }
        }
        .padding(.horizontal, 17).padding(.vertical, 15)
        .loopCard()
    }

    private func check(online: Bool) {
        Task { await service.accountOperation("accounts.check", params: LoginParams(id: account.id, online: online)) }
    }

    // MARK: 换号与重置

    @ViewBuilder private var rules: some View {
        if let strategy = directory.strategy {
            let rule = directory.rule(for: account.id)
            VStack(alignment: .leading, spacing: 9) {
                HStack {
                    Text("换号与重置").font(.system(size: 13, weight: .semibold))
                    Spacer()
                    Button("编辑规则 ›") { AppNavigation.shared.show(.automation, select: account.id) }.buttonStyle(.pill(.link))
                }
                if let rule {
                    HStack(spacing: 5) {
                        let style: Pill.Style = rule.participate ? .neutral : .dashed
                        if let rank = directory.rank(of: account) { Pill("第 \(rank) 位", style: style) }
                        Pill("≤ \(rule.below)%", style: style)
                        Pill(rule.order.label, style: style).help(rule.order.explanation)
                        Pill(rule.participate ? "参与" : "不参与")
                    }
                    if strategy.config.enabled, let message = strategy.accountStates?.first(where: { $0.id == account.id })?.message {
                        Text(message).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                    }
                } else {
                    Text("还没有这个账号的规则；到「换号与重置」保存一次就会加入。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
                if !strategy.config.enabled {
                    Text("自动换号与重置现在关着。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            }
            .padding(.horizontal, 17).padding(.vertical, 15)
            .loopCard()
        }
    }

    // MARK: 移除

    private var removal: some View {
        let blocker = account.removeBlocker(directory)
        return VStack(alignment: .leading, spacing: 2) {
            Button("从 AuthManager 移除…", role: .destructive) { open(.remove(account.id)) }
                .buttonStyle(.pill(.danger))
                .disabled(blocker != nil || locked)
            Text((blocker ?? "") + "只删 AuthManager 保存的记录和凭据，不注销官方账号。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 3.5)
    }
}
