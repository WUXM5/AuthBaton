import AppKit
import SwiftUI

// External account sources (11-27, Gemini 稿): accounts a credential centre signs in and renews, read and used
// here only with the access it lends. On the Codex and Claude pages they sit in a group of their own under 不参与;
// those that joined the relay are in the queue with their source beside the rule. What this Mac may do with each
// is its mode, which the person sets: 只看, 可用重置卡 or 可换上本机. Everything shown comes from the service's
// snapshot; nothing here works out a mode, a target or a state of its own.

enum ExternalMode: String, CaseIterable, Identifiable, Sendable {
    case view, reset, swap = "switch"

    var id: Self { self }
    var label: String {
        switch self {
        case .view: "只看"
        case .reset: "可用重置卡"
        case .swap: "可换上本机"
        }
    }
    // What the mode allows, under its name in the choice; source is whose quota a switch spends.
    func detail(_ source: String) -> String {
        switch self {
        case .view: "只看额度和重置卡，不能在本机操作"
        case .reset: "允许手动用重置卡，不进接力队列，不能换上本机"
        case .swap: "允许换上本机和加入自动接力（含按规则自动用卡），会用掉 \(source) 的额度"
        }
    }
    var rank: Int { ExternalMode.allCases.firstIndex(of: self) ?? 0 }
}

extension AccountExternal {
    // Missing or unknown reads as 只看: the least a row may do.
    var level: ExternalMode { mode.flatMap(ExternalMode.init(rawValue:)) ?? .view }
    var allowsReset: Bool { level != .view }
    var allowsSwitch: Bool { level == .swap }
    var name: String { sourceLabel ?? "外部来源" }
    var trouble: Bool { sourceState == "error" }
    // Its quota cannot be read, or the source no longer lends it: the reason stands where the numbers would.
    var blocked: Bool { quotaReadable == false || (status.map { $0 != "ready" } ?? false) }
}

extension ExternalSource {
    var name: String { label ?? "外部来源" }
    var trouble: Bool { state == "error" }
    // The amber word beside a source that does not answer; the service's sentence says which it is.
    var troubleTag: String { (error ?? "").contains("配对已失效") ? "配对已失效" : "连不上" }
    var host: String? { siteUrl.flatMap { URL(string: $0)?.host } ?? siteUrl }
    // "3 分钟前同步".
    var syncedText: String? {
        guard let readAt else { return nil }
        let text = When.text(readAt)
        return text + (text.last?.isNumber == true ? " 同步" : "同步")
    }
}

extension ManagedAccount {
    // Expiry dates of the cards still left, earliest first.
    var resetDates: [Date] {
        let dates = provider == "codex" ? codexCredits.compactMap(\.expiryDate)
            : (usage?.claudeResets?.grants?.items ?? []).filter { $0.remaining > 0 }.compactMap { QuotaResetCredit.date($0.endsAt) }
        return dates.sorted()
    }
    // In use by its tool: Claude Code's, or Codex's (the desktop's too while hot switching).
    func inUse(_ directory: AccountDirectory) -> Bool {
        native || (provider == "codex" && directory.desktop?.selected == id)
    }
}

// The source's own web page, where its accounts are signed in, renewed and removed. Only https; the preview
// records the address instead of opening it.
@MainActor enum ExternalLink {
    static var opener: @MainActor (URL) -> Void = { NSWorkspace.shared.open($0) }
    static func open(_ address: String?) {
        guard let address, let url = URL(string: address), url.scheme == "https" else { return }
        opener(url)
    }
}

// The source's name on a row: a link and its label. Amber while the source does not answer.
struct ExternalTag: View {
    let text: String
    var trouble = false
    // Its choice of mode is open.
    var lit = false
    var large = false
    // Where the row has no room for the name: the link alone (the name is in its help).
    var iconOnly = false

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 6, style: .continuous)
        HStack(spacing: large ? 4 : 3.5) {
            Icon(.link, size: large ? 11 : 10, color: trouble ? Palette.attentionText : lit ? Palette.deep : Palette.ink3, weight: 2)
            if !iconOnly { Text(text) }
        }
        .font(.system(size: large ? 11 : 10, weight: .medium))
        .foregroundStyle(trouble ? Palette.attentionText : lit ? Palette.deep : Palette.ink2)
        .lineLimit(1).fixedSize()
        .padding(.horizontal, large ? 7 : 6).frame(height: large ? 21 : 20)
        .background(trouble ? Palette.attentionFill : lit ? Palette.card : Palette.chip, in: shape)
        .overlay(shape.strokeBorder(trouble ? Palette.attentionDash.opacity(0.4) : lit ? Palette.brand : Palette.ring, lineWidth: 1))
    }
}

