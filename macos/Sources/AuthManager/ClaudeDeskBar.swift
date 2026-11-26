import SwiftUI

// Claude 桌面's chat / work stays on one account while the other accounts relay Code (11-25, Gemini 稿): it is a
// bar under the page header, not half of 在用. The bar says which account the desktop uses and how its sync is;
// 换号与设置 opens the rest: the cold switch (always confirmed, Claude 桌面 quits and reopens), the three ways to
// pick the next desktop account, and the desktop's settings.
struct ClaudeDeskBar: View {
    @EnvironmentObject private var service: ServiceController
    let status: ClaudeDesktopStatus
    let entries: [ClaudeEntry]
    let use: ClaudeUse
    let directory: AccountDirectory
    let present: (ClaudeSheet) -> Void
    @State private var open = ClaudeDeskBar.previewOpen
    // Preview renders only: draws the bar with its panel open.
    @MainActor static var previewOpen = false

    private var locked: Bool { service.accountBusy || status.busy || !status.installed }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                Icon(.chat, size: 15, color: Palette.ink2, weight: 1.8)
                Text(status.codeRelay?.enabled == true ? "Claude Chat" : "桌面对话").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
                Rectangle().fill(Palette.divider).frame(width: 1, height: 14)
                if let current = status.current {
                    Seal(current.display, style: .inUse, size: 18)
                    Text(accountName: entry(current)?.name ?? current.display).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                    // The account's own menu (rename, renewal, remove), as on its card.
                    if let match = entry(current) { ClaudeEntryMenu(entry: match, use: use, directory: directory, open: present) }
                } else {
                    Text(status.installed ? "桌面还没认出在用的账号" : "没找到 Claude 桌面").font(.system(size: 12)).foregroundStyle(Palette.ink3)
                }
                chips
                Spacer(minLength: 8)
                Button { open = true } label: {
                    HStack(spacing: 5) {
                        Text("换号与设置")
                        Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4)
                    }
                }
                .buttonStyle(.pill(.secondary, height: 26))
                .popover(isPresented: $open, arrowEdge: .bottom) {
                    ClaudeDeskPanel(status: status, entries: entries, directory: directory) { item in
                        open = false
                        // The panel closes first, then the sheet opens on the page.
                        DispatchQueue.main.async { present(item) }
                    }
                    .environmentObject(service)
                }
            }
            .padding(.horizontal, 14).frame(height: 44)
            DesktopNotices(status: status, locked: locked, compact: true) { present(.desktop($0)) }
        }
        .loopCard(radius: 14)
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    // How the desktop's sync, its Chrome login and Code 无感换号 are: quiet when all is well, amber when not.
    @ViewBuilder private var chips: some View {
        if status.installed {
            if status.syncing {
                Pill("正在同步…", small: true)
            } else if status.restartNeeded {
                Pill("要重启一次 Claude", style: .attention, small: true).help("重启一次 Claude 才看得到新会话；点「换号与设置」看会话同步")
            } else if status.pending > 0 || status.known.contains(where: { $0.behind > 0 }) || status.lastSync?.ok == false {
                Pill("会话没同步", style: .attention, small: true).help("点「换号与设置」看会话同步")
            } else {
                Pill("\(status.sessions) 个会话已同步", small: true)
            }
        }
        if let chrome = status.chromeSync, chrome.enabled {
            Pill("Chrome 同步", style: ["mismatch", "missing_session", "error"].contains(chrome.state) ? .attention : .neutral, small: true)
                .help(chrome.message)
        }
        if let relay = status.codeRelay, relay.enabled {
            Pill("无感换号 开", style: ["team_mismatch", "blocked_remote", "error"].contains(relay.state) || relay.installState == "needs_update" ? .attention : .brand, small: true)
                .help(relay.message)
        }
    }

    private func entry(_ desk: ClaudeDesktopStatus.Account) -> ClaudeEntry? { ClaudeEntry.find(desk.handle, in: entries) }
}

struct ClaudeDeskPanel: View {
    @EnvironmentObject private var service: ServiceController
    let status: ClaudeDesktopStatus
    let entries: [ClaudeEntry]
    let directory: AccountDirectory
    let present: (ClaudeSheet) -> Void

