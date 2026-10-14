import AppKit
import SwiftUI

private func openCloud(_ address: String) {
    guard let url = URL(string: address), url.scheme == "https",
          let safari = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Safari") else { return }
    NSWorkspace.shared.open([url], withApplicationAt: safari, configuration: NSWorkspace.OpenConfiguration())
}

// 个人云端: the mode, the pairing with the user's own authentication centre, and its devices.
struct CloudPage: View {
    @EnvironmentObject private var service: ServiceController

    var body: some View {
        let cloud = service.directory?.cloud
        PageScaffold(title: "个人云端",
                     subtitle: Text("\(cloud?.mode == "cloud" ? "云端模式" : "本地模式") · \(cloud?.paired == true ? "已配对" : "还没配对")"),
                     spacing: 14) {
            CloudConnectionView()
        }
    }
}

struct CloudConnectionView: View {
    @EnvironmentObject private var service: ServiceController
    @State private var endpoint = ""
    @State private var deviceName = ""
    @State private var pendingMode: String?
    private var cloud: CloudStatus? { service.directory?.cloud }
    private var busy: Bool { service.accountBusy || cloud?.busy == true }

    var body: some View {
        modeCard
        if let message = cloud?.error ?? service.accountError { Banner(text: message) }
        pairCard
        modesCard
        if let cloud, !cloud.devices.isEmpty { devicesCard(cloud.devices) }
        Color.clear.frame(height: 0).onAppear { endpoint = cloud?.endpoint ?? ""; deviceName = cloud?.label ?? "我的 Mac" }
    }

