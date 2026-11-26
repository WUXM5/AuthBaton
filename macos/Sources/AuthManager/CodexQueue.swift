import SwiftUI
import UniformTypeIdentifiers

// Codex laid out as the Claude page is (11-25, Gemini 稿): 在用 with the account the rules would switch to beside it,
// the relay queue, and a drawer of the accounts outside it. Codex has one login for the CLI, Codex 桌面 and pi,
// so 在用 is one card.

// MARK: 在用

struct CodexStage: View {
    let directory: AccountDirectory
    let selection: String?
    let open: (AccountSheet) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("在用").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink).padding(.horizontal, 3.5)
            let wing = self.wing
            // One card: the account in use, and the wing it closes on the right.
            HStack(alignment: .top, spacing: -1) {
                if let current = directory.current("codex") {
                    CodexCard(account: current, directory: directory, selected: selection == current.id, open: open, joined: wing != nil)
                        .frame(maxWidth: .infinity)
                } else {
                    Text("还没有在用的 Codex 账号").font(.system(size: 12)).foregroundStyle(Palette.ink3)
                        .frame(maxWidth: .infinity, minHeight: 96).loopCard(radius: 19)
                }
                if let wing { wingView(wing).frame(width: RelayNextCard.width) }
            }
            .fixedSize(horizontal: false, vertical: true)
            .joinedShadow(wing != nil, radius: 19)
        }
    }

    // The account the rules would switch to now. Where no switch can happen (Codex 桌面 not connected, only the CLI)
    // the page's band says why; the card only says that the queue has nobody left.
    private var wing: RelayWingKind? {
        guard let current = directory.current("codex"), let rule = directory.rule(for: current.id),
              directory.strategy?.config.enabled == true, rule.participate, rule.order.switches else { return nil }
        let rules = directory.relayRules("codex")
        if let id = directory.relayNext("codex"), let account = directory.accounts.first(where: { $0.id == id }) {
            return .next(account, rank: (rules.firstIndex { $0.id == id } ?? 0) + 1)
        }
        return directory.desktop.map({ $0.enabled && $0.connected }) == true && rules.contains(where: { $0.participate && $0.id != current.id })
            ? .empty : nil
    }

    @ViewBuilder private func wingView(_ wing: RelayWingKind) -> some View {
        switch wing {
        case .next(let account, let rank):
            RelayNextCard(account: account, rank: rank, directory: directory, radius: 19) { open(.switchTo(account.id)) }
        case .empty:
            RelayNoneCard(directory: directory, provider: "codex", spare: spare, radius: 19) { open(.switchTo($0.id)) }
        }
    }

    private var spare: ManagedAccount? {
        directory.accounts.filter { account in
            account.provider == "codex" && !account.native && directory.rule(for: account.id)?.participate != true
                && directory.primaryAction(for: account) == .switchTo && (account.lowestPrimary ?? 0) > 0
        }
        .max { ($0.lowestPrimary ?? 0) < ($1.lowestPrimary ?? 0) }
    }
}

// MARK: 自动接力队列

struct CodexRelayDeck: View {
    static let expandedKey = "codex.deckExpanded"
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @AppStorage(CodexRelayDeck.expandedKey) private var expanded = false
    @State private var draft: [String]?
    @State private var dragging: String?
    @State private var openRow: String?
    @State private var editing: String? = CodexRelayDeck.previewEditing
    // Preview renders only: a row's rule line drawn open.
    @MainActor static var previewEditing: String?
    let directory: AccountDirectory
    // Only the CLI: no switch can happen, so the page says what the rules still do.
    var note: String? = nil
    let open: (AccountSheet) -> Void