// "去来源网页 ↗": the place where the account itself is looked after.
struct ExternalWebLink: View {
    let title: String
    let address: String?

    var body: some View {
        Button { ExternalLink.open(address) } label: {
            HStack(spacing: 3) {
                Text(title)
                Text("↗").font(.system(size: 11, weight: .semibold))
            }
        }
        .buttonStyle(.pill(.link))
        .foregroundStyle(Palette.ink2)
        .disabled(address == nil)
        .help(address ?? "")
    }
}

// MARK: 档位

// An external row's mode under its name, or (in the queue) its source tag; either opens the choice.
struct ExternalModeButton: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    // The queue shows the source's tag beside the rule; the group the mode itself.
    var asTag = false
    // A narrow queue row: the tag's link alone.
    var iconOnly = false
    // The in-use card's 「外部 · 来源」, a size up.
    var large = false
    @State private var showing = false

    var body: some View {
        let external = account.external ?? AccountExternal()
        Button { showing = true } label: {
            if asTag {
                ExternalTag(text: large ? "外部 · \(external.name)" : external.name, trouble: external.trouble, lit: showing, large: large, iconOnly: iconOnly)
            } else {
                ExternalModeChip(mode: external.level, open: showing)
            }
        }
        .buttonStyle(.plain)
        // While the source does not answer the mode stays as it is (the rows keep their last numbers).
        .disabled(external.trouble || service.accountBusy)
        .help(external.trouble ? "\(external.name) 现在连不上，暂时不能改" : "本机可用操作：\(external.level.label)")
        .accessibilityLabel(asTag ? "来源 \(external.name)，本机可用操作：\(external.level.label)" : "本机可用操作：\(external.level.label)")
        .popover(isPresented: $showing, arrowEdge: .bottom) {
            ExternalModePanel(account: account, directory: directory, close: { showing = false }).environmentObject(service)
        }
    }
}

// 只看 grey, 可用重置卡 outlined, 可换上本机 in the brand's wash; a chevron says it opens.
struct ExternalModeChip: View {
    let mode: ExternalMode
    var open = false

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 6, style: .continuous)
        HStack(spacing: 4) {
            Text(mode.label)
            Icon(.chevronDown, size: 8, color: mode == .swap ? Palette.deep : mode == .view ? Palette.ink3 : Palette.ink2, weight: 2.4)
                .rotationEffect(.degrees(open ? 180 : 0))
        }
        .font(.system(size: 10.5, weight: .medium))
        .foregroundStyle(mode == .swap ? Palette.deep : mode == .view ? Palette.ink3 : Palette.ink)
        .lineLimit(1).fixedSize()
        .padding(.horizontal, 7).frame(height: 20)
        .background(mode == .swap ? Palette.wash : mode == .view ? Palette.chip : Palette.card, in: shape)
        .overlay(shape.strokeBorder(open ? Palette.brand : mode == .swap ? Palette.soft : mode == .reset ? Palette.edge : Color.clear, lineWidth: 1))
    }
}