    // Switching mode changes what the accounts page, menu bar, strategy and pi use, so it asks first.
    private var modeCard: some View {
        let mode = cloud?.mode ?? "local"
        return HStack(spacing: 14) {
            VStack(alignment: .leading, spacing: 2) {
                Text("运行模式").font(.system(size: 13, weight: .semibold))
                Text("切换模式不会上传账号，也不会切换正在运行的客户端。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            Spacer(minLength: 10)
            PillSegments(segments: [.init(value: "local", title: "本地"), .init(value: "cloud", title: "云端")],
                         selection: mode, select: { if $0 != mode { pendingMode = $0 } })
                .disabled(busy || service.directory?.busy == true)
        }
        .padding(.horizontal, 17).padding(.vertical, 15)
        .loopCard()
        .alert(pendingMode == "cloud" ? "换到云端模式？" : "换回本地模式？",
               isPresented: Binding(get: { pendingMode != nil }, set: { if !$0 { pendingMode = nil } })) {
            Button("取消", role: .cancel) { pendingMode = nil }
            Button("切换") {
                if let target = pendingMode { Task { await service.cloudAction("configure", mode: target) } }
                pendingMode = nil
            }
        } message: {
            Text(pendingMode == "cloud"
                 ? "切换后：账号页和菜单栏只显示云端账号\(cloud?.paired == true ? "" : "（还没配对，会是空的）")；自动换号与重置（包括自动重置）暂停；pi 的默认入口改用云端账号\(cloud?.paired == true ? "" : "，没配对时 pi 会提示先选账号")。本机账号、认证维护和额度查询照常，随时可以切回。"
                 : "切回后：账号页、菜单栏和 pi 恢复用本机账号，自动换号与重置几秒内按原规则恢复。云端账号保留。")
        }
    }

    private var pairCard: some View {
        let paired = cloud?.paired == true
        return VStack(alignment: .leading, spacing: 0) {
            Text("连到你自己的认证中心").font(.system(size: 13, weight: .semibold))
            HStack(alignment: .top, spacing: 0) {
                end("这台 Mac") {
                    Icon(.mac, size: 22, color: Palette.ink2)
                        .frame(width: 44, height: 44)
                        .background(Palette.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.edge, lineWidth: 1))
                }
                VStack(spacing: 7) {
                    RelayLine(style: paired ? .open : .idle, arrow: false)
                    Text(paired ? "已配对" : cloud?.pair != nil ? "等你在 Safari 里批准" : "还没配对")
                        .font(.system(size: 11)).foregroundStyle(paired ? Palette.deep : Palette.ink3)
                }
                .padding(.top, 14)
                .frame(maxWidth: .infinity)
                end("认证中心") {
                    Icon(.cloud, size: 22, color: Palette.solid)
                        .frame(width: 44, height: 44)
                        .background(Palette.wash, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                }
            }
            .frame(maxWidth: 442)
            .frame(maxWidth: .infinity)
            .padding(.top, 15)
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
            .padding(.top, 19)
            HStack(spacing: 9) {
                Button("保存连接") { Task { await service.cloudAction("configure", endpoint: endpoint, label: deviceName) } }
                    .buttonStyle(.pill(.primary, height: 29))
                if paired {
                    Button("管理已授权设备") { openCloud((cloud?.endpoint ?? "") + "/admin") }.buttonStyle(.pill(.secondary, height: 29))
                } else {
                    Button("发起设备配对") { Task { await service.cloudAction("pair") } }
                        .buttonStyle(.pill(.secondary, height: 29))
                        .disabled(cloud?.endpoint.isEmpty != false)
                    if cloud?.endpoint.isEmpty != false {
                        Text("先保存地址，才能配对。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    }
                }
                if busy { ProgressView().controlSize(.small) }
            }
            .padding(.top, 14)
            if let pair = cloud?.pair {
                VStack(spacing: 7) {
                    Text("在 Safari 里核对这个配对码，再批准设备").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    Text(pair.code).font(.system(size: 22, weight: .semibold, design: .monospaced)).kerning(2.6).textSelection(.enabled)
                    Text("有效期到 \(When.text(pair.expiresAt))").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    HStack(spacing: 9) {
                        Button("在 Safari 中授权设备") { openCloud(pair.url) }.buttonStyle(.pill(.primary, height: 27))
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
        .padding(.horizontal, 19).padding(.top, 19).padding(.bottom, 17)
        .loopCard(radius: 19, raised: true)
    }

    private func end<Tile: View>(_ caption: String, @ViewBuilder tile: () -> Tile) -> some View {
        VStack(spacing: 7) {
            tile()
            Text(caption).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
        }
        .frame(width: 78)
    }

    private var modesCard: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("两种模式").font(.system(size: 13, weight: .semibold))
            HStack(alignment: .top, spacing: 14) {
                mode("本地", "沿用这台 Mac 上的登录和维护，账号不离开本机。")
                mode("云端", "只用你主动导入的账号。中心在本机应用退出后也能续期；本机客户端的凭据同步仍要 AuthManager 在运行。")
            }
        }
        .padding(.horizontal, 17).padding(.top, 15).padding(.bottom, 17)
        .loopCard()
    }

    private func mode(_ title: String, _ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 3.5) {
            Text(title).font(.system(size: 12, weight: .semibold))
            Text(detail).font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 14).padding(.vertical, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .loopPanel()
    }

    private func devicesCard(_ devices: [CloudDevice]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("已授权设备").font(.system(size: 13, weight: .semibold)).padding(.bottom, 5)
            ForEach(devices) { device in
                HStack(spacing: 9) {
                    Icon(.mac, size: 14, color: Palette.ink2)
                    Text(device.label).font(.system(size: 12))
                    Spacer()
                    Text(device.lastSeen.map { "\(When.text($0)) 领取过" } ?? "还没领取过").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
                .frame(height: 34)
                .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
            }
        }
        .padding(.horizontal, 17).padding(.vertical, 15)
        .loopCard()
    }
}

// The accounts page in cloud mode: the accounts the centre keeps, and how this Mac uses them.
struct CloudAccountsView: View {
    @EnvironmentObject private var service: ServiceController
    var openSettings: () -> Void
    @State private var intent: CloudIntent?
    @State private var loginProvider: String?
    @State private var loginCloudAccountID: String?
    private var cloud: CloudStatus? { service.directory?.cloud }
    private var busy: Bool { service.accountBusy || cloud?.busy == true }

    var body: some View {
        if cloud?.paired != true {
            VStack(spacing: 12) {
                Icon(.cloud, size: 24, color: Palette.solid)
                    .frame(width: 44, height: 44)
                    .background(Palette.wash, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                Text("先连接个人认证中心").font(.system(size: 14, weight: .bold))
                Text("本地账号保持原样。连接后再选要导入的账号。").font(.system(size: 12)).foregroundStyle(Palette.ink2)
                Button("配置云端连接", action: openSettings).buttonStyle(.pill(.primary, height: 27))
            }
            .frame(maxWidth: .infinity).padding(.vertical, 34)
            .loopCard(radius: 19, raised: true)
        } else {
            HStack(spacing: 7) {
                Menu {
                    Button("先登录 Codex，再从已保存账号导入") { service.clearLogin(); loginCloudAccountID = nil; loginProvider = "codex" }
                    Button("先登录 Claude，再从已保存账号导入") { service.clearLogin(); loginCloudAccountID = nil; loginProvider = "claude" }
                    Divider()
                    Button("导入本机当前 Codex 登录") { intent = CloudIntent(action: "import_current", id: "codex", label: "本机当前 Codex 登录") }
                    Button("导入本机当前 Claude 登录") { intent = CloudIntent(action: "import_current", id: "claude", label: "本机当前 Claude 登录") }
                } label: {
                    menuLabel("添加账号", primary: true)
                }
                .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
                Menu {
                    ForEach(service.directory?.accounts.filter { $0.identity != nil } ?? []) { account in
                        Button(account.label) { intent = CloudIntent(action: "import", id: account.id, label: account.label) }
                            .disabled(cloud?.transfers.contains { $0.localId == account.id } == true)
                    }
                } label: {
                    menuLabel("导入已保存账号", primary: false)
                }
                .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
                Spacer()
                if busy { ProgressView().controlSize(.small) }
                Button("刷新状态") { Task { await service.cloudAction("refresh") } }.buttonStyle(.pill(.secondary, height: 27))
                Button("连接与设备", action: openSettings).buttonStyle(.pill(.secondary, height: 27))
            }
            .disabled(busy)
            if let error = cloud?.error ?? service.accountError { Banner(text: error) }
            if let pending = cloud?.pendingActivation {
                Banner(glyph: .sync, title: "上次原生切换没完成", text: "结束相关任务后恢复；不会重用旧的刷新凭据。") {
                    Button("恢复原生切换") { intent = CloudIntent(action: "activate", id: pending, label: "待恢复的云端账号") }
                        .buttonStyle(.pill(.onAttention, height: 20)).disabled(busy)
                }
            }
            ForEach(cloud?.transfers.filter { $0.phase != "cloud_managed" } ?? [], id: \.localId) { transfer in
                let name = service.directory?.accounts.first { $0.id == transfer.localId }?.label ?? "账号"
                Banner(glyph: .sync, text: "\(name) · 交接没完成") {
                    Button("恢复交接") { intent = CloudIntent(action: "resume_import", id: transfer.localId, label: name) }
                        .buttonStyle(.pill(.onAttention, height: 20))
                    Button("取消交接") { intent = CloudIntent(action: "cancel_import", id: transfer.localId, label: name) }
                        .buttonStyle(.pill(.onAttention, height: 20))
                }
                .disabled(busy)
            }
            ForEach(cloud?.accounts ?? []) { account in cloudCard(account) }
            if cloud?.accounts.isEmpty == true {
                Text("还没有云端账号。可以直接导入本机已有的登录，不用重新授权。").font(.system(size: 12)).foregroundStyle(Palette.ink3)
                    .frame(maxWidth: .infinity).padding(.vertical, 27)
            }
            Text("用量每 5 分钟单独查询。原生同步是实验功能：第一次启用要退出对应客户端。Claude 提前收到新凭据已有合成验证，401 后的原请求不保证自动重试；请让本机 AuthManager 保持运行。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
        }
        Color.clear.frame(height: 0)
            .alert(intent?.title ?? "确认操作", isPresented: Binding(get: { intent != nil }, set: { if !$0 { intent = nil } })) {
                Button("取消", role: .cancel) { intent = nil }
                Button("确认") { if let request = intent { Task { await perform(request) } }; intent = nil }
            } message: { Text(intent?.message ?? "") }
            .sheet(isPresented: Binding(get: { loginProvider != nil }, set: { if !$0 { loginProvider = nil } })) {
                LoginView(provider: loginProvider ?? "codex", cloudAccountID: loginCloudAccountID).environmentObject(service)
            }
    }

    private func menuLabel(_ title: String, primary: Bool) -> some View {
        HStack(spacing: 7) {
            Text(title).font(.system(size: 12, weight: .semibold)).foregroundStyle(primary ? .white : Palette.ink)
            Icon(.chevronDown, size: 9, color: primary ? .white : Palette.ink3, weight: 2.4)
        }
        .padding(.leading, 14).padding(.trailing, 10).frame(height: 27)
        .background(primary ? Palette.solid : Palette.card, in: Capsule())
        .overlay { if !primary { Capsule().strokeBorder(Palette.edge, lineWidth: 1) } }
        .contentShape(Capsule())
    }

    private func cloudCard(_ account: CloudAccount) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Seal(account.label, style: cloud?.selected.values.contains(account.id) == true ? .inUse : .standby, size: 31)
                VStack(alignment: .leading, spacing: 1) {
                    HStack(spacing: 7) {
                        Text(account.label).font(.system(size: 13, weight: .semibold)).lineLimit(1)
                        if cloud?.selected[account.provider] == account.id { Pill("本机在用", style: .brand, small: true) }
                        if cloud?.selected["codex_broker"] == account.id { Pill("pi 在用", style: .brand, small: true) }
                    }
                    Text("\(GlyphTile.name(account.provider)) · \(account.identity) · \(account.plan)").font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                }
                Spacer(minLength: 7)
                Pill(account.stateLabel, style: account.state == "ready" ? .neutral : .attention)
            }
            if let usage = account.usage, usage.checkedAt + 600 > Date().timeIntervalSince1970 {
                VStack(alignment: .leading, spacing: 9) {
                    ForEach(Array(usage.windows.enumerated()), id: \.offset) { _, window in
                        let remaining = Int(max(0, 100 - window.usedPercent).rounded(.down))
                        HStack(spacing: 10) {
                            Text(window.label).font(.system(size: 11)).foregroundStyle(Palette.ink2).lineLimit(1).frame(width: 77, alignment: .leading)
                            QuotaBar(remaining: remaining, height: 4.5)
                            Text("\(remaining)%").font(.system(size: 12, weight: .semibold, design: .rounded)).monospacedDigit()
                                .lineLimit(1).frame(width: 37, alignment: .trailing)
                            Text(window.resetsAt.map { When.countdown(to: Date(timeIntervalSince1970: $0)).map { "\($0)回满" } ?? "回满时间已过" } ?? "")
                                .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1).frame(width: 94, alignment: .leading)
                        }
                    }
                    Text("\(When.text(usage.checkedAt)) 查询").font(.system(size: 10)).foregroundStyle(Palette.ink3)
                }
            }
            if let error = account.usageError { Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText) }
            HStack(spacing: 7) {
                Button("检查用量") { Task { await service.cloudAction("usage", id: account.id) } }.buttonStyle(.pill(.secondary))
                Button("认证检查") { Task { await service.cloudAction("check", id: account.id) } }.buttonStyle(.pill(.secondary))
                if account.provider == "codex" {
                    Button("用于 pi") { Task { await service.cloudAction("select", id: account.id) } }.buttonStyle(.pill(.secondary))
                }
                Menu {
                    Button("重新登录并更新云端") { service.clearLogin(); loginCloudAccountID = account.id; loginProvider = account.provider }
                    Button("设为本机原生当前账号（实验）") { intent = CloudIntent(action: "activate", id: account.id, label: account.label) }
                    if cloud?.selected.values.contains(account.id) == true {
                        Button("停止本机使用") { intent = CloudIntent(action: "unselect", id: account.id, label: account.label) }
                    }
                    Divider()
                    Button("从云端移除", role: .destructive) { intent = CloudIntent(action: "remove", id: account.id, label: account.label) }
                } label: {
                    MoreDots(color: Palette.ink2).frame(width: 24, height: 24).background(Palette.field, in: Circle()).contentShape(Circle())
                }
                .menuStyle(.button).buttonStyle(.plain).menuIndicator(.hidden).fixedSize()
                .accessibilityLabel("\(account.label) 的更多操作")
                Spacer()
                Text("自动维护").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                Toggle("自动维护", isOn: Binding(get: { account.automatic }, set: { value in Task { await service.cloudAction("update", id: account.id, automatic: value) } }))
                    .labelsHidden().toggleStyle(.switch).controlSize(.small)
            }
            .disabled(busy || account.state == "awaiting_handoff")
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 3.5) {
                GridRow { Text("Access 到期").foregroundStyle(Palette.ink3); Text(When.text(account.expiresAt)) }
                if let expiry = account.refreshExpiresAt { GridRow { Text("登录期限").foregroundStyle(Palette.ink3); Text(When.text(expiry)) } }
                GridRow { Text("最近续期").foregroundStyle(Palette.ink3); Text("\(account.lastRefresh.map(When.text) ?? "还没续期过") · 凭据版本 \(account.version)") }
                if let error = account.error { GridRow { Text("维护状态").foregroundStyle(Palette.ink3); Text(error) } }
            }
            .font(.system(size: 11))
            .padding(.top, 10)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
        }
        .padding(.horizontal, 17).padding(.vertical, 15)
        .loopCard()
    }

    private func perform(_ request: CloudIntent) async {
        if request.action == "import_current" {
            await service.accountOperation("accounts.import", params: LoginParams(provider: request.id))
            guard service.accountError == nil, let current = service.directory?.accounts.first(where: { $0.provider == request.id && $0.native }) else { return }
            await service.cloudAction("import", id: current.id, confirm: true)
        } else { await service.cloudAction(request.action, id: request.id, confirm: true) }
    }
}

private struct CloudIntent {
    let action: String; let id: String; let label: String
    var title: String { action == "unselect" ? "停止本机使用？" : action == "remove" ? "移除云端账号？" : action == "cancel_import" ? "取消交接并恢复本地？" : action == "activate" ? "切换本机原生登录？" : "把账号交给云端维护？" }
    var message: String {
        switch action {
        case "unselect": "本机不再同步「\(label)」，并清除它的 pi 默认选择。已发出的短期凭据保留到过期；不会恢复旧的刷新凭据。之后可以选别的账号。"
        case "remove": "移除「\(label)」后，其他设备也不能再领取它的凭据。不会撤销上游已经发出的短期凭据。"
        case "activate": "把「\(label)」写入本机原生存储，使用云端提供的短期凭据。先结束任务并退出对应客户端；AuthManager 不会替你退出。"
        case "cancel_import": "只有云端还没接管时才能恢复本地维护；已轮换的旧凭据不会恢复。先退出对应客户端。"
        default: "把「\(label)」的登录凭据发到已配置的个人认证中心。确认其他机器没有单独使用同一份刷新凭据；如果它是本机在用的登录，先退出对应客户端。完成后本机不再单独续期，不用重新登录。"
        }
    }
}
