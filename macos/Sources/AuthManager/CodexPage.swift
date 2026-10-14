import SwiftUI

// Codex: every account as a card, in relay order, with its rule and its actions on the footer.
struct CodexPage: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @State private var sheet: AccountSheet?

    var body: some View {
        Group {
            if let directory = service.directory {
                let accounts = directory.relay("codex")
                ScrollViewReader { proxy in
                    PageScaffold(title: "Codex", subtitle: Text(subtitle(directory, accounts)), provider: "codex") {
                        RefreshPill(checkedAt: accounts.compactMap { $0.usage?.checkedAt }.max(), busy: service.isRefreshing) {
                            Task {
                                await service.refresh()
                                await service.refreshQuotaIfNeeded()
                            }
                        }
                        Button { sheet = .login(provider: "codex", accountID: nil) } label: {
                            HStack(spacing: 5) {
                                Icon(.plus, size: 12, color: .white, weight: 2)
                                Text("添加 Codex 账号")
                            }
                        }
                        .buttonStyle(.pill(.primary, height: 27))
                        .disabled(service.accountBusy || directory.busy || directory.recoveryNeeded)
                    } content: {
                        if directory.recoveryNeeded { RecoveryBanner() }
                        if directory.desktopBlocked {
                            Banner(glyph: .desktop, text: directory.strategy?.config.enabled == true
                                   ? "Codex 桌面没连上，现在换不了号，见底时只会重置。" : "Codex 桌面没连上，现在换不了号。") {
                                Button("去连接") { nav.show(.desktop) }.buttonStyle(.pill(.onAttention, height: 20))
                            }
                        }
                        if accounts.isEmpty {
                            AddCard(provider: "codex", open: { sheet = $0 }, lead: "还没有 Codex 账号", detail: "登录一个，或导入这台 Mac 上已有的登录。")
                        } else {
                            // Two cards a row, each row as tall as its taller card so the footers line up.
                            Grid(horizontalSpacing: 14, verticalSpacing: 14) {
                                ForEach(Array(stride(from: 0, to: accounts.count, by: 2)), id: \.self) { start in
                                    GridRow {
                                        ForEach(accounts[start..<min(start + 2, accounts.count)]) { account in
                                            CodexCard(account: account, directory: directory, selected: nav.selection == account.id) { sheet = $0 }
                                                .frame(maxHeight: .infinity, alignment: .top)
                                                .id(account.id)
                                        }
                                        if start + 1 == accounts.count { Color.clear.frame(maxWidth: .infinity, maxHeight: 1) }
                                    }
                                }
                            }
                            if directory.strategy != nil {
                                HStack(spacing: 0) {
                                    Text("卡片按接力顺序排；改顺序和规则去")
                                    Button("换号与重置") { nav.show(.automation) }.buttonStyle(.pill(.link))
                                    Text("。")
                                }
                                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                            }
                        }
                    }
                    .onAppear { if let id = nav.selection { proxy.scrollTo(id, anchor: .center) } }
                    .onChange(of: nav.selection) { _, id in if let id { withAnimation { proxy.scrollTo(id, anchor: .center) } } }
                }
            } else {
                Color.clear
            }
        }
        .task { await service.refreshQuotaIfNeeded() }
        .sheet(item: $sheet) { item in
            AccountSheetView(item: item, close: { sheet = nil }).environmentObject(service)
        }
    }

    private func subtitle(_ directory: AccountDirectory, _ accounts: [ManagedAccount]) -> String {
        var parts = ["\(accounts.count) 个账号"]
        if let current = directory.current("codex") { parts.append("在用 \(current.displayName)") }
        if service.snapshot?.providers.first(where: { $0.id == "pi" })?.state == "configured" { parts.append("pi 跟着在用的账号走") }
        return parts.joined(separator: " · ")
    }
}

private struct CodexCard: View {
    let account: ManagedAccount
    let directory: AccountDirectory
    let selected: Bool
    let open: (AccountSheet) -> Void

    var body: some View {
        let rule = directory.rule(for: account.id)
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 9) {
                    Seal(account.displayName, style: account.native ? .inUse : rule?.participate == false ? .out : .standby, size: 31)
                    VStack(alignment: .leading, spacing: 1) {
                        HStack(spacing: 7) {
                            Text(account.displayName).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                            if account.native { Pill("在用", style: .brand, small: true) }
                            if !account.native, directory.desktop?.selected == account.id { Pill("桌面在用", style: .brand, small: true) }
                        }
                        Text(account.subtitle.isEmpty ? " " : account.subtitle).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                    }
                    Spacer(minLength: 7)
                    AccountMenu(account: account, directory: directory, open: open)
                }
                QuotaFigures(account: account, notch: directory.notch(for: account), size: 36, inlineCaption: true) {
                    ChancesTicket(account: account)
                }
                .padding(.top, 14)
                if !account.extraWindows.isEmpty {
                    Text("独立额度：" + account.extraWindows.map { "\($0.scope ?? $0.label) 剩余 \($0.remaining)%" }.joined(separator: "，"))
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                        .help("模型或功能单独的额度，不参与自动换号判断")
                        .padding(.top, 9)
                }
            }
            .padding(.horizontal, 17).padding(.top, 15).padding(.bottom, 14)
            Spacer(minLength: 0)
            HStack(spacing: 5) {
                if let rule {
                    if let rank = directory.rank(of: account) { Pill("第 \(rank) 位", style: rule.participate ? .neutral : .dashed) }
                    Pill("≤ \(rule.below)%", style: rule.participate ? .neutral : .dashed)
                    Pill(rule.order.label, style: rule.participate ? .neutral : .dashed).help(rule.order.explanation)
                    if !rule.participate { Pill("不参与") }
                } else if directory.strategy != nil {
                    Pill("还没有规则", style: .dashed).help("在「换号与重置」保存一次规则后加入接力")
                }
                Spacer(minLength: 7)
                AccountActions(account: account, directory: directory, open: open)
            }
            .padding(.horizontal, 17).padding(.vertical, 10)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
            AccountStrips(account: account, directory: directory, open: open, desktop: .takeOver).padding(.horizontal, 17)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopCard(radius: 19, raised: account.native)
        .overlay {
            if selected { RoundedRectangle(cornerRadius: 19, style: .continuous).strokeBorder(Palette.brand, lineWidth: 1.5) }
        }
    }
}
