import SwiftUI

// 换号与重置: one lane per tool with its accounts as stations in relay order, and the picked station's
// rule in a panel under its lane. Saving keeps the old contract: the whole config is sent with
// confirm=true, after an explicit confirmation whenever the strategy is on; turning it off goes
// through accounts.strategy_stop. Claude accounts switch only among Claude accounts, in place.
struct AutomationPage: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var reminders = ReminderModel.shared
    @State private var draft = CodexStrategyConfig()
    @State private var baseline: CodexStrategyConfig?
    @State private var picked: String?
    @State private var confirm: Confirm?
    @State private var confirmForget = false
    @State private var sheet: AccountSheet?
    @State private var saving = false

    private enum Confirm { case enable, save }

    // automation.rs validate(): the service refuses a strategy with more rules than this.
    static let ruleLimit = 32
    private var status: CodexStrategyStatus? { service.directory?.strategy }
    // Accounts a rule may name. One rule for an account handed to the cloud makes the service refuse them all.
    private var accounts: [ManagedAccount] {
        guard let directory = service.directory else { return [] }
        return directory.accounts.filter { ["codex", "claude"].contains($0.provider) && !directory.cloudBound($0.id) }
    }
    private func account(_ id: String) -> ManagedAccount? { accounts.first { $0.id == id } }
    private func tool(of id: String) -> String? { account(id)?.provider }
    private var dirty: Bool { baseline != nil && draft != baseline }
    private var editable: Bool { status?.run == nil && status?.busy != true }
    // Codex switches only through a connected desktop; without it a rule can only reset.
    private var codexCanSwitch: Bool { service.directory?.desktop.map { $0.enabled && $0.connected } ?? false }

    var body: some View {
        Group {
            if let status {
                PageScaffold(title: "换号与重置", subtitle: Text("额度降到见底线时，按顺序换号，或用重置卡回满。")) {
                    Button { sheet = .history(nil) } label: {
                        HStack(spacing: 5) {
                            Icon(.clock, size: 12, color: Palette.ink2)
                            Text("执行记录")
                        }
                    }
                    .buttonStyle(.pill(.secondary, height: 27))
                } content: {
                    if !dirty, let error = service.accountError {
                        Banner(text: error) {
                            Button("知道了") { service.clearAccountError() }.buttonStyle(.pill(.onAttention, height: 20))
                        }
                    }
                    master(status)
                    if let run = status.run {
                        runCard(run, status: status)
                    } else if status.phase == "attention" {
                        Banner(glyph: .relay, title: "自动换号停下了", text: status.message)
                    }
                    limitNotice
                    ForEach(["codex", "claude"], id: \.self) { provider in lane(provider, status: status) }
                    Text("只看 5 分钟内成功查到的主额度（Claude 是 5 小时和 7 天额度）；独立额度和查询失败都不算见底。每轮见底最多重置一次，换号后这一轮就结束；额度回到见底线以上，才开始下一轮。完全退出应用后停止。")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 3.5)
                }
                .safeAreaInset(edge: .bottom) { if dirty { saveBar } }
            } else if service.directory != nil {
                PageScaffold(title: "换号与重置", subtitle: Text("这个后台服务版本没有提供换号与重置。")) {}
            } else {
                ProgressView("正在载入…").frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear {
            sync()
            let selection = AppNavigation.shared.selection
            picked = draft.accounts.contains { $0.id == selection } ? selection : draft.accounts.first?.id
        }
        .onChange(of: status?.config) { _, config in
            if !dirty { sync() }
            // Turning the strategy on or off saves at once; rule edits in progress stay.
            else if let config { draft.enabled = config.enabled; baseline?.enabled = config.enabled }
        }
        .onChange(of: accounts.map(\.id)) { _, _ in if !dirty { sync() } }
        .sheet(item: $sheet) { item in AccountSheetView(item: item, close: { sheet = nil }).environmentObject(service) }
        .alert(confirm == .save ? "按改过的规则自动执行？" : "允许按这些规则自动执行？",
               isPresented: Binding(get: { confirm != nil }, set: { if !$0 { confirm = nil } }), presenting: confirm) { mode in
            Button("返回检查", role: .cancel) {}
            Button(mode == .enable ? "确认开启" : "确认保存") { save(enabling: mode == .enable) }
        } message: { _ in
            Text(confirmation)
        }
        .alert("已在桌面核对这些会话？", isPresented: $confirmForget) {
            Button("返回", role: .cancel) {}
            Button("确认已核对") {
                Task { await service.accountOperation("accounts.strategy_forget", params: LoginParams(confirm: true)) }
            }
        } message: {
            Text("只清除这次换号的记录并关闭自动换号；不会启动或停止任何会话。没完成的认证切换仍要从原来的入口处理。")
        }
    }

    private var confirmation: String {
        "每个参与的账号剩余降到自己的见底线时，按「见底时」处理；换号按接力顺序在同类账号里选。Codex 会暂停、换号、接续桌面任务；Claude 直接换默认登录，运行中的会话约 30 秒内改用新账号，被额度打断的会话要在原会话按一次回车继续。\n\n"
            + (draft.accounts.contains { $0.participate && $0.order.resets }
               ? "设成先重置、先换号或只重置的账号见底时，会自动用掉重置卡，不逐次弹窗。\n\n" : "没有账号会自动用重置卡。\n\n")
            + (draft.accounts.contains { $0.participate && tool(of: $0.id) == "claude" } ? "参与的 Claude 账号会用掉官方给的重置卡。\n\n" : "")
            + "结果不明确时会停下并留下记录；随时可以关闭。"
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
        if let picked, !next.accounts.contains(where: { $0.id == picked }) { self.picked = next.accounts.first?.id }
    }

    private func save(enabling: Bool = false) {
        saving = true
        var config = draft
        if enabling { config.enabled = true }
        // An account handed to the cloud or removed while the draft was open can no longer have a rule.
        config.accounts.removeAll { rule in !accounts.contains { $0.id == rule.id } }
        Task {
            await service.accountOperation("accounts.strategy", params: LoginParams(confirm: true, strategy: config.input))
            saving = false
            if service.accountError == nil { sync() }
        }
    }

    // MARK: 总开关

    private func master(_ status: CodexStrategyStatus) -> some View {
        let watching = status.config.accounts.filter(\.participate).count
        return HStack(spacing: 14) {
            Icon(.relay, size: 19, color: Palette.solid)
                .frame(width: 34, height: 34)
                .background(Palette.wash, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text("见底时自动换号或重置").font(.system(size: 14, weight: .semibold))
                Text(status.config.enabled ? "已开启 · 正在看着 \(watching) 个账号" : "已关闭 · 打开后按下面的规则自动处理")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                if status.config.enabled, status.phase != "attention", status.run == nil, !status.message.isEmpty {
                    Text(status.message).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 10)
            if status.busy || saving { ProgressView().controlSize(.small) }
            Toggle("见底时自动换号或重置", isOn: Binding(get: { status.config.enabled }, set: { on in
                service.clearAccountError()
                if on { confirm = .enable } else { sheet = .strategyStop }
            }))
            .labelsHidden().toggleStyle(.switch).controlSize(.small)
            .help(status.config.enabled ? "关闭前会再确认一次" : "打开前会再确认一次")
            .disabled(saving || service.accountBusy || (!status.config.enabled && !editable))
        }
        .padding(.horizontal, 19).padding(.vertical, 15)
        .loopCard(radius: 19, raised: true)
    }

    // MARK: 正在换号

    private func runCard(_ run: CodexStrategyStatus.Run, status: CodexStrategyStatus) -> some View {
        let stuck = status.phase == "attention"
        return VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 9) {
                if stuck { Icon(.alert, size: 14, color: Palette.attention) } else { ProgressView().controlSize(.small) }
                Text("正在换号：\(account(run.source)?.displayName ?? "原账号") → \(account(run.target)?.displayName ?? "新账号")")
                    .font(.system(size: 13, weight: .semibold)).lineLimit(1)
                Spacer(minLength: 7)
                if stuck {
                    Button("已在桌面核对，清除记录") { confirmForget = true }.buttonStyle(.pill(.secondary)).disabled(status.busy)
                }
                Button("停止并核对…") { sheet = .strategyStop }.buttonStyle(.pill(.secondary))
            }
            if stuck, !status.message.isEmpty {
                Text(status.message).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
            }
            VStack(spacing: 0) {
                ForEach(run.tasks, id: \.threadId) { task in
                    HStack {
                        Text("会话 \(task.threadId.prefix(12))").font(.system(size: 11, design: .monospaced)).foregroundStyle(Palette.ink2)
                        Spacer()
                        Text(task.label).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                    .frame(height: 26)
                }
            }
            .padding(.horizontal, 12)
            .loopPanel(radius: 10)
        }
        .padding(.horizontal, 17).padding(.vertical, 14)
        .loopCard()
    }

    @ViewBuilder private var limitNotice: some View {
        let left = accounts.filter { account in !draft.accounts.contains { $0.id == account.id } }
        if draft.accounts.count >= Self.ruleLimit, !left.isEmpty {
            Banner(glyph: .info, title: "规则最多 \(Self.ruleLimit) 个账号",
                   text: "\(left.map(\.displayName).joined(separator: "、")) 不在规则里，不会自动换号或重置。要让它们参与，先从 BatonPass 移除不用的账号。")
        }
    }

    // MARK: 接力

    private func lane(_ provider: String, status: CodexStrategyStatus) -> some View {
        let rules = draft.accounts.filter { tool(of: $0.id) == provider }
        let count = accounts.filter { $0.provider == provider }.count
        let index = draft.accounts.firstIndex { $0.id == picked && tool(of: $0.id) == provider }
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 9) {
                Icon(.provider(provider), size: 15)
                Text(GlyphTile.name(provider)).font(.system(size: 13, weight: .semibold))
                Text(provider == "codex" ? "从左往右接力" : "只在 Claude 账号之间换号，不暂停会话")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                Spacer(minLength: 7)
                if provider == "codex", rules.count > 1, !codexCanSwitch { desktopChip }
            }
            if rules.isEmpty && count > 0 {
                // Past the rule limit: the accounts exist but none of them is on the relay.
                Text("\(count) 个账号不在规则里，见底时不会自动处理。").font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 12)
            } else {
                stations(rules, provider: provider, add: count <= 1).padding(.top, 19)
            }
            if let index {
                editor($draft.accounts[index], lane: rules).padding(.top, 14)
                Text(rules.count > 1 ? "拖动账号调整顺序；点一个账号，在下面改它的规则。" : "点一个账号，在下面改它的规则。")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 10)
            } else if !rules.isEmpty {
                Text("点一个账号，改它的规则。").font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 10)
            }
        }
        .padding(.horizontal, 19).padding(.top, 15).padding(.bottom, 17)
        .loopCard(radius: 19)
        .disabled(!editable)
    }

    private var desktopChip: some View {
        let off = service.directory?.desktop?.enabled != true
        return HStack(spacing: 7) {
            Text(off ? "热切换关着：现在只会重置，不会换号" : "桌面没连上：现在只会重置，不会换号")
            Button(off ? "去打开" : "去连接") { AppNavigation.shared.show(.desktop) }
                .buttonStyle(.plain)
                .font(.system(size: 11, weight: .semibold))
                .padding(.horizontal, 7).frame(height: 17)
                .background(Palette.card, in: Capsule())
        }
        .font(.system(size: 11)).foregroundStyle(Palette.attentionText)
        .padding(.leading, 9).padding(.trailing, 3.5).frame(height: 22)
        .background(Palette.attentionFill, in: Capsule())
    }

    @ViewBuilder private func stations(_ rules: [CodexStrategyConfig.Rule], provider: String, add: Bool) -> some View {
        let row = HStack(alignment: .top, spacing: 0) {
            ForEach(Array(rules.enumerated()), id: \.element.id) { offset, rule in
                Station(account: account(rule.id), rule: rule,
                        next: offset + 1 < rules.count ? connector(rule, rules[offset + 1], provider: provider) : add ? .idle : nil,
                        picked: picked == rule.id, pick: { picked = rule.id })
                    .frame(width: rules.count > 5 ? 153 : nil)
                    .frame(maxWidth: rules.count > 5 ? nil : .infinity)
                    .draggable(rule.id)
                    .dropDestination(for: String.self) { items, _ in
                        guard let id = items.first else { return false }
                        return place(id, at: rule.id)
                    }
            }
            if add { addStation(provider, empty: rules.isEmpty) }
            // Keep a station a quarter of the lane wide when there are only a few.
            ForEach(0..<max(0, 4 - rules.count - (add ? 1 : 0)), id: \.self) { _ in
                Color.clear.frame(maxWidth: .infinity, maxHeight: 1)
            }
        }
        if rules.count > 5 {
            ScrollView(.horizontal, showsIndicators: false) { row }
        } else {
            row
        }
    }

    // A line means a switch can happen from this station to the next: both take part and this one switches.
    private func connector(_ from: CodexStrategyConfig.Rule, _ to: CodexStrategyConfig.Rule, provider: String) -> RelayLine.Style {
        guard draft.enabled, from.participate, from.order.switches, to.participate else { return .idle }
        return provider == "codex" && !codexCanSwitch ? .blocked : .open
    }

    // Moves a dragged rule to the dropped-on station's place, within one lane.
    private func place(_ id: String, at target: String) -> Bool {
        guard id != target, tool(of: id) == tool(of: target),
              let from = draft.accounts.firstIndex(where: { $0.id == id }),
              let to = draft.accounts.firstIndex(where: { $0.id == target }) else { return false }
        let rule = draft.accounts.remove(at: from)
        draft.accounts.insert(rule, at: to)
        picked = id
        return true
    }

    private func addStation(_ provider: String, empty: Bool) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            ZStack {
                RoundedRectangle(cornerRadius: 7.5, style: .continuous)
                    .strokeBorder(Palette.dash, style: StrokeStyle(lineWidth: 1.5, dash: [3, 2.5]))
                    .frame(width: 27, height: 27).rotationEffect(.degrees(45))
                Icon(.plus, size: 12, color: Palette.ink3, weight: 2)
            }
            .frame(width: 37, height: 37)
            Button { sheet = .login(provider: provider, accountID: nil) } label: {
                VStack(alignment: .leading, spacing: 1) {
                    Text("添加 \(provider == "codex" ? "Codex" : "Claude") 账号").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.solid)
                    Text(empty ? "有账号后才能加入接力" : "有第二个账号才能换号").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
                .padding(.horizontal, 10).padding(.vertical, 9)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(service.accountBusy || service.directory?.busy == true || service.directory?.recoveryNeeded == true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: 规则

    private func editor(_ rule: Binding<CodexStrategyConfig.Rule>, lane: [CodexStrategyConfig.Rule]) -> some View {
        let id = rule.wrappedValue.id
        let rank = (lane.firstIndex { $0.id == id } ?? 0) + 1
        let target = account(id)
        let info = HStack(spacing: 7) {
            VStack(alignment: .leading, spacing: 2) {
                Text(target?.displayName ?? "账号").font(.system(size: 12, weight: .semibold)).lineLimit(1)
                note(target, rule.wrappedValue).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
            }
            .frame(minWidth: 153, maxWidth: .infinity, alignment: .leading)
            if lane.count > 1 {
                HStack(spacing: 2) {
                    moveButton(right: false) { move(id, by: -1, lane: lane) }.disabled(rank == 1)
                    moveButton(right: true) { move(id, by: 1, lane: lane) }.disabled(rank == lane.count)
                }
            }
        }
        let controls = HStack(spacing: 19) {
            HStack(spacing: 9) {
                Text("见底线").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                HStack(spacing: 0) {
                    stepButton("−", label: "调低见底线") { rule.wrappedValue.below = max(0, rule.wrappedValue.below - 1) }
                        .disabled(rule.wrappedValue.below == 0)
                    Text("剩余 \(rule.wrappedValue.below)%").font(.system(size: 12, weight: .semibold)).monospacedDigit()
                        .frame(minWidth: 54)
                    stepButton("+", label: "调高见底线") { rule.wrappedValue.below = min(99, rule.wrappedValue.below + 1) }
                        .disabled(rule.wrappedValue.below == 99)
                }
                .background(Palette.card, in: Capsule())
                .overlay(Capsule().strokeBorder(Palette.edge, lineWidth: 1))
            }
            .disabled(!rule.wrappedValue.participate)
            HStack(spacing: 9) {
                Text("见底时").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                Menu {
                    Picker("见底时", selection: rule.order) {
                        ForEach(CodexStrategyOrder.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.inline).labelsHidden()
                } label: {
                    HStack(spacing: 7) {
                        Text(rule.wrappedValue.order.label).font(.system(size: 12, weight: .semibold))
                        Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4)
                    }
                    .padding(.leading, 10).padding(.trailing, 9).frame(height: 26)
                    .background(Palette.card, in: Capsule())
                    .overlay(Capsule().strokeBorder(Palette.edge, lineWidth: 1))
                    .contentShape(Capsule())
                }
                .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
                .help(rule.wrappedValue.order.explanation)
            }
            .disabled(!rule.wrappedValue.participate)
            HStack(spacing: 9) {
                Text("参与").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                Toggle("参与", isOn: rule.participate).labelsHidden().toggleStyle(.switch).controlSize(.small)
            }
        }
        .fixedSize()
        return VStack(alignment: .leading, spacing: 9) {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 19) { info; controls }
                VStack(alignment: .leading, spacing: 10) { info; controls }
            }
            Text(rule.wrappedValue.participate ? rule.wrappedValue.order.explanation : "不参与：见底时不会自动换走它，也不会自动给它用重置卡。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 15).padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopPanel(radius: 14)
    }

    // What the picked account has to work with: in use or not, how far from its line, its reset cards.
    private func note(_ account: ManagedAccount?, _ rule: CodexStrategyConfig.Rule) -> Text {
        guard let account else { return Text("") }
        var lead: [String] = []
        if account.native { lead.append("在用") }
        if let lowest = account.lowestPrimary {
            lead.append("剩余 \(lowest)%" + (rule.participate && lowest <= rule.below ? "，已到见底线" : ""))
        }
        let chances = account.chances(dismissed: reminders.dismissed)
        let cards: String = chances.map { chances in
            guard chances.total > 0 else { return "现在没有重置卡" }
            guard chances.soon, let earliest = chances.earliestSoon else { return "有 \(chances.total) 张重置卡" }
            return "有 \(chances.total) 张重置卡，\(chances.soonCount) 张 \(When.day(earliest))\(chances.soonCount > 1 ? "起" : "")到期"
        } ?? "重置卡数量还没查到"
        // A rule that resets but has no card left will skip straight to its next step.
        let short = rule.participate && rule.order.resets && chances?.total == 0
        var text = Text(lead.map { $0 + "。" }.joined()) + Text(cards + "。").foregroundStyle(short ? Palette.attentionText : Palette.ink3)
        if draft.enabled, let message = status?.accountStates?.first(where: { $0.id == account.id })?.message, !message.isEmpty {
            text = text + Text(message.hasSuffix("。") ? message : message + "。")
        }
        return text
    }

    private func move(_ id: String, by offset: Int, lane: [CodexStrategyConfig.Rule]) {
        guard let rank = lane.firstIndex(where: { $0.id == id }), lane.indices.contains(rank + offset) else { return }
        _ = place(id, at: lane[rank + offset].id)
    }

    private func moveButton(right: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Icon(.chevronRight, size: 10, color: Palette.ink2, weight: 2.2)
                .rotationEffect(.degrees(right ? 0 : 180))
                .frame(width: 22, height: 22)
                .background(Palette.card, in: Circle())
                .overlay(Circle().strokeBorder(Palette.edge, lineWidth: 1))
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .help(right ? "往后移一位" : "往前移一位")
        .accessibilityLabel(right ? "往后移一位" : "往前移一位")
    }

    private func stepButton(_ symbol: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(symbol).font(.system(size: 14)).foregroundStyle(Palette.ink2)
                .frame(width: 26, height: 26).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    private var saveBar: some View {
        HStack(spacing: 9) {
            Circle().fill(Palette.brand).frame(width: 7, height: 7)
            Text("有未保存的修改").font(.system(size: 12))
            if let error = service.accountError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText).lineLimit(2)
            }
            Spacer()
            Button("还原") { sync() }.buttonStyle(.pill(.secondary, height: 27))
            Button(saving ? "正在保存…" : "保存规则") {
                service.clearAccountError()
                if draft.enabled { confirm = .save } else { save() }
            }
            .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut("s")
            .disabled(saving || service.accountBusy || !editable)
        }
        .padding(.horizontal, 27).padding(.vertical, 10)
        .background(Palette.card)
        .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
    }
}