// The choice of mode. Lowering a row the relay uses takes it out of the queue: that is said and confirmed here,
// in place. The account in use keeps 可换上本机 until something else is switched to.
struct ExternalModePanel: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let close: () -> Void
    // A lower mode picked for a row in the relay, waiting for 移出并降档. Preview renders set it from the start.
    @State var pending: ExternalMode? = nil

    var body: some View {
        let external = account.external ?? AccountExternal()
        let current = external.level
        let locked = account.inUse(directory) && current == .swap
        let fallback = directory.source(of: account)?.defaultMode.flatMap(ExternalMode.init(rawValue:)) ?? .view
        VStack(alignment: .leading, spacing: 2) {
            Text("本机可用操作").font(.system(size: 11, weight: .medium)).foregroundStyle(Palette.ink3)
                .padding(.horizontal, 8).padding(.top, 4).padding(.bottom, 4)
            ForEach(ExternalMode.allCases) { mode in
                option(mode, current: current, isDefault: mode == fallback, disabled: locked && mode != current, source: external.name)
            }
            if let pending, let rule = directory.rule(for: account.id) {
                confirm(pending, rule: rule)
            }
            if locked {
                HStack(alignment: .top, spacing: 7) {
                    Icon(.info, size: 13, color: Palette.attention).padding(.top, 1)
                    Text("\(account.provider == "claude" ? "Code" : "Codex") 正在使用 \(account.displayName)，请先换到其他账号再降档。")
                        .font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
                }
                .padding(.horizontal, 10).padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                .padding(.top, 5)
            }
            if service.accountErrorID == account.id, let error = service.accountError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 8).padding(.top, 5)
            }
            Text("外部账号不计入上方账号数；只有设为「可换上本机」才能加入接力。登录、续期和删号请在来源网页操作。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 8).padding(.top, 8).padding(.bottom, 3)
                .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1).padding(.top, 2) }
                .padding(.top, 4)
        }
        .padding(8)
        .frame(width: 336, alignment: .leading)
    }

    private func option(_ mode: ExternalMode, current: ExternalMode, isDefault: Bool, disabled: Bool, source: String) -> some View {
        let on = mode == current
        let waiting = mode == pending
        return Button { pick(mode, current: current) } label: {
            HStack(alignment: .top, spacing: 8) {
                Group {
                    if waiting {
                        Circle().strokeBorder(Palette.attention, lineWidth: 1.8).frame(width: 10, height: 10)
                    } else if on {
                        Icon(.check, size: 13, color: Palette.brand, weight: 2.4)
                    }
                }
                .frame(width: 15, height: 15).padding(.top, 1)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(mode.label).font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(disabled ? Palette.disabled : waiting ? Palette.attentionText : on ? Palette.deep : Palette.ink)
                        if isDefault {
                            Text("默认").font(.system(size: 10, weight: .medium)).foregroundStyle(Palette.ink3)
                                .padding(.horizontal, 5).frame(height: 16).background(Palette.chip, in: RoundedRectangle(cornerRadius: 4, style: .continuous))
                        }
                        if disabled {
                            Text("不可选").font(.system(size: 10)).foregroundStyle(Palette.disabled)
                        }
                    }
                    Text(mode.detail(source)).font(.system(size: 11)).foregroundStyle(disabled ? Palette.disabled : Palette.ink3)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 8).padding(.vertical, 7)
            .background(waiting ? Palette.attentionWash : on ? Palette.wash : .clear, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay { if waiting { RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Palette.attentionDash.opacity(0.4), lineWidth: 1) } }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(disabled || service.accountBusy)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    // Lowering a row that has a rule asks first, here; anything else is sent at once.
    private func pick(_ mode: ExternalMode, current: ExternalMode) {
        guard mode != current else { pending = nil; return }
        if mode.rank < current.rank, directory.rule(for: account.id) != nil {
            withAnimation(.easeOut(duration: 0.15)) { pending = mode }
            return
        }
        send(mode, confirm: false)
    }

    private func send(_ mode: ExternalMode, confirm: Bool) {
        service.clearAccountError()
        Task {
            await service.external("set_mode", id: account.id, mode: mode.rawValue, confirm: confirm)
            if service.accountError == nil { pending = nil; close() }
        }
    }

    private func confirm(_ mode: ExternalMode, rule: CodexStrategyConfig.Rule) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 7) {
                Icon(.alert, size: 13, color: Palette.attention).padding(.top, 1)
                Text("降为「\(mode.label)」会将此号移出自动接力队列，见底线与规则（≤\(rule.below)% · \(rule.order.label)）不会保留。")
                    .font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 6) {
                Spacer(minLength: 0)
                Button("取消") { withAnimation(.easeOut(duration: 0.15)) { pending = nil } }.buttonStyle(.pill(.secondary))
                Button("移出并降档") { send(mode, confirm: true) }.buttonStyle(.pill(.primary)).disabled(service.accountBusy)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 9)
        .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(Palette.attentionDash.opacity(0.4), lineWidth: 1))
        .padding(.top, 5)
    }
}

// MARK: 外部账号组

// Under 不参与 on the Codex and Claude pages: each source's rows that are not in the relay queue (nor in use, which
// is the card above), in a dashed tray; its own accounts this Mac already has, merged into the local one, at the foot.
struct ExternalGroups: View {
    let provider: String
    let directory: AccountDirectory
    // 换到 Code on the Claude page, 换上 on the Codex page, and what it does.
    let switchTitle: String
    let switchTo: (ManagedAccount) -> Void
    let open: (AccountSheet) -> Void

