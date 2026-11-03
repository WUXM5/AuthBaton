import AppKit
import SwiftUI

// Claude 桌面: every account's local Code sessions kept the same, and the desktop's sign-in
// switched without signing out. Ported from cc-same (MIT); the service does all of the work.
struct ClaudeDesktopPage: View {
    @EnvironmentObject private var service: ServiceController
    @State private var sheet: DesktopSheet?

    var body: some View {
        Group {
            if let status = service.directory?.claudeDesktop {
                PageScaffold(title: "Claude 桌面", subtitle: Text(status.headline)) {
                    nextButton(status)
                    Button { sheet = .add } label: {
                        HStack(spacing: 5) {
                            Icon(.plus, size: 12, color: .white, weight: 2)
                            Text("添加账号")
                        }
                    }
                    .buttonStyle(.pill(.primary, height: 27))
                    .help("Claude 桌面会退出并打开登录页，现在的登录先存好")
                    .disabled(locked(status) || !status.switchSupported)
                } content: {
                    notices(status)
                    SyncCard(status: status, locked: locked(status)) { sheet = $0 }
                    AccountsCard(status: status, locked: locked(status)) { sheet = $0 }
                    footer(status)
                }
            } else if service.directory != nil {
                PageScaffold(title: "Claude 桌面") {
                    Banner(glyph: .info, text: "后台服务还没读出 Claude 桌面的状态；稍等几秒。一直没有的话，重新连接后台服务再看。")
                }
            } else {
                Color.clear
            }
        }
        .sheet(item: $sheet) { item in
            DesktopSheetView(item: item, close: { sheet = nil }).environmentObject(service)
        }
    }

    private func locked(_ status: ClaudeDesktopStatus) -> Bool {
        service.accountBusy || status.busy || !status.installed
    }

    // Round the numbered list, as the menu bar's 下一个 does; the menu offers the other two ways.
    private func nextButton(_ status: ClaudeDesktopStatus) -> some View {
        let picks = status.picks
        let target = picks?.next
        return HStack(spacing: 0) {
            Button("下一个账号") { if let target { sheet = .switchTo(target) } }
                .buttonStyle(.pill(.secondary, height: 27))
                .help("按编号换到下一个保存了登录、参与「下一个」的账号")
                .disabled(target == nil || locked(status) || !status.switchSupported)
            Menu {
                Button("下一个还有余量的") { if let pick = picks?.nextAvailable { sheet = .switchTo(pick) } }
                    .disabled(picks?.nextAvailable == nil)
                Button("剩余最多的") { if let pick = picks?.best { sheet = .switchTo(pick) } }
                    .disabled(picks?.best == nil)
            } label: {
                Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4).frame(width: 22, height: 27).contentShape(Rectangle())
            }
            .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
            .accessibilityLabel("换号方式")
            .disabled(locked(status) || !status.switchSupported)
        }
    }

    @ViewBuilder private func notices(_ status: ClaudeDesktopStatus) -> some View {
        if let error = service.accountError, service.accountErrorID == nil {
            Banner(text: error) { Button("知道了") { service.clearAccountError() }.buttonStyle(.pill(.onAttention)) }
        }
        if let operation = status.operation, ["switch", "add"].contains(operation.kind), operation.running {
            Banner(glyph: .sync, text: operation.kind == "add" ? "正在添加账号…" : "正在换上 \(operation.target ?? "账号")…") {
                Button("查看") { sheet = .progress }.buttonStyle(.pill(.onAttention))
            }
        } else if let operation = status.operation, !operation.running, operation.ok == false, let message = operation.message {
            Banner(text: message) {
                Button("知道了") { Task { await service.claudeDesktop("dismiss") } }.buttonStyle(.pill(.onAttention))
            }
        }
        if !status.installed {
            Banner(glyph: .info, text: "这台 Mac 上没找到 Claude 桌面。装好并登录以后，这里会列出账号和会话。")
        } else if !status.switchSupported {
            Banner(glyph: .info, text: "这一版 Claude 桌面不能在 AuthBaton 里换号；会话同步照常。")
        }
        if let retention = status.retention, retention.atRisk {
            Banner(glyph: .clock, text: retention.days.map { "Claude Code 会在 \($0) 天后清掉桌面会话的对话原文，清掉的会话打开是空的。" }
                   ?? "Claude Code 会定期清掉桌面会话的对话原文，清掉的会话打开是空的。") {
                if retention.canKeep {
                    Button("一直保留") { Task { await service.claudeDesktop("retention_keep") } }
                        .buttonStyle(.pill(.onAttention)).disabled(locked(status))
                }
            }
        }
        if status.symlinks > 0 {
            Banner(text: "有 \(status.symlinks) 个会话文件夹是软链，Claude 桌面会存不住会话。") {
                Button("改回真文件夹") { Task { await service.claudeDesktop("fix_symlinks", confirm: true) } }
                    .buttonStyle(.pill(.onAttention)).disabled(locked(status))
            }
        }
    }

    private func footer(_ status: ClaudeDesktopStatus) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            if let retention = status.retention, retention.kept {
                HStack(spacing: 5) {
                    Text("桌面会话的对话原文已改为一直保留。")
                    if retention.canUndo {
                        Button("撤销") { Task { await service.claudeDesktop("retention_undo") } }.buttonStyle(.pill(.link))
                    }
                }
            }
            Text("同步的是 Claude 桌面自己的会话列表和登录文件；对话原文本来就在 ~/.claude/projects，CLI 和桌面共用。连接器、远程控制、置顶和侧栏分组按账号各自保留。会话同步与换号移植自 cc-same（MIT）。")
                .fixedSize(horizontal: false, vertical: true)
        }
        .font(.system(size: 11)).foregroundStyle(Palette.ink3)
        .padding(.horizontal, 3.5)
    }
}

