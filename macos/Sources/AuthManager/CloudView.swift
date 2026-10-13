import AppKit
import SwiftUI

private func openCloud(_ address: String) {
    guard let url = URL(string: address), url.scheme == "https",
          let safari = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Safari") else { return }
    NSWorkspace.shared.open([url], withApplicationAt: safari, configuration: NSWorkspace.OpenConfiguration())
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
        VStack(alignment: .leading, spacing: 16) {
            Text("认证中心地址").font(.headline)
            TextField("https://authmanager-personal.你的子域.workers.dev", text: $endpoint).textFieldStyle(.roundedBorder)
                .disabled(cloud?.paired == true)
            Text("设备名称").font(.headline)
            TextField("例如：我的 MacBook", text: $deviceName).textFieldStyle(.roundedBorder)
            HStack {
                Button("保存连接") { Task { await service.cloudAction("configure", endpoint: endpoint, label: deviceName) } }
                if cloud?.paired == true {
                    Label("设备已配对", systemImage: "checkmark.shield.fill").foregroundStyle(Palette.success)
                    Button("管理已授权设备") { openCloud((cloud?.endpoint ?? "") + "/admin") }
                } else {
                    Button("发起设备配对") { Task { await service.cloudAction("pair") } }.buttonStyle(.borderedProminent)
                        .disabled(cloud?.endpoint.isEmpty != false)
                }
            }
            if let pair = cloud?.pair {
                Divider()
                Text("在 Safari 中核对这个配对码，再批准设备：")
                Text(pair.code).font(.system(.title2, design: .monospaced).weight(.semibold)).textSelection(.enabled)
                Text("有效期至 \(When.text(pair.expiresAt))").font(.caption).foregroundStyle(.secondary)
                Button("在 Safari 中授权设备") { openCloud(pair.url) }
                Button("检查配对结果") { Task { await service.cloudAction("pair_poll") } }
            }
            if busy { ProgressView("正在处理…").controlSize(.small) }
        }.card().disabled(busy)
        if let message = cloud?.error ?? service.accountError { Callout(icon: "exclamationmark.circle", tone: .warning, message: message) }
        VStack(alignment: .leading, spacing: 8) {
            Text("两种模式").font(.headline)
            Text("本地模式沿用本机登录与维护。云端模式只使用你主动导入的账号；切换模式不会上传全部账号，也不会切换正在运行的客户端。")
            Text("云端接管后，中心可在本机应用退出后继续续期。本机原生客户端的凭据同步仍需 AuthManager 保持运行。")
        }.font(.callout).foregroundStyle(.secondary).card()
        if let cloud, !cloud.devices.isEmpty {
            VStack(alignment: .leading, spacing: 12) {
                Text("已授权设备").font(.headline)
                ForEach(cloud.devices) { device in HStack { Label(device.label, systemImage: "desktopcomputer"); Spacer(); Text(device.lastSeen.map(When.text) ?? "尚未领取").font(.caption).foregroundStyle(.secondary) } }
            }.card()
        }
        Color.clear.frame(height: 0).onAppear { endpoint = cloud?.endpoint ?? ""; deviceName = cloud?.label ?? "我的 Mac" }
    }

    // Switching mode changes what the accounts page, menu bar, strategy and pi use, so it asks first.
    private var modeCard: some View {
        let mode = cloud?.mode ?? "local"
        return HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 3) {
                Text("运行模式").font(.headline)
                Text("现在是\(mode == "cloud" ? "云端" : "本地")模式。切换模式不会上传账号，也不会切换正在运行的客户端。")
                    .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            Picker("模式", selection: Binding(get: { mode }, set: { if $0 != mode { pendingMode = $0 } })) {
                Text("本地").tag("local")
                Text("云端").tag("cloud")
            }
            .pickerStyle(.segmented).labelsHidden().fixedSize()
            .disabled(busy || service.directory?.busy == true)
        }
        .card()
        .alert(pendingMode == "cloud" ? "切换到云端模式？" : "切回本地模式？",
               isPresented: Binding(get: { pendingMode != nil }, set: { if !$0 { pendingMode = nil } })) {
            Button("取消", role: .cancel) { pendingMode = nil }
            Button("切换") {
                if let target = pendingMode { Task { await service.cloudAction("configure", mode: target) } }
                pendingMode = nil
            }
        } message: {
            Text(pendingMode == "cloud"
                 ? "切换后：账号页和菜单栏只显示云端账号\(cloud?.paired == true ? "" : "（还没有配对，会是空的）")；Codex 自动换号与重置（包括自动重置）暂停；pi 的默认入口改用云端账号\(cloud?.paired == true ? "" : "，未配对时 pi 会提示先选择账号")。本机账号、认证维护和额度查询照常，随时可以切回。"
                 : "切回后：账号页、菜单栏和 pi 恢复使用本机账号，自动换号与重置在几秒内按原规则恢复。云端账号保留。")
        }
    }
}