    var body: some View {
        ForEach(directory.external?.sources ?? []) { source in
            ExternalGroup(source: source, provider: provider, directory: directory, switchTitle: switchTitle, switchTo: switchTo, open: open)
        }
    }
}

struct ExternalGroup: View {
    static let openKey = "external.groupOpen"
    @EnvironmentObject private var service: ServiceController
    @AppStorage(ExternalGroup.openKey) private var opened = true
    let source: ExternalSource
    let provider: String
    let directory: AccountDirectory
    let switchTitle: String
    let switchTo: (ManagedAccount) -> Void
    let open: (AccountSheet) -> Void

    var body: some View {
        let rows = directory.relay(provider).filter { $0.external?.source == source.id }
        let queued = rows.filter { directory.rule(for: $0.id)?.participate == true }
        let waiting = rows.filter { directory.rule(for: $0.id)?.participate != true && !$0.inUse(directory) }
        let merged = (source.merged ?? []).filter { $0.provider == provider }
        if !rows.isEmpty || !merged.isEmpty {
            VStack(alignment: .leading, spacing: 6) {
                header(waiting: waiting.count, queued: queued.count)
                if opened { tray(waiting: waiting, queued: queued.count, merged: merged) }
            }
        }
    }

    private func header(waiting: Int, queued: Int) -> some View {
        HStack(spacing: 7) {
            Button { withAnimation(.easeOut(duration: 0.18)) { opened.toggle() } } label: {
                HStack(spacing: 7) {
                    Icon(.link, size: 13, color: Palette.ink2)
                    Text("外部账号").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
                    Text(source.name).font(.system(size: 11, weight: .medium)).foregroundStyle(Palette.ink2).lineLimit(1)
                        .padding(.horizontal, 7).frame(height: 20)
                        .background(Palette.chip, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
                    if source.trouble {
                        HStack(spacing: 4) {
                            Icon(.alert, size: 11, color: Palette.attentionText)
                            Text(source.troubleTag)
                        }
                        .font(.system(size: 10.5, weight: .semibold)).foregroundStyle(Palette.attentionText)
                        .padding(.horizontal, 7).frame(height: 20)
                        .background(Palette.attentionFill, in: Capsule())
                        .help(source.error ?? "")
                    }
                    Text("余 \(waiting) 个" + (queued > 0 ? " · 另有 \(queued) 个在接力队列" : ""))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            Spacer(minLength: 8)
            if source.job == "syncing" {
                HStack(spacing: 5) {
                    ProgressView().controlSize(.mini)
                    Text("正在同步…")
                }
                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
            } else if let synced = source.syncedText {
                Text(synced).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1).monospacedDigit()
                    .help(source.readAt.map { When.exact($0) } ?? "")
            }
            Button("管理来源 ›") { AppNavigation.shared.show(.cloud) }.buttonStyle(.pill(.link))
                .help("在个人云端里看这个来源、同步或断开")
            Button { withAnimation(.easeOut(duration: 0.18)) { opened.toggle() } } label: {
                HStack(spacing: 4) {
                    Text(opened ? "收起" : "展开")
                    Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4).rotationEffect(.degrees(opened ? 180 : 0))
                }
                .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 4).frame(height: 26)
    }

    private func tray(waiting: [ManagedAccount], queued: Int, merged: [ExternalSource.Merged]) -> some View {
        let shape = RoundedRectangle(cornerRadius: 15, style: .continuous)
        return VStack(spacing: 0) {
            if waiting.isEmpty && queued > 0 {
                Text("\(queued) 个外部账号均已加入上方接力队列").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    .padding(.horizontal, 14).frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
            }
            ForEach(Array(waiting.enumerated()), id: \.element.id) { index, account in
                ExternalRow(account: account, directory: directory, source: source, switchTitle: switchTitle, switchTo: switchTo, open: open)
                    .overlay(alignment: .top) { if index > 0 { Rectangle().fill(Palette.divider).frame(height: 1) } }
            }
            if !merged.isEmpty {
                let emails = merged.compactMap(\.email)
                HStack(spacing: 7) {
                    Icon(.copy, size: 12, color: Palette.ink3)
                    (Text("已合并 \(merged.count) 个本机已有账号：") + Text(emails.joined(separator: "、")).foregroundStyle(Palette.ink2).fontWeight(.medium)
                     + Text("（只显示本机这份）"))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(2)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 14).padding(.vertical, 8)
                .background(Palette.ink.opacity(0.025))
                .overlay(alignment: .top) { if !waiting.isEmpty || queued > 0 { Rectangle().fill(Palette.divider).frame(height: 1) } }
            }
        }
        .background(Palette.card.opacity(0.64), in: shape)
        .clipShape(shape)
        .overlay(shape.strokeBorder(Palette.dashStrong.opacity(0.55), style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
    }
}

// One row of the group: who it is with its mode, its numbers (or why there are none), and what its mode allows.
struct ExternalRow: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let source: ExternalSource
    let switchTitle: String
    let switchTo: (ManagedAccount) -> Void
    let open: (AccountSheet) -> Void

    private var external: AccountExternal { account.external ?? AccountExternal() }

    var body: some View {
        ViewThatFits(in: .horizontal) {
            line(wide: true).frame(minWidth: 1080)
            line(wide: false)
        }
        .padding(.leading, 12).padding(.trailing, 12).padding(.vertical, 10)
    }

    private func line(wide: Bool) -> some View {
        HStack(spacing: 9) {
            if wide {
                identity.frame(width: 300, alignment: .leading)
                quota(wide: true).frame(width: 452, alignment: .leading)
                Spacer(minLength: 0)
            } else {
                identity.frame(minWidth: 116, maxWidth: .infinity, alignment: .leading)
                quota(wide: false).frame(width: 298, alignment: .leading)
            }
            actions.fixedSize().frame(minWidth: 186, alignment: .trailing)
        }
    }

    private var identity: some View {
        HStack(spacing: 8) {
            Seal(account.displayName, style: external.quotaReadable == false ? .externalDim : .external, size: 26)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 5) {
                    Text(accountName: account.displayName).font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(external.quotaReadable == false ? Palette.ink2 : Palette.ink).lineLimit(1).layoutPriority(1)
                    if let plan = account.plan { Text(plan.uppercased()).font(.system(size: 10.5, weight: .medium)).foregroundStyle(Palette.ink3).fixedSize() }
                    if account.needsAction { Pill(account.health.title, style: .attention, small: true).fixedSize() }
                }
                if account.provider == "claude" && external.quotaReadable == false {
                    // Only a setup-token at the source: nothing to read, nothing to choose.
                    Text("仅 setup-token").font(.system(size: 10.5, weight: .medium)).foregroundStyle(Palette.disabled)
                        .padding(.horizontal, 7).frame(height: 20)
                        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Palette.divider, lineWidth: 1))
                } else {
                    ExternalModeButton(account: account, directory: directory)
                }
            }
        }
    }

    @ViewBuilder private func quota(wide: Bool) -> some View {
        if external.blocked {
            HStack(spacing: 7) {
                Icon(.info, size: 13, color: Palette.ink3)
                Text(external.reason ?? account.message).font(.system(size: 11.5)).foregroundStyle(Palette.ink3).lineLimit(2)
            }
        } else if account.primaryWindows.isEmpty {
            Text(account.usage?.refreshing == true || service.quotaRequests.contains(account.id) ? "正在查询…" : account.quotaCaption ?? "额度暂不可查")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
        } else {
            // Outside the relay it has no 见底线: low numbers are not amber here.
            ClaudeMiniQuota(windows: Array(account.primaryWindows.prefix(2)), extras: account.provider == "claude" ? account.extraWindows : [],
                            notch: nil, skipped: false, wide: wide, plain: true)
                .opacity(external.trouble ? 0.48 : 1)
        }
    }

    @ViewBuilder private var actions: some View {
        let locked = service.accountBusy || directory.busy
        let count = account.creditCount ?? 0
        let dates = account.resetDates
        if external.blocked {
            if external.status != "gone" {
                Button {
                    ExternalLink.open(external.manageUrl ?? source.manageUrl)
                } label: {
                    HStack(spacing: 4) {
                        Text(external.status == "disabled" ? "去来源网页" : "去网页登录")
                        Text("↗").font(.system(size: 11, weight: .semibold))
                    }
                }
                .buttonStyle(.pill(.secondary))
                .help(external.manageUrl ?? source.manageUrl ?? "")
            }
        } else {
            switch external.level {
            case .view:
                if count > 0 {
                    VStack(alignment: .trailing, spacing: 2) {
                        Pill("\(count) 张", small: false).help("剩 \(count) 张重置卡；设为「可用重置卡」后才能在本机用")
                        if let first = dates.first {
                            Text((count > 1 ? "最早 " : "") + When.day(first) + "到期").font(.system(size: 10)).foregroundStyle(Palette.ink3)
                        }
                    }
                }
            case .reset:
                VStack(alignment: .trailing, spacing: 2) {
                    if count > 0 {
                        Button("用 1 张重置卡") { reset() }
                            .buttonStyle(.pill(.secondary))
                            .disabled(account.nextReset == nil || directory.resetBlocked(account, busy: service.accountBusy))
                        Text("\(count) 张" + ((account.nextReset?.expiresAt).flatMap(QuotaResetCredit.date).map { " · \(When.day($0))到期" } ?? ""))
                            .font(.system(size: 10)).foregroundStyle(Palette.ink3)
                    } else {
                        Text("没有重置卡").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                }
            case .swap:
                HStack(spacing: 6) {
                    if count > 0 {
                        Button { reset() } label: { Text("\(count) 张") }
                            .buttonStyle(.pill(.quiet))
                            .disabled(account.nextReset == nil || directory.resetBlocked(account, busy: service.accountBusy))
                            .help("用 1 张重置卡" + (dates.first.map { "；最早 \(When.day($0))到期" } ?? ""))
                    }
                    let full = (directory.strategy?.config.accounts.count ?? 0) >= RelayRules.limit
                    Button("加入接力") {
                        service.clearAccountError()
                        Task { await RelayRules.save(directory.relayConfig(account.id, participate: true), service) }
                    }
                    .buttonStyle(.pill(.secondary))
                    .disabled(locked || full || directory.strategy == nil)
                    .help(full ? "接力最多 \(RelayRules.limit) 个账号；先移除不用的账号" : "加入后排在队列最后")
                    Button(switchTitle) { switchTo(account) }
                        .buttonStyle(.pill(.secondary))
                        .disabled(locked || directory.primaryAction(for: account) != .switchTo || directory.switchDisabled(for: account))
                }
            }
        }
    }

    private func reset() {
        guard let next = account.nextReset else { return }
        open(.reset(accountID: account.id, creditID: next.creditID, requestID: UUID().uuidString, expiresAt: next.expiresAt, retry: false))
    }
}