enum DesktopSheet: Identifiable {
    case switchTo(String), progress, add, snapshots, remove(String), rename(String)

    var id: String {
        switch self {
        case .switchTo(let handle): "switch-\(handle)"
        case .progress: "progress"
        case .add: "add"
        case .snapshots: "snapshots"
        case .remove(let handle): "remove-\(handle)"
        case .rename(let handle): "rename-\(handle)"
        }
    }
}

// MARK: 会话同步

private struct SyncCard: View {
    @EnvironmentObject private var service: ServiceController
    let status: ClaudeDesktopStatus
    let locked: Bool
    let open: (DesktopSheet) -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 24) {
            SessionLoop(accounts: status.known, size: 124)
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Text("\(status.sessions)").font(.system(size: 44, weight: .semibold, design: .rounded)).monospacedDigit()
                        .kerning(-0.9)
                    Text("个会话").font(.system(size: 13, weight: .semibold)).foregroundStyle(Palette.ink3)
                }
                stateLine.padding(.top, 2)
                HStack(spacing: 9) {
                    Toggle("后台同步", isOn: Binding(get: { status.background }, set: { value in
                        Task { await service.claudeDesktop("background", enabled: value) }
                    }))
                    .labelsHidden().toggleStyle(.switch).controlSize(.small)
                    .disabled(service.accountBusy || !status.installed)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("后台同步").font(.system(size: 12, weight: .semibold))
                        Text("AuthBaton 开着时，每个账号的会话列表一直保持一样").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                }
                .padding(.top, 14)
                HStack(spacing: 7) {
                    Button(status.syncing ? "同步中" : "立即同步") { Task { await service.claudeDesktop("sync") } }
                        .buttonStyle(.pill(.secondary))
                        .disabled(locked || status.syncing)
                    Button("快照与恢复 ›") { open(.snapshots) }.buttonStyle(.pill(.link))
                }
                .padding(.top, 12)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 22).padding(.vertical, 19)
        .loopCard(radius: 19, raised: true)
    }

    @ViewBuilder private var stateLine: some View {
        Group {
            if !status.installed {
                Text("这台 Mac 上没找到 Claude 桌面").foregroundStyle(Palette.ink3)
            } else if status.syncing {
                HStack(spacing: 5) { ProgressView().controlSize(.mini); Text("正在同步…") }.foregroundStyle(Palette.ink2)
            } else if status.restartNeeded {
                Text("要重启一次 Claude 才看得到新会话").foregroundStyle(Palette.attentionText)
            } else if status.pending > 0 {
                Text("还有 \(status.pending) 处没同步" + (status.background ? "，马上补上" : "；打开后台同步或立即同步"))
                    .foregroundStyle(Palette.ink2)
            } else if let last = status.lastSync {
                if last.ok {
                    Text("全部一致 · \(When.text(last.at)) 同步").foregroundStyle(Palette.ink2).help(When.exact(last.at))
                } else {
                    Text(last.message).foregroundStyle(Palette.attentionText).help(When.exact(last.at))
                }
            } else {
                Text("还没同步过").foregroundStyle(Palette.ink3)
            }
        }
        .font(.system(size: 12))
        .fixedSize(horizontal: false, vertical: true)
    }
}

