import AppKit
import SwiftUI

private func openCloud(_ address: String) {
    guard let url = URL(string: address), url.scheme == "https",
          let safari = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Safari") else { return }
    NSWorkspace.shared.open([url], withApplicationAt: safari, configuration: NSWorkspace.OpenConfiguration())
}

// 个人云端: how the accounts the user's own authentication centre renews are doing, the pairing with it, and the
// Macs it has authorised. Which account the centre renews is chosen on the account's own card.
struct CloudPage: View {
    @EnvironmentObject private var service: ServiceController
    @State private var sheet: AccountSheet?

    var body: some View {
        let cloud = service.directory?.cloud
        let renewed = service.directory?.accounts.filter(\.cloudRenewed) ?? []
        let sources = service.directory?.external?.sources?.count ?? 0
        PageScaffold(title: "个人云端", subtitle: Text(subtitle(cloud, renewed: renewed.count) + (sources > 0 ? " · \(sources) 个外部来源" : "")),
                     spacing: 12, narrow: true) {
            if let message = cloud?.error ?? service.accountError { Banner(text: message) }
            if cloud?.paired == true, !renewed.isEmpty { RenewalSummary(accounts: renewed) { sheet = $0 } }
            CloudConnectionView()
            if cloud?.paired == true, let cloud, !cloud.devices.isEmpty { DevicesCard(cloud: cloud) }
            // Another centre's accounts, read here with the access it lends (contract 3).
            ExternalSourceCard()
        }
        .sheet(item: $sheet) { item in AccountSheetView(item: item, close: { sheet = nil }).environmentObject(service) }
    }

    private func subtitle(_ cloud: CloudStatus?, renewed: Int) -> String {
        guard let cloud, cloud.paired else { return "还没配对" }
        return "已配对 · \(cloud.devices.count) 台设备 · \(renewed) 个云端续期账号"
    }
}

// The accounts the centre renews, as one summary: when it last renewed and when it renews next, then a line per
// account. Quiet while all is well; an account it could not renew turns amber with the way back.
private struct RenewalSummary: View {
    let accounts: [ManagedAccount]
    let open: (AccountSheet) -> Void

