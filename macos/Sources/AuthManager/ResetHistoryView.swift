import SwiftUI

// Every reset the strategy or a person started, newest first, as a timeline. Reading it never resets.
struct ResetHistoryView: View {
    @EnvironmentObject private var service: ServiceController
    @Environment(\.dismiss) private var dismiss
    @State private var filter: String?
    @State private var entries: [ResetHistoryEntry] = []
    @State private var nextCursor: String?
    @State private var total = 0
    @State private var loading = false
    @State private var error: String?

    init(accountID: String? = nil) {
        _filter = State(initialValue: accountID)
    }

    private var accounts: [ManagedAccount] {
        (service.directory?.accounts ?? []).filter { ["codex", "claude"].contains($0.provider) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 9) {
                Text("执行记录").font(.system(size: 14, weight: .bold))
                accountMenu
                Spacer()
                Button("刷新") { Task { await load(reset: true) } }.buttonStyle(.pill(.secondary)).disabled(loading)
                Button("完成") { dismiss() }.buttonStyle(.pill(.primary)).keyboardShortcut(.cancelAction)
            }
            Text("时间按本机时区，精确到秒。自动、手动和重试分开记；记录存在本机，重启后还在。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 7)
            if let error { Banner(text: error).padding(.top, 10) }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    if entries.isEmpty && !loading && error == nil {
                        Text("还没有执行记录").font(.system(size: 12)).foregroundStyle(Palette.ink3)
                            .frame(maxWidth: .infinity).padding(.vertical, 51)
                    }
                    ForEach(Array(entries.enumerated()), id: \.element.id) { index, entry in
                        // The name the account goes by today; a removed account keeps the label it was recorded with.
                        ResetHistoryRow(entry: entry, name: accounts.first { $0.id == entry.accountId }?.displayName ?? entry.accountLabel,
                                        last: index == entries.count - 1 && nextCursor == nil)
                    }
                    if loading { ProgressView().controlSize(.small).frame(maxWidth: .infinity).padding(.vertical, 10) }
                    if nextCursor != nil {
                        Button("加载更多") { Task { await load(reset: false) } }
                            .buttonStyle(.pill(.secondary)).disabled(loading).frame(maxWidth: .infinity).padding(.vertical, 7)
                    }
                }
                .padding(.top, 15)
            }
            Text("已显示 \(entries.count) / \(total) 条 · 查看记录不会发起重置")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 10)
                .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
        }
        .padding(.horizontal, 20).padding(.vertical, 19)
        .frame(width: 578, height: 476)
        .background(Palette.card)
        .task(id: filter) { await load(reset: true) }
    }

    private var accountMenu: some View {
        Menu {
            Picker("账号", selection: $filter) {
                Text("所有账号").tag(String?.none)
                ForEach(accounts) { Text("\($0.providerName) · \($0.displayName)").tag(Optional($0.id)) }
            }
            .pickerStyle(.inline).labelsHidden()
        } label: {
            HStack(spacing: 7) {
                Text(accounts.first { $0.id == filter }?.displayName ?? "所有账号").font(.system(size: 11, weight: .semibold)).lineLimit(1)
                Icon(.chevronDown, size: 9, color: Palette.ink3, weight: 2.4)
            }
            .padding(.leading, 10).padding(.trailing, 9).frame(height: 24)
            .background(Palette.field, in: Capsule())
            .contentShape(Capsule())
        }
        .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
    }

    private func load(reset: Bool) async {
        if loading && !reset { return }
        loading = true
        defer { loading = false }
        let account = filter
        do {
            let page = try await service.resetHistory(accountID: account, before: reset ? nil : nextCursor)
            // A newer filter started its own load; this page belongs to the old one.
            guard account == filter else { return }
            entries = reset ? page.entries : entries + page.entries
            nextCursor = page.nextCursor
            total = page.total
            error = nil
        } catch { self.error = error.localizedDescription }
    }
}

private struct ResetHistoryRow: View {
    let entry: ResetHistoryEntry
    let name: String
    let last: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(spacing: 3.5) {
                Seal(name, style: .standby, size: 26)
                if !last { Capsule().fill(Palette.soft).frame(width: 2).frame(maxHeight: .infinity) }
            }
            .frame(width: 26)
            VStack(alignment: .leading, spacing: 7) {
                HStack(spacing: 7) {
                    Text(name).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                    Pill(entry.sourceLabel, style: entry.source == "automatic" || entry.source == "manual" ? .neutral : .dashed, small: true)
                    if entry.retry { Pill("重试", small: true) }
                    outcome
                }
                .padding(.top, 3.5)
                Grid(alignment: .leading, horizontalSpacing: 9, verticalSpacing: 3.5) {
                    GridRow {
                        Text("时间").foregroundStyle(Palette.ink3)
                        Text(time).monospacedDigit()
                    }
                    if let trigger = entry.trigger {
                        GridRow {
                            Text("触发时").foregroundStyle(Palette.ink3)
                            Text("剩余 \(String(format: "%.1f", trigger.remainingPercent))% · 见底线 ≤ \(trigger.thresholdPercent)%").monospacedDigit()
                        }
                    }
                    if !entry.message.isEmpty {
                        GridRow {
                            Text(entry.source == "automatic" || entry.source == "manual" ? "结果" : "说明").foregroundStyle(Palette.ink3)
                            Text(entry.message).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                .font(.system(size: 11))
                .padding(.horizontal, 10).padding(.vertical, 9)
                .frame(maxWidth: .infinity, alignment: .leading)
                .loopPanel(radius: 10)
            }
            .padding(.bottom, 15)
        }
        // Sized to the details so the timeline line runs the full height of the entry.
        .fixedSize(horizontal: false, vertical: true)
    }

    // Succeeded is the brand chip, unconfirmed is amber, everything else stays grey.
    @ViewBuilder private var outcome: some View {
        if ["reset", "already_redeemed"].contains(entry.outcome) {
            HStack(spacing: 3.5) {
                Icon(.check, size: 9, color: Palette.deep, weight: 2.2)
                Text(entry.outcomeLabel)
            }
            .font(.system(size: 10, weight: .semibold)).foregroundStyle(Palette.deep)
            .padding(.horizontal, 7).frame(height: 17)
            .background(Palette.wash, in: Capsule())
        } else {
            Pill(entry.outcomeLabel, style: entry.outcome == "unknown" ? .attention : .neutral, small: true)
        }
    }

    private var time: String {
        let start = ResetHistoryEntry.time(entry.startedAt)
        guard let ended = entry.finishedAt else {
            return start + (entry.outcome == "pending" ? " 开始 · 等待结果" : " 开始 · 没记下结果时间")
        }
        let end = Date(timeIntervalSince1970: ended)
        let sameDay = Calendar.current.isDate(end, inSameDayAs: Date(timeIntervalSince1970: entry.startedAt))
        return start + " → " + (sameDay ? end.formatted(.dateTime.hour(.twoDigits(amPM: .omitted)).minute(.twoDigits).second(.twoDigits))
                                        : ResetHistoryEntry.time(ended))
    }
}
