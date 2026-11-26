import AppKit
import SwiftUI

// The relay that 换号与重置 used to hold, now on each tool's own page (11-25, Gemini 稿): the queue's header
// with the one switch and the run history, each row's line and 见底时 opened in place, a drop that takes effect
// as it lands, and the account that takes over beside 在用. Every change sends the whole config, confirm=true.

// MARK: 规则

enum RelayRules {
    // automation.rs validate(): the service refuses a strategy with more rules than this.
    static let limit = 32

    @MainActor static func save(_ config: CodexStrategyConfig?, _ service: ServiceController) async {
        guard let config else { return }
        await service.accountOperation("accounts.strategy", params: LoginParams(confirm: true, strategy: config.input))
    }

    // What turning these rules on allows, and nothing they cannot do: each point is there only when a rule that
    // takes part makes it true.
    static func confirmation(_ rules: [CodexStrategyConfig.Rule], tool: (String) -> String?) -> [String] {
        let joined = rules.filter(\.participate)
        guard !joined.isEmpty else { return ["现在没有账号参与：不会自动换号，也不会自动用重置卡。把账号加入接力后才生效。"] }
        var points = ["在用的账号降到自己的见底线时，按它的「见底时」处理。"]
        var switching: [String] = []
        if joined.contains(where: { $0.order.switches && tool($0.id) == "codex" }) {
            switching.append("Codex 换号前先暂停桌面里正在跑的任务，换完逐个接续")
        }
        if joined.contains(where: { $0.order.switches && tool($0.id) == "claude" }) {
            switching.append("Claude 直接换默认登录，运行中的会话约 30 秒内改用新账号，被额度打断的会话要在原会话按一次回车继续；从不自动换 Claude 桌面对话")
        }
        if !switching.isEmpty { points.append("换号从第 1 位往下找第一个没见底、能换的同类账号。" + switching.joined(separator: "；") + "。") }
        if joined.contains(where: \.order.resets) {
            points.append("设成先重置、先换号或只重置的账号会自动用掉重置卡，不逐次弹窗：见底时用；卡离到期不到 30 分钟、额度用过时也会用。"
                          + (joined.contains { $0.order.resets && tool($0.id) == "claude" } ? "Claude 账号用的是官方排的下一张，不能选类别。" : ""))
        } else {
            points.append("没有账号会自动用重置卡。")
        }
        if joined.contains(where: { $0.order.switches && tool($0.id) == "claude" }) {
            points.append("Claude Code 正在用 Fable 时，在用的 Claude 账号的 Fable 额度到了见底线也会换号，只换到 Fable 额度也高于见底线的账号。")
        }
        points.append("结果不明确时会停下并留下记录；随时可以关闭。")
        return points
    }
}

extension AccountDirectory {
    // One tool's rules in relay order; each one's place is its 第 N 位.
    func relayRules(_ provider: String) -> [CodexStrategyConfig.Rule] {
        (strategy?.config.accounts ?? []).filter { rule in accounts.contains { $0.id == rule.id && $0.provider == provider } }
    }

    // Where the rules would move this tool's login if its account reached its line now. The service's own
    // relay_target decides it, the same rule the switch uses; nothing is worked out here.
    func relayNext(_ provider: String) -> String? {
        guard strategy?.config.enabled == true else { return nil }
        return provider == "claude" ? strategy?.nextClaude : strategy?.nextCodex
    }

    // The saved rules with one account's changed; every other rule keeps its place. An account with no rule
    // yet gets one at the end of the relay (only switching, 2%), unless the limit is reached.
    func relayConfig(_ id: String, change: (inout CodexStrategyConfig.Rule) -> Void) -> CodexStrategyConfig? {
        guard var config = strategy?.config else { return nil }
        if let index = config.accounts.firstIndex(where: { $0.id == id }) {
            change(&config.accounts[index])
        } else {
            guard config.accounts.count < RelayRules.limit else { return nil }
            var rule = CodexStrategyConfig.Rule(id: id, participate: true)
            change(&rule)
            config.accounts.append(rule)
        }
        config.accounts.removeAll { rule in !accounts.contains { $0.id == rule.id } }
        return config
    }

