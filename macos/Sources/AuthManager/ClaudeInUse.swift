import SwiftUI
import UniformTypeIdentifiers

// The Claude page in three layers (11-22, Gemini 稿 A). On top, who is in use: one slot per client, Claude 桌面's
// chat / work and Code (the desktop's Code tab and the CLI), or one card when both use the same account. Below,
// the accounts the rules relay Code to, in relay order. At the bottom, a drawer of the ones they never switch to.

// MARK: 规则与数据

enum SlotSide: Hashable { case desk, code }

extension ClaudeUse {
    // Code is a slot of its own when this Mac uses it, or when the desktop's Code tab follows it.
    static func sides(_ use: ClaudeUse, _ directory: AccountDirectory) -> (desk: Bool, code: Bool) {
        let relayOn = directory.claudeDesktop?.codeRelay?.enabled == true
        return (use.desk && directory.claudeDesktop != nil, use.code || relayOn)
    }
}

extension AccountQuota {
    var isWeek: Bool { kind == nil && label == "7 天窗口" }
}

extension ManagedAccount {
    // A week used up leaves the 5-hour window nothing to give; only when the week is back counts (11-22).
    var weekSpent: AccountQuota? { primaryWindows.first { $0.isWeek && $0.remaining <= 0 } }
}

extension AccountDirectory {
    // Claude's rules in relay order. The place of each is its 第 N 位.
    var claudeRules: [CodexStrategyConfig.Rule] {
        (strategy?.config.accounts ?? []).filter { rule in accounts.contains { $0.id == rule.id && $0.provider == "claude" } }
    }

    // The saved rules with one account joining or leaving the relay. Joining without a rule switches only:
    // nothing spends a reset card until the person picks an order that does.
    func relayConfig(_ id: String, participate: Bool) -> CodexStrategyConfig? {
        guard var config = strategy?.config else { return nil }
        if let index = config.accounts.firstIndex(where: { $0.id == id }) {
            config.accounts[index].participate = participate
        } else if participate {
            config.accounts.append(.init(id: id, participate: true))
        }
        config.accounts.removeAll { rule in !accounts.contains { $0.id == rule.id } }
        return config
    }

}

// MARK: 在用

// 在用. While Code is in use it is the Code slot with, beside it, the account the rules would switch Code to now;
// Claude 桌面's chat / work stays on one account and sits in the bar above the page (11-25). Only Claude 桌面
// in use (without 无感换号): its slot alone, as before.
struct ClaudeStage: View {
    @EnvironmentObject private var service: ServiceController
    let entries: [ClaudeEntry]
    let use: ClaudeUse
    let directory: AccountDirectory
    let present: (ClaudeSheet) -> Void
    let switchTo: (ClaudeEntry, SwitchScope?) -> Void

    var body: some View {
        let sides = ClaudeUse.sides(use, directory)
        VStack(alignment: .leading, spacing: 8) {
            Text("在用").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink).padding(.horizontal, 3.5)
            if sides.code {
                let wing = self.wing
                // One card: the Code slot, and the wing it closes on the right.
                HStack(alignment: .top, spacing: -1) {
                    slot(entries.first { $0.inCode }, [.code], joined: wing != nil).frame(maxWidth: .infinity)
                    if let wing { wingView(wing).frame(width: RelayNextCard.width) }
                }
                .fixedSize(horizontal: false, vertical: true)
                .joinedShadow(wing != nil, radius: 17)
            } else {
                slot(entries.first { $0.inDesk }, [.desk], joined: false)
            }
        }
    }

    private func slot(_ entry: ClaudeEntry?, _ sides: [SlotSide], joined: Bool) -> some View {
        ClaudeSlotCard(entry: entry, sides: sides, use: use, directory: directory, entries: entries,
                       present: present, switchTo: switchTo, joined: joined)
    }

    // The next account while the rules are on and Code's account takes part and switches; when none in the
    // queue can take over, that, with the best account outside it.
    private var wing: RelayWingKind? {
        guard let code = entries.first(where: { $0.inCode })?.cli, let rule = directory.rule(for: code.id),
              directory.strategy?.config.enabled == true, rule.participate, rule.order.switches else { return nil }
        if let id = directory.relayNext("claude"), let account = directory.accounts.first(where: { $0.id == id }) {
            return .next(account, rank: (directory.claudeRules.firstIndex { $0.id == id } ?? 0) + 1)
        }
        return directory.claudeRules.contains(where: { $0.participate && $0.id != code.id }) ? .empty : nil
    }

    @ViewBuilder private func wingView(_ wing: RelayWingKind) -> some View {
        switch wing {
        case .next(let account, let rank):
            RelayNextCard(account: account, rank: rank, directory: directory, radius: 17) {
                Task { await service.accountOperation("accounts.activate", params: LoginParams(id: account.id, confirm: true)) }
            }
        case .empty:
            RelayNoneCard(directory: directory, provider: "claude", spare: spare?.cli, radius: 17) { _ in
                if let spare { switchTo(spare, .code) }
            }
        }
    }

    // Outside the relay, switchable, and with the most left of what it has least of.
    private var spare: ClaudeEntry? {
        entries.filter { candidate in
            guard let account = candidate.cli, !candidate.inCode, directory.rule(for: account.id)?.participate != true,
                  directory.primaryAction(for: account) == .switchTo, account.weekSpent == nil else { return false }
            return (account.lowestPrimary ?? 0) > 0
        }
        .max { ($0.cli?.lowestPrimary ?? 0) < ($1.cli?.lowestPrimary ?? 0) }
    }
}