    private var saved: [String] { directory.relayRules("codex").map(\.id) }
    private var order: [String] { draft ?? saved }
    private var next: String? { directory.relayNext("codex") }
    private var queue: [ManagedAccount] {
        order.compactMap { id in directory.rule(for: id)?.participate == true ? directory.accounts.first { $0.id == id } : nil }
    }
    private var movable: Bool {
        queue.count > 1 && !service.accountBusy && directory.strategy?.busy != true && directory.strategy?.run == nil && directory.strategy != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            RelayQueueHeader(directory: directory, count: queue.count, expanded: queue.isEmpty ? nil : $expanded, present: open)
            if queue.isEmpty {
                Text("还没有账号参与自动接力").font(.system(size: 12)).foregroundStyle(Palette.ink2)
                    .padding(.horizontal, 14).frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.dash, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            } else if expanded {
                VStack(spacing: 6) { ForEach(queue.filter(\.native)) { row($0) } }
                CardGrid {
                    ForEach(queue.filter { !$0.native }) { account in
                        CodexCard(account: account, directory: directory, selected: nav.selection == account.id, open: open)
                            .frame(maxHeight: .infinity, alignment: .top)
                    }
                }
            } else {
                VStack(spacing: 6) { ForEach(queue) { row($0) } }
            }
            if let note, !queue.isEmpty { Text(note).font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.horizontal, 3.5) }
        }
        .onChange(of: saved) { _, _ in draft = nil }
        .modifier(DragEnd(dragging: $dragging, ended: commit))
        .onAppear { openRequestedRule() }
        .onChange(of: nav.ruleEditor) { _, _ in openRequestedRule() }
    }

    private func openRequestedRule() {
        guard let id = nav.ruleEditor, saved.contains(id) else { return }
        nav.ruleEditor = nil
        editing = id
    }

    @ViewBuilder private func row(_ account: ManagedAccount) -> some View {
        let rank = order.firstIndex(of: account.id).map { $0 + 1 }
        let rule = directory.rule(for: account.id)
        let editingRow = Binding(get: { editing == account.id }, set: { editing = $0 ? account.id : nil })
        Group {
            if openRow == account.id {
                CodexCard(account: account, directory: directory, selected: false, open: open,
                          collapse: { withAnimation(.easeOut(duration: 0.18)) { openRow = nil } })
            } else {
                CodexRelayRow(account: account, place: rank.map { "第 \($0) 位" } ?? "–", next: account.id == next, movable: movable,
                              rule: rule, editing: editingRow, directory: directory, open: open,
                              expand: account.native ? nil : { withAnimation(.easeOut(duration: 0.18)) { openRow = account.id } })
            }
        }
        .opacity(dragging == account.id ? 0.4 : 1)
        .modifier(RowDrag(id: account.id, enabled: movable, dragging: $dragging))
        .onDrop(of: [UTType.text], delegate: RowDrop(target: account.id, dragging: $dragging, move: { from, to in
            withAnimation(.easeOut(duration: 0.18)) { place(from, at: to) }
        }, dropped: commit))
    }

    private func place(_ id: String, at target: String) {
        var ids = order
        guard id != target, let from = ids.firstIndex(of: id), let to = ids.firstIndex(of: target) else { return }
        ids.remove(at: from)
        ids.insert(id, at: to)
        draft = ids
    }

    // Let go: the order on screen takes effect at once, with three seconds to put it back.
    private func commit(_ id: String) {
        guard let draft, draft != saved, let config = directory.relayConfig("codex", order: draft) else { self.draft = nil; return }
        let name = directory.accounts.first { $0.id == id }?.displayName ?? "账号"
        RelayUndo.shared.offer("\(name) 已移到第 \((draft.firstIndex(of: id) ?? 0) + 1) 位", provider: "codex", order: saved)
        service.clearAccountError()
        Task {
            await RelayRules.save(config, service)
            if service.accountError != nil { self.draft = nil }
        }
    }
}

