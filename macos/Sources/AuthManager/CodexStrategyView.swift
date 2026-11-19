import AppKit
import SwiftUI
import UniformTypeIdentifiers

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
    // The rule being dragged across its lane; its slot shows where it will land.
    @State private var dragging: String? = AutomationPage.previewDragging
    // Set once the strategy has been on, or the first-time card was put away.
    @AppStorage(Self.onboardedKey) private var onboarded = false
    // "show" after the first-time card turned the strategy on, "done" once its note is closed.
    @AppStorage(Self.hintKey) private var doneHint = ""

    private enum Confirm { case enable, save }

    static let onboardedKey = "relay.onboarded"
    static let hintKey = "relay.doneHint"
    // Preview renders only: draws this rule mid-drag.
    @MainActor static var previewDragging: String?

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
    // Changes other than the order; an order change alone is saved from its lane's title.
    private var rulesDirty: Bool {
        guard let baseline else { return false }
        return unordered(draft) != unordered(baseline)
    }
    private func unordered(_ config: CodexStrategyConfig) -> CodexStrategyConfig {
        var config = config
        config.accounts.sort { $0.id < $1.id }
        return config
    }
    private func laneOrder(_ config: CodexStrategyConfig?, _ provider: String) -> [String] {
        (config?.accounts ?? []).map(\.id).filter { tool(of: $0) == provider }
    }
    private func orderChanged(_ provider: String) -> Bool { baseline != nil && laneOrder(draft, provider) != laneOrder(baseline, provider) }
    // Never turned on, never put away: the master card becomes the first-time card. A strategy that has
    // acted before (last_at) belongs to someone who already used it, even if it is off now.
    private var guiding: Bool { !onboarded && status?.config.enabled == false && status?.run == nil && status?.lastAt == nil }
    private var cliOnly: Bool {
        service.directory.map { CodexUse.resolve(UserDefaults.standard.string(forKey: CodexUse.key) ?? "", $0) == .cli } ?? false
    }
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
                    if guiding {
                        guide(status)
                    } else {
                        master(status)
                        if status.config.enabled, doneHint == "show" { startedNote }
                    }
                    if let run = status.run {
                        runCard(run, status: status)
                    } else if status.phase == "attention" {
                        Banner(glyph: .relay, title: "自动换号停下了", text: status.message)
                    }
                    limitNotice
                    if !(guiding && accounts.isEmpty) {
                        ForEach(["codex", "claude"], id: \.self) { provider in lane(provider, status: status) }
                    }
                    Text("只看 5 分钟内成功查到的主额度（Claude 是 5 小时和 7 天额度）；独立额度和查询失败都不算见底，只有 Claude 的 Fable 额度例外：Claude Code 正在用 Fable 时，它到了见底线也换号，但不用重置卡。每轮见底最多重置一次，换号后这一轮就结束；额度回到见底线以上，才开始下一轮。重置卡离到期不到 30 分钟时，只要额度用过就自动用掉，不等见底（只换号的账号除外）。完全退出应用后停止。")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, 3.5)
                }
                .safeAreaInset(edge: .bottom) { if rulesDirty { saveBar } }
            } else if service.directory != nil {
                PageScaffold(title: "换号与重置", subtitle: Text("当前后台服务版本不支持换号与重置。")) {}
            } else {
                ProgressView("正在载入…").frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear {
            // Someone who already runs the strategy needs no first-time card.
            if status?.config.enabled == true { onboarded = true }
            sync()
            let selection = AppNavigation.shared.selection
            picked = draft.accounts.contains { $0.id == selection } ? selection : draft.accounts.first?.id
        }
        .onChange(of: status?.config.enabled) { old, now in
            guard now == true, !onboarded else { return }
            onboarded = true
            // Turned on from the first-time card (not merely loaded as on): one note on what happens next.
            if old == false, doneHint != "done" { doneHint = "show" }
        }
        // A drag let go outside any slot ends without a drop; the mouse coming up ends it.
        .task(id: dragging) {
            guard dragging != nil, Self.previewDragging == nil else { return }
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 250_000_000)
                if NSEvent.pressedMouseButtons == 0 { dragging = nil; break }
            }
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
               ? "设成先重置、先换号或只重置的账号见底时，或者重置卡离到期不到 30 分钟时，会自动用掉重置卡，不逐次弹窗。\n\n" : "没有账号会自动用重置卡。\n\n")
            + (draft.accounts.contains { $0.participate && tool(of: $0.id) == "claude" } ? "参与的 Claude 账号会用掉官方给的重置卡。\n\n" : "")
            + (draft.accounts.contains { $0.participate && $0.order.switches && tool(of: $0.id) == "claude" }
               ? "Claude Code 正在用 Fable 时，当前 Claude 账号的 Fable 额度到了见底线也会换号，只换到 Fable 额度也高于见底线的账号。\n\n" : "")
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

    // MARK: 第一次

    // Turning the strategy on spends reset cards and switches accounts by itself, so the first time the
    // master card asks for two checks: what can and cannot happen now, then the usual confirmation.
    private func guide(_ status: CodexStrategyStatus) -> some View {
        let empty = accounts.isEmpty
        return VStack(alignment: .leading, spacing: 13) {
            HStack(alignment: .top, spacing: 12) {
                Icon(.relay, size: 17, color: Palette.solid)
                    .frame(width: 30, height: 30)
                    .background(Palette.wash, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text("开启自动接力前，先核对这两件事").font(.system(size: 13, weight: .semibold))
                    Text("自动规则默认关闭；确认下方顺序和策略后再开启，之后随时可关。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
                Spacer(minLength: 8)
                Button("先不用") { onboarded = true }
                    .buttonStyle(.plain).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    .help("收起这张卡，以后用右上角的开关开启")
            }
            HStack(alignment: .top, spacing: 11) {
                VStack(alignment: .leading, spacing: 8) {
                    stepTitle(1, "核对下方顺序与前提")
                    if empty {
                        Text("还没有可接力的账号，先去 Codex 或 Claude 页添加至少 1 个账号").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        if let codex = codexCheck { checkRow(codex) }
                        if let claude = claudeCheck { checkRow(claude) }
                    }
                }
                .padding(.horizontal, 12).padding(.vertical, 11)
                .frame(maxWidth: .infinity, minHeight: 82, alignment: .topLeading)
                .loopPanel(radius: 12)
                VStack(alignment: .leading, spacing: 8) {
                    stepTitle(2, "确认开启自动执行")
                    HStack(alignment: .bottom, spacing: 10) {
                        Text("点开启会弹出最终确认；每轮见底最多重置一次，仅看 5 分钟内额度。")
                            .font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if empty {
                            Button("去添加账号") { AppNavigation.shared.show(.accounts) }
                                .buttonStyle(.pill(.primary, height: 27)).fixedSize()
                        } else {
                            Button(dirty ? "保存并开启" : "开启自动接力") {
                                service.clearAccountError()
                                confirm = .enable
                            }
                            .buttonStyle(.pill(.primary, height: 27)).fixedSize()
                            .disabled(saving || service.accountBusy || !editable)
                        }
                    }
                }
                .padding(.horizontal, 12).padding(.vertical, 11)
                .frame(maxWidth: .infinity, minHeight: 82, alignment: .topLeading)
                .background(Palette.wash, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.brand.opacity(0.18), lineWidth: 1))
            }
        }
        .padding(.horizontal, 19).padding(.top, 15).padding(.bottom, 17)
        .loopCard(radius: 19, raised: true)
    }

    private func stepTitle(_ number: Int, _ title: String) -> some View {
        HStack(spacing: 7) {
            Text("\(number)").font(.system(size: 10, weight: .bold, design: .rounded)).foregroundStyle(.white)
                .frame(width: 17, height: 17).background(Palette.brand, in: Circle())
            Text(title).font(.system(size: 12, weight: .semibold))
        }
    }

    private struct Check {
        enum Tone { case ready, blocked, quiet }
        let tone: Tone
        let text: String
        var action: (title: String, run: () -> Void)? = nil
    }

    private func checkRow(_ check: Check) -> some View {
        HStack(spacing: 6) {
            Circle().fill(check.tone == .blocked ? Palette.attentionDot : check.tone == .ready ? Palette.brand : Palette.disabled)
                .frame(width: 5, height: 5)
            Text(check.text).font(.system(size: 11)).foregroundStyle(check.tone == .blocked ? Palette.attentionText : Palette.ink2)
                .lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 4)
            if let action = check.action {
                Button(action.title, action: action.run)
                    .buttonStyle(.plain)
                    .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.attentionText)
                    .padding(.horizontal, 7).frame(height: 18)
                    .background(Palette.attentionFill, in: Capsule())
            }
        }
    }

    // Codex switches only through a connected desktop; otherwise its rules can only reset.
    private var codexCheck: Check? {
        let count = accounts.filter { $0.provider == "codex" }.count
        guard count > 0 else { return nil }
        if cliOnly { return Check(tone: .quiet, text: "Codex：只用 CLI 模式，见底只会重置") }
        let desktop = service.directory?.desktop
        if desktop?.enabled != true {
            return Check(tone: .blocked, text: "Codex：热切换关着，目前只会重置", action: ("去打开", { AppNavigation.shared.showDesktopConnection() }))
        }
        if desktop?.connected != true {
            return Check(tone: .blocked, text: "Codex：桌面没连上，目前只会重置", action: ("去连接", { AppNavigation.shared.showDesktopConnection() }))
        }
        return Check(tone: .ready, text: "Codex：\(count) 个账号已就绪，桌面已连可热切换")
    }

    // New Claude accounts stay out until someone turns 参与 on, so say how many are in.
    private var claudeCheck: Check? {
        let rules = draft.accounts.filter { tool(of: $0.id) == "claude" }
        guard !rules.isEmpty else { return nil }
        let joined = rules.filter(\.participate).count
        if joined == 0 { return Check(tone: .quiet, text: "Claude Code：\(rules.count) 个账号默认未参与，接力需手动开") }
        return Check(tone: .ready, text: "Claude Code：\(joined) 个账号参与接力" + (joined < rules.count ? "，\(rules.count - joined) 个未参与" : ""))
    }

    // Once, after the first-time card turned the strategy on.
    private var startedNote: some View {
        HStack(spacing: 9) {
            BrandTile(size: 16)
            Text("自动规则已开始运行。每次换号或用卡都会记在右上角「执行记录」里，改规则保存时会再请你确认。")
                .font(.system(size: 12)).foregroundStyle(Palette.ink).fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button { doneHint = "done" } label: {
                Icon(.plus, size: 11, color: Palette.deep, weight: 2)
                    .rotationEffect(.degrees(45))
                    .frame(width: 22, height: 22).contentShape(Circle())
            }
            .buttonStyle(.plain)
            .help("不再显示")
            .accessibilityLabel("关闭提示，不再显示")
        }
        .padding(.leading, 12).padding(.trailing, 8).padding(.vertical, 8)
        .frame(minHeight: 40)
        .background(Palette.wash, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.brand.opacity(0.18), lineWidth: 1))
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
                   text: "\(left.map(\.displayName).joined(separator: "、")) 不在规则里，不会自动换号或重置。要让它们参与，先从 AuthBaton 移除不用的账号。")
        }
    }

    // MARK: 接力

    private func lane(_ provider: String, status: CodexStrategyStatus) -> some View {
        let rules = draft.accounts.filter { tool(of: $0.id) == provider }
        let count = accounts.filter { $0.provider == provider }.count
        let index = draft.accounts.firstIndex { $0.id == picked && tool(of: $0.id) == provider }
        let caption = provider == "codex" ? (guiding ? "从左往右接力 · 新号默认参与、只换号" : "从左往右接力")
                                          : (guiding ? "只换 Claude Code，不换桌面；新号默认不参与、只重置" : "只换 Claude Code，不暂停会话")
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 9) {
                Icon(.provider(provider), size: 15)
                Text(GlyphTile.name(provider)).font(.system(size: 13, weight: .semibold))
                Text(caption).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                Spacer(minLength: 7)
                if orderChanged(provider) { orderEdits(provider) }
                if provider == "codex", rules.count > 1, !codexCanSwitch, !orderChanged(provider) { desktopChip }
                if count == 1 {
                    Text("仅 1 个账号 · 暂无法接力，只能重置").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                        .padding(.horizontal, 9).frame(height: 22).background(Palette.chip, in: Capsule())
                }
            }
            .frame(minHeight: 24)
            if rules.isEmpty && count > 0 {
                // Past the rule limit: the accounts exist but none of them is on the relay.
                Text("\(count) 个账号不在规则里，见底时不会自动处理。").font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 12)
            } else {
                stations(rules, provider: provider, add: count <= 1).padding(.top, 14)
            }
            if let index {
                editor($draft.accounts[index], lane: rules).padding(.top, 14)
            }
        }
        .padding(.horizontal, 19).padding(.top, 15).padding(.bottom, 17)
        .loopCard(radius: 19)
        .disabled(!editable)
    }

    // The lane's new order, put back or saved from where it was changed.
    private func orderEdits(_ provider: String) -> some View {
        HStack(spacing: 6) {
            Text("顺位已改").font(.system(size: 11)).foregroundStyle(Palette.deep)
            Button("撤销") { undoOrder(provider) }.buttonStyle(.pill(.secondary, height: 24))
            // With other edits waiting too, the save bar below saves everything; one button, not two.
            if !rulesDirty {
                Button(saving ? "正在保存…" : "保存规则") {
                    service.clearAccountError()
                    if draft.enabled { confirm = .save } else { save() }
                }
                .buttonStyle(.pill(.primary, height: 24))
                .disabled(saving || service.accountBusy || !editable)
            }
        }
    }

    // Puts one lane back in its saved order and keeps every other edit.
    private func undoOrder(_ provider: String) {
        let saved = laneOrder(baseline, provider)
        var lane = draft.accounts.filter { tool(of: $0.id) == provider }
            .sorted { (saved.firstIndex(of: $0.id) ?? .max) < (saved.firstIndex(of: $1.id) ?? .max) }
            .makeIterator()
        withAnimation(.easeOut(duration: 0.18)) {
            draft.accounts = draft.accounts.map { tool(of: $0.id) == provider ? (lane.next() ?? $0) : $0 }
        }
    }

    private var desktopChip: some View {
        let off = service.directory?.desktop?.enabled != true
        return HStack(spacing: 7) {
            Text(off ? "热切换关着：现在只会重置，不会换号" : "桌面没连上：现在只会重置，不会换号")
            Button(off ? "去打开" : "去连接") { AppNavigation.shared.showDesktopConnection() }
                .buttonStyle(.plain)
                .font(.system(size: 11, weight: .semibold))
                .padding(.horizontal, 7).frame(height: 17)
                .background(Palette.card, in: Capsule())
        }
        .font(.system(size: 11)).foregroundStyle(Palette.attentionText)
        .padding(.leading, 9).padding(.trailing, 3.5).frame(height: 22)
        .background(Palette.attentionFill, in: Capsule())
    }

    // Each account is a brick under a slot number: the number belongs to the place, so dragging a brick
    // to another slot changes the relay order. While a brick is dragged, its slot shows where it lands.
    // Bricks keep one width and line up from the left, so a wider window shows more of the lane; a lane
    // longer than the window scrolls sideways.
    @ViewBuilder private func stations(_ rules: [CodexStrategyConfig.Rule], provider: String, add: Bool) -> some View {
        let movable = rules.count > 1 && editable
        let row = HStack(alignment: .brickMiddle, spacing: 0) {
            ForEach(Array(rules.enumerated()), id: \.element.id) { offset, rule in
                slot(rule, number: offset + 1, movable: movable)
                    .frame(width: Self.brickWidth)
                if offset + 1 < rules.count || add {
                    RelayLine(style: offset + 1 < rules.count ? connector(rule, rules[offset + 1], provider: provider) : .idle,
                              arrow: offset + 1 < rules.count && rule.participate && rules[offset + 1].participate)
                        .frame(width: 14)
                        .padding(.horizontal, 4)
                        .alignmentGuide(.brickMiddle) { $0[VerticalAlignment.center] }
                }
            }
            if add { addSlot(provider, empty: rules.isEmpty).frame(width: Self.brickWidth) }
        }
        ViewThatFits(in: .horizontal) {
            row.frame(maxWidth: .infinity, alignment: .leading)
            ScrollView(.horizontal, showsIndicators: false) { row.padding(.vertical, 2) }
        }
    }

    static let brickWidth: CGFloat = 172

    private func slot(_ rule: CodexStrategyConfig.Rule, number: Int, movable: Bool) -> some View {
        let landing = dragging == rule.id
        let brick = Brick(account: account(rule.id), rule: rule, picked: picked == rule.id, movable: movable) { picked = rule.id }
        return VStack(spacing: 5) {
            Text(rule.participate ? "\(number)" : "–")
                .font(.system(size: landing ? 11 : 10, weight: landing ? .bold : .semibold, design: .rounded)).monospacedDigit()
                .foregroundStyle(landing ? Palette.brand : !rule.participate ? Palette.dash : picked == rule.id ? Palette.deep : Palette.disabled)
                .frame(height: 13)
                .help(rule.participate ? "接力第 \(number) 位" : "没参与接力，没有顺位")
                .accessibilityHidden(true)
            Group {
                if landing {
                    // Same size as the brick, so the lane does not move while the drag passes over it.
                    brick.hidden().overlay { LandingSlot() }
                } else if movable {
                    brick
                        .onDrag {
                            dragging = rule.id
                            return NSItemProvider(object: rule.id as NSString)
                        } preview: {
                            Brick(account: account(rule.id), rule: rule, picked: false, movable: true, lifted: true) {}
                                .frame(width: 150)
                                .padding(10)
                        }
                } else {
                    brick
                }
            }
            .alignmentGuide(.brickMiddle) { $0[VerticalAlignment.center] }
            .onDrop(of: [UTType.text], delegate: BrickDrop(target: rule.id, dragging: $dragging) { id, target in
                withAnimation(.easeOut(duration: 0.18)) { _ = place(id, at: target) }
            })
        }
    }

    // A line means a switch can happen from this station to the next: both take part and this one switches.
    // The first-time card shows the lines as they would be once on, so a desktop that is off shows up here too.
    private func connector(_ from: CodexStrategyConfig.Rule, _ to: CodexStrategyConfig.Rule, provider: String) -> RelayLine.Style {
        guard draft.enabled || guiding, from.participate, from.order.switches, to.participate else { return .idle }
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

    // A lane with one account or none ends in an empty brick that adds one.
    private func addSlot(_ provider: String, empty: Bool) -> some View {
        VStack(spacing: 5) {
            Color.clear.frame(height: 13)
            Button { sheet = .login(provider: provider, accountID: nil) } label: {
                VStack(alignment: .leading, spacing: 0) {
                    ZStack {
                        RoundedRectangle(cornerRadius: 4.5, style: .continuous)
                            .fill(Palette.wash)
                            .overlay(RoundedRectangle(cornerRadius: 4.5, style: .continuous)
                                .strokeBorder(Palette.brand, style: StrokeStyle(lineWidth: 1.3, dash: [3, 2.5])))
                            .frame(width: 15, height: 15).rotationEffect(.degrees(45))
                        Icon(.plus, size: 9, color: Palette.deep, weight: 2.4)
                    }
                    .frame(width: 22, height: 22)
                    Text("添加 \(provider == "codex" ? "Codex" : "Claude") 账号").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.deep)
                        .padding(.top, 5)
                    Text(empty ? "有账号后才能加入接力" : "有 2 个以上才能接力").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                        .padding(.top, 1)
                }
                .frame(maxWidth: .infinity, minHeight: Brick.contentHeight, alignment: .leading)
                .padding(.horizontal, 10).padding(.vertical, 9)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .strokeBorder(Palette.soft, style: StrokeStyle(lineWidth: 1.3, dash: [4, 3])))
                .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            }
            .buttonStyle(.plain)
            .alignmentGuide(.brickMiddle) { $0[VerticalAlignment.center] }
            .disabled(service.accountBusy || service.directory?.busy == true || service.directory?.recoveryNeeded == true)
        }
        .frame(maxWidth: .infinity)
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
            Text(rule.wrappedValue.participate ? rule.wrappedValue.order.explanation : "不参与：见底时不会自动换走，也不会自动用重置卡。")
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