// What a slot's head says: the client's icons, as Claude 桌面 draws them, and its name.
struct ClaudeChannel: View {
    let side: SlotSide
    let relayOn: Bool

    var body: some View {
        // Without 无感换号 the desktop's Code tab stays with the desktop's sign-in, so its </> sits there.
        let glyphs: [Glyph] = side == .desk ? (relayOn ? [.chat] : [.chat, .code]) : (relayOn ? [.code, .terminal] : [.terminal])
        let title = side == .desk ? (relayOn ? "Claude Chat" : "Claude 桌面") : (relayOn ? "Code · 桌面与 CLI" : "Claude Code")
        HStack(spacing: 5) {
            ForEach(Array(glyphs.enumerated()), id: \.offset) { _, glyph in
                Icon(glyph, size: 12, color: side == .code ? Palette.deep : Palette.ink2, weight: 1.9)
                    .frame(width: 21, height: 21)
                    .background(side == .code ? Palette.wash : Palette.chip, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            }
            Text(title).font(.system(size: 12, weight: .semibold)).foregroundStyle(side == .code ? Palette.deep : Palette.ink)
                .lineLimit(1).fixedSize()
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
    }
}

struct ClaudeSlotCard: View {
    @EnvironmentObject private var service: ServiceController
    let entry: ClaudeEntry?
    let sides: [SlotSide]
    let use: ClaudeUse
    let directory: AccountDirectory
    let entries: [ClaudeEntry]
    let present: (ClaudeSheet) -> Void
    let switchTo: (ClaudeEntry, SwitchScope?) -> Void
    // The relay's wing closes this card on the right.
    var joined = false

    private var status: ClaudeDesktopStatus? { directory.claudeDesktop }
    private var relayOn: Bool { status?.codeRelay?.enabled == true }
    private var deskLocked: Bool { service.accountBusy || status?.busy == true || status?.installed != true }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            head
            if let entry {
                // The relay's pointer is about Code: on a page without it, the desktop's account is nobody's next.
                ClaudeEntryCard(entry: entry, use: use, directory: directory, open: present, switchTo: { switchTo(entry, $0) },
                                chrome: false, next: ClaudeUse.sides(use, directory).code && entry.cli != nil
                                    && directory.strategy?.config.enabled == true && directory.strategy?.nextClaude == entry.cli?.id)
            } else {
                placeholder
            }
            Spacer(minLength: 0)
            if sides.contains(.desk), let status {
                DesktopNotices(status: status, locked: deskLocked, compact: true) { present(.desktop($0)) }
                // Without Claude 桌面 there is nothing to sync; the notice above says so.
                if status.installed { SyncStrip(status: status, locked: deskLocked, embedded: true) { present(.desktop($0)) } }
                if status.installed, let chrome = status.chromeSync {
                    ChromeSyncStrip(chrome: chrome, locked: deskLocked || chrome.busy, embedded: true) { present($0) }
                }
            }
            // With Claude 桌面 in use its bar above holds this switch; with only Code, the Code slot does (11-22).
            if sides.contains(.code), !ClaudeUse.sides(use, directory).desk, let status, status.installed, let relay = status.codeRelay {
                CodeRelayStrip(relay: relay, locked: deskLocked || relay.busy, embedded: true) { present($0) }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .loopCard(radius: 17, joined: joined, raised: true)
    }

    private var head: some View {
        HStack(spacing: 8) {
            ForEach(sides, id: \.self) { side in ClaudeChannel(side: side, relayOn: relayOn) }
            if sides.count > 1 { Pill("当前同号", style: .brand, small: true) }
            Spacer(minLength: 6)
            if sides.contains(.desk), let status { ClaudeDeskNext(status: status, entries: entries, locked: deskLocked || !status.switchSupported, present: present) }
        }
        .padding(.horizontal, 14).frame(height: 42)
        .background(sides == [.code] ? Palette.wash.opacity(0.55) : Palette.panel.opacity(0.7))
        .overlay(alignment: .bottom) { Rectangle().fill(Palette.divider).frame(height: 1) }
    }

    private var placeholder: some View {
        let text: String = sides.contains(.desk)
            ? (status?.installed == false ? "没找到 Claude 桌面" : "桌面还没认出在用的账号")
            : "还没有在用的 Code 账号"
        return Text(text).font(.system(size: 12)).foregroundStyle(Palette.ink3)
            .frame(maxWidth: .infinity, minHeight: 96, alignment: .center)
    }
}

// Round the desktop's numbered list, as the menu bar's 下一个 does; the menu offers the other two ways.
struct ClaudeDeskNext: View {
    let status: ClaudeDesktopStatus
    let entries: [ClaudeEntry]
    let locked: Bool
    let present: (ClaudeSheet) -> Void

    var body: some View {
        let picks = status.picks
        HStack(spacing: 0) {
            Button { ask(picks?.next) } label: { Text("下一个桌面号…").padding(.leading, 11).padding(.trailing, 8).frame(height: 24).contentShape(Rectangle()) }
                .buttonStyle(.plain)
                .help("Claude 桌面按编号换到下一个已保存登录、参与「下一个」的账号")
                .disabled(picks?.next == nil || locked)
            Rectangle().fill(Palette.edge).frame(width: 1, height: 24)
            Menu {
                Button("下一个还有额度的") { ask(picks?.nextAvailable) }.disabled(picks?.nextAvailable == nil)
                Button("剩余最多的") { ask(picks?.best) }.disabled(picks?.best == nil)
            } label: {
                Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4).frame(width: 22, height: 24).contentShape(Rectangle())
            }
            .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
            .accessibilityLabel("换号方式")
            .disabled(locked)
        }
        .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink)
        .background(Capsule().fill(Palette.card))
        .overlay(Capsule().strokeBorder(Palette.edge, lineWidth: 1))
        .fixedSize()
    }

