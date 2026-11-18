import AppKit
import SwiftUI

// Claude 桌面's own sheets: adding an account, following a switch, snapshots, removing and renaming a saved
// login. The page itself is the Claude page (ClaudeHome.swift). Ported from cc-same (MIT); the service does the work.
enum DesktopSheet: Identifiable {
    case progress, add, snapshots, remove(String), rename(String)

    var id: String {
        switch self {
        case .progress: "progress"
        case .add: "add"
        case .snapshots: "snapshots"
        case .remove(let handle): "remove-\(handle)"
        case .rename(let handle): "rename-\(handle)"
        }
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
        case .add: started ? AnyView(progress) : AnyView(confirmAdd)
        case .snapshots: snapshots
        case .remove(let handle): confirmRemove(handle)
        case .rename(let handle): rename(handle)
        }
    }

    private var confirmAdd: some View {
        SheetScaffold(title: "添加 Claude 桌面账号", lead: "Claude 会退出并打开登录页。用新账号登录就行，现在的登录会先存好。", width: 442) {
            VStack(alignment: .leading, spacing: 7) {
                CheckItem(kind: .system, text: "现在的登录原样存进 AuthBaton，随时能换回来")
                CheckItem(kind: .you, text: "在 Claude 登录页用新账号登录，走 Anthropic 自己的登录")
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
                                ? "第一次改动前拍一张（一直保留），之后最多每小时一张，留最近 20 张。删掉和被替换的文件在回收站留 30 天。"
                                : "每个账号的会话列表回到那时的样子。需要先退出 Claude；恢复前会再拍一张，随时能恢复回来。",
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
                Banner(text: "Claude 桌面还开着。先退出 Claude（⌘Q），再恢复。")
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
                             lead: "AuthBaton 会忘掉存好的 Claude 桌面登录。账号的会话还在，其他账号照常同步。") {
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
                             lead: "列表里显示这个名字。小写字母、数字和 - _ .，最多 24 个字符；留空就显示邮箱。") {
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