// The signature: one ring per account. Accounts whose lists match sit on top of one another;
// one that is behind drifts out with what it lacks.
struct SessionLoop: View {
    let accounts: [ClaudeDesktopStatus.Account]
    var size: CGFloat = 124

    var body: some View {
        let ring = size * 0.62
        ZStack {
            if accounts.isEmpty {
                Circle().strokeBorder(Palette.dash, style: StrokeStyle(lineWidth: 2, dash: [4, 3])).frame(width: ring, height: ring)
            }
            ForEach(Array(accounts.enumerated()), id: \.element.handle) { index, account in
                let place = offset(index, behind: account.behind > 0)
                Circle()
                    .strokeBorder(stroke(account), style: StrokeStyle(lineWidth: account.inUse ? 2.6 : 2, dash: account.behind > 0 ? [4, 3] : []))
                    .frame(width: ring, height: ring)
                    .offset(place)
            }
            // Each seal sits on its own ring at its own angle, so matching rings never hide one another's seal.
            ForEach(Array(accounts.enumerated()), id: \.element.handle) { index, account in
                let place = offset(index, behind: account.behind > 0)
                let angle = self.angle(index)
                let seal = CGSize(width: place.width + cos(angle) * ring / 2, height: place.height + sin(angle) * ring / 2)
                Seal(account.display, style: account.inUse ? .inUse : account.participates ? .standby : .out, size: 22)
                    .background(Circle().fill(Palette.card).frame(width: 20, height: 20))
                    .offset(seal)
                if account.behind > 0 {
                    Text("差 \(account.behind)").font(.system(size: 10, weight: .semibold)).monospacedDigit()
                        .foregroundStyle(Palette.attentionText)
                        .padding(.horizontal, 5).frame(height: 15)
                        .background(Palette.attentionFill, in: Capsule())
                        .offset(x: seal.width, y: seal.height + 17)
                }
            }
        }
        .frame(width: size, height: size)
        .accessibilityElement()
        .accessibilityLabel(label)
    }

    private func angle(_ index: Int) -> Double {
        Double(index) / Double(max(accounts.count, 1)) * 2 * .pi - .pi / 2
    }

    private func offset(_ index: Int, behind: Bool) -> CGSize {
        guard accounts.count > 1 || behind else { return .zero }
        let radius = behind ? size * 0.16 : size * 0.04
        return CGSize(width: cos(angle(index)) * radius, height: sin(angle(index)) * radius)
    }

    private func stroke(_ account: ClaudeDesktopStatus.Account) -> Color {
        if account.behind > 0 { return Palette.attentionDash }
        return account.inUse ? Palette.brand : Palette.soft
    }

    private var label: String {
        let behind = accounts.filter { $0.behind > 0 }
        if accounts.isEmpty { return "还没有账号" }
        if behind.isEmpty { return "\(accounts.count) 个账号的会话一样" }
        return behind.map { "\($0.display) 差 \($0.behind) 个" }.joined(separator: "，")
    }
}

// MARK: 账号

