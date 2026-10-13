import SwiftUI

// 换号与重置: the strategy editor. Saving keeps the old contract: the whole config is
// sent with confirm=true, after an explicit confirmation whenever the strategy is enabled.
// Claude accounts switch only among Claude accounts, in place: running sessions are not paused.
struct AutomationPage: View {
    @EnvironmentObject private var service: ServiceController
    @State private var draft = CodexStrategyConfig()
    @State private var baseline: CodexStrategyConfig?
    @State private var confirmSave = false
    @State private var confirmForget = false
    @State private var sheet: AccountSheet?
    @State private var saving = false

    // automation.rs validate(): the service refuses a strategy with more rules than this.
    static let ruleLimit = 32
    private var status: CodexStrategyStatus? { service.directory?.strategy }
    // Accounts a rule may name. One rule for an account handed to the cloud makes the service refuse them all.
    private var accounts: [ManagedAccount] {
        guard let directory = service.directory else { return [] }
        return directory.accounts.filter { ["codex", "claude"].contains($0.provider) && !directory.cloudBound($0.id) }
    }
    private func isClaude(_ id: String) -> Bool { accounts.first { $0.id == id }?.provider == "claude" }
    private var dirty: Bool { baseline != nil && draft != baseline }
    private var editable: Bool { status?.run == nil && status?.busy != true }

    var body: some View {
        VStack(spacing: 0) {
            if let status {
                ScrollView {
                    content(status)
                        .padding(24)
                        .frame(maxWidth: 960, alignment: .leading)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .safeAreaInset(edge: .bottom) { if dirty { saveBar } }
            } else {
                ProgressView("正在载入…").frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .pageBar("换号与重置", subtitle: status.map { $0.config.enabled ? "已开启" : "已关闭" }) {
            Button("执行记录") { sheet = .history(nil) }
            if let status, status.config.enabled || status.run != nil || status.busy {
                Button(status.run == nil ? "关闭自动处理…" : "停止并核对…") { sheet = .strategyStop }
            }
        }
        .onAppear(perform: sync)
        .onChange(of: status?.config) { _, _ in if !dirty { sync() } }
        .onChange(of: accounts.map(\.id)) { _, _ in if !dirty { sync() } }
        .sheet(item: $sheet) { item in AccountSheetView(item: item, close: { sheet = nil }).environmentObject(service) }
        .alert("允许按此规则自动执行？", isPresented: $confirmSave) {
            Button("返回检查", role: .cancel) {}
            Button("确认启用") { save() }
        } message: {
            Text("每个参与账号剩余降到自己的见底线时，按「额度见底时」处理；换号按表格顺序在同类账号里选候选。Codex 会暂停、换号、接续桌面任务；Claude 直接换默认登录，运行中的会话约 30 秒内改用新账号，被额度打断的会话要在原会话按一次回车继续。\n\n"
                 + (draft.accounts.contains { $0.participate && $0.order.resets }
                    ? "设为先重置、先换号或只重置的账号见底时，会自动消耗可用次数，不逐次弹窗。\n\n" : "没有账号会自动使用重置次数。\n\n")
                 + (draft.accounts.contains { $0.participate && isClaude($0.id) } ? "参与的 Claude 账号会用掉官方提供的重置机会。\n\n" : "")
                 + "结果不明确时会停下并留下记录；你可以随时关闭。")
        }
        .alert("已在桌面核对这些会话？", isPresented: $confirmForget) {
            Button("返回", role: .cancel) {}
            Button("确认已核对") {
                Task { await service.accountOperation("accounts.strategy_forget", params: LoginParams(confirm: true)) }
            }
        } message: {
            Text("只清除本次自动流程记录并关闭策略；不会启动或停止任何会话。未完成的认证切换仍需通过原有恢复入口处理。")
        }
    }

    private func sync() {
        guard let config = status?.config else { return }
        // Saved rules for accounts that can still have one; leftovers would make every save fail.
        let kept = config.accounts.filter { rule in accounts.contains { $0.id == rule.id } }
        let fresh = ["codex", "claude"].flatMap { provider in
            accounts.filter { account in account.provider == provider && !kept.contains { $0.id == account.id } }
        }
        // New accounts never spend credits until the user opts in; past the limit they stay out.
        let added = fresh.prefix(max(0, Self.ruleLimit - kept.count)).map { account in
            account.provider == "codex" ? CodexStrategyConfig.Rule(id: account.id, participate: true)
                                        : CodexStrategyConfig.Rule(id: account.id, order: .resetOnly, participate: false)
        }
        var next = config
        // Codex rules first, then Claude rules, each group in its stored priority.
        next.accounts = ["codex", "claude"].flatMap { provider in
            (kept + added).filter { rule in accounts.contains { $0.id == rule.id && $0.provider == provider } }
        }
        draft = next
        baseline = next
    }

    @ViewBuilder private func content(_ status: CodexStrategyStatus) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("额度见底时自动处理").font(.system(size: 20, weight: .semibold))
                    Text("Codex 和 Claude 账号的主额度见底时，按下面的规则换号或使用重置次数；换号只在同类账号之间进行。每次只做一件事，结果不明确就停下并留下记录。")
                        .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 16)
                Toggle("启用", isOn: $draft.enabled).toggleStyle(.switch).disabled(!editable)
            }
            if let desktop = service.directory?.desktop, !(desktop.enabled && desktop.connected) {
                Callout(icon: "macwindow", tone: .warning, title: "Codex 桌面未连接：Codex 现在不会换号",
                        message: "Codex 的自动换号要靠连上的桌面来暂停和接续会话；Claude 换号和各账号的自动重置不受影响，照常进行。") {
                    Button("去连接桌面") { AppNavigation.shared.show(.desktop) }.controlSize(.small)
                }
            }
            let left = accounts.filter { account in !draft.accounts.contains { $0.id == account.id } }
            if draft.accounts.count >= Self.ruleLimit, !left.isEmpty {
                Callout(icon: "info.circle", tone: .warning, title: "规则最多包含 \(Self.ruleLimit) 个账号",
                        message: "\(left.map(\.displayName).joined(separator: "、")) 不在规则里，不会自动换号或重置。要让它们参与，先从 AuthManager 移除不用的账号。")
            }
            if let run = status.run { runCard(run, status: status) }
            if !status.message.isEmpty {
                Label(status.message, systemImage: status.phase == "attention" ? "exclamationmark.triangle.fill" : "waveform.path.ecg")
                    .font(.callout).foregroundStyle(status.phase == "attention" ? Palette.warning : Palette.muted)
            }
            ruleSummary
            table(status)
            preview
            Label("模型或功能的独立额度、查询失败都不算见底；只看 5 分钟内成功查到的主额度（Claude 是 5 小时和 7 天额度）。桌面额度事件优先，后台每 5 分钟查询一次。完全退出应用后停止。",
                  systemImage: "info.circle")
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .disabled(!editable && status.run == nil)
    }