    private func ask(_ handle: String?) {
        guard let handle, let entry = ClaudeEntry.find(handle, in: entries) else { return }
        present(.switching(ClaudeSwitchRequest(entryID: entry.id, code: false, desk: true, choice: false)))
    }
}

// MARK: 接力队列

struct ClaudeRelayDeck: View {
    static let expandedKey = "claude.deckExpanded"
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @AppStorage(ClaudeRelayDeck.expandedKey) private var expanded = false
    @State private var draft: [String]?
    @State private var dragging: String?
    @State private var openRow: String?
    // The row whose rule line is open.
    @State private var editing: String? = ClaudeRelayDeck.previewEditing
    // Preview renders only: a row's rule line drawn open.
    @MainActor static var previewEditing: String?
    let entries: [ClaudeEntry]
    let use: ClaudeUse
    let directory: AccountDirectory
    let present: (ClaudeSheet) -> Void
    let switchTo: (ClaudeEntry, SwitchScope?) -> Void

    private var saved: [String] { directory.claudeRules.map(\.id) }
    private var order: [String] { draft ?? saved }
    private var next: String? { directory.relayNext("claude") }

    // The accounts that take part, in the order being shown (a drag in progress included).
    private var queue: [ClaudeEntry] {
        order.compactMap { id in
            directory.rule(for: id)?.participate == true ? entries.first { $0.cli?.id == id } : nil
        }
    }