// One account in the queue: handle, place, who it is with its rule under the name, its numbers and its one
// button. The one in use stays in its place as a thin dashed line; it is the card above.
struct CodexRelayRow: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let place: String
    let next: Bool
    let movable: Bool
    let rule: CodexStrategyConfig.Rule?
    let editing: Binding<Bool>
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void
    var expand: (() -> Void)? = nil

    var body: some View {
        VStack(spacing: 0) {
            if account.native {
                anchorLine
            } else {
                // The Claude rows' two layouts, so the numbers line up from page to page.
                ViewThatFits(in: .horizontal) {
                    line(wide: true).frame(minWidth: 1080)
                    line(wide: false)
                }
                .padding(.leading, 9).padding(.trailing, 9).padding(.vertical, 8)
            }
            // A move between this Mac and the centre that failed: its 重试 / 放弃 stay on the row.
            if !account.native, account.transferFailed != nil { TransferBox(account: account).padding(.horizontal, 12).padding(.bottom, 10) }
            if let rule, editing.wrappedValue { RelayRuleEditor(rule: rule, directory: directory) }
        }
        .background(account.native ? (next ? Palette.wash.opacity(0.5) : Color.clear) : Palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)
            if account.native {
                shape.strokeBorder(Palette.dash, style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
            } else if account.transferFailed != nil {
                shape.strokeBorder(Palette.attention, lineWidth: 1.2)
            } else {
                shape.strokeBorder(next ? Palette.brand.opacity(0.35) : Palette.ring, lineWidth: 1)
            }
        }
        .relayLinked(next ? account.id : nil)
        .contentShape(Rectangle())
    }

    private var anchorLine: some View {
        HStack(spacing: 9) {
            DragGrip(active: movable)
            RankMark(place: place, next: next)
            Seal(account.displayName, style: .inUse, size: 18)
            Text(accountName: account.displayName).font(.system(size: 12, weight: .semibold)).lineLimit(1)
            CloudBadge(account: account, size: 11)
            Pill("在用", style: .brand, small: true)
            Spacer(minLength: 8)
            if let rule, rule.participate { RelayRuleChip(rule: rule, open: editing) }
            HStack(spacing: 3) {
                Text("↑")
                Text("在上方")
            }
            .font(.system(size: 11)).foregroundStyle(Palette.ink3)
        }
        .padding(.leading, 8).padding(.trailing, 12).frame(height: 34)
    }

    private func line(wide: Bool) -> some View {
        HStack(spacing: 9) {
            DragGrip(active: movable)
            RankMark(place: place, next: next)
            if wide {
                identity.frame(width: 300, alignment: .leading)
                CodexRowQuota(account: account, directory: directory, wide: true).frame(width: 452, alignment: .leading)
                Spacer(minLength: 0)
            } else {
                identity.frame(minWidth: 116, maxWidth: .infinity, alignment: .leading)
                CodexRowQuota(account: account, directory: directory, wide: false).frame(width: 298, alignment: .leading)
            }
            HStack(spacing: 6) {
                CodexRowActions(account: account, directory: directory, open: open)
                AccountMenu(account: account, directory: directory, open: open)
                if let expand {
                    Button(action: expand) {
                        Icon(.chevronRight, size: 11, color: Palette.ink3, weight: 2).frame(width: 22, height: 24).contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("在这里展开")
                    .accessibilityLabel("展开 \(account.displayName)")
                }
            }
            .fixedSize().frame(minWidth: 186, alignment: .trailing)
        }
    }

    private var identity: some View {
        HStack(spacing: 8) {
            Seal(account.displayName, style: .standby, size: 24)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 5) {
                    Text(accountName: account.displayName).font(.system(size: 13, weight: .semibold)).lineLimit(1).layoutPriority(1)
                    CloudBadge(account: account, size: 11)
                    if directory.desktop?.selected == account.id { Pill("桌面在用", style: .brand, small: true).fixedSize() }
                    if account.needsAction { Pill(account.health.title, style: .attention, small: true).fixedSize() }
                }
                HStack(spacing: 6) {
                    if let rule, rule.participate { RelayRuleChip(rule: rule, open: editing) }
                    Text(account.subtitle.isEmpty ? " " : account.subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                }
            }
        }
    }
}

// A row's numbers: the windows as small bars, or why there are none.
struct CodexRowQuota: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    var wide = false

    var body: some View {
        if account.primaryWindows.isEmpty {
            Text(account.usage?.refreshing == true || service.quotaRequests.contains(account.id) ? "正在查询…" : account.quotaCaption ?? "额度暂不可查")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
        } else {
            ClaudeMiniQuota(windows: Array(account.primaryWindows.prefix(2)), extras: [], notch: directory.notch(for: account), skipped: false, wide: wide)
        }
    }
}

// A row's buttons: how many reset cards, then the one thing to do now. At its line with a card left, the card
// is the way back (amber); otherwise 换上, or what keeps it from being switched to.
struct CodexRowActions: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var reminders = ReminderModel.shared
    let account: ManagedAccount
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void

    var body: some View {
        let locked = service.accountBusy || directory.busy || directory.recoveryNeeded
        let low = directory.notch(for: account).map { line in account.primaryWindows.contains { $0.remaining <= line } } ?? false
        HStack(spacing: 6) {
            // Cards about to expire are amber here as on the card; the full ticket is a click away (展开).
            if let chances = account.chances(dismissed: reminders.dismissed), chances.total > 0 {
                Pill("\(chances.total) 张", style: chances.soon ? .attention : .neutral, small: true).help(chances.help)
            }
            switch directory.primaryAction(for: account) {
            case .capture:
                Button("本地检查") {
                    Task { await service.accountOperation("accounts.check", params: LoginParams(id: account.id, online: false)) }
                }
                .buttonStyle(.pill(.primary)).disabled(locked)
            case .recover:
                Button(account.recoverTitle) { open(.recover(account.id)) }.buttonStyle(.pill(.primary)).disabled(locked)
            case .relogin:
                Button("重新登录") { open(.login(provider: account.provider, accountID: account.id)) }.buttonStyle(.pill(.primary)).disabled(locked)
            case .switchTo where low && account.nextReset != nil:
                AccountActions(account: account, directory: directory, open: open, showSwitch: false, prominent: true)
            case .switchTo:
                Button("换上") { open(.switchTo(account.id)) }
                    .buttonStyle(.pill(.secondary))
                    .disabled(service.accountBusy || directory.switchDisabled(for: account))
            default:
                if account.transferRunning { Text(account.health.title).font(.system(size: 12)).foregroundStyle(Palette.ink2).lineLimit(1) }
            }
        }
    }
}