// One account on its lane, as one piece: seal, name and grip, how it stands, its rule. Clicking picks it;
// dragging it (when the lane has more than one) moves it to another slot.
private struct Brick: View {
    let account: ManagedAccount?
    let rule: CodexStrategyConfig.Rule
    let picked: Bool
    let movable: Bool
    var lifted = false
    let pick: () -> Void
    @State private var hovering = false

    static let contentHeight: CGFloat = 66

    init(account: ManagedAccount?, rule: CodexStrategyConfig.Rule, picked: Bool, movable: Bool, lifted: Bool = false, pick: @escaping () -> Void) {
        self.account = account
        self.rule = rule
        self.picked = picked
        self.movable = movable
        self.lifted = lifted
        self.pick = pick
    }

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        Button(action: pick) {
            content
                .padding(.horizontal, 10).padding(.vertical, 9)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(picked || lifted || hovering ? Palette.card : Palette.hover, in: shape)
                .overlay { outline(shape) }
                .shadow(color: .black.opacity(lifted ? 0.16 : picked ? 0.07 : hovering ? 0.05 : 0), radius: lifted ? 13 : 5, y: lifted ? 9 : 3)
                .contentShape(shape)
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(movable ? "点一下改规则；按住拖到别的位置，换接力顺序" : "点一下改规则")
        .accessibilityLabel(account?.displayName ?? "账号")
        .accessibilityAddTraits(picked ? .isSelected : [])
    }