    // The saved rules with one tool's in a new order; every other rule keeps its place.
    func relayConfig(_ provider: String, order: [String]) -> CodexStrategyConfig? {
        guard var config = strategy?.config else { return nil }
        let ids = Set(relayRules(provider).map(\.id))
        guard Set(order) == ids, order.count == ids.count else { return nil }
        let rules = Dictionary(config.accounts.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let places = config.accounts.indices.filter { ids.contains(config.accounts[$0].id) }
        for (offset, index) in places.enumerated() {
            guard let rule = rules[order[offset]] else { return nil }
            config.accounts[index] = rule
        }
        config.accounts.removeAll { rule in !accounts.contains { $0.id == rule.id } }
        return config
    }
}

// MARK: 撤销

// A drop takes effect at once; for three seconds a note at the foot of the page (and ⌘Z) puts the old order back.
@MainActor
final class RelayUndo: ObservableObject {
    static let shared = RelayUndo()
    struct Item: Identifiable, Equatable {
        let id = UUID()
        let text: String
        let provider: String
        let order: [String]
    }
    @Published private(set) var item: Item?
    private var expiry: Task<Void, Never>?

    func offer(_ text: String, provider: String, order: [String]) {
        withAnimation(.easeOut(duration: 0.18)) { item = Item(text: text, provider: provider, order: order) }
        expiry?.cancel()
        expiry = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 3_000_000_000)
            if !Task.isCancelled { withAnimation(.easeOut(duration: 0.18)) { self?.item = nil } }
        }
    }

    func undo(_ service: ServiceController) {
        guard let item, let config = service.directory?.relayConfig(item.provider, order: item.order) else { return }
        self.item = nil
        expiry?.cancel()
        Task { await RelayRules.save(config, service) }
    }
}

struct RelayUndoToast: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var undo = RelayUndo.shared
    let provider: String

    var body: some View {
        if let item = undo.item, item.provider == provider {
            HStack(spacing: 9) {
                Circle().fill(Palette.brand).frame(width: 6, height: 6)
                Text(item.text).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink).lineLimit(1)
                Button { undo.undo(service) } label: {
                    HStack(spacing: 5) {
                        Text("撤销")
                        Text("⌘Z").font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.deep.opacity(0.7))
                    }
                    .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.deep)
                    .padding(.horizontal, 8).frame(height: 22)
                    .background(Palette.wash, in: Capsule())
                    .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .keyboardShortcut("z", modifiers: .command)
                .disabled(service.accountBusy)
            }
            .padding(.leading, 12).padding(.trailing, 5).frame(height: 32)
            .background(Capsule().fill(Palette.card).shadow(color: .black.opacity(0.12), radius: 10, y: 4))
            .overlay(Capsule().strokeBorder(Palette.ring, lineWidth: 1))
            .padding(.bottom, 14)
            .transition(.move(edge: .bottom).combined(with: .opacity))
        }
    }
}

// MARK: 队列标题

// 自动接力队列 · N 个参与, and on the right the one switch (共用: Codex and Claude share it), the run history
// and, where rows can open into cards, 紧凑 / 展开.
struct RelayQueueHeader: View {
    @EnvironmentObject private var service: ServiceController
    let directory: AccountDirectory
    let count: Int
    var expanded: Binding<Bool>? = nil
    let present: (AccountSheet) -> Void

    var body: some View {
        let enabled = directory.strategy?.config.enabled == true
        HStack(spacing: 7) {
            Icon(.relay, size: 13, color: Palette.brand, weight: 1.9)
            Text("自动接力队列").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
            Text(enabled ? "\(count) 个参与" : "\(count) 个参与 · 自动接力关着").font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
            Spacer(minLength: 8)
            HStack(spacing: 6) {
                Text("自动接力").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                Text("共用").font(.system(size: 10)).foregroundStyle(Palette.ink3)
                    .padding(.horizontal, 5).frame(height: 16).background(Palette.chip, in: Capsule())
                    .help("Codex 和 Claude 共用这个开关")
                Toggle("自动接力", isOn: Binding(get: { enabled }, set: { on in
                    service.clearAccountError()
                    present(on ? .strategyStart : .strategyStop)
                }))
                .labelsHidden().toggleStyle(.switch).controlSize(.mini)
                .disabled(service.accountBusy || directory.strategy == nil || (!enabled && directory.strategy?.run != nil))
                .help(enabled ? "关闭前会再确认一次" : "开启前会再确认一次")
            }
            .padding(.leading, 9).padding(.trailing, 6).frame(height: 26)
            .background(Capsule().fill(Palette.card))
            .overlay(Capsule().strokeBorder(Palette.ring, lineWidth: 1))
            Button { present(.history(nil)) } label: {
                HStack(spacing: 5) {
                    Icon(.clock, size: 12, color: Palette.ink2)
                    Text("执行记录")
                }
            }
            .buttonStyle(.pill(.secondary, height: 26))
            if let expanded {
                PillSegments(segments: [.init(value: false, title: "紧凑"), .init(value: true, title: "展开")],
                             selection: expanded.wrappedValue, select: { expanded.wrappedValue = $0 })
                    .fixedSize()
            }
        }
        .padding(.horizontal, 3.5)
    }
}