// Under a queue row whose source does not answer: the relay passes it by and keeps its rule.
struct ExternalSkipNote: View {
    let external: AccountExternal
    let source: ExternalSource?

    var body: some View {
        let tag = source?.troubleTag ?? "连不上"
        HStack(spacing: 7) {
            Icon(.alert, size: 13, color: Palette.attention)
            Text("自动接力会跳过 · 连不上 \(external.name)" + (tag == "配对已失效" ? "（配对已失效）" : "") + "，原规则已保留")
                .font(.system(size: 11)).foregroundStyle(Palette.attentionText).lineLimit(2)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("去处理 ›") { AppNavigation.shared.show(.cloud) }
                .buttonStyle(.pill(.onAttention, height: 22))
        }
        .padding(.horizontal, 12).padding(.vertical, 7)
        .background(Palette.attentionWash)
        .overlay(alignment: .top) { Rectangle().fill(Palette.attentionDash.opacity(0.3)).frame(height: 1) }
    }
}

// The "…" of an external row: what still makes sense here. Signing in, renewing and removing happen at its source.
struct ExternalAccountMenu: View {
    @EnvironmentObject private var service: ServiceController
    let account: ManagedAccount
    let directory: AccountDirectory
    let open: (AccountSheet) -> Void
    var filled = false