    @ViewBuilder private func outline(_ shape: RoundedRectangle) -> some View {
        if picked || lifted {
            shape.strokeBorder(Palette.brand, style: StrokeStyle(lineWidth: 1.5, dash: rule.participate || lifted ? [] : [4, 3]))
        } else if rule.participate {
            shape.strokeBorder(Palette.ring, lineWidth: 1)
        } else {
            shape.strokeBorder(Palette.dashStrong.opacity(0.6), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
        }
    }

    private var content: some View {
        let name = account?.displayName ?? "账号"
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Seal(name, style: !rule.participate ? .out : account?.native == true ? .inUse : .standby, size: 22)
                Text(name).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink).lineLimit(1)
                Spacer(minLength: 4)
                if movable { Grip(color: lifted ? Palette.brand : picked ? Palette.deep : hovering ? Palette.ink3 : Palette.dashStrong) }
            }
            .frame(height: 22)
            status.padding(.top, 5)
            HStack(spacing: 5) {
                if rule.participate {
                    Pill("≤ \(rule.below)%")
                    Pill(rule.order.label)
                } else {
                    Pill("未参与", style: .dashed)
                    Pill(rule.order.label, style: .dashed)
                }
            }
            .padding(.top, 7)
        }
        .frame(height: Self.contentHeight, alignment: .top)
    }

    // In use (with a dot), how much is left; what needs a hand replaces it in amber.
    @ViewBuilder private var status: some View {
        if let account, account.needsAction {
            Text(account.health.title).font(.system(size: 11)).foregroundStyle(Palette.attentionText).lineLimit(1)
        } else {
            let inUse = account?.native == true
            let parts = [inUse ? "在用" : nil, account?.lowestPrimary.map { "剩余 \($0)%" }].compactMap { $0 }
            HStack(spacing: 5) {
                if inUse { Circle().fill(rule.participate ? Palette.brand : Palette.disabled).frame(width: 5, height: 5) }
                Text(parts.isEmpty ? (account?.quotaCaption ?? " ") : parts.joined(separator: " · "))
                    .font(.system(size: 11)).monospacedDigit().lineLimit(1)
                    .foregroundStyle(!rule.participate ? Palette.ink3 : inUse ? Palette.deep : Palette.ink2)
            }
            .frame(height: 14)
        }
    }
}