// One account on its lane: its seal with the line to the next station, then a card with its rule.
private struct Station: View {
    let account: ManagedAccount?
    let rule: CodexStrategyConfig.Rule
    let next: RelayLine.Style?
    let picked: Bool
    let pick: () -> Void
    @State private var hovering = false

    var body: some View {
        let style: Pill.Style = rule.participate ? .neutral : .dashed
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 0) {
                Seal(account?.displayName ?? "?", style: !rule.participate ? .out : account?.native == true ? .inUse : .standby, size: 37)
                if let next { RelayLine(style: next).padding(.leading, 5).padding(.trailing, 3.5) }
            }
            .frame(height: 37)
            Button(action: pick) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(account?.displayName ?? "账号").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink).lineLimit(1)
                    status
                    HStack(spacing: 5) {
                        Pill("≤ \(rule.below)%", style: style)
                        Pill(rule.order.label, style: style)
                    }
                    .padding(.top, 6)
                }
                .padding(.horizontal, 10).padding(.vertical, 9)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(picked ? Palette.card : hovering ? Palette.hover : .clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay { if picked { RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.brand, lineWidth: 1.5) } }
                .shadow(color: .black.opacity(picked ? 0.08 : 0), radius: 7, y: 5)
                .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            }
            .buttonStyle(.plain)
            .onHover { hovering = $0 }
            .accessibilityAddTraits(picked ? .isSelected : [])
            .padding(.trailing, 12)
        }
    }

    // In use, not taking part, and the lowest remaining; what needs a hand replaces it in amber.
    @ViewBuilder private var status: some View {
        if let account, account.needsAction {
            Text(account.health.title).font(.system(size: 11)).foregroundStyle(Palette.attentionText).lineLimit(1)
        } else {
            let parts = [account?.native == true ? "在用" : nil, rule.participate ? nil : "不参与",
                         account?.lowestPrimary.map { "剩余 \($0)%" }].compactMap { $0 }
            Text(parts.isEmpty ? (account?.quotaCaption ?? " ") : parts.joined(separator: " · "))
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).monospacedDigit().lineLimit(1)
        }
    }
}