    private var movable: Bool {
        queue.count > 1 && !service.accountBusy && directory.strategy?.busy != true && directory.strategy?.run == nil && directory.strategy != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            RelayQueueHeader(directory: directory, count: queue.count, expanded: queue.isEmpty ? nil : $expanded) { present(.account($0)) }
            if queue.isEmpty {
                Text("还没有账号参与自动接力").font(.system(size: 12)).foregroundStyle(Palette.ink2)
                    .padding(.horizontal, 14).frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.dash, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
            } else if expanded {
                VStack(spacing: 6) {
                    ForEach(queue.filter { anchor($0) != nil }) { row($0) }
                }
                CardGrid {
                    ForEach(queue.filter { anchor($0) == nil }) { entry in
                        ClaudeEntryCard(entry: entry, use: use, directory: directory, open: present, switchTo: { switchTo(entry, $0) },
                                        next: entry.cli?.id == next)
                            .frame(maxHeight: .infinity, alignment: .top)
                    }
                }
            } else {
                VStack(spacing: 6) {
                    ForEach(queue) { row($0) }
                }
            }
        }
        .onChange(of: saved) { _, _ in draft = nil }
        .modifier(DragEnd(dragging: $dragging, ended: commit))
        .onAppear { openRequestedRule() }
        .onChange(of: nav.ruleEditor) { _, _ in openRequestedRule() }
    }

    // Another page asked for an account's rule line.
    private func openRequestedRule() {
        guard let id = nav.ruleEditor, saved.contains(id) else { return }
        nav.ruleEditor = nil
        editing = id
    }

    // Which slot above shows this account; nil when it waits in the queue.
    private func anchor(_ entry: ClaudeEntry) -> String? {
        let sides = ClaudeUse.sides(use, directory)
        let code = sides.code && entry.inCode
        let desk = sides.desk && entry.inDesk
        // With 无感换号 on, the desktop's login is only Chat / Cowork's (11-26).
        let chat = directory.claudeDesktop?.codeRelay?.enabled == true
        return code && desk ? (chat ? "Chat + Code 在用" : "桌面 + Code 在用") : desk ? (chat ? "Chat 在用" : "桌面在用") : code ? "Code 在用" : nil
    }

    @ViewBuilder private func row(_ entry: ClaudeEntry) -> some View {
        let id = entry.cli?.id ?? entry.id
        let rank = order.firstIndex(of: id).map { $0 + 1 }
        let rule = directory.rule(for: id)
        let open = Binding(get: { editing == id }, set: { editing = $0 ? id : nil })
        Group {
            if let anchor = anchor(entry) {
                ClaudeAnchorRow(entry: entry, place: rank.map { "第 \($0) 位" } ?? "–", slot: anchor, next: id == next, movable: movable,
                                rule: rule, editing: open, directory: directory)
            } else if openRow == id {
                ClaudeEntryCard(entry: entry, use: use, directory: directory, open: present, switchTo: { switchTo(entry, $0) },
                                next: id == next, collapse: { withAnimation(.easeOut(duration: 0.18)) { openRow = nil } })
            } else {
                ClaudeRelayRow(entry: entry, kind: .queue(rank: rank, movable: movable), use: use, directory: directory,
                               next: id == next, present: present, switchTo: { switchTo(entry, $0) },
                               expand: { withAnimation(.easeOut(duration: 0.18)) { openRow = id } }, rule: rule, editing: open)
            }
        }
        .opacity(dragging == id ? 0.4 : 1)
        .modifier(RowDrag(id: id, enabled: movable, dragging: $dragging))
        .onDrop(of: [UTType.text], delegate: RowDrop(target: id, dragging: $dragging, move: { from, to in
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
        guard let draft, draft != saved, let config = directory.relayConfig("claude", order: draft) else { self.draft = nil; return }
        let before = saved
        let name = entries.first { $0.cli?.id == id }?.name ?? "账号"
        RelayUndo.shared.offer("\(name) 已移到第 \((draft.firstIndex(of: id) ?? 0) + 1) 位", provider: "claude", order: before)
        service.clearAccountError()
        Task {
            await RelayRules.save(config, service)
            if service.accountError != nil { self.draft = nil }
        }
    }
}

// Dragging over a row moves the dragged one into its place; the drop only ends the drag.
struct RowDrop: DropDelegate {
    let target: String
    @Binding var dragging: String?
    let move: (String, String) -> Void
    // The drop ends the drag; the order it left takes effect.
    var dropped: (String) -> Void = { _ in }

    func validateDrop(info: DropInfo) -> Bool { dragging != nil }
    func dropEntered(info: DropInfo) {
        guard let dragging, dragging != target else { return }
        move(dragging, target)
    }
    func dropUpdated(info: DropInfo) -> DropProposal? { DropProposal(operation: .move) }
    func performDrop(info: DropInfo) -> Bool {
        if let dragging { dropped(dragging) }
        dragging = nil
        return true
    }
}

// A whole row picks up for dragging.
struct RowDrag: ViewModifier {
    let id: String
    let enabled: Bool
    @Binding var dragging: String?

    func body(content: Content) -> some View {
        if enabled {
            content.onDrag {
                dragging = id
                return NSItemProvider(object: id as NSString)
            }
        } else {
            content
        }
    }
}

// Six dots: this row can be picked up and moved.
struct DragGrip: View {
    var active = true

    var body: some View {
        VStack(spacing: 2.4) {
            ForEach(0..<3, id: \.self) { _ in
                HStack(spacing: 2.4) {
                    Circle().frame(width: 2.6, height: 2.6)
                    Circle().frame(width: 2.6, height: 2.6)
                }
            }
        }
        .foregroundStyle(active ? Palette.ink3 : Palette.dash)
        .frame(width: 12, height: 16)
        .accessibilityHidden(true)
    }
}

// 第 N 位 in the queue's first column; where the relay's pointer stops it carries the pointer.
struct RankMark: View {
    let place: String
    let next: Bool

    var body: some View {
        Group {
            if next { PointerRank(text: place) } else { Pill(place, small: true) }
        }
        .frame(width: 56, alignment: .leading)
    }
}

// An account shown in a slot above keeps its place in the queue as a thin line, so the order reads 1 to N.
struct ClaudeAnchorRow: View {
    let entry: ClaudeEntry
    // 第 N 位 in the relay, #N in the desktop's list.
    let place: String
    let slot: String
    let next: Bool
    let movable: Bool
    // In the relay queue: its rule, and whether its line to change it is open.
    var rule: CodexStrategyConfig.Rule? = nil
    var editing: Binding<Bool>? = nil
    var directory: AccountDirectory? = nil

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                DragGrip(active: movable)
                RankMark(place: place, next: next)
                Seal(entry.name, style: entry.cli?.external != nil ? .external : .inUse, size: 18)
                Text(accountName: entry.name).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink).lineLimit(1)
                if let cli = entry.cli { CloudBadge(account: cli, size: 11) }
                Pill(slot, style: .brand, small: true)
                Spacer(minLength: 8)
                if let rule, let editing, rule.participate { RelayRuleChip(rule: rule, open: editing) }
                HStack(spacing: 3) {
                    Text("↑")
                    Text("在上方")
                }
                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            .padding(.leading, 8).padding(.trailing, 12).frame(height: 34)
            if let rule, let directory, editing?.wrappedValue == true { RelayRuleEditor(rule: rule, directory: directory) }
        }
        .background(next ? Palette.wash.opacity(0.5) : Color.clear, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
            .strokeBorder(next ? Palette.brand.opacity(0.35) : Palette.dash, style: StrokeStyle(lineWidth: 1, dash: next ? [] : [4, 3])))
        .relayLinked(next ? entry.cli?.id : nil)
        .contentShape(Rectangle())
    }
}

// MARK: 紧凑行