    var body: some View {
        let troubled = accounts.filter { $0.cloudTrouble != nil }.count
        let last = accounts.compactMap { $0.cloud?.lastRefresh }.max()
        let next = accounts.compactMap { $0.cloud?.nextRefresh }.min()
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text("云端续期账号").font(.system(size: 13, weight: .semibold))
                Spacer(minLength: 8)
                if troubled > 0 {
                    HStack(spacing: 4) {
                        Icon(.alert, size: 11, color: Palette.attentionText)
                        Text("\(troubled) 个异常")
                    }
                    .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.attentionText)
                    .padding(.horizontal, 8).frame(height: 20)
                    .background(Palette.attentionFill, in: Capsule())
                } else {
                    Text(count).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            }
            HStack(alignment: .top, spacing: 12) {
                metric("最近一次续期", last.map { When.text($0) } ?? "还没续期过")
                metric("下一次续期", next.map(nextText) ?? "—")
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .background(Palette.field, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            VStack(spacing: 0) {
                ForEach(Array(accounts.enumerated()), id: \.element.id) { index, account in
                    row(account)
                        .overlay(alignment: .top) {
                            if index > 0 && account.cloudTrouble == nil && accounts[index - 1].cloudTrouble == nil {
                                Rectangle().fill(Palette.divider).frame(height: 1)
                            }
                        }
                }
            }
        }
        .padding(16)
        .loopCard(radius: 16)
        .overlay { if troubled > 0 { RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Palette.attention, lineWidth: 1.5) } }
    }

    private var count: String {
        let codex = accounts.filter { $0.provider == "codex" }.count
        let claude = accounts.count - codex
        return "\(accounts.count) 个账号 · Codex \(codex) · Claude \(claude)"
    }

    private func nextText(_ at: Double) -> String {
        let date = Date(timeIntervalSince1970: at)
        return When.clock(date) + (When.countdown(to: date).map { "（约 \($0)）" } ?? "")
    }

    private func metric(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title).font(.system(size: 11)).foregroundStyle(Palette.ink3)
            Text(value).font(.system(size: 13, weight: .semibold)).monospacedDigit().lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder private func row(_ account: ManagedAccount) -> some View {
        if let trouble = account.cloudTrouble {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    tool(account.provider)
                    Text(accountName: account.displayName).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.attentionText).lineLimit(1)
                    CloudBadge(account: account, size: 11)
                    Spacer(minLength: 8)
                    Button("重新登录…") { open(.login(provider: account.provider, accountID: account.id)) }
                        .buttonStyle(.pill(.attention, height: 22))
                }
                HStack(alignment: .top, spacing: 6) {
                    Icon(.alert, size: 12, color: Palette.attention).padding(.top, 1)
                    Text(trouble).font(.system(size: 11)).foregroundStyle(Palette.attentionText).fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.horizontal, 10).padding(.vertical, 9)
            .background(Palette.attentionFill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .padding(.vertical, 4)
        } else {
            HStack(spacing: 8) {
                tool(account.provider)
                Seal(account.displayName, style: .standby, size: 16)
                Text(accountName: account.displayName).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                if let identity = account.identityLine {
                    Text(identity).font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                }
                Spacer(minLength: 8)
                Text(account.cloud?.lastRefresh.map { When.text($0) + "续期" } ?? "还没续期过")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).monospacedDigit().lineLimit(1)
            }
            .frame(minHeight: 36)
        }
    }

    // The tool as a small chip: its glyph and short name.
    private func tool(_ provider: String) -> some View {
        HStack(spacing: 4) {
            Icon(.provider(provider), size: 11)
            Text(provider == "codex" ? "Codex" : "Claude")
        }
        .font(.system(size: 11)).foregroundStyle(Palette.ink2)
        .padding(.horizontal, 7).frame(height: 20)
        .background(Palette.chip, in: Capsule())
    }
}

// The pairing: this Mac and the centre with what links them, the address and this Mac's name, and the way out.
struct CloudConnectionView: View {
    @EnvironmentObject private var service: ServiceController
    @State private var leaving = false
    @State private var endpoint = ""
    @State private var deviceName = ""
    private var cloud: CloudStatus? { service.directory?.cloud }
    private var busy: Bool { service.accountBusy || cloud?.busy == true }