struct CloudAccountsView: View {
    @EnvironmentObject private var service: ServiceController
    var openSettings: () -> Void
    @State private var intent: CloudIntent?
    @State private var loginProvider: String?
    @State private var loginCloudAccountID: String?
    private var cloud: CloudStatus? { service.directory?.cloud }
    private var busy: Bool { service.accountBusy || cloud?.busy == true }
    var body: some View {
        Text("账号保存在你的个人认证中心，设备按需取得可用凭据。").font(.callout).foregroundStyle(.secondary)
        if cloud?.paired != true {
            VStack(spacing: 16) {
                Image(systemName: "cloud.badge.person.crop").font(.system(size: 40)).foregroundStyle(Palette.accent)
                Text("先连接个人认证中心").font(.title3.weight(.semibold))
                Text("本地账号保持原样。连接后再选择要导入的账号。").foregroundStyle(.secondary)
                Button("配置云端连接", action: openSettings).buttonStyle(.borderedProminent)
            }.frame(maxWidth: .infinity).padding(32).card()
        } else {
            HStack {
                Menu("添加账号") {
                    Button("先登录 Codex，再从已保存账号导入") { service.clearLogin(); loginCloudAccountID = nil; loginProvider = "codex" }
                    Button("先登录 Claude，再从已保存账号导入") { service.clearLogin(); loginCloudAccountID = nil; loginProvider = "claude" }
                    Divider()
                    Button("导入本机当前 Codex 登录") { intent = CloudIntent(action: "import_current", id: "codex", label: "本机当前 Codex 登录") }
                    Button("导入本机当前 Claude 登录") { intent = CloudIntent(action: "import_current", id: "claude", label: "本机当前 Claude 登录") }
                }
                Menu("导入已保存账号") {
                    ForEach(service.directory?.accounts.filter { $0.identity != nil } ?? []) { account in
                        Button(account.label) { intent = CloudIntent(action: "import", id: account.id, label: account.label) }
                            .disabled(cloud?.transfers.contains { $0.localId == account.id } == true)
                    }
                }
                Spacer()
                Button("刷新状态") { Task { await service.cloudAction("refresh") } }
                Button("连接与设备", action: openSettings)
                if busy { ProgressView().controlSize(.small) }
            }.disabled(busy)
            if let error = cloud?.error ?? service.accountError { Callout(icon: "exclamationmark.circle", tone: .warning, message: error) }
            if let pending = cloud?.pendingActivation {
                Callout(icon: "arrow.triangle.2.circlepath", tone: .warning, message: "上次原生切换尚未完成。请结束相关任务后恢复；不会重用旧刷新凭据。")
                Button("恢复原生切换") { intent = CloudIntent(action: "activate", id: pending, label: "待恢复的云端账号") }.disabled(busy)
            }
            ForEach(cloud?.transfers.filter { $0.phase != "cloud_managed" } ?? [], id: \.localId) { transfer in
                let name = service.directory?.accounts.first { $0.id == transfer.localId }?.label ?? "账号"
                HStack {
                    Label("\(name) · 交接未完成", systemImage: "arrow.triangle.2.circlepath")
                    Spacer()
                    Button("恢复交接") { intent = CloudIntent(action: "resume_import", id: transfer.localId, label: name) }
                    Button("取消交接") { intent = CloudIntent(action: "cancel_import", id: transfer.localId, label: name) }
                }.card().disabled(busy)
            }
            ForEach(cloud?.accounts ?? []) { account in cloudCard(account) }
            if cloud?.accounts.isEmpty == true { Text("尚无云端账号。可以直接导入本机已有登录，无需重新授权。").foregroundStyle(.secondary).padding(.vertical, 32) }
            Text("用量每 5 分钟独立查询。原生同步为实验功能：首次启用需要退出对应客户端。Claude 提前接收新凭据已有合成验证，401 后的原请求不保证自动重试；请保持本机 AuthManager 运行。").font(.caption).foregroundStyle(.secondary)
        }
        Color.clear.frame(height: 0)
            .alert(intent?.title ?? "确认操作", isPresented: Binding(get: { intent != nil }, set: { if !$0 { intent = nil } })) {
                Button("取消", role: .cancel) { intent = nil }
                Button("确认") { if let request = intent { Task { await perform(request) } }; intent = nil }
            } message: { Text(intent?.message ?? "") }
            .sheet(isPresented: Binding(get: { loginProvider != nil }, set: { if !$0 { loginProvider = nil } })) { LoginView(provider: loginProvider ?? "codex", cloudAccountID: loginCloudAccountID).environmentObject(service) }
    }
    private func cloudCard(_ account: CloudAccount) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                VStack(alignment: .leading, spacing: 5) {
                    Text(account.label).font(.title3.weight(.semibold))
                    Text("\(account.identity) · \(account.plan)").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Text(account.stateLabel).font(.caption.weight(.medium)).foregroundStyle(account.state == "ready" ? Palette.success : Palette.warning)
            }
            if cloud?.selected[account.provider] == account.id { Label("本机原生当前账号", systemImage: "desktopcomputer").font(.caption).foregroundStyle(Palette.accent) }
            if cloud?.selected["codex_broker"] == account.id { Label("pi 当前账号", systemImage: "terminal").font(.caption).foregroundStyle(Palette.accent) }
            if let usage = account.usage, usage.checkedAt + 600 > Date().timeIntervalSince1970 {
                ForEach(Array(usage.windows.enumerated()), id: \.offset) { _, window in
                    HStack { Text(window.label); Spacer(); Text("剩余 \(Int(100 - window.usedPercent))%") }.font(.caption)
                    ProgressView(value: 100 - window.usedPercent, total: 100).tint(Palette.accent)
                    if let reset = window.resetsAt { Text("重置于 \(When.text(reset))").font(.caption2).foregroundStyle(.secondary) }
                }
                Text("\(When.text(usage.checkedAt)) 查询").font(.caption2).foregroundStyle(.secondary)
            }
            if let error = account.usageError { Text(error).font(.caption).foregroundStyle(Palette.warning) }
            HStack {
                Button("检查用量") { Task { await service.cloudAction("usage", id: account.id) } }
                Button("认证检查") { Task { await service.cloudAction("check", id: account.id) } }
                if account.provider == "codex" { Button("用于 pi") { Task { await service.cloudAction("select", id: account.id) } } }
                Menu("更多") {
                    Button("重新登录并更新云端") { service.clearLogin(); loginCloudAccountID = account.id; loginProvider = account.provider }
                    Button("设为本机原生当前账号（实验）") { intent = CloudIntent(action: "activate", id: account.id, label: account.label) }
                    if cloud?.selected.values.contains(account.id) == true {
                        Button("停止本机使用") { intent = CloudIntent(action: "unselect", id: account.id, label: account.label) }
                    }
                    Button("从云端移除", role: .destructive) { intent = CloudIntent(action: "remove", id: account.id, label: account.label) }
                }
                Spacer()
                Toggle("自动维护", isOn: Binding(get: { account.automatic }, set: { value in Task { await service.cloudAction("update", id: account.id, automatic: value) } })).toggleStyle(.switch).controlSize(.small)
            }.disabled(busy || account.state == "awaiting_handoff")
            DisclosureGroup("详情") {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Access 到期：\(When.text(account.expiresAt))")
                    if let expiry = account.refreshExpiresAt { Text("登录期限：\(When.text(expiry))") }
                    Text("最近续期：\(account.lastRefresh.map(When.text) ?? "尚未续期") · 凭据版本 \(account.version)")
                    if let error = account.error { Text("维护状态：\(error)") }
                }.font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).padding(.top, 8)
            }
        }.card()
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
    var title: String { action == "unselect" ? "停止本机使用？" : action == "remove" ? "移除云端账号？" : action == "cancel_import" ? "取消交接并恢复本地？" : action == "activate" ? "切换本机原生登录？" : "将账号交给云端维护？" }
    var message: String {
        switch action {
        case "unselect": "停止本机继续同步「\(label)」，并清除它的 pi 默认选择。已发出的短期凭据保留到过期；不会恢复旧刷新凭据。可随后选择其他账号。"
        case "remove": "移除「\(label)」后，其他设备也无法再领取它的凭据。不会撤销上游已经发出的短期凭据。"
        case "activate": "将「\(label)」写入本机原生存储，使用云端提供的短期凭据。请先结束任务并退出对应客户端；AuthManager 不会替你退出。"
        case "cancel_import": "只有云端尚未接管时才能恢复本地维护；已轮换的旧凭据不会被恢复。请先退出对应客户端。"
        default: "将「\(label)」的登录凭据发送到已配置的个人认证中心。请确认其他机器没有独立使用同一份刷新凭据；若它是本机当前登录，先退出对应客户端。完成后本机停止独立续期，无需重新登录。"
        }
    }
}