// MARK: 见底规则

// ≤3% · 先换号 on a row; open, the row grows a line to change it.
struct RelayRuleChip: View {
    let rule: CodexStrategyConfig.Rule
    @Binding var open: Bool

    var body: some View {
        Button { withAnimation(.easeOut(duration: 0.18)) { open.toggle() } } label: {
            HStack(spacing: 4) {
                Text("≤\(rule.below)% · \(rule.order.label)").monospacedDigit()
                Icon(.chevronDown, size: 8, color: open ? Palette.deep : Palette.ink3, weight: 2.4).rotationEffect(.degrees(open ? 180 : 0))
            }
            .font(.system(size: 11, weight: .semibold)).foregroundStyle(open ? Palette.deep : Palette.ink2)
            .padding(.leading, 8).padding(.trailing, 7).frame(height: 22)
            .background(open ? Palette.wash : Palette.chip, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(open ? Palette.brand.opacity(0.5) : Color.clear, lineWidth: 1))
            .contentShape(Rectangle())
            .fixedSize()
        }
        .buttonStyle(.plain)
        .help(rule.order.explanation)
        .accessibilityLabel("见底规则：剩余 \(rule.below)% 时\(rule.order.label)")
    }
}

// The opened line: 见底线, 见底时 and 参与. Each change takes effect at once; the line steps wait a moment so a few
// clicks send one change.
struct RelayRuleEditor: View {
    @EnvironmentObject private var service: ServiceController
    let rule: CodexStrategyConfig.Rule
    let directory: AccountDirectory
    @State private var below: Int

    init(rule: CodexStrategyConfig.Rule, directory: AccountDirectory) {
        self.rule = rule
        self.directory = directory
        _below = State(initialValue: rule.below)
    }