    private var ruleSummary: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("账号剩余降到自己的「见底线」就算见底，按「额度见底时」做第一件能做的事；做不了再做下一件。")
                .font(.system(size: 14, weight: .medium))
            Text("每一轮见底最多重置一次；换号后这一轮就结束。额度回到见底线以上，才开始下一轮。")
                .font(.callout).foregroundStyle(.secondary)
        }
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(padding: 14)
    }

    private func table(_ status: CodexStrategyStatus) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Text("#").frame(width: 46, alignment: .leading)
                Text("账号").frame(minWidth: 150, maxWidth: .infinity, alignment: .leading)
                Text("剩余").frame(width: 44, alignment: .leading)
                Text("见底线").frame(width: 84, alignment: .leading)
                Text("额度见底时").frame(width: 256, alignment: .leading)
                Text("参与").frame(width: 40, alignment: .leading)
            }
            .font(.system(size: 11)).foregroundStyle(.tertiary)
            .padding(.horizontal, 14).frame(height: 28).background(Palette.raised)
            ForEach($draft.accounts) { $rule in
                if !isClaude(rule.id) {
                    Divider()
                    ruleRow($rule, status: status)
                }
            }
            if draft.accounts.contains(where: { isClaude($0.id) }) {
                Divider()
                Text("Claude 账号 · 只在 Claude 账号之间换号，不暂停会话")
                    .font(.system(size: 11)).foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14).frame(height: 28).background(Palette.raised)
                ForEach($draft.accounts) { $rule in
                    if isClaude(rule.id) {
                        Divider()
                        ruleRow($rule, status: status)
                    }
                }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.hairline))
        .disabled(!editable)
    }

    private func ruleRow(_ rule: Binding<CodexStrategyConfig.Rule>, status: CodexStrategyStatus) -> some View {
        let id = rule.wrappedValue.id
        let account = accounts.first { $0.id == id }
        let claude = account?.provider == "claude"
        // Priority only orders candidates of the same provider; each group is contiguous.
        let group = draft.accounts.filter { isClaude($0.id) == claude }
        let rank = group.firstIndex { $0.id == id }! + 1
        let message = status.accountStates?.first { $0.id == id }?.message
        return HStack(spacing: 10) {
            HStack(spacing: 2) {
                Text("\(rank)").font(.caption.weight(.bold)).monospacedDigit()
                    .foregroundStyle(account?.native == true ? Color.white : Palette.muted)
                    .frame(width: 20, height: 20)
                    .background(account?.native == true ? Palette.accent : Palette.track, in: Circle())
                VStack(spacing: 0) {
                    Button { draft.move(id: id, by: -1) } label: { Image(systemName: "chevron.up").font(.system(size: 8, weight: .bold)) }
                        .disabled(rank == 1).help("提高优先级")
                    Button { draft.move(id: id, by: 1) } label: { Image(systemName: "chevron.down").font(.system(size: 8, weight: .bold)) }
                        .disabled(rank == group.count).help("降低优先级")
                }
                .buttonStyle(.borderless)
            }
            .frame(width: 46, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(account?.displayName ?? "账号").font(.callout.weight(.semibold)).lineLimit(1)
                    if account?.native == true { Tag(text: "当前", tone: .accent, filled: true) }
                    if rule.wrappedValue.order.resets, account?.usableResets == 0 {
                        Text("无次数").font(.caption).foregroundStyle(Palette.warning)
                    }
                }
                Text(message.flatMap { draft.enabled ? $0 : nil } ?? "\(account?.usableResets ?? 0) 次重置可用")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            .frame(minWidth: 150, maxWidth: .infinity, alignment: .leading)
            Text(account?.lowestPrimary.map { "\($0)%" } ?? "—").font(.callout.weight(.semibold)).monospacedDigit()
                .frame(width: 44, alignment: .leading)
            Stepper(value: rule.below, in: 0...99) {
                Text("≤ \(rule.wrappedValue.below)%").font(.callout).monospacedDigit()
            }
            .controlSize(.small).fixedSize()
            .frame(width: 84, alignment: .leading)
            .disabled(!rule.wrappedValue.participate)
            Picker("额度见底时", selection: rule.order) {
                ForEach(CodexStrategyOrder.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden().frame(width: 256)
            .help(rule.wrappedValue.order.explanation)
            .disabled(!rule.wrappedValue.participate)
            Toggle("参与", isOn: rule.participate).labelsHidden().toggleStyle(.switch).controlSize(.mini)
                .frame(width: 40, alignment: .leading)
        }
        .padding(.horizontal, 14).padding(.vertical, 8).frame(minHeight: 50)
    }

    private var preview: some View {
        let result = StrategyPreview(config: draft, accounts: service.directory?.accounts ?? [], desktop: service.directory?.desktop)
        let current = accounts.first { $0.provider == "codex" && $0.native }
        return VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(result.below.map { "预演：如果 \(current?.displayName ?? "当前账号") 的剩余降到 ≤ \($0)%" } ?? "预演").font(.callout.weight(.semibold))
                Spacer()
                Text(dirty ? "按未保存的规则和当前真实状态" : "按当前规则和真实状态").font(.caption).foregroundStyle(.secondary)
            }
            if !draft.enabled {
                Text("自动处理已关闭：下面只是演示，保存并开启后才会执行。").font(.caption).foregroundStyle(Palette.warning)
            }
            HStack(alignment: .top, spacing: 10) {
                ForEach(Array(result.steps.enumerated()), id: \.element.id) { index, step in
                    VStack(alignment: .leading, spacing: 4) {
                        Text("第 \(index + 1) 步").font(.caption2.weight(.semibold)).foregroundStyle(.tertiary)
                        Text(step.title).font(.callout.weight(.semibold)).strikethrough(!step.active && step.detail.hasPrefix("跳过"))
                        Text(step.detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(12).frame(maxWidth: .infinity, alignment: .topLeading)
                    .background(step.active ? Palette.accent.opacity(0.10) : Palette.raised, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(step.active ? Palette.accent.opacity(0.5) : .clear))
                    .opacity(step.active ? 1 : 0.75)
                }
            }
            if !result.others.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    Text("其他账号（不需要桌面，各自独立处理）").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    ForEach(result.others, id: \.self) { Text("· \($0)").font(.caption).foregroundStyle(.secondary) }
                }
            }
        }
        .card(padding: 16)
    }

    private func runCard(_ run: CodexStrategyStatus.Run, status: CodexStrategyStatus) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("正在执行的换号流程").font(.callout.weight(.semibold))
            ForEach(run.tasks, id: \.threadId) { task in
                HStack {
                    Text("会话 \(task.threadId.prefix(12))").font(.caption.monospaced())
                    Spacer()
                    Text(task.label).font(.caption).foregroundStyle(.secondary)
                }
            }
            if status.phase == "attention" {
                Button("已在桌面核对，清除流程记录") { confirmForget = true }.controlSize(.small).disabled(status.busy)
            }
        }
        .card(padding: 14)
    }

    private var saveBar: some View {
        HStack(spacing: 10) {
            Image(systemName: "pencil.circle.fill").foregroundStyle(Palette.accent)
            Text("有未保存的修改").font(.callout)
            if let error = service.accountError { Text(error).font(.caption).foregroundStyle(Palette.warning).lineLimit(2) }
            Spacer()
            Button("还原") { sync() }
            Button(saving ? "正在保存…" : "保存规则") {
                service.clearAccountError()
                if draft.enabled { confirmSave = true } else { save() }
            }
            .buttonStyle(.borderedProminent).keyboardShortcut("s")
            .disabled(saving || service.accountBusy || !editable)
        }
        .padding(.horizontal, 24).padding(.vertical, 10)
        .background(.regularMaterial)
        .overlay(alignment: .top) { Divider() }
    }

    private func save() {
        saving = true
        // An account handed to the cloud or removed while the draft was open can no longer have a rule.
        var config = draft
        config.accounts.removeAll { rule in !accounts.contains { $0.id == rule.id } }
        Task {
            await service.accountOperation("accounts.strategy", params: LoginParams(confirm: true, strategy: config.input))
            saving = false
            if service.accountError == nil { sync() }
        }
    }
}