    var body: some View {
        let querying = service.quotaRequests.contains(account.id) || account.usage?.refreshing == true
        let external = account.external ?? AccountExternal()
        Menu {
            Button(querying ? "正在查询额度…" : "查询额度") { Task { await service.checkQuota(id: account.id) } }
                .disabled(querying || external.blocked)
            if external.allowsSwitch {
                let joined = directory.rule(for: account.id)?.participate == true
                Button(joined ? "移出自动接力" : "加入自动接力") {
                    if let config = directory.relayConfig(account.id, participate: !joined) { Task { await RelayRules.save(config, service) } }
                }
                .disabled(directory.strategy == nil)
            }
            Button("执行记录") { open(.history(account.id)) }
            Divider()
            Button("去来源网页 ↗") { ExternalLink.open(external.manageUrl ?? directory.source(of: account)?.manageUrl) }
        } label: {
            MoreDots(color: filled ? Palette.ink2 : Palette.ink3)
                .frame(width: 26, height: 26)
                .background(filled ? Palette.field : .clear, in: Circle())
                .contentShape(Circle())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        .disabled(service.accountBusy || directory.busy)
        .help("查询额度、自动接力、执行记录；登录、续期和删号在 \(external.name) 网页")
        .accessibilityLabel("\(account.displayName) 的更多操作")
    }
}

// MARK: 个人云端 · 外部账号来源

// At the foot of 个人云端: pair with a source by pasting the string its web page gives, then what it lends and how
// this Mac treats the accounts it adds; a source that does not answer takes a new string or a retry.
struct ExternalSourceCard: View {
    @EnvironmentObject private var service: ServiceController
    @State private var enrollment = ""
    @State private var leaving = false
    private var source: ExternalSource? { service.directory?.external?.sources?.first }
    private var busy: Bool { service.accountBusy || source?.job != nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) {
                Text("外部账号来源").font(.system(size: 13, weight: .semibold))
                Spacer(minLength: 8)
                status
            }
            if let source {
                if source.trouble { troubled(source) } else { connected(source) }
            } else {
                unpaired
            }
        }
        .padding(16)
        .loopCard(radius: 16)
        .sheet(isPresented: $leaving) {
            if let source { ExternalDisconnectSheet(source: source, close: { leaving = false }).environmentObject(service) }
        }
    }

    @ViewBuilder private var status: some View {
        if let source {
            HStack(spacing: 5) {
                if source.job == "connecting" || source.job == "syncing" {
                    ProgressView().controlSize(.mini)
                } else {
                    Circle().fill(source.trouble ? Palette.attentionDot : Palette.brand).frame(width: 5, height: 5)
                }
                Text(source.trouble ? source.troubleTag : source.job == "connecting" ? "正在连接…" : "已连接")
            }
            .font(.system(size: 11, weight: .semibold)).foregroundStyle(source.trouble ? Palette.attentionText : Palette.deep)
            .padding(.horizontal, 8).frame(height: 20)
            .background(source.trouble ? Palette.attentionFill : Palette.wash, in: Capsule())
        } else {
            Text("未连接").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink3)
                .padding(.horizontal, 8).frame(height: 20)
                .background(Palette.chip, in: Capsule())
        }
    }

    private var unpaired: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("连接外部来源后，可在本机看额度和重置卡；登录、续期和删号仍在来源网页做。")
                .font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
            pairing(label: "配对串", placeholder: "粘贴配对串", action: "连接")
            Text("在来源网页的账号页点「连接 AuthBaton」，复制配对串。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                .padding(.top, -6)
        }
    }

    // Pasting a string and sending it once; the service decodes it and keeps the keys.
    private func pairing(label: String, placeholder: String, action: String) -> some View {
        let text = enrollment.trimmingCharacters(in: .whitespacesAndNewlines)
        return VStack(alignment: .leading, spacing: 5) {
            Text(label).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
            HStack(spacing: 8) {
                LoopField(placeholder: placeholder, text: $enrollment)
                Button(source?.job == "connecting" ? "正在连接…" : action) {
                    service.clearAccountError()
                    Task {
                        await service.external("connect", enrollment: text)
                        if service.accountError == nil { enrollment = "" }
                    }
                }
                .buttonStyle(.pill(.primary, height: 27))
                .disabled(text.isEmpty || busy)
            }
        }
    }

    private func connected(_ source: ExternalSource) -> some View {
        let codex = source.counts?["codex"] ?? 0
        let claude = source.counts?["claude"] ?? 0
        let mode = source.defaultMode.flatMap(ExternalMode.init(rawValue:)) ?? .view
        return VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 10) {
                    Icon(.link, size: 15, color: Palette.solid, weight: 2)
                        .frame(width: 32, height: 32)
                        .background(Palette.wash, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(Palette.soft, lineWidth: 1))
                    VStack(alignment: .leading, spacing: 1) {
                        Text(source.name).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                        if let host = source.host { Text(host).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1) }
                    }
                    Spacer(minLength: 8)
                    Button { ExternalLink.open(source.manageUrl ?? source.siteUrl) } label: {
                        HStack(spacing: 4) {
                            Text("打开网页")
                            Text("↗").font(.system(size: 11, weight: .semibold))
                        }
                    }
                    .buttonStyle(.pill(.secondary))
                }
                Rectangle().fill(Palette.divider).frame(height: 1).padding(.top, 12)
                HStack(alignment: .top, spacing: 14) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("同步账号").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                        (Text("共 \(codex + claude) 个账号").font(.system(size: 13, weight: .semibold))
                         + Text("  Codex \(codex) · Claude \(claude)").font(.system(size: 11)).foregroundStyle(Palette.ink3))
                            .monospacedDigit().lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("上次同步").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                        Text(source.job == "syncing" ? "正在同步…" : source.readAt.map { When.text($0) } ?? "还没同步过")
                            .font(.system(size: 13, weight: .semibold)).monospacedDigit().lineLimit(1)
                            .help(source.readAt.map { When.exact($0) } ?? "")
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(.top, 11)
            }
            .padding(14)
            .background(Palette.field, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("新账号默认权限").font(.system(size: 12, weight: .semibold))
                    Text("只作用于新同步进来的外部账号，不影响已单独设置的号").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                PillSegments(segments: ExternalMode.allCases.map { .init(value: $0, title: $0.label) }, selection: mode) { picked in
                    guard picked != mode else { return }
                    service.clearAccountError()
                    Task { await service.external("set_default", mode: picked.rawValue) }
                }
                .fixedSize()
                .disabled(busy)
            }
            Text("本机读到的额度会同步回 \(source.name)，两边读数一致。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            footer(retry: false)
        }
    }

    private func troubled(_ source: ExternalSource) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 9) {
                Icon(.alert, size: 14, color: Palette.attention).padding(.top, 1)
                Text(source.error ?? "连不上 \(source.name)").font(.system(size: 12)).foregroundStyle(Palette.attentionText)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .background(Palette.attentionWash, in: RoundedRectangle(cornerRadius: 11, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 11, style: .continuous).strokeBorder(Palette.attentionDash.opacity(0.4), lineWidth: 1))
            pairing(label: "重新配对", placeholder: "粘贴新的配对串", action: "重新连接")
            footer(retry: true)
        }
    }

    private func footer(retry: Bool) -> some View {
        HStack(spacing: 9) {
            Button {
                service.clearAccountError()
                Task { await service.external("sync") }
            } label: {
                HStack(spacing: 5) {
                    if !retry { Icon(.refresh, size: 12, color: Palette.ink2) }
                    Text(retry ? "重试" : source?.job == "syncing" ? "正在同步…" : "立即同步")
                }
            }
            .buttonStyle(.pill(.secondary, height: 27))
            .disabled(busy)
            Spacer(minLength: 0)
            Button("断开来源…") { leaving = true }.buttonStyle(.pill(.secondary, height: 27))
                .disabled(service.accountBusy)
        }
    }
}