    var body: some View {
        HStack(spacing: 12) {
            Text("见底线").font(.system(size: 11)).foregroundStyle(Palette.ink2)
            HStack(spacing: 0) {
                step("−", label: "调低见底线") { below = max(0, below - 1) }.disabled(below == 0)
                Text("剩余 \(below)%").font(.system(size: 12, weight: .semibold)).monospacedDigit().frame(minWidth: 58)
                step("+", label: "调高见底线") { below = min(99, below + 1) }.disabled(below == 99)
            }
            .background(Palette.card, in: Capsule())
            .overlay(Capsule().strokeBorder(Palette.edge, lineWidth: 1))
            Spacer(minLength: 10)
            Text("见底时").font(.system(size: 11)).foregroundStyle(Palette.ink2)
            HStack(spacing: 0) {
                ForEach(CodexStrategyOrder.ruleOrder) { order in
                    let on = order == rule.order
                    Button { change { $0.order = order } } label: {
                        Text(order.label)
                            .font(.system(size: 11, weight: on ? .semibold : .regular))
                            .foregroundStyle(on ? Palette.ink : Palette.ink2)
                            .padding(.horizontal, 10).frame(height: 22)
                            .background { if on { Capsule().fill(Palette.card).shadow(color: .black.opacity(0.12), radius: 1, y: 1) } }
                            .contentShape(Capsule())
                    }
                    .buttonStyle(.plain)
                    .help(order.explanation)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            .padding(2)
            .background(Palette.rail, in: Capsule())
            Spacer(minLength: 10)
            Text("参与").font(.system(size: 11)).foregroundStyle(Palette.ink2)
            Toggle("参与", isOn: Binding(get: { rule.participate }, set: { on in change { $0.participate = on } }))
                .labelsHidden().toggleStyle(.switch).controlSize(.mini)
                .help("关掉就移出接力：见底时不会自动换走，也不会自动用重置卡")
        }
        .disabled(service.accountBusy || directory.strategy?.busy == true || directory.strategy?.run != nil)
        .padding(.horizontal, 12).frame(height: 40)
        .background(Palette.panel)
        .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
        .onChange(of: rule.below) { _, value in below = value }
        .task(id: below) {
            guard below != rule.below else { return }
            try? await Task.sleep(nanoseconds: 600_000_000)
            guard !Task.isCancelled, below != rule.below else { return }
            change { $0.below = below }
        }
    }

    private func change(_ edit: (inout CodexStrategyConfig.Rule) -> Void) {
        guard let config = directory.relayConfig(rule.id, change: edit) else { return }
        service.clearAccountError()
        Task { await RelayRules.save(config, service) }
    }

    private func step(_ symbol: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(symbol).font(.system(size: 13, weight: .medium)).foregroundStyle(Palette.ink2)
                .frame(width: 26, height: 24).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

extension CodexStrategyOrder {
    // The four ways in the order the editor shows them: switching first, as most rules do.
    static let ruleOrder: [CodexStrategyOrder] = [.switchFirst, .resetFirst, .switchOnly, .resetOnly]
}

// MARK: 下一个

// The account the rules would switch to now, beside 在用: ▶ 第 N 位 says what it is, no words around it.
struct RelayNextCard: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let rank: Int
    let directory: AccountDirectory
    // 提前换: what switching to it by hand does on this page.
    let switchNow: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                PointerRank(text: "第 \(rank) 位", small: false)
                Spacer(minLength: 6)
                if let plan = account.plan { Pill(plan.uppercased(), small: true) }
            }
            .padding(.horizontal, 14).frame(height: 42)
            .background(Palette.wash.opacity(0.55))
            .overlay(alignment: .bottom) { Rectangle().fill(Palette.divider).frame(height: 1) }
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 9) {
                    Seal(account.displayName, style: .standby, size: 28)
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 5) {
                            Text(accountName: account.displayName).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                            CloudBadge(account: account, size: 11)
                        }
                        if !account.subtitle.isEmpty {
                            Text(account.subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                        }
                    }
                }
                if let week = account.weekSpent {
                    Text(week.refill()).font(.system(size: 12)).foregroundStyle(Palette.attentionText)
                } else {
                    HStack(alignment: .top, spacing: 14) {
                        ForEach(Array(account.primaryWindows.prefix(2).enumerated()), id: \.offset) { _, window in
                            VStack(alignment: .leading, spacing: 5) {
                                HStack(alignment: .firstTextBaseline, spacing: 2) {
                                    Text("\(window.remaining)").font(.system(size: 26, weight: .semibold, design: .rounded)).monospacedDigit()
                                    Text("%").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink2)
                                    Text(window.shortLabel).font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.leading, 4)
                                }
                                MiniBar(remaining: window.remaining)
                                Text(window.remaining >= 100 ? "未使用" : window.refill()).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
            }
            .padding(14)
            Spacer(minLength: 0)
            HStack(spacing: 8) {
                Pill((account.creditCount ?? 0) > 0 ? "\(account.creditCount ?? 0) 张重置卡" : "没有重置卡", small: true)
                Spacer(minLength: 6)
                Button(action: switchNow) {
                    HStack(spacing: 5) {
                        SkipAheadMark()
                        Text("提前换")
                    }
                }
                .buttonStyle(.pill(.soft))
                .disabled(service.accountBusy || directory.primaryAction(for: account) != .switchTo || directory.switchDisabled(for: account))
                .help("现在就换到第 \(rank) 位 \(account.displayName)，不等见底")
                .accessibilityLabel("提前换到 \(account.displayName)")
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .loopCard(radius: 17, raised: true)
    }
}

// No account in the queue can take over: say so, when the first one is back, and the best one outside it.
struct RelayNoneCard: View {
    @EnvironmentObject private var service: ServiceController
    let directory: AccountDirectory
    let provider: String
    // Outside the relay, switchable and with the most left; the page knows how to switch to it.
    let spare: ManagedAccount?
    let switchTo: (ManagedAccount) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 7) {
                Icon(.alert, size: 14, color: Palette.attention)
                Text("队列里都见底了").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.attentionText)
            }
            if let first = earliest {
                Text("最早回满：\(first.account.displayName)，\(first.when)").font(.system(size: 11)).foregroundStyle(Palette.attentionInk)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            if let spare {
                HStack(spacing: 8) {
                    Seal(spare.displayName, style: .out, size: 22)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(accountName: spare.displayName).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                        Text("不参与 · 还剩 \(spare.lowestPrimary ?? 0)%").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                    Spacer(minLength: 4)
                    Button("换上") { switchTo(spare) }
                        .buttonStyle(.pill(.onAttention))
                        .disabled(service.accountBusy || directory.switchDisabled(for: spare))
                }
                .padding(.horizontal, 10).padding(.vertical, 8)
                .background(Palette.card, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 17, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 17, style: .continuous).strokeBorder(Palette.attentionDash.opacity(0.45), lineWidth: 1))
    }

    // The queue account whose lowest window comes back first.
    private var earliest: (account: ManagedAccount, when: String)? {
        let ids = directory.relayRules(provider).filter(\.participate).map(\.id)
        return directory.accounts.filter { ids.contains($0.id) && !$0.native }
            .compactMap { account -> (ManagedAccount, Double, String)? in
                guard let window = account.primaryWindows.filter({ $0.remaining <= (directory.notch(for: account) ?? 0) })
                        .max(by: { ($0.resetDate ?? .distantPast) < ($1.resetDate ?? .distantPast) }),
                      let date = window.resetDate else { return nil }
                return (account, date.timeIntervalSince1970, window.refill())
            }
            .min { $0.1 < $1.1 }
            .map { ($0.0, $0.2) }
    }
}