private struct AccountsCard: View {
    @EnvironmentObject private var service: ServiceController
    let status: ClaudeDesktopStatus
    let locked: Bool
    let open: (DesktopSheet) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("账号").font(.system(size: 13, weight: .semibold))
                Text("\(status.known.count)").font(.system(size: 12)).foregroundStyle(Palette.ink3).monospacedDigit()
                Spacer()
            }
            .padding(.bottom, 5)
            if status.known.isEmpty {
                Text("还没有账号。Claude 桌面登录过的账号会自动加进来，也可以点「添加账号」。")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.vertical, 9)
            }
            ForEach(Array(status.known.enumerated()), id: \.element.handle) { index, account in
                DesktopAccountRow(account: account, count: status.known.count, locked: locked || !status.switchSupported, open: open)
                    .overlay(alignment: .top) { if index > 0 { Rectangle().fill(Palette.divider).frame(height: 1) } }
            }
            if !status.onlyFolders.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    Text("只有会话、还没保存登录").font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink2)
                    Text("AuthBaton 保存登录之前用过的账号。在 Claude 里登录一次就会保存，之后能直接换上。")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                    ForEach(status.onlyFolders) { account in
                        HStack(spacing: 9) {
                            Seal(account.display, style: .out, size: 20)
                            Text(account.display).font(.system(size: 12)).foregroundStyle(Palette.ink2).lineLimit(1)
                            Spacer()
                            Text("\(account.sessions) 个会话").font(.system(size: 11)).foregroundStyle(Palette.ink3).monospacedDigit()
                        }
                        .frame(minHeight: 30)
                    }
                }
                .padding(.top, 12)
                .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
            }
        }
        .padding(.horizontal, 17).padding(.vertical, 15)
        .loopCard()
    }
}

private struct DesktopAccountRow: View {
    @EnvironmentObject private var service: ServiceController
    let account: ClaudeDesktopStatus.Account
    let count: Int
    let locked: Bool
    let open: (DesktopSheet) -> Void

    var body: some View {
        HStack(spacing: 12) {
            NumberBadge(number: account.number, dashed: !account.participates)
            Seal(account.display, style: account.inUse ? .inUse : account.participates ? .standby : .out, size: 26)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 7) {
                    Text(account.display).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                    if account.inUse { Pill("在用", style: .brand, small: true) }
                    if !account.loginSaved { Pill("没保存登录", style: .dashed, small: true).help("在 Claude 里登录一次就会保存") }
                }
                Text(subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1).textSelection(.enabled)
            }
            .frame(minWidth: 150, maxWidth: .infinity, alignment: .leading)
            usage.frame(width: 176)
            Text(account.behind > 0 ? "差 \(account.behind) 个" : "\(account.sessions) 个会话")
                .font(.system(size: 11)).monospacedDigit()
                .foregroundStyle(account.behind > 0 ? Palette.attentionText : Palette.ink3)
                .frame(width: 66, alignment: .trailing)
            if account.canSwitch {
                Button("换上") { open(.switchTo(account.handle)) }.buttonStyle(.pill(.secondary)).disabled(locked)
            } else {
                // Same width as the button, so every row's columns line up.
                Button("换上") {}.buttonStyle(.pill(.secondary)).hidden().accessibilityHidden(true)
            }
            menu
        }
        .frame(minHeight: 54)
    }

    private var subtitle: String {
        var parts: [String] = []
        if account.alias != nil, let email = account.email { parts.append(email) }
        if account.linkedAccount != nil { parts.append("Claude Code 里也有") }
        return parts.isEmpty ? (account.email == nil ? "邮箱还不知道" : "备用") : parts.joined(separator: " · ")
    }

    private var usage: some View {
        VStack(alignment: .leading, spacing: 5) {
            if account.fiveHour == nil && account.weekly == nil {
                Text("还没读到用量").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    .help("用量是 Claude 桌面自己读到的；登录这个账号用一会儿就有")
            } else {
                window("5 小时", account.fiveHour)
                window("每周", account.weekly)
            }
        }
    }

    @ViewBuilder private func window(_ label: String, _ usage: ClaudeDesktopStatus.Usage?) -> some View {
        HStack(spacing: 7) {
            Text(label).font(.system(size: 10)).foregroundStyle(Palette.ink3).frame(width: 30, alignment: .leading)
            if let usage {
                QuotaBar(remaining: usage.remaining, height: 4.5).opacity(usage.stale ? 0.45 : 1)
                Text("\(usage.remaining)%").font(.system(size: 11, weight: .semibold, design: .rounded)).monospacedDigit()
                    .foregroundStyle(usage.stale ? Palette.ink3 : Palette.ink)
                    .frame(width: 34, alignment: .trailing)
            } else {
                Text("—").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                Spacer(minLength: 0)
            }
        }
        .help(usage.map { "剩余 \($0.remaining)% · Claude 桌面 \(When.text($0.at)) 读到" + ($0.stale ? "，已经过时" : "") } ?? "还没读到")
    }

    private var menu: some View {
        Menu {
            Button("重命名…") { open(.rename(account.handle)) }
            Button(account.participates ? "不参与「下一个」" : "参与「下一个」") {
                Task { await service.claudeDesktop(account.participates ? "disable" : "enable", account: account.handle) }
            }
            if count > 1 {
                Menu("移到第几位") {
                    ForEach(1...count, id: \.self) { number in
                        Button("第 \(number) 位") { Task { await service.claudeDesktop("move", account: account.handle, number: number) } }
                            .disabled(number == account.number)
                    }
                }
            }
            Divider()
            Button("移除…", role: .destructive) { open(.remove(account.handle)) }
                .disabled(account.inUse)
        } label: {
            MoreDots().frame(width: 26, height: 26).contentShape(Rectangle())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
        .accessibilityLabel("更多操作")
        .disabled(locked && !account.inUse)
    }
}

