import SwiftUI

struct AccountInspector: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount?
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void

    private var locked: Bool { service.accountBusy || directory.busy || directory.recoveryNeeded }

    var body: some View {
        ScrollView {
            if let account {
                VStack(alignment: .leading, spacing: 18) {
                    header(account)
                    quota(account)
                    if account.provider == "codex" {
                        codexCredits(account)
                    } else {
                        ClaudeGrantsSection(status: account.usage?.claudeResets, history: { open(.history(account.id)) }) {
                            if let reset = account.reset { lastReset(reset, account: account) }
                        }
                        warmup(account)
                    }
                    automation(account)
                    maintenance(account)
                    removal(account)
                }
                .padding(18)
            } else {
                ContentUnavailableView("选择一个账号", systemImage: "person.crop.circle", description: Text("在左侧选中账号后，在这里查看额度并处理。"))
                    .padding(.top, 80)
            }
        }
        .background(Palette.card)
    }

    // MARK: 头部与主操作

    private func header(_ account: ManagedAccount) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                ProviderGlyph(provider: account.provider, size: 34)
                VStack(alignment: .leading, spacing: 3) {
                    Text(account.displayName).font(.system(size: 17, weight: .semibold)).lineLimit(2)
                    if let identity = account.identityLine { Text(identity).font(.callout).foregroundStyle(.secondary).textSelection(.enabled) }
                    HStack(spacing: 6) {
                        if account.native { Tag(text: "当前默认", tone: .accent, filled: true) }
                        if directory.desktop?.selected == account.id { Tag(text: "桌面使用中", tone: .accent) }
                        if let plan = account.plan { Tag(text: plan.uppercased()) }
                        HealthLabel(health: account.health)
                    }
                    .padding(.top, 2)
                }
            }
            HStack(spacing: 6) {
                primaryButton(account)
                moreMenu(account)
            }
            if let detail = account.health.detail ?? (account.message.isEmpty ? nil : account.message) {
                Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            if account.provider == "codex", directory.primaryAction(for: account) == .switchTo, !account.native,
               directory.switchRoute(for: account) == .desktopDisconnected {
                Text("热切换开着但 Codex 桌面没连上，现在切不过去。点按钮看两种处理办法。")
                    .font(.caption).foregroundStyle(Palette.warning).fixedSize(horizontal: false, vertical: true)
            }
            if let action = directory.desktop?.action, action.id == account.id {
                HStack(alignment: .top, spacing: 6) {
                    if action.pending { ProgressView().controlSize(.mini) }
                    // A finished attempt carries its time, so it reads as history rather than current state.
                    Text((action.pending ? "" : action.at.map { "\(When.text($0)) · " } ?? "") + action.message)
                        .font(.caption).foregroundStyle(action.state == "failed" ? Palette.warning : Palette.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            if service.accountErrorID == account.id, let error = service.accountError {
                Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: error) {
                    Button("知道了") { service.clearAccountError() }.controlSize(.small)
                }
            }
        }
    }

    @ViewBuilder private func primaryButton(_ account: ManagedAccount) -> some View {
        switch directory.primaryAction(for: account) {
        case .recover:
            wide(account.recoverTitle, icon: "arrow.triangle.2.circlepath") { open(.recover(account.id)) }
                .disabled(locked)
        case .relogin:
            wide("重新登录", icon: "person.badge.key") { open(.login(provider: account.provider, accountID: account.id)) }
                .disabled(locked)
        case .switchTo:
            wide(account.native ? "接管 Codex 桌面" : "设为 \(account.providerName) 当前账号", icon: "arrow.left.arrow.right") { open(.switchTo(account.id)) }
                .disabled(locked)
        case .wait:
            // The detail line below says what it is waiting for.
            wide(account.health.title, icon: "hourglass") {}.disabled(true)
        case .current:
            wide("当前默认账号", icon: "checkmark.circle") {}.disabled(true)
        }
    }

    private func wide(_ title: String, icon: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(title, systemImage: icon).frame(maxWidth: .infinity)
        }
        .buttonStyle(.borderedProminent).controlSize(.large)
    }

    private func moreMenu(_ account: ManagedAccount) -> some View {
        let checkDisabled = service.accountBusy || (directory.checksBlocked ?? true) || account.checkStatus != nil
        return Menu {
            Button("重新登录…") { open(.login(provider: account.provider, accountID: account.id)) }
            Button("重命名…") { open(.rename(account.id)) }
            Divider()
            Button("本地检查（只读本机凭据，不联网）") { check(account, online: false) }.disabled(checkDisabled)
            Button(account.provider == "claude" ? "认证检查（联网验证，必要时协同续期）" : "认证检查（联网验证，必要时续期）") { check(account, online: true) }
                .disabled(checkDisabled || account.state == "unsaved")
            Divider()
            Button("从 AuthManager 移除…", role: .destructive) { open(.remove(account.id)) }.disabled(removeBlocker(account) != nil)
        } label: {
            Image(systemName: "ellipsis")
        }
        .menuStyle(.button).menuIndicator(.hidden).controlSize(.large).fixedSize()
        .disabled(locked)
        .accessibilityLabel("更多操作")
    }

    private func check(_ account: ManagedAccount, online: Bool) {
        Task { await service.accountOperation("accounts.check", params: LoginParams(id: account.id, online: online)) }
    }

    // MARK: 额度

    private func quota(_ account: ManagedAccount) -> some View {
        let pending = service.quotaRequests.contains(account.id) || account.usage?.refreshing == true
        return VStack(alignment: .leading, spacing: 0) {
            SectionLabel(title: "额度") {
                Button(pending ? "查询中…" : "立即查询") { Task { await service.checkQuota(id: account.id) } }
                    .disabled(pending).help("立即查询这个账号的额度，不影响认证")
            }
            VStack(alignment: .leading, spacing: 12) {
                if let error = service.quotaErrors[account.id] {
                    Label(error, systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(Palette.warning)
                }
                ForEach(Array((account.usage?.windows ?? []).enumerated()), id: \.offset) { _, window in
                    VStack(alignment: .leading, spacing: 5) {
                        HStack(alignment: .firstTextBaseline) {
                            Text(window.title).font(.callout).lineLimit(1).truncationMode(.middle)
                            if window.kind != nil { Tag(text: "独立") }
                            Spacer(minLength: 8)
                            let tone = Meter.tone(Double(window.remaining))
                            Text("剩余 \(window.remaining)%").font(.callout.weight(.semibold)).monospacedDigit()
                                .foregroundStyle(tone == .accent ? Color.primary : tone.color)
                        }
                        Meter(remaining: Double(window.remaining))
                        Text(window.resetCaption).font(.caption).foregroundStyle(.secondary)
                    }
                }
                if let usage = account.usage {
                    if usage.state != "ready" {
                        Text(usage.message).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                    if let checked = usage.checkedAt {
                        Text("\(When.text(checked))查询\(usage.state == "ready" ? "" : "（上次结果）") · 每 5 分钟自动查询")
                            .font(.caption2).foregroundStyle(.tertiary).help(When.exact(checked))
                    }
                } else {
                    Text("尚未查询额度").font(.caption).foregroundStyle(.secondary)
                }
                if !account.extraWindows.isEmpty {
                    Text("「独立」是模型或功能单独的额度，不参与自动换号判断。").font(.caption2).foregroundStyle(.tertiary)
                }
            }
            .panel(padding: 12)
        }
    }

    // MARK: 重置次数（Codex）

    private func codexCredits(_ account: ManagedAccount) -> some View {
        let usage = account.usage
        let resetDisabled = locked || directory.maintenanceRunning != true || usage?.refreshing == true
            || usage?.state != "ready" || account.reset?.unresolved == true
        return VStack(alignment: .leading, spacing: 0) {
            SectionLabel(title: "重置次数") {
                Button("执行记录") { open(.history(account.id)) }
            }
            VStack(alignment: .leading, spacing: 0) {
                if let credits = usage?.resetCredits {
                    let list = account.codexCredits
                    if list.isEmpty {
                        Text(credits.availableCount > 0 ? "有 \(credits.availableCount) 次，但官方没有返回明细；请立即查询后重试。" : "暂无可用次数")
                            .font(.callout).foregroundStyle(.secondary)
                    }
                    ForEach(Array(list.enumerated()), id: \.element.id) { index, credit in
                        if index > 0 { Divider().padding(.vertical, 7) }
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("可用 1 次").font(.callout)
                                let soon = credit.expiryDate.map { $0.timeIntervalSinceNow <= ExpiryReminder.window } ?? false
                                Text("\(QuotaResetCredit.expiryLabel(credit.expiresAt)) 到期" + (credit.expiryDate.flatMap { When.countdown(to: $0) }.map { " · \($0)" } ?? ""))
                                    .font(.caption).foregroundStyle(soon ? Palette.warning : Palette.muted)
                            }
                            Spacer()
                            Button("使用…") {
                                open(.reset(accountID: account.id, creditID: credit.id, requestID: UUID().uuidString, expiresAt: credit.expiresAt, retry: false))
                            }
                            .controlSize(.small).disabled(resetDisabled)
                        }
                    }
                    if list.count < credits.availableCount, !list.isEmpty {
                        Text("仅返回部分明细，总次数以官方为准。").font(.caption2).foregroundStyle(.secondary).padding(.top, 6)
                    }
                } else {
                    Text(usage?.state == "ready" ? "官方没有返回重置次数" : "额度查询成功后显示").font(.callout).foregroundStyle(.secondary)
                }
                if let reset = account.reset { lastReset(reset, account: account) }
            }
            .panel(padding: 12)
        }
    }

    // The last attempt is a result, not an ongoing instruction; old results collapse to one line.
    @ViewBuilder private func lastReset(_ reset: QuotaResetAttempt, account: ManagedAccount) -> some View {
        Divider().padding(.vertical, 8)
        if reset.unresolved {
            VStack(alignment: .leading, spacing: 6) {
                Text(reset.message).font(.caption).foregroundStyle(Palette.warning).fixedSize(horizontal: false, vertical: true)
                if reset.outcome != "pending" {
                    Button("重试上次重置…") {
                        open(.reset(accountID: account.id, creditID: reset.creditId, requestID: reset.requestId, expiresAt: reset.expiresAt, retry: true))
                    }
                    .controlSize(.small).disabled(locked || account.usage?.refreshing == true)
                }
            }
        } else {
            let when = reset.finishedAt ?? reset.startedAt
            let recent = when.map { Date().timeIntervalSince1970 - $0 < 600 } ?? false
            Text(recent ? reset.message : "上次重置：\(ResetHistoryEntry.outcomeText(reset.outcome))" + (when.map { " · \(When.text($0))" } ?? ""))
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: 5 小时窗口预热（Claude）

    private func warmup(_ account: ManagedAccount) -> some View {
        let plan = account.warmup
        let save: (Bool, Int) -> Void = { enabled, hour in
            Task { await service.accountOperation("accounts.warmup", params: LoginParams(id: account.id, enabled: enabled, hour: hour)) }
        }
        return VStack(alignment: .leading, spacing: 0) {
            SectionLabel("5 小时窗口预热")
            VStack(alignment: .leading, spacing: 10) {
                SwitchRow(title: "每天定时开启窗口", subtitle: "到点用最小的模型发一条短消息，让 5 小时窗口提前开始计时",
                          isOn: Binding(get: { plan?.enabled == true }, set: { save($0, plan?.hour ?? 6) }))
                if let plan, plan.enabled {
                    HStack {
                        Text("预热时间").foregroundStyle(.secondary)
                        Spacer()
                        Picker("预热时间", selection: Binding(get: { plan.hour }, set: { save(true, $0) })) {
                            ForEach(0..<24, id: \.self) { Text(String(format: "每天 %02d:00", $0)).tag($0) }
                        }
                        .labelsHidden().fixedSize()
                    }
                    .font(.caption)
                    if plan.running {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.mini)
                            Text("正在发送…").font(.caption).foregroundStyle(.secondary)
                        }
                    } else if let last = plan.last {
                        Text("\(When.text(last.at)) · \(last.message)").font(.caption)
                            .foregroundStyle(last.ok ? Palette.muted : Palette.warning).fixedSize(horizontal: false, vertical: true)
                            .help(When.exact(last.at))
                    }
                }
                Text("例：10 点开始工作就设 6 点。开工时第一个窗口只剩 1 小时，11 点换上新窗口，上午多出一个窗口的额度。电脑睡眠错过时，只要在设定时间后 5 小时内醒来就会补发，再晚就等第二天。")
                    .font(.caption2).foregroundStyle(.tertiary).fixedSize(horizontal: false, vertical: true)
            }
            .disabled(service.accountBusy)
            .panel(padding: 12)
        }
    }

    // MARK: 自动化

    @ViewBuilder private func automation(_ account: ManagedAccount) -> some View {
        if ["codex", "claude"].contains(account.provider), let strategy = directory.strategy {
            // Switch priority only orders accounts of the same provider.
            let rules = strategy.config.accounts.filter { r in directory.accounts.contains { $0.id == r.id && $0.provider == account.provider } }
            let rule = rules.first { $0.id == account.id }
            VStack(alignment: .leading, spacing: 0) {
                SectionLabel(title: "自动换号与重置") {
                    Button("编辑规则") { AppNavigation.shared.show(.automation) }
                }
                VStack(alignment: .leading, spacing: 8) {
                    if let rule {
                        kv("优先顺序", "第 \((rules.firstIndex { $0.id == account.id } ?? 0) + 1) 位")
                        kv("见底线", "剩余 ≤ \(rule.below)%")
                        kv("额度见底时", rule.order.label + (rule.order.resets && account.usableResets == 0 ? " · 现在没有重置次数" : ""))
                        kv("参与自动处理", rule.participate ? "是" : "否")
                        if let message = strategy.accountStates?.first(where: { $0.id == account.id })?.message, strategy.config.enabled {
                            Text(message).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                        }
                    } else {
                        Text("还没有这个账号的规则；打开「换号与重置」保存一次即可加入。").font(.caption).foregroundStyle(.secondary)
                    }
                    if !strategy.config.enabled {
                        Text("自动换号与重置目前关闭。").font(.caption).foregroundStyle(.secondary)
                    }
                }
                .panel(padding: 12)
            }
        }
    }

    // MARK: 认证维护

    private func maintenance(_ account: ManagedAccount) -> some View {
        var stamps: [(String, Double?)] = [("最近本地检查", account.lastLocal), ("最近认证检查", account.lastOnline), ("最近成功续期", account.lastRefresh)]
        if account.automatic && !["reauth", "attention", "pending"].contains(account.state) {
            stamps.append(("下次自动处理", account.nextCheck))
        }
        if let expiry = account.expiresAt { stamps.append(("Access 到期", expiry)) }
        if let expiry = account.refreshExpiresAt { stamps.append(("登录期限", expiry)) }
        let paused = account.automatic && account.state == "reauth"
        return VStack(alignment: .leading, spacing: 0) {
            SectionLabel("认证维护")
            VStack(alignment: .leading, spacing: 10) {
                SwitchRow(title: "自动检查与续期",
                          subtitle: paused ? "已开启，但要先处理上面的问题才会继续"
                              : account.native && account.provider == "codex" ? "当前默认账号由官方客户端续期"
                              : account.renewalLabel ?? "定期联网检查，接近到期时续期",
                          isOn: Binding(get: { account.automatic }, set: { value in
                              Task { await service.accountOperation("accounts.update", params: LoginParams(id: account.id, automatic: value)) }
                          }))
                    .disabled(locked)
                Divider()
                Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 6) {
                    ForEach(stamps, id: \.0) { stamp in
                        GridRow {
                            Text(stamp.0).foregroundStyle(.secondary)
                            Text(stamp.1.map { When.text($0) } ?? "暂无记录").monospacedDigit()
                                .foregroundStyle(stamp.1 == nil ? Palette.muted : Color.primary)
                                .frame(maxWidth: .infinity, alignment: .trailing)
                                .help(stamp.1.map { When.exact($0) } ?? "")
                        }
                    }
                }
                .font(.caption)
                if let notice = account.loginNotice {
                    Text(notice.message).font(.caption2).foregroundStyle(notice.needsAttention ? Palette.warning : Palette.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .panel(padding: 12)
        }
    }

    private func removeBlocker(_ account: ManagedAccount) -> String? {
        if account.native || directory.desktop?.selected == account.id { return "当前账号不能移除，请先把别的账号设为当前。" }
        if account.reset?.unresolved == true { return "请先确认上次重置结果。" }
        if account.usage?.refreshing == true { return "额度查询进行中，稍后再移除。" }
        if account.warmup?.running == true { return "正在发送预热，稍后再移除。" }
        return nil
    }

    private func removal(_ account: ManagedAccount) -> some View {
        let blocker = removeBlocker(account)
        return VStack(alignment: .leading, spacing: 3) {
            Divider().padding(.bottom, 8)
            Button("从 AuthManager 移除…", role: .destructive) { open(.remove(account.id)) }
                .buttonStyle(.link).foregroundStyle(blocker == nil ? Palette.danger : Palette.muted)
                .disabled(blocker != nil || locked)
            Text(blocker ?? "只删除 AuthManager 保存的记录和凭据，不注销官方账号。").font(.caption2).foregroundStyle(.tertiary)
        }
    }

    private func kv(_ key: String, _ value: String) -> some View {
        HStack {
            Text(key).foregroundStyle(.secondary)
            Spacer()
            Text(value)
        }
        .font(.caption)
    }
}

// MARK: Claude 官方重置机会

// Manual use stays in the official client; the strategy can spend a grant when an account runs low.
struct ClaudeGrantsSection<Footer: View>: View {
    let status: ClaudeResetStatus?
    let history: () -> Void
    @ViewBuilder let footer: Footer

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionLabel(title: "官方重置机会") {
                Button("执行记录", action: history)
            }
            VStack(alignment: .leading, spacing: 10) {
                if let grants = status?.grants {
                    if !grants.eligible { Text(grants.message).font(.caption).foregroundStyle(.secondary) }
                    if grants.remaining == nil { Text("剩余次数暂未返回，不能按 0 次计算。").font(.caption).foregroundStyle(.secondary) }
                    ForEach(Array(grants.items.enumerated()), id: \.offset) { index, grant in
                        if index > 0 { Divider() }
                        VStack(alignment: .leading, spacing: 3) {
                            HStack {
                                Text("剩余 \(grant.remaining) 次").font(.callout.weight(.medium))
                                Spacer()
                                Text(grant.availability(eligible: grants.eligible)).font(.caption).foregroundStyle(.secondary)
                            }
                            Text(grant.label).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                            let soon = QuotaResetCredit.date(grant.endsAt).map { $0.timeIntervalSinceNow <= ExpiryReminder.window } ?? false
                            Text("有效期至 \(QuotaResetCredit.expiryLabel(grant.endsAt))").font(.caption).foregroundStyle(soon ? Palette.warning : Palette.muted)
                            Text((grant.limits.isEmpty ? "适用额度尚未提供" : "适用：\(grant.limits.joined(separator: "、"))")
                                 + (grant.requiresLimit.map { $0 ? " · 达到上限后可用" : " · 允许提前使用" } ?? ""))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    if let until = grants.cooldownUntil { Text("冷却至 \(QuotaResetCredit.expiryLabel(until))").font(.caption) }
                }
                if let weekly = status?.weekly {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("每周会话额度重置").font(.callout.weight(.medium))
                        Text([weekly.perWeek.map { "每周提供 \($0) 次" } ?? "每周次数尚未提供",
                              weekly.available.map { $0 ? "当前可用" : "当前不可用" } ?? "可用状态尚未提供"].joined(separator: " · "))
                            .font(.caption).foregroundStyle(.secondary)
                        if let next = weekly.nextAvailableAt { Text("下次可用：\(QuotaResetCredit.expiryLabel(next))").font(.caption) }
                    }
                }
                if status?.grants == nil && status?.weekly == nil {
                    Text("官方尚未返回重置信息；可以立即查询后再看。").font(.caption).foregroundStyle(.secondary)
                }
                footer
                Text("可在「换号与重置」里设为见底时自动使用；手动使用请到 Claude 官方客户端。").font(.caption2).foregroundStyle(.tertiary)
            }
            .panel(padding: 12)
        }
    }
}