// Six dots: this piece can be picked up.
private struct Grip: View {
    let color: Color
    var body: some View {
        VStack(spacing: 2.5) {
            ForEach(0..<3, id: \.self) { _ in
                HStack(spacing: 2.5) {
                    Circle().frame(width: 2, height: 2)
                    Circle().frame(width: 2, height: 2)
                }
            }
        }
        .foregroundStyle(color)
        .accessibilityHidden(true)
    }
}

// Where the dragged brick will land: a dashed brand slot the size of a brick.
private struct LandingSlot: View {
    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
        shape.fill(Palette.wash)
            .overlay(shape.strokeBorder(Palette.brand, style: StrokeStyle(lineWidth: 1.5, dash: [5, 4])))
            .overlay {
                RoundedRectangle(cornerRadius: 3, style: .continuous)
                    .strokeBorder(Palette.brand.opacity(0.3), lineWidth: 1.3)
                    .frame(width: 12, height: 12).rotationEffect(.degrees(45))
            }
            .accessibilityHidden(true)
    }
}

// Live reorder: entering another brick's slot moves the dragged rule there; letting go ends the drag.
private struct BrickDrop: DropDelegate {
    let target: String
    @Binding var dragging: String?
    let move: (String, String) -> Void

    func validateDrop(info: DropInfo) -> Bool { dragging != nil }
    func dropEntered(info: DropInfo) {
        guard let dragging, dragging != target else { return }
        move(dragging, target)
    }
    func dropUpdated(info: DropInfo) -> DropProposal? { DropProposal(operation: .move) }
    func performDrop(info: DropInfo) -> Bool {
        dragging = nil
        return true
    }
}

private extension VerticalAlignment {
    // The middle of a lane's bricks, below their slot numbers: where the lines between them run.
    enum BrickMiddle: AlignmentID {
        static func defaultValue(in context: ViewDimensions) -> CGFloat { context[VerticalAlignment.center] }
    }
    static let brickMiddle = VerticalAlignment(BrickMiddle.self)
}