    var body: some View {
        let paired = cloud?.paired == true
        let host = cloud.flatMap { URL(string: $0.endpoint)?.host }
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text("认证中心").font(.system(size: 13, weight: .semibold))
                Spacer(minLength: 8)
                if paired {
                    HStack(spacing: 5) {
                        Circle().fill(Palette.brand).frame(width: 5, height: 5)
                        Text("已配对")
                    }
                    .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.deep)
                    .padding(.horizontal, 8).frame(height: 20)
                    .background(Palette.wash, in: Capsule())
                }
            }
            CloudFlow(from: FlowEnd(title: service.directory?.thisMac ?? "这台 Mac", caption: "这台 Mac"),
                      to: FlowEnd(title: "认证中心", caption: host ?? "未连接", centre: true, dashed: !paired),
                      caption: paired ? "已连接" : cloud?.pair != nil ? "等你在 Safari 里批准" : "未配对", linked: paired)
                .padding(.top, 12)
            HStack(alignment: .top, spacing: 14) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("认证中心地址").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                    LoopField(placeholder: "https://authmanager-personal.你的子域.workers.dev", text: $endpoint).disabled(paired)
                }
                .frame(maxWidth: .infinity)
                VStack(alignment: .leading, spacing: 5) {
                    Text("设备名称").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                    LoopField(placeholder: "例如：我的 MacBook", text: $deviceName)
                }
                .frame(width: 204)
            }
            .padding(.top, 14)
            HStack(spacing: 9) {
                Button(paired ? "保存更改" : "保存连接") { Task { await service.cloudAction("configure", endpoint: endpoint, label: deviceName) } }
                    .buttonStyle(.pill(paired ? .secondary : .primary, height: 27))
                if paired {
                    Button("检查连接") { Task { await service.cloudAction("refresh") } }.buttonStyle(.pill(.secondary, height: 27))
                } else {
                    Button("配对设备") { Task { await service.cloudAction("pair") } }
                        .buttonStyle(.pill(.secondary, height: 27))
                        .disabled(cloud?.endpoint.isEmpty != false)
                    if cloud?.endpoint.isEmpty != false {
                        Text("先保存地址，才能配对。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                }
                if busy { ProgressView().controlSize(.small) }
                Spacer(minLength: 0)
                if paired {
                    Button("断开配对…") { leaving = true }.buttonStyle(.pill(.secondary, height: 27))
                }
            }
            .padding(.top, 14)
            if let pair = cloud?.pair {
                VStack(spacing: 7) {
                    Text("在 Safari 里核对配对码，再批准设备").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    Text(pair.code).font(.system(size: 22, weight: .semibold, design: .monospaced)).kerning(2.6).textSelection(.enabled)
                    Text("有效期到 \(When.text(pair.expiresAt))").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    HStack(spacing: 9) {
                        Button("在 Safari 里批准设备") { openCloud(pair.url) }.buttonStyle(.pill(.primary, height: 27))
                        Button("检查配对结果") { Task { await service.cloudAction("pair_poll") } }.buttonStyle(.pill(.secondary, height: 27))
                    }
                    .padding(.top, 3.5)
                }
                .frame(maxWidth: .infinity).padding(15)
                .loopPanel(radius: 14)
                .padding(.top, 14)
            }
        }
        .disabled(busy)
        .padding(16)
        .loopCard(radius: 16)
        .onAppear { endpoint = cloud?.endpoint ?? ""; deviceName = cloud?.label ?? "我的 Mac" }
        .sheet(isPresented: $leaving) { CloudLeaveSheet(close: { leaving = false }).environmentObject(service) }
    }
}

// The Macs the centre has authorised. Revoking one happens on the centre's own admin page, behind its login:
// devices cannot revoke each other.
private struct DevicesCard: View {
    let cloud: CloudStatus

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("已授权设备").font(.system(size: 13, weight: .semibold))
                Spacer()
                Text("\(cloud.devices.count) 台").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            .padding(.bottom, 4)
            ForEach(Array(cloud.devices.enumerated()), id: \.element.id) { index, device in
                HStack(spacing: 9) {
                    Icon(.mac, size: 14, color: Palette.ink2)
                        .frame(width: 26, height: 26)
                        .background(Palette.chip, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    Text(device.label).font(.system(size: 12, weight: .semibold)).lineLimit(1)
                    if device.this == true { Pill("这台", style: .brand, small: true) }
                    Spacer(minLength: 8)
                    if device.this == true {
                        HStack(spacing: 5) {
                            Circle().fill(Palette.brand).frame(width: 5, height: 5)
                            Text("在线")
                        }
                        .font(.system(size: 11)).foregroundStyle(Palette.deep)
                    } else {
                        Text(device.lastSeen.map { "\(When.text($0))在线" } ?? "还没在线过").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                        Button("取消授权") { openCloud(cloud.adminUrl ?? cloud.endpoint + "/admin") }
                            .buttonStyle(.pill(.secondary, height: 22))
                            .help("在认证中心的管理页里取消授权；设备之间不能互相撤销")
                    }
                }
                .frame(minHeight: 40)
                .overlay(alignment: .top) { if index > 0 { Rectangle().fill(Palette.divider).frame(height: 1) } }
            }
        }
        .padding(16)
        .loopCard(radius: 16)
    }
}
