import SwiftUI

struct ResetHistoryView: View {
    @EnvironmentObject private var service: ServiceController
    @Environment(\.dismiss) private var dismiss
    var accountID: String? = nil
    var accountLabel: String? = nil
    @State private var entries: [ResetHistoryEntry] = []
    @State private var nextCursor: String?
    @State private var total = 0
    @State private var loading = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("重置执行记录").font(.title2.weight(.semibold))
                Spacer()
                Button("刷新") { Task { await load(reset: true) } }.disabled(loading)
                Button("完成") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            Text(accountLabel ?? "所有 Codex 账号").font(.callout).foregroundStyle(.secondary)
            Text("时间按本机时区显示，精确到秒。自动、手动及重试分别留痕；记录保存于本机，重启后保留。")
                .font(.caption).foregroundStyle(.secondary)
            if let error { Text(error).font(.caption).foregroundStyle(Palette.warning) }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if entries.isEmpty && !loading && error == nil {
                        Text("暂无执行记录").foregroundStyle(.secondary).padding(.vertical, 40)
                    }
                    ForEach(entries) { ResetHistoryRow(entry: $0) }
                    if loading { ProgressView("正在读取记录…").frame(maxWidth: .infinity) }
                    if nextCursor != nil {
                        Button("加载更多") { Task { await load(reset: false) } }
                            .disabled(loading).frame(maxWidth: .infinity)
                    }
                }
            }
            Text("已显示 \(entries.count) / \(total) 条 · 查看记录不会发起重置")
                .font(.caption).foregroundStyle(.secondary)
        }
        .padding(24).frame(width: 680, height: 600)
        .task { await load(reset: true) }
    }
    private func load(reset: Bool) async {
        guard !loading else { return }
        loading = true
        defer { loading = false }
        do {
            let page = try await service.resetHistory(accountID: accountID, before: reset ? nil : nextCursor)
            entries = reset ? page.entries : entries + page.entries
            nextCursor = page.nextCursor
            total = page.total
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

struct ResetHistoryRow: View {
    let entry: ResetHistoryEntry
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(entry.accountLabel).font(.callout.weight(.semibold))
                Text(entry.sourceLabel + (entry.retry ? " · 重试" : "")).font(.caption).foregroundStyle(.secondary)
                Spacer()
                StatusPill(text: entry.outcomeLabel,
                    tone: ["reset", "already_redeemed"].contains(entry.outcome) ? .success : entry.outcome == "unknown" ? .warning : .neutral)
            }
            Text("开始：\(ResetHistoryEntry.time(entry.startedAt))").font(.caption).monospacedDigit()
            if let ended = entry.finishedAt {
                Text("结果：\(ResetHistoryEntry.time(ended))").font(.caption).monospacedDigit().foregroundStyle(.secondary)
            } else {
                Text(entry.outcome == "pending" ? "等待执行结果" : "未记录结果返回时间")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if let trigger = entry.trigger {
                Text("触发时剩余 \(trigger.remainingPercent, specifier: "%.1f")% · 重置阈值 ≤ \(trigger.thresholdPercent)%")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Text(entry.message).font(.caption).foregroundStyle(.secondary)
        }.padding(14).frame(maxWidth: .infinity, alignment: .leading).card()
    }
}