    private var locked: Bool { service.accountBusy || status.busy || !status.installed || !status.switchSupported }
    // With 无感换号 on, the desktop's login is only Chat / Cowork's, and the panel says Chat (11-26).
    private var chat: Bool { status.codeRelay?.enabled == true }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 7) {
                Icon(.chat, size: 15, color: Palette.ink2, weight: 1.8)
                Text(chat ? "Claude Chat" : "桌面 · 对话 / 工作").font(.system(size: 13, weight: .semibold))
                Spacer(minLength: 6)
                Pill("仅手动切换", small: true).help("Claude 桌面从不自动换号；换桌面号要退出并重开它")
            }
            if let current = status.current { currentBox(current) }
            if !status.known.filter({ !$0.inUse }).isEmpty {
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text("下一个桌面号").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                        Spacer()
                        Text("需退出并重开 Claude 桌面").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                    HStack(spacing: 8) {
                        pick("下一个", status.picks?.next)
                        pick("下一个有额度", status.picks?.nextAvailable)
                        pick("剩余最多的", status.picks?.best)
                    }
                    VStack(spacing: 0) {
                        ForEach(Array(others.enumerated()), id: \.element.id) { offset, desk in
                            if offset > 0 { Divider() }
                            row(desk)
                        }
                    }
                    .padding(.horizontal, 10)
                    .loopCard(radius: 10)
                }
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("同步与无感换号").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                VStack(spacing: 0) {
                    if status.installed { SyncStrip(status: status, locked: service.accountBusy || status.busy, embedded: true) { present(.desktop($0)) } }
                    if status.installed, let chrome = status.chromeSync {
                        ChromeSyncStrip(chrome: chrome, locked: service.accountBusy || status.busy || chrome.busy, embedded: true) { present($0) }
                    }
                    if status.installed, let relay = status.codeRelay {
                        CodeRelayStrip(relay: relay, locked: service.accountBusy || status.busy || relay.busy, embedded: true) { present($0) }
                    }
                }
                .loopCard(radius: 10)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            }
        }
        .padding(14)
        .frame(width: 452)
        .background(Palette.ground)
        .foregroundStyle(Palette.ink)
        .tint(Palette.solid)
    }

    private var others: [ClaudeDesktopStatus.Account] {
        status.known.filter { !$0.inUse }.sorted { ($0.number ?? .max) < ($1.number ?? .max) }
    }

    private func entry(_ handle: String?) -> ClaudeEntry? { handle.flatMap { ClaudeEntry.find($0, in: entries) } }

    // What an account has left, as the queue shows it: its AuthBaton login's lowest window, or the desktop's own read.
    private func left(_ entry: ClaudeEntry?) -> String? {
        if let account = entry?.cli { return account.weekSpent.map { $0.refill() } ?? account.lowestPrimary.map { "\($0)%" } }
        return entry?.desk?.tokenQuota?.primary.map(\.remaining).min().map { "\($0)%" }
    }

    private func currentBox(_ current: ClaudeDesktopStatus.Account) -> some View {
        let match = entry(current.handle)
        let sameAsCode = match?.inCode == true
        return VStack(alignment: .leading, spacing: 10) {
            currentLine(current, match: match, sameAsCode: sameAsCode)
            // Signed in to Claude 桌面 only: what it has left read through the desktop's login, or the ways to read it.
            if match?.cli == nil, let desk = match?.desk {
                DeskOnlyQuota(desk: desk, login: { present(.account(.login(provider: "claude", accountID: nil))) }, turnOn: { present(.tokenQuota) })
            }
        }
    }

    private func currentLine(_ current: ClaudeDesktopStatus.Account, match: ClaudeEntry?, sameAsCode: Bool) -> some View {
        HStack(spacing: 10) {
            Seal(current.display, style: .inUse, size: 28)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(accountName: match?.name ?? current.display).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                    Pill("在用", style: .brand, small: true)
                }
                if sameAsCode {
                    Text("和 Code 共用这个号").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            }
            Spacer(minLength: 6)
            if let account = match?.cli {
                VStack(alignment: .trailing, spacing: 2) {
                    ForEach(Array(account.primaryWindows.prefix(2).reversed().enumerated()), id: \.offset) { _, window in
                        Text("\(window.shortLabel) \(window.remaining)%").font(.system(size: 11, weight: .semibold)).monospacedDigit().foregroundStyle(Palette.ink2)
                    }
                }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .loopPanel(radius: 12)
    }

    // One way of picking: who it would switch to now and what that account has left.
    private func pick(_ title: String, _ handle: String?) -> some View {
        let target = entry(handle)
        let desk = handle.flatMap { h in status.accounts.first { $0.handle == h } }
        return Button {
            if let target { ask(target) }
        } label: {
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.system(size: 11, weight: .semibold)).foregroundStyle(target == nil ? Palette.disabled : Palette.ink)
                HStack {
                    Text(target?.name ?? desk?.display ?? "没有").font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                    Spacer(minLength: 4)
                    if let left = left(target) { Text(left).font(.system(size: 11, weight: .semibold)).monospacedDigit().foregroundStyle(Palette.ink2) }
                }
            }
            .padding(.horizontal, 10).padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Palette.card, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(target == nil || locked)
        .help(target.map { "把 Claude 桌面换到 \($0.name)（会先确认）" } ?? "现在没有可换的桌面号")
    }

    private func row(_ desk: ClaudeDesktopStatus.Account) -> some View {
        let target = entry(desk.handle)
        return HStack(spacing: 8) {
            if let number = desk.number { Text("#\(number)").font(.system(size: 11, weight: .semibold)).monospacedDigit().foregroundStyle(Palette.ink3).frame(width: 22, alignment: .leading) }
            Seal(target?.name ?? desk.display, style: .standby, size: 20)
            Text(accountName: target?.name ?? desk.display).font(.system(size: 12, weight: .semibold)).lineLimit(1)
            if let cli = target?.cli, cli.weekSpent != nil || (directory.notch(for: cli).map { line in cli.primaryWindows.contains { $0.remaining <= line } } ?? false) {
                Pill("已见底", style: .attention, small: true)
            } else if !desk.participates {
                Pill("不参与", small: true).help("不参与「下一个」")
            }
            Spacer(minLength: 6)
            if let left = left(target) { Text(left).font(.system(size: 11)).monospacedDigit().foregroundStyle(Palette.ink3) }
            Button(chat ? "换到 Chat…" : "换到桌面…") { if let target { ask(target) } }
                .buttonStyle(.pill(.secondary))
                .disabled(target == nil || locked || !desk.canSwitch)
        }
        .frame(height: 34)
    }

    private func ask(_ target: ClaudeEntry) {
        present(.switching(ClaudeSwitchRequest(entryID: target.id, code: false, desk: true, choice: false)))
    }
}