// MARK: 正在换号

// While a Codex switch pauses, switches and resumes the desktop's tasks, and when one stopped half way.
struct RelayRunCard: View {
    @EnvironmentObject private var service: ServiceController
    let run: CodexStrategyStatus.Run
    let status: CodexStrategyStatus
    let directory: AccountDirectory
    let present: (AccountSheet) -> Void
    @State private var confirmForget = false

    var body: some View {
        let stuck = status.phase == "attention"
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 9) {
                if stuck { Icon(.alert, size: 14, color: Palette.attention) } else { ProgressView().controlSize(.small) }
                Text((stuck ? "上次换号没确认完：" : "正在自动换号：") + "\(name(run.source)) → \(name(run.target))")
                    .font(.system(size: 12, weight: .semibold)).foregroundStyle(stuck ? Palette.attentionText : Palette.deep).lineLimit(1)
                Spacer(minLength: 7)
                if stuck {
                    Button("忘掉这次") { confirmForget = true }.buttonStyle(.pill(.secondary)).disabled(status.busy)
                }
                Button(stuck ? "核对" : "停止并核对…") { present(.strategyStop) }.buttonStyle(.pill(stuck ? .attention : .secondary))
            }
            if stuck, !status.message.isEmpty {
                Text(status.message).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
            }
            if !run.tasks.isEmpty {
                VStack(spacing: 0) {
                    ForEach(run.tasks, id: \.threadId) { task in
                        HStack {
                            Text("会话 \(task.threadId.prefix(12))").font(.system(size: 11, design: .monospaced)).foregroundStyle(Palette.ink2)
                            Spacer()
                            Text(task.label).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                        }
                        .frame(height: 24)
                    }
                }
                .padding(.horizontal, 12)
                .loopPanel(radius: 10)
            }
        }
        .padding(.horizontal, 15).padding(.vertical, 12)
        .background(stuck ? Palette.attentionWash : Palette.wash.opacity(0.6), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous)
            .strokeBorder(stuck ? Palette.attentionDash.opacity(0.45) : Palette.brand.opacity(0.25), lineWidth: 1))
        .alert("已在桌面核对这些会话？", isPresented: $confirmForget) {
            Button("返回", role: .cancel) {}
            Button("确认已核对") {
                Task { await service.accountOperation("accounts.strategy_forget", params: LoginParams(confirm: true)) }
            }
        } message: {
            Text("只清除这次换号的记录，自动接力保持关闭；不会启动或停止任何会话。")
        }
    }

    private func name(_ id: String) -> String { directory.accounts.first { $0.id == id }?.displayName ?? "账号" }
}

// The run, or the strategy stopped with nothing running: the slot under the page header.
struct RelayStatusSlot: View {
    let directory: AccountDirectory
    let provider: String
    let present: (AccountSheet) -> Void

    var body: some View {
        if let status = directory.strategy {
            if let run = status.run, directory.accounts.contains(where: { $0.id == run.target && $0.provider == provider }) {
                RelayRunCard(run: run, status: status, directory: directory, present: present)
            } else if status.run == nil, status.phase == "attention", !status.message.isEmpty {
                Banner(glyph: .relay, title: "自动接力停下了", text: status.message)
            }
        }
    }
}

// MARK: 拖动

// A drag let go between rows or outside the list ends without a drop, and no row hears of it. The mouse coming
// up ends it: the order on screen is kept, as a drop would keep it.
struct DragEnd: ViewModifier {
    @Binding var dragging: String?
    var ended: (String) -> Void = { _ in }

    func body(content: Content) -> some View {
        content.task(id: dragging) {
            guard dragging != nil else { return }
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 250_000_000)
                if NSEvent.pressedMouseButtons == 0, let id = dragging {
                    dragging = nil
                    ended(id)
                    break
                }
            }
        }
    }
}