// Letting a source go: its accounts leave AuthBaton, the modes set here wait 30 days, the source keeps its own
// accounts. An account of it in use is switched away from first, by the person or by the service.
struct ExternalDisconnectSheet: View {
    @EnvironmentObject private var service: ServiceController
    let source: ExternalSource
    let close: () -> Void

    var body: some View {
        let directory = service.directory
        let inUse = (directory?.accounts ?? []).filter { account in
            account.external?.source == source.id && directory.map { account.inUse($0) } == true
        }
        let name = source.name
        SheetScaffold(title: "断开 \(name)？", lead: "这台 Mac 不再从 \(name) 读额度，也不再领它的 Access。") {
            VStack(alignment: .leading, spacing: 9) {
                CheckItem(kind: .you, text: "\(name) 的外部账号会从 AuthBaton 消失，接力队列里的也一起移出。")
                CheckItem(kind: .info, text: "本机给它们设的权限留 30 天，这期间重新连接会还原。")
                CheckItem(kind: .system, text: "\(name) 那边的账号不受影响；这台 Mac 的配对在 \(name) 网页上吊销。")
            }
            if !inUse.isEmpty {
                AmberNote {
                    Text("在用的 \(inUse.map(\.displayName).joined(separator: "、")) 是 \(name) 的外部账号：先换到本机账号再断开，或让 AuthBaton 先换回上一个本机账号。")
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            SheetError()
        } actions: {
            Button("取消", action: close).buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
            Button(inUse.isEmpty ? "断开来源" : "换回本机账号并断开", role: .destructive) {
                Task {
                    await service.external("disconnect", confirm: true, switchBack: inUse.isEmpty ? nil : true)
                    if service.accountError == nil { close() }
                }
            }
            .buttonStyle(.pill(.destructive, height: 27))
            .disabled(service.accountBusy)
        }
    }
}