private struct NumberBadge: View {
    let number: Int?
    let dashed: Bool

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 7, style: .continuous)
        Text(number.map(String.init) ?? "–")
            .font(.system(size: 11, weight: .semibold, design: .rounded)).monospacedDigit()
            .foregroundStyle(dashed ? Palette.ink3 : Palette.ink2)
            .frame(width: 22, height: 22)
            .background { if !dashed { shape.fill(Palette.chip) } }
            .overlay { if dashed { shape.strokeBorder(Palette.dashStrong, style: StrokeStyle(lineWidth: 1, dash: [3, 2])) } }
            .help(dashed ? "不参与「下一个」，仍可直接换上" : "「下一个」按这个编号轮")
    }
}

// MARK: 弹框

struct DesktopSheetView: View {
    @EnvironmentObject private var service: ServiceController
    let item: DesktopSheet
    let close: () -> Void
    // Once a switch or an add is confirmed the same sheet follows it through.
    @State private var started = false
    @State private var alias = ""
    @State private var restoring: ClaudeDesktopStatus.Snapshot?

    private var status: ClaudeDesktopStatus? { service.directory?.claudeDesktop }
    private func account(_ handle: String) -> ClaudeDesktopStatus.Account? { status?.accounts.first { $0.handle == handle } }

    var body: some View {
        switch item {
        case .progress: progress
        case .switchTo(let handle): started ? AnyView(progress) : AnyView(confirmSwitch(handle))
        case .add: started ? AnyView(progress) : AnyView(confirmAdd)
        case .snapshots: snapshots
        case .remove(let handle): confirmRemove(handle)
        case .rename(let handle): rename(handle)
        }
    }

    private func confirmSwitch(_ handle: String) -> some View {
        let target = account(handle)
        return SheetScaffold(title: "换上 \(target?.title ?? "这个账号")？", lead: "Claude 桌面会退出再打开，换成这个账号。") {
            VStack(alignment: .leading, spacing: 7) {
                CheckItem(kind: .system, text: "现在的登录原样存进 AuthBaton，随时能换回来")
                CheckItem(kind: .you, text: "还有任务在跑的话，Claude 会先问你要不要退出")
                CheckItem(kind: .info, text: "不用重新登录，也不会注销任何账号；会话列表照常同步")
            }
            errorLine
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button("换上") {
                Task {
                    await service.claudeDesktop("switch", account: handle, confirm: true)
                    if service.accountError == nil { started = true }
                }
            }
            .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            .disabled(service.accountBusy || target?.canSwitch != true)
        }
    }

    private var confirmAdd: some View {
        SheetScaffold(title: "添加 Claude 桌面账号", lead: "Claude 会退出并打开登录页。用新账号登录就行，现在的登录会先存好。", width: 442) {
            VStack(alignment: .leading, spacing: 7) {
                CheckItem(kind: .system, text: "现在的登录原样存进 AuthBaton，随时能换回来")
                CheckItem(kind: .you, text: "在 Claude 的登录页用新账号登录，走的是 Anthropic 自己的登录")
                CheckItem(kind: .you, text: "不要在 Claude 里点「退出登录」：退出会让存好的登录在服务器上失效")
                CheckItem(kind: .info, text: "不会注销任何账号")
            }
            errorLine
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button("继续") {
                Task {
                    await service.claudeDesktop("add", confirm: true)
                    if service.accountError == nil { started = true }
                }
            }
            .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            .disabled(service.accountBusy)
        }
    }