struct ClaudeRelayRow: View {
    enum Kind {
        case queue(rank: Int?, movable: Bool)   // 自动接力队列: a place and a handle
        case rotation(movable: Bool)            // 只用桌面: the desktop's numbered list
        case drawer                             // outside the relay
    }

    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    let entry: ClaudeEntry
    let kind: Kind
    let use: ClaudeUse
    let directory: AccountDirectory
    var tag: String? = nil
    // The relay's pointer stops here: Code moves to this account once its own reaches the line.
    var next = false
    let present: (ClaudeSheet) -> Void
    let switchTo: (SwitchScope?) -> Void
    var expand: (() -> Void)? = nil
    var join: (title: String, action: () -> Void)? = nil
    // In the relay queue: its rule on the second line, and whether its line to change it is open.
    var rule: CodexStrategyConfig.Rule? = nil
    var editing: Binding<Bool>? = nil

    private var cli: ManagedAccount? { entry.cli }
    private var notch: Int? { cli.flatMap { directory.notch(for: $0) } }
    private var spent: Bool { cli?.weekSpent != nil }

    var body: some View {
        // Two fixed layouts, so every row's numbers line up: a wide window (rows over 1080) gives the name 300
        // and the bars 452 and leaves the rest before the buttons; otherwise the name takes what is left.
        VStack(spacing: 0) {
            ViewThatFits(in: .horizontal) {
                line(wide: true).frame(minWidth: 1080)
                line(wide: false)
            }
            .padding(.leading, 9).padding(.trailing, 9).padding(.vertical, 8)
            if let rule, editing?.wrappedValue == true { RelayRuleEditor(rule: rule, directory: directory) }
            // Its source does not answer: the relay passes it by, its rule stays.
            if let external = cli?.external, external.trouble, case .queue = kind {
                ExternalSkipNote(external: external, source: cli.flatMap(directory.source(of:)))
            }
        }
        .background(spent ? Palette.attentionWash.opacity(0.6) : Palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
            .strokeBorder(spent ? Palette.attentionDash.opacity(0.55) : tag != nil || next ? Palette.brand.opacity(0.35) : Palette.ring, lineWidth: 1))
        .relayLinked(next ? cli?.id : nil)
    }

    private func line(wide: Bool) -> some View {
        HStack(spacing: 9) {
            switch kind {
            case .queue(let rank, let movable):
                DragGrip(active: movable)
                RankMark(place: rank.map { "第 \($0) 位" } ?? "–", next: next)
            case .rotation(let movable):
                DragGrip(active: movable)
                if let desk = entry.desk { DeskNumber(desk: desk).frame(width: 30, alignment: .leading) }
            case .drawer:
                EmptyView()
            }
            if wide {
                identity.frame(width: 300, alignment: .leading)
                telemetry(wide: true).frame(width: 452, alignment: .leading)
                Spacer(minLength: 0)
            } else {
                identity.frame(minWidth: 116, maxWidth: .infinity, alignment: .leading)
                telemetry(wide: false).frame(width: 298, alignment: .leading)
            }
            // Most rows' buttons fit 186, so their numbers line up; a longer set just takes more.
            actions.fixedSize().frame(minWidth: 186, alignment: .trailing)
        }
    }