// MARK: 不参与

// Accounts outside the relay: left out on purpose, new and without a rule yet, or handed over by another Mac.
// Joining takes effect at once.
struct CodexDrawer: View {
    static let openKey = "codex.drawerOpen"
    @EnvironmentObject private var service: ServiceController
    @AppStorage(CodexDrawer.openKey) private var opened = true
    let accounts: [ManagedAccount]
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void

    var body: some View {
        let rules = directory.strategy?.config.accounts.count ?? 0
        VStack(alignment: .leading, spacing: 6) {
            Button { withAnimation(.easeOut(duration: 0.18)) { opened.toggle() } } label: {
                HStack(spacing: 7) {
                    Icon(.tray, size: 13, color: Palette.ink2)
                    Text("不参与自动接力").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
                    Text("\(accounts.count) 个账号 · " + (opened ? "只能手动换上" : accounts.map(\.displayName).joined(separator: "、")))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                    Spacer(minLength: 8)
                    // The service keeps at most 32 rules; near the limit, say how many are left.
                    if rules >= 24 {
                        Text("\(rules) / \(RelayRules.limit) 个账号").font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(rules >= RelayRules.limit ? Palette.attentionText : Palette.ink3)
                            .help(rules >= RelayRules.limit ? "接力最多 \(RelayRules.limit) 个账号；先移除不用的账号，再加入新的" : "接力最多 \(RelayRules.limit) 个账号")
                    }
                    HStack(spacing: 4) {
                        Text(opened ? "收起" : "展开")
                        Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4).rotationEffect(.degrees(opened ? 180 : 0))
                    }
                    .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                }
                .padding(.horizontal, 4).frame(height: 26)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if opened {
                ForEach(accounts) { account in row(account, rules: rules) }
            }
        }
        .padding(8)
        .background(Palette.panel, in: RoundedRectangle(cornerRadius: 15, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 15, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
    }

    private func row(_ account: ManagedAccount, rules: Int) -> some View {
        VStack(spacing: 8) {
            ViewThatFits(in: .horizontal) {
                line(account, rules: rules, wide: true).frame(minWidth: 1080)
                line(account, rules: rules, wide: false)
            }
            if account.transferFailed != nil { TransferBox(account: account) }
        }
        .padding(.horizontal, 10).padding(.vertical, 8)
        .background(Palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(account.cloud?.fresh == true ? Palette.dashStrong : Palette.ring,
                                                                                       style: StrokeStyle(lineWidth: 1, dash: account.cloud?.fresh == true ? [5, 4] : [])))
    }

    private func line(_ account: ManagedAccount, rules: Int, wide: Bool) -> some View {
        let rule = directory.rule(for: account.id)
        let full = rule == nil && rules >= RelayRules.limit
        return HStack(spacing: 9) {
            HStack(spacing: 8) {
                Seal(account.displayName, style: .out, size: 24)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 5) {
                        Text(accountName: account.displayName).font(.system(size: 13, weight: .semibold)).lineLimit(1).layoutPriority(1)
                        CloudBadge(account: account, size: 11)
                        if account.native { Pill("在用", style: .brand, small: true) }
                        if rule == nil { Pill(account.cloud?.fresh == true ? "未加入接力" : "未设规则", style: .dashed, small: true).fixedSize() }
                        if account.needsAction { Pill(account.health.title, style: .attention, small: true).fixedSize() }
                    }
                    Text(account.subtitle.isEmpty ? " " : account.subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                }
            }
            .frame(minWidth: 116, maxWidth: wide ? 300 : .infinity, alignment: .leading)
            CodexRowQuota(account: account, directory: directory, wide: wide).frame(width: wide ? 452 : 298, alignment: .leading)
            if wide { Spacer(minLength: 0) }
            HStack(spacing: 6) {
                Button("加入接力") {
                    service.clearAccountError()
                    Task { await RelayRules.save(directory.relayConfig(account.id, participate: true), service) }
                }
                .buttonStyle(.pill(.link))
                .disabled(service.accountBusy || full || directory.strategy == nil || account.transferRunning)
                .help(full ? "接力最多 \(RelayRules.limit) 个账号；先移除不用的账号" : "加入后排在队列最后")
                CodexRowActions(account: account, directory: directory, open: open)
                AccountMenu(account: account, directory: directory, open: open)
            }
            .fixedSize().frame(minWidth: 186, alignment: .trailing)
        }
    }
}
