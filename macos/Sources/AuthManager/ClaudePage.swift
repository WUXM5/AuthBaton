import SwiftUI

// One Claude account in full: quota, reset cards, warmup, checks and rules on the left and right; the row of
// seals at the top moves between accounts without going back to the overview.
struct ClaudeAccountPage: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    let entry: ClaudeEntry
    let entries: [ClaudeEntry]
    let use: ClaudeUse
    let directory: AccountDirectory
    let open: (ClaudeSheet) -> Void
    let switchTo: (ClaudeEntry, SwitchScope?) -> Void

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 12) {
                    Button { nav.claudeDetail = nil } label: {
                        HStack(spacing: 4) {
                            Icon(.chevronLeft, size: 10, color: Palette.ink2, weight: 2.4)
                            Text("Claude")
                        }
                    }
                    .buttonStyle(.pill(.secondary, height: 27))
                    .keyboardShortcut("[", modifiers: .command)
                    if entries.count > 1 { picker }
                    Spacer(minLength: 7)
                    RefreshPill(checkedAt: entry.cli?.usage?.checkedAt, busy: service.isRefreshing) {
                        Task {
                            await service.refresh()
                            await service.refreshQuotaIfNeeded()
                        }
                    }
                }
                .padding(.top, 34)
                if use.code && directory.recoveryNeeded { RecoveryBanner() }
                ViewThatFits(in: .horizontal) {
                    HStack(alignment: .top, spacing: 14) {
                        main.frame(minWidth: 323, idealWidth: 323, maxWidth: .infinity)
                        side.frame(minWidth: 255, idealWidth: 255, maxWidth: 306)
                    }
                    VStack(spacing: 14) {
                        main
                        side
                    }
                }
            }
            .pageColumn()
        }
    }

    @ViewBuilder private var main: some View {
        if let account = entry.cli {
            ClaudeMain(account: account, entry: entry, use: use, directory: directory, open: { open(.account($0)) },
                       switchTo: { switchTo(entry, $0) }, deskLogin: { open(.desktop(.add)) })
        } else if let desk = entry.desk {
            DeskOnlyMain(entry: entry, desk: desk, use: use, directory: directory, open: open, switchTo: { switchTo(entry, $0) })
        }
    }

    @ViewBuilder private var side: some View {
        if let account = entry.cli {
            ClaudeSide(account: account, entry: entry, use: use, directory: directory, open: open)
        } else if let desk = entry.desk {
            DeskOnlySide(desk: desk, open: open)
        }
    }

    private var picker: some View {
        HStack(spacing: 3.5) {
            ForEach(entries) { item in
                let on = item.id == entry.id
                Button { nav.claudeDetail = item.id } label: {
                    HStack(spacing: 6) {
                        ClaudeSeal(entry: item, size: 19, rule: item.cli.flatMap { directory.rule(for: $0.id) })
                        Text(item.name).font(.system(size: 12, weight: on ? .semibold : .regular)).lineLimit(1)
                        if use == .both && use.code && item.inCode { Icon(.terminal, size: 10, color: Palette.deep, weight: 2.2) }
                        if use == .both && item.inDesk { Icon(.desktop, size: 10, color: Palette.deep, weight: 2) }
                        if item.cli?.needsAction == true { Circle().fill(Palette.attentionDot).frame(width: 6, height: 6) }
                    }
                    .padding(.leading, 5).padding(.trailing, 10).frame(height: 27)
                    .background {
                        if on { Capsule().fill(Palette.card).shadow(color: .black.opacity(0.1), radius: 1, y: 1) }
                    }
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(item.inUse ? "\(item.name)，在用" : item.name)
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
    let entry: ClaudeEntry
    let use: ClaudeUse
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void
    let switchTo: (SwitchScope?) -> Void
    let deskLogin: () -> Void

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
                    ClaudeSeal(entry: entry, size: 34, rule: rule)
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 7) {
                            Text(account.displayName).font(.system(size: 14, weight: .semibold)).lineLimit(1)
                            InUsePill(entry: entry, use: use)
                        }
                        Text(subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                            .textSelection(.enabled).lineLimit(1)
                    }
                    Spacer(minLength: 7)
                    AccountMenu(account: account, directory: directory, open: open, filled: true)
                }
                QuotaFigures(account: account, notch: directory.notch(for: account), size: 44, exact: true) {
                    Spacer(minLength: 0)
                    if let lag { Text(lag).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1) }
                    AccountActions(account: account, directory: directory, open: open, showSwitch: false)
                    ClaudeSwitchControl(entry: entry, use: use, directory: directory, switchTo: switchTo, login: deskLogin)
                }
                .padding(.top, 17)
            }
            .padding(.horizontal, 19).padding(.top, 17).padding(.bottom, 15)
            AccountStrips(account: account, directory: directory, open: open).padding(.horizontal, 19)
        }
        .loopCard(radius: 19, raised: true)
    }

    private var subtitle: String {
        let desk = use.desk ? entry.desk.map { desk in "桌面登录已保存" + (desk.number.map { "（#\($0)）" } ?? "") } : nil
        let parts = [account.subtitle, desk].compactMap { $0 }.filter { !$0.isEmpty }
        return parts.isEmpty ? " " : parts.joined(separator: " · ")
    }

    // Both clients in use and only Claude Code on this account: say where Claude 桌面 still is.
    private var lag: String? {
        guard use == .both, entry.inCode, !entry.inDesk, entry.desk != nil,
              let other = directory.claudeDesktop?.current?.display else { return nil }
        return "桌面还在用 \(other)"
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

    // Official reset cards by kind; a rule spends the official next one, the card's button the kind chosen.
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
                    Text("这次没返回剩余张数，不能按 0 张算。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
                ForEach(Array(grants.items.enumerated()), id: \.offset) { _, grant in
                    let soon = QuotaResetCredit.date(grant.endsAt).map { $0.timeIntervalSinceNow <= ExpiryReminder.window } ?? false
                    HStack(spacing: 12) {
                        // Kinds differ in name length; a common width keeps the dates in one column.
                        Ticket(text: ClaudeResetKind.counted(grant.remaining, grant.kind), soon: soon && grant.remaining > 0, large: true)
                            .frame(minWidth: 124, alignment: .leading)
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
                Text("官方还没返回重置信息，立即查询后再看。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            if let reset = account.reset, !reset.unresolved, let when = reset.finishedAt ?? reset.startedAt,
               Date().timeIntervalSince1970 - when >= 600 {
                Text("上次重置：\(ResetHistoryEntry.outcomeText(reset.outcome)) · \(When.text(when))")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
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
    let entry: ClaudeEntry
    let use: ClaudeUse
    let directory: AccountDirectory
    let open: (ClaudeSheet) -> Void

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
        let sending = plan?.running == true
        let save: (Bool, Int) -> Void = { enabled, hour in
            Task { await service.accountOperation("accounts.warmup", params: LoginParams(id: account.id, enabled: enabled, hour: hour)) }
        }
        // Rounded to the minute: a window resets a second before its 10-minute mark.
        let next = plan.flatMap { $0.enabled ? $0.next : nil }.map { Date(timeIntervalSince1970: ($0 / 60).rounded() * 60) }
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 9) {
                Icon(.sun, size: 15, color: Palette.ink2)
                Text("5 小时窗口预热").font(.system(size: 13, weight: .semibold))
                    .help("从每天设定的时间起，5 小时窗口没有重置时间时，用最小的模型发一条短消息，让窗口开始计时；窗口一结束就接着发，一天最多 4 个窗口，最后一个在第二天设定时间前结束。7 天额度到了见底线就不发。例：10 点开工就设 6 点。")
                Spacer(minLength: 7)
                Toggle("5 小时窗口预热", isOn: Binding(get: { plan?.enabled == true }, set: { save($0, plan?.hour ?? 6) }))
                    .labelsHidden().toggleStyle(.switch).controlSize(.small)
            }
            if let plan, plan.enabled {
                HStack(spacing: 9) {
                    Text("每天从几点开始").font(.system(size: 12)).foregroundStyle(Palette.ink2)
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
            } else {
                Text("到点发一条短消息，窗口结束后接着发。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    .padding(.top, 5)
            }
            VStack(alignment: .leading, spacing: 3) {
                if sending {
                    HStack(spacing: 5) {
                        ProgressView().controlSize(.mini)
                        Text("正在发送…")
                    }
                } else {
                    if let last = plan?.last {
                        Text("\(When.text(last.at)) · \(last.message)")
                            .foregroundStyle(last.ok ? Palette.ink3 : Palette.attentionText)
                            .help(When.exact(last.at))
                    }
                    if let next {
                        Text("下次约 \(When.text(next))").help(When.exact(next))
                    }
                }
            }
            .font(.system(size: 11)).foregroundStyle(Palette.ink3)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.top, 7)
            Button(sending ? "预热中" : "立即预热") {
                Task { await service.accountOperation("accounts.warmup_now", params: LoginParams(id: account.id)) }
            }
            .buttonStyle(.pill(.secondary))
            .help("现在发一条，不看窗口状态，失败不重试")
            .disabled(sending)
            .padding(.top, 12)
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
        let desk = use.desk ? entry.desk : nil
        if let strategy = directory.strategy, use.code {
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
                if let desk {
                    Rectangle().fill(Palette.divider).frame(height: 1).padding(.vertical, 2)
                    NextToggle(desk: desk)
                }
            }
            .padding(.horizontal, 17).padding(.vertical, 15)
            .loopCard()
        } else if let desk {
            NextToggle(desk: desk)
                .padding(.horizontal, 17).padding(.vertical, 13)
                .loopCard()
        }
    }

    // MARK: 移除

    private var removal: some View {
        let blocker = account.removeBlocker(directory)
        return VStack(alignment: .leading, spacing: 2) {
            Button("从 AuthBaton 移除…", role: .destructive) { open(.account(.remove(account.id))) }
                .buttonStyle(.pill(.danger))
                .disabled(blocker != nil || locked)
            Text((blocker ?? "") + "只删 AuthBaton 保存的记录和凭据，不注销官方账号。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
            if use.desk, let desk = entry.desk {
                DeskRemoval(desk: desk, open: open).padding(.top, 9)
            }
        }
        .padding(.horizontal, 3.5)
    }
}

// MARK: 桌面这一端

// Whether 下一个账号 stops at this account in Claude 桌面.
private struct NextToggle: View {
    @EnvironmentObject private var service: ServiceController
    let desk: ClaudeDesktopStatus.Account

    var body: some View {
        HStack(spacing: 9) {
            Text("参与「下一个」").font(.system(size: 12)).foregroundStyle(Palette.ink2)
            if let number = desk.number {
                Text("#\(number)").font(.system(size: 11, weight: .semibold, design: .rounded)).monospacedDigit().foregroundStyle(Palette.ink3)
            }
            Spacer(minLength: 7)
            Toggle("参与「下一个」", isOn: Binding(get: { desk.participates }, set: { value in
                Task { await service.claudeDesktop(value ? "enable" : "disable", account: desk.handle) }
            }))
            .labelsHidden().toggleStyle(.switch).controlSize(.small)
            .disabled(service.accountBusy || service.directory?.claudeDesktop?.busy == true)
        }
        .help("Claude 桌面的「下一个账号」按编号轮；关掉后跳过这个账号，仍可直接换上")
    }
}

private struct DeskRemoval: View {
    @EnvironmentObject private var service: ServiceController
    let desk: ClaudeDesktopStatus.Account
    let open: (ClaudeSheet) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Button("移除桌面登录…", role: .destructive) { open(.desktop(.remove(desk.handle))) }
                .buttonStyle(.pill(.danger))
                .disabled(desk.inUse || service.accountBusy || service.directory?.claudeDesktop?.busy == true)
            Text(desk.inUse ? "Claude 桌面正在用这个登录，先换到别的账号再移除。" : "只删 AuthBaton 存的这份桌面登录，会话不动。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
        }
    }
}

// An account only Claude 桌面 knows: the hero says how to get its quota, the switch still works.
private struct DeskOnlyMain: View {
    let entry: ClaudeEntry
    let desk: ClaudeDesktopStatus.Account
    let use: ClaudeUse
    let directory: AccountDirectory
    let open: (ClaudeSheet) -> Void
    let switchTo: (SwitchScope?) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                ClaudeSeal(entry: entry, size: 34)
                VStack(alignment: .leading, spacing: 1) {
                    HStack(spacing: 7) {
                        Text(entry.name).font(.system(size: 14, weight: .semibold)).lineLimit(1)
                        InUsePill(entry: entry, use: use)
                    }
                    Text([desk.email, desk.number.map { "桌面登录已保存（#\($0)）" } ?? "桌面登录已保存"].compactMap { $0 }.joined(separator: " · "))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).textSelection(.enabled).lineLimit(1)
                }
                Spacer(minLength: 7)
                Button("重命名…") { open(.desktop(.rename(desk.handle))) }.buttonStyle(.pill(.secondary))
            }
            DeskOnlyQuota(desk: desk, login: { open(.account(.login(provider: "claude", accountID: nil))) }, turnOn: { open(.tokenQuota) })
                .padding(.top, 17)
            HStack {
                Spacer()
                ClaudeSwitchControl(entry: entry, use: use, directory: directory, switchTo: switchTo, login: { open(.desktop(.add)) })
            }
            .padding(.top, 12)
        }
        .padding(.horizontal, 19).padding(.top, 17).padding(.bottom, 15)
        .loopCard(radius: 19, raised: entry.inUse)
    }
}

private struct DeskOnlySide: View {
    let desk: ClaudeDesktopStatus.Account
    let open: (ClaudeSheet) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 7) {
                NextToggle(desk: desk)
                Text("预热、自动检查与续期、自动换号都要先补登到 AuthBaton。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 17).padding(.vertical, 13)
            .loopCard()
            DeskRemoval(desk: desk, open: open).padding(.horizontal, 3.5)
        }
    }
}