    private var identity: some View {
        // An external source's account has no detail page here: its source looks after it.
        Button { if cli?.external == nil { nav.claudeDetail = entry.id } } label: {
            HStack(spacing: 8) {
                ClaudeSeal(entry: entry, size: 24, rule: cli.flatMap { directory.rule(for: $0.id) })
                VStack(alignment: .leading, spacing: 1) {
                    // A long name leaves no room for 「Code 无感可用」: drop it rather than squeeze it to a sliver.
                    ViewThatFits(in: .horizontal) {
                        title(relayPill: true)
                        title(relayPill: false)
                    }
                    if let rule, let editing, rule.participate {
                        if let cli, cli.external != nil {
                            // An external source's account: its source beside the rule, which opens its mode; a narrow row
                            // keeps the rule and the source's link.
                            ViewThatFits(in: .horizontal) {
                                HStack(spacing: 6) {
                                    RelayRuleChip(rule: rule, open: editing)
                                    ExternalModeButton(account: cli, directory: directory, asTag: true)
                                }
                                HStack(spacing: 6) {
                                    RelayRuleChip(rule: rule, open: editing)
                                    ExternalModeButton(account: cli, directory: directory, asTag: true, iconOnly: true)
                                }
                            }
                        } else {
                            HStack(spacing: 6) {
                                RelayRuleChip(rule: rule, open: editing)
                                Text(meta).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                            }
                        }
                    } else {
                        Text(meta).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                    }
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(cli?.external.map { "\(entry.name) · 由 \($0.name) 登录和续期" } ?? "\(entry.name) · 点开看账号详情")
    }

    private func title(relayPill: Bool) -> some View {
        HStack(spacing: 5) {
            Text(accountName: entry.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(Palette.ink).lineLimit(1)
                .layoutPriority(1)
            if let cli { CloudBadge(account: cli, size: 11) }
            if case .rotation = kind {} else if use.desk, let desk = entry.desk, desk.number != nil { DeskNumber(desk: desk).fixedSize() }
            if let tag { Pill(tag, style: .brand, small: true).fixedSize() }
            // Signed in to AuthBaton only, while 无感换号 lends Code logins: the desktop's Code tab can use it.
            if relayPill, use.desk, entry.desk == nil, cli != nil, cli?.external == nil, directory.claudeDesktop?.codeRelay?.state == "active" {
                Pill("Code 无感可用", style: .brand, small: true)
                    .help(entry.inCode ? "已开启 Code 无感换号：桌面 Code 对话已在用这个账号，不用在桌面重复登录"
                          : "已开启 Code 无感换号：同 Team 账号不用在桌面重复登录，点「换到 Code」就能在桌面 Code 对话里直接切换")
            }
            if let cli {
                if directory.primaryAction(for: cli) == .relogin {
                    Pill(cli.health.title, style: .attention, small: true).fixedSize()
                } else {
                    LowPill(account: cli, notch: notch)
                }
            } else {
                Pill("仅桌面", small: true).fixedSize()
            }
        }
    }

    private var meta: String {
        if let cli { return cli.subtitle.isEmpty ? " " : cli.subtitle }
        return entry.desk?.email ?? "只在 Claude 桌面登录过"
    }

    @ViewBuilder private func telemetry(wide: Bool) -> some View {
        if let cli {
            if directory.primaryAction(for: cli) == .relogin {
                Text(cli.health.detail ?? "登录失效了，自动接力会跳过它").font(.system(size: 11)).foregroundStyle(Palette.attentionText).lineLimit(2)
            } else if cli.primaryWindows.isEmpty {
                quiet(cli.usage?.refreshing == true || service.quotaRequests.contains(cli.id) ? "正在查询…" : cli.quotaCaption ?? "额度暂不可查")
            } else {
                ClaudeMiniQuota(windows: Array(cli.primaryWindows.prefix(2)), extras: cli.extraWindows, notch: notch,
                                skipped: directory.rule(for: cli.id)?.participate == true && directory.strategy?.config.enabled == true, wide: wide)
                    // Its source does not answer: the last numbers, faded.
                    .opacity(cli.external?.trouble == true ? 0.48 : 1)
            }
        } else if let quota = entry.desk?.tokenQuota, !quota.primary.isEmpty {
            ClaudeMiniQuota(windows: quota.primary, extras: [], notch: nil, skipped: false, wide: wide)
        } else if directory.claudeDesktop?.tokenQuota?.enabled == true {
            VStack(alignment: .leading, spacing: 1) {
                quiet(entry.desk?.tokenQuota?.message ?? "还没读到额度")
                if entry.desk?.tokenQuota?.state == "no_token" {
                    // A new account has no Claude Code login in Claude 桌面 until its first Code-mode conversation.
                    Text("去 Claude 桌面用 Code 模式聊一次，5 分钟内自动刷新").font(.system(size: 10.5)).foregroundStyle(Palette.ink3).lineLimit(1)
                }
            }
        } else {
            HStack(spacing: 6) {
                quiet("读不到额度")
                Button("用桌面登录查") { present(.tokenQuota) }.buttonStyle(.pill(.link))
            }
        }
    }

    private func quiet(_ text: String) -> some View {
        Text(text).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
    }

    private var actions: some View {
        HStack(spacing: 6) {
            if let join { Button(join.title, action: join.action).buttonStyle(.pill(.link)).disabled(service.accountBusy) }
            if let count = cli?.creditCount, count > 0 { Pill("\(count) 张", small: true).help("剩 \(count) 张重置卡") }
            primary
            if let expand, cli != nil {
                Button(action: expand) {
                    Icon(.chevronRight, size: 11, color: Palette.ink3, weight: 2).frame(width: 22, height: 24).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("在这里展开")
                .accessibilityLabel("展开 \(entry.name)")
            }
        }
    }

    @ViewBuilder private var primary: some View {
        if let cli {
            let low = notch.map { line in cli.primaryWindows.prefix(2).contains { $0.remaining <= line } } ?? false
            if cli.external?.trouble == true {
                // Its source does not answer: nothing to switch to until it does.
                Button("换到 Code") {}.buttonStyle(.pill(.secondary)).disabled(true)
                    .help("连不上 \(cli.external?.name ?? "来源")，现在换不过去")
            } else if directory.primaryAction(for: cli) == .relogin {
                Button("重新登录") { present(.account(.login(provider: "claude", accountID: cli.id))) }
                    .buttonStyle(.pill(.onAttention))
                    .disabled(service.accountBusy)
            } else if (spent || low), let next = cli.nextReset {
                Button("用 1 张重置卡") {
                    present(.account(.reset(accountID: cli.id, creditID: next.creditID, requestID: UUID().uuidString, expiresAt: next.expiresAt, retry: false)))
                }
                .buttonStyle(.pill(.attention))
                .disabled(directory.resetBlocked(cli, busy: service.accountBusy))
            } else {
                ClaudeSwitchControl(entry: entry, use: use, directory: directory, switchTo: switchTo,
                                    login: { present(.desktop(.add)) })
            }
        } else {
            Button("补登 Code") { present(.account(.login(provider: "claude", accountID: nil))) }
                .buttonStyle(.pill(.soft))
                .help("用这个账号登录 AuthBaton，之后就能参与 Code 换号、用重置卡和预热")
            ClaudeSwitchControl(entry: entry, use: use, directory: directory, switchTo: switchTo,
                                login: { present(.desktop(.add)) })
        }
    }
}

// A row's windows as small numbers on thin bars. A used-up week replaces them all with when it is back.
struct ClaudeMiniQuota: View {
    let windows: [AccountQuota]
    let extras: [AccountQuota]
    let notch: Int?
    // The rules skip this account while its week is gone.
    let skipped: Bool
    // 452 wide instead of 298.
    var wide = false
    // An external source's account outside the relay: nothing about it is amber, a used-up week included (11-27).
    var plain = false

    var body: some View {
        if !plain, let week = windows.first(where: { $0.isWeek && $0.remaining <= 0 }) {
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    if let due = RefillDue(week) {
                        Text(due.label).font(.system(size: 15, weight: .semibold, design: .rounded)).monospacedDigit().foregroundStyle(Palette.ink)
                        Text(due.caption).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.attentionText)
                    } else {
                        Text(week.refill()).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                    Spacer(minLength: 6)
                    Text("7 天剩 0%" + (notch.map { " · 见底线 \($0)%" } ?? "") + (skipped ? " · 自动跳过" : ""))
                        .font(.system(size: 10.5)).foregroundStyle(Palette.attentionText).lineLimit(1)
                }
                MiniBar(remaining: 0, amber: true)
            }
        } else {
            // Top-aligned: the windows have a third line, Fable has not, and the bars line up.
            HStack(alignment: .top, spacing: 10) {
                ForEach(Array(windows.enumerated()), id: \.offset) { _, window in
                    mini(window).frame(width: window.isWeek ? (wide ? 220 : 140) : (wide ? 124 : 80), alignment: .leading)
                }
                if let fable = extras.first(where: \.isFable) {
                    VStack(alignment: .leading, spacing: 4) {
                        HStack(alignment: .firstTextBaseline, spacing: 3) {
                            Text("Fable").font(.system(size: 10)).foregroundStyle(Palette.ink3).fixedSize()
                            Spacer(minLength: 2)
                            Text("\(fable.remaining)%").font(.system(size: 11, weight: .semibold)).monospacedDigit().fixedSize()
                                .foregroundStyle(low(fable) ? Palette.attentionText : Palette.ink2)
                        }
                        MiniBar(remaining: fable.remaining, amber: low(fable))
                    }
                    .frame(width: wide ? 88 : 58)
                    .help("Fable 周额度" + (low(fable) ? "：到了见底线，用 Fable 时会换号" : ""))
                }
            }
        }
    }

    private func low(_ window: AccountQuota) -> Bool { notch.map { window.remaining <= $0 } ?? false }

    private func mini(_ window: AccountQuota) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 4) {
                if low(window), let due = RefillDue(window) {
                    // A narrow column keeps the time and lets the amber bar say the rest.
                    ViewThatFits(in: .horizontal) {
                        HStack(alignment: .firstTextBaseline, spacing: 4) {
                            dueLabel(due)
                            Text(due.caption).font(.system(size: 10.5, weight: .semibold)).foregroundStyle(Palette.attentionText)
                        }
                        dueLabel(due)
                    }
                } else {
                    Text("\(window.remaining)%").font(.system(size: 13, weight: .semibold, design: .rounded)).monospacedDigit().foregroundStyle(Palette.ink)
                    Spacer(minLength: 3)
                    Text(window.shortLabel.trimmingCharacters(in: .whitespaces))
                        .font(.system(size: 10.5)).foregroundStyle(Palette.ink3).lineLimit(1)
                }
            }
            MiniBar(remaining: window.remaining, amber: low(window))
            // When it is back sits under the bar (11-26, Gemini 稿): beside the label there is no room for a day and a time.
            Text(refillLine(window)).font(.system(size: 10.5)).monospacedDigit().foregroundStyle(Palette.ink3).lineLimit(1)
        }
        .help(window.refillHelp())
    }