    // MARK: 进行中

    private var progress: some View {
        let operation = status?.operation
        let adding = operation?.kind == "add" || { if case .add = item { return true } else { return false } }()
        let steps: [(String, String)] = adding
            ? [("quitting", "让 Claude 退出"), ("moving", "把现在的登录存好"), ("launching", "打开登录页"), ("waiting_sign_in", "在 Claude 里登录新账号"), ("done", "完成")]
            : [("quitting", "让 Claude 退出"), ("moving", "换登录"), ("launching", "打开 Claude"), ("done", "完成")]
        let current = stepIndex(operation?.step, steps: steps)
        let title = adding ? "添加 Claude 桌面账号" : "换上 \(operation?.target ?? "账号")"
        return SheetScaffold(title: title, width: adding ? 442 : 408) {
            VStack(alignment: .leading, spacing: 9) {
                ForEach(Array(steps.enumerated()), id: \.offset) { index, step in
                    HStack(spacing: 9) {
                        stepMark(index: index, current: current, failed: operation?.ok == false)
                        Text(step.1).font(.system(size: 12, weight: index == current ? .semibold : .regular))
                            .foregroundStyle(index <= current ? Palette.ink : Palette.ink3)
                    }
                }
            }
            if let operation {
                if operation.step == "asking" {
                    Banner(text: "Claude 在问要不要退出，请在 Claude 里回答。选「等 Claude」的话这里会一直等。")
                } else if operation.step == "after_work" {
                    Banner(glyph: .clock, text: "Claude 会在手上的事做完后退出，这里会一直等；不想等可以点「不等了」。")
                } else if operation.step == "updating" {
                    Banner(glyph: .clock, text: "Claude 正在更新，等它装完再换。")
                }
                if !operation.running, let message = operation.message {
                    Text(message).font(.system(size: 12))
                        .foregroundStyle(operation.ok == false ? Palette.attentionText : Palette.ink2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        } actions: {
            if operation?.running == true {
                if operation?.waitsOnClaude == true {
                    Button("不等了") { Task { await service.claudeDesktop("stop") } }
                        .buttonStyle(.pill(.secondary, height: 27))
                        .help("停止等待；什么都不改")
                } else {
                    Text("几秒就好，这一步不能停").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
                Button("在后台继续", action: close).buttonStyle(.pill(.secondary, height: 27))
            } else {
                Button("好") {
                    Task {
                        await service.claudeDesktop("dismiss")
                        close()
                    }
                }
                .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            }
        }
    }

    private func stepIndex(_ step: String?, steps: [(String, String)]) -> Int {
        switch step {
        case "asking", "after_work", "updating", "quitting", nil: return 0
        case "done": return steps.count - 1
        case "failed", "stopped": return -1
        default: return steps.firstIndex { $0.0 == step } ?? 0
        }
    }

    @ViewBuilder private func stepMark(index: Int, current: Int, failed: Bool) -> some View {
        if index == current && status?.operation?.step == "waiting_sign_in" {
            Icon(.clock, size: 14, color: Palette.ink2)
        } else if index < current || (current == index && status?.operation?.running == false && !failed) {
            Icon(.check, size: 14, color: Palette.solid)
        } else if index == current && status?.operation?.running == true {
            ProgressView().controlSize(.mini).frame(width: 14, height: 14)
        } else {
            Circle().strokeBorder(Palette.dash, lineWidth: 1.5).frame(width: 12, height: 12).frame(width: 14, height: 14)
        }
    }

    // MARK: 快照

    private var snapshots: some View {
        let items = status?.snapshots ?? []
        return SheetScaffold(title: restoring == nil ? "快照与恢复" : "恢复到 \(restoring.map { When.text($0.at) } ?? "")？",
                             lead: restoring == nil
                                ? "第一次改动前拍一张（一直保留），之后最多每小时一张、留最近 20 张。删掉和被替换的文件在回收站留 30 天。"
                                : "每个账号的会话列表回到那时的样子。恢复前要先退出 Claude；恢复前会再拍一张，随时能再恢复回来。",
                             width: 578) {
            if restoring == nil {
                if items.isEmpty {
                    Text("还没有快照。第一次同步改动前会拍。").font(.system(size: 12)).foregroundStyle(Palette.ink3).padding(.vertical, 9)
                }
                ScrollView {
                    VStack(spacing: 0) {
                        ForEach(Array(items.enumerated()), id: \.element.id) { index, snapshot in
                            HStack(spacing: 12) {
                                Text(When.text(snapshot.at)).font(.system(size: 12)).monospacedDigit().help(When.exact(snapshot.at))
                                    .frame(width: 128, alignment: .leading)
                                Pill(snapshot.kindLabel, style: snapshot.kind == "baseline" ? .brand : .neutral, small: true)
                                Text("\(snapshot.folders) 个会话列表").font(.system(size: 11)).foregroundStyle(Palette.ink3).monospacedDigit()
                                Spacer()
                                Button("恢复") { restoring = snapshot }.buttonStyle(.pill(.secondary))
                                    .disabled(service.accountBusy || status?.busy == true)
                            }
                            .frame(minHeight: 36)
                            .overlay(alignment: .top) { if index > 0 { Rectangle().fill(Palette.divider).frame(height: 1) } }
                        }
                    }
                }
                .frame(maxHeight: 280)
            } else if status?.running == true {
                Banner(text: "Claude 桌面还开着。先在 Claude 里退出（⌘Q），再恢复。")
            }
            errorLine
        } actions: {
            if let snapshot = restoring {
                Button("返回") { restoring = nil }.buttonStyle(.pill(.secondary, height: 27))
                Button("恢复") {
                    Task {
                        await service.claudeDesktop("restore", snapshot: snapshot.id, confirm: true)
                        if service.accountError == nil { restoring = nil }
                    }
                }
                .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
                .disabled(service.accountBusy || status?.running == true)
            } else {
                Button("在访达中打开回收站 ›") { openTrash() }.buttonStyle(.pill(.link))
                Button("完成", action: close).keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            }
        }
    }

    // Removed and replaced files wait here for 30 days; it exists once the first one has.
    private func openTrash() {
        let trash = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/AuthManager/claude-desktop/trash", isDirectory: true)
        let target = FileManager.default.fileExists(atPath: trash.path) ? trash : trash.deletingLastPathComponent()
        NSWorkspace.shared.open(target)
    }

    // MARK: 移除、重命名

    private func confirmRemove(_ handle: String) -> some View {
        let target = account(handle)
        return SheetScaffold(title: "从列表移除 \(target?.title ?? "这个账号")？",
                             lead: "AuthBaton 会忘掉为它保存的 Claude 桌面登录。它的会话还在，其他账号照常同步。") {
            errorLine
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button("移除", role: .destructive) {
                Task {
                    await service.claudeDesktop("remove", account: handle, confirm: true)
                    if service.accountError == nil { close() }
                }
            }
            .buttonStyle(.pill(.destructive, height: 27))
            .disabled(service.accountBusy || target?.inUse != false)
        }
    }

    private func rename(_ handle: String) -> some View {
        let target = account(handle)
        return SheetScaffold(title: "给 \(target?.email ?? target?.display ?? "这个账号") 起个名字",
                             lead: "列表里显示这个名字。小写字母、数字和 - _ .，最多 24 个字；留空就显示邮箱。") {
            TextField("例如 work", text: $alias)
                .textFieldStyle(.roundedBorder)
                .onAppear { alias = target?.alias ?? "" }
            errorLine
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button("保存") {
                Task {
                    await service.claudeDesktop("alias", account: handle, label: alias.trimmingCharacters(in: .whitespaces))
                    if service.accountError == nil { close() }
                }
            }
            .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            .disabled(service.accountBusy)
        }
    }

    @ViewBuilder private var errorLine: some View {
        if let error = service.accountError {
            Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
        }
    }
}