    private func dueLabel(_ due: RefillDue) -> some View {
        Text(due.label).font(.system(size: 13, weight: .semibold, design: .rounded)).monospacedDigit().foregroundStyle(Palette.ink).lineLimit(1)
    }

    // The 7-day window always says when it is back, the 5-hour one once it is used; a low one says it above the bar.
    // A blank line keeps the bars level.
    private func refillLine(_ window: AccountQuota) -> String {
        guard !low(window), window.resetDate != nil, window.isWeek || window.remaining < 100 else { return " " }
        return window.refill()
    }
}

struct MiniBar: View {
    let remaining: Int
    var amber = false

    var body: some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule().fill(amber ? Palette.attentionFill : Palette.chip)
                Capsule().fill(amber ? Palette.attention : Palette.brand)
                    .frame(width: max(0, min(1, CGFloat(remaining) / 100)) * proxy.size.width)
            }
        }
        .frame(height: 4)
        .accessibilityHidden(true)
    }
}

// MARK: 不参与

// The accounts the rules never switch Code to: their own choice, or no Code login yet (only signed in to
// Claude 桌面). They are still a click away; collapsed, the drawer is one line.
struct ClaudeDrawer: View {
    static let openKey = "claude.drawerOpen"
    @EnvironmentObject private var service: ServiceController
    @AppStorage(ClaudeDrawer.openKey) private var opened = true
    let title: String
    let entries: [ClaudeEntry]
    let use: ClaudeUse
    let directory: AccountDirectory
    let present: (ClaudeSheet) -> Void
    let switchTo: (ClaudeEntry, SwitchScope?) -> Void
    // What joins an account: 加入接力 on Code pages, 参与「下一个」 on the desktop-only page.
    let join: (ClaudeEntry) -> (title: String, action: () -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button { withAnimation(.easeOut(duration: 0.18)) { opened.toggle() } } label: {
                HStack(spacing: 7) {
                    Icon(.tray, size: 13, color: Palette.ink2)
                    Text(title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
                    Text("\(entries.count) 个账号 · " + (opened ? "只能手动换上" : entries.map(\.name).joined(separator: "、")))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                    Spacer(minLength: 8)
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
                ForEach(entries) { entry in
                    ClaudeRelayRow(entry: entry, kind: .drawer, use: use, directory: directory, present: present,
                                   switchTo: { switchTo(entry, $0) }, join: join(entry))
                }
            }
        }
        .padding(8)
        .background(Palette.panel, in: RoundedRectangle(cornerRadius: 15, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 15, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
    }
}

// MARK: 只用桌面

// Claude 桌面 never switches by itself: in place of the relay, its numbered list for 下一个账号, which a drag
// reorders on the spot as 移到第几位 does.
struct ClaudeDeskRotation: View {
    @EnvironmentObject private var service: ServiceController
    @State private var dragging: String?
    @State private var draft: [String]?
    let entries: [ClaudeEntry]
    let use: ClaudeUse
    let directory: AccountDirectory
    let present: (ClaudeSheet) -> Void
    let switchTo: (ClaudeEntry, SwitchScope?) -> Void

    private var listed: [ClaudeEntry] {
        entries.filter { $0.desk?.participates == true && $0.desk?.known == true }
            .sorted { ($0.desk?.number ?? .max) < ($1.desk?.number ?? .max) }
    }
    private var shown: [ClaudeEntry] {
        guard let draft else { return listed }
        return draft.compactMap { handle in listed.first { $0.desk?.handle == handle } }
    }
    private var movable: Bool { listed.count > 1 && !service.accountBusy && directory.claudeDesktop?.busy != true }

    var body: some View {
        let status = directory.claudeDesktop
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 7) {
                Icon(.chat, size: 13, color: Palette.ink2)
                Text("桌面「下一个」轮换").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
                Text("\(listed.count) 个参与 · 点「下一个桌面号」时按编号轮换").font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 3.5)
            VStack(spacing: 6) {
                ForEach(shown) { entry in
                    let handle = entry.desk?.handle ?? entry.id
                    Group {
                        if entry.inDesk {
                            ClaudeAnchorRow(entry: entry, place: entry.desk?.number.map { "#\($0)" } ?? "–", slot: "桌面在用", next: false, movable: movable)
                        } else {
                            ClaudeRelayRow(entry: entry, kind: .rotation(movable: movable), use: use, directory: directory,
                                           tag: status?.picks?.next == handle ? "下一个" : nil, present: present,
                                           switchTo: { switchTo(entry, $0) })
                        }
                    }
                    .opacity(dragging == handle ? 0.4 : 1)
                    .modifier(RowDrag(id: handle, enabled: movable, dragging: $dragging))
                    .onDrop(of: [UTType.text], delegate: RotationDrop(target: handle, dragging: $dragging, move: { from, to in
                        withAnimation(.easeOut(duration: 0.18)) { place(from, at: to) }
                    }, commit: commit))
                }
            }
        }
        .onChange(of: listed.map(\.id)) { _, _ in draft = nil }
        // Let go off the rows: the order on screen is kept, as a drop would keep it.
        .modifier(DragEnd(dragging: $dragging, ended: commit))
    }

    private func place(_ handle: String, at target: String) {
        var handles = shown.compactMap { $0.desk?.handle }
        guard handle != target, let from = handles.firstIndex(of: handle), let to = handles.firstIndex(of: target) else { return }
        handles.remove(at: from)
        handles.insert(handle, at: to)
        draft = handles
    }

    // Dropped: the desktop's number of the place it landed on, kept by the service as 移到第几位 keeps it.
    private func commit(_ handle: String) {
        guard let draft, let offset = draft.firstIndex(of: handle), let number = listed[offset].desk?.number,
              listed[offset].desk?.handle != handle else { self.draft = nil; return }
        Task { await service.claudeDesktop("move", account: handle, number: number) }
    }
}

struct RotationDrop: DropDelegate {
    let target: String
    @Binding var dragging: String?
    let move: (String, String) -> Void
    let commit: (String) -> Void

    func validateDrop(info: DropInfo) -> Bool { dragging != nil }
    func dropEntered(info: DropInfo) {
        guard let dragging, dragging != target else { return }
        move(dragging, target)
    }
    func dropUpdated(info: DropInfo) -> DropProposal? { DropProposal(operation: .move) }
    func performDrop(info: DropInfo) -> Bool {
        if let dragging { commit(dragging) }
        dragging = nil
        return true
    }
}

// MARK: 只有一个账号

struct ClaudeSecondAccount: View {
    let add: () -> Void

    var body: some View {
        HStack(spacing: 9) {
            Icon(.relay, size: 13, color: Palette.brand, weight: 1.9)
            Text("再添加 1 个账号，见底时就能自动接力").font(.system(size: 12)).foregroundStyle(Palette.ink2)
            Spacer(minLength: 8)
            Button("添加第 2 个账号", action: add).buttonStyle(.pill(.soft))
        }
        .padding(.horizontal, 14).frame(minHeight: 44)
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.dash, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
    }
}
