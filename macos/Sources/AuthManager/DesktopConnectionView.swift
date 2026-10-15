import SwiftUI

// Codex 桌面: AuthManager and the desktop joined by a line that says whether hot switch can work.
struct DesktopPage: View {
    @EnvironmentObject private var service: ServiceController
    @State private var port = "9522"
    @State private var showAdvanced = false

    private var directory: AccountDirectory? { service.directory }
    private var desktop: DesktopConnection? { directory?.desktop }
    private var disabled: Bool { service.accountBusy || directory?.busy == true || directory?.recoveryNeeded == true }

    // Connected only means the debug port answers; taken over means AuthManager supplies the login.
    static func status(_ desktop: DesktopConnection) -> String {
        !desktop.enabled ? "热切换已关闭"
            : !desktop.connected ? "热切换已开启 · 没连上"
            : desktop.managed ? "热切换已开启 · 已连接 · 已接管" : "热切换已开启 · 已连接 · 未接管"
    }

    var body: some View {
        PageScaffold(title: "Codex 桌面", subtitle: Text(desktop.map(Self.status) ?? "正在读取桌面状态")) {
            if let error = service.accountError {
                Banner(text: error) {
                    Button("知道了") { service.clearAccountError() }.buttonStyle(.pill(.onAttention, height: 20))
                }
            }
            connection
            if desktop?.enabled == true && desktop?.connected != true { steps }
            hotSwitch
        }
        .onAppear { port = String(desktop?.port ?? 9522) }
    }

    // MARK: 连接

    private var connection: some View {
        let enabled = desktop?.enabled == true
        let connected = enabled && desktop?.connected == true
        return VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 0) {
                end(caption: "AuthBaton") { BrandTile(size: 48).shadow(color: .black.opacity(0.12), radius: 7.5, y: 7) }
                VStack(spacing: 9) {
                    RelayLine(style: connected ? .open : enabled ? .blocked : .idle, thickness: 3, arrow: false)
                    chip(enabled: enabled, connected: connected)
                }
                .padding(.top, 15)
                .frame(maxWidth: .infinity)
                end(caption: "Codex 桌面") {
                    Icon(.codex, size: 26)
                        .frame(width: 48, height: 48)
                        .background(Palette.card, in: RoundedRectangle(cornerRadius: 13, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 13, style: .continuous).strokeBorder(Palette.edge, lineWidth: 1))
                        .shadow(color: .black.opacity(0.08), radius: 7.5, y: 7)
                }
            }
            .frame(maxWidth: 527)
            Text("连上以后，换号不用退出 Codex 桌面；自动换号也靠它暂停和接续会话。")
                .font(.system(size: 12)).foregroundStyle(Palette.ink2).multilineTextAlignment(.center)
                .frame(maxWidth: 442)
                .padding(.top, 17)
            HStack(spacing: 9) {
                if !enabled {
                    Button("打开热切换") {
                        Task { await service.accountOperation("accounts.desktop", params: LoginParams(enabled: true)) }
                    }
                    .buttonStyle(.pill(.primary, height: 29))
                    .disabled(disabled || desktop?.busy == true)
                } else {
                    Button(connected ? "重新以调试模式启动" : "以调试模式重新打开") {
                        Task { await service.accountOperation("accounts.desktop_launch") }
                    }
                    .buttonStyle(.pill(connected ? .secondary : .primary, height: 29))
                    .disabled(disabled)
                    Button(showAdvanced ? "收起高级设置" : "高级：端口 \(desktop.map { String($0.port) } ?? "9522")") { showAdvanced.toggle() }
                        .buttonStyle(.pill(.quiet, height: 29))
                }
                if desktop?.busy == true { ProgressView().controlSize(.small) }
            }
            .padding(.top, 14)
            if enabled && showAdvanced { advanced.padding(.top, 12) }
        }
        .padding(.horizontal, 22).padding(.top, 22).padding(.bottom, 19)
        .frame(maxWidth: .infinity)
        .loopCard(radius: 19, raised: true)
    }

    private func end<Tile: View>(caption: String, @ViewBuilder tile: () -> Tile) -> some View {
        VStack(spacing: 7) {
            tile()
            Text(caption).font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
        }
        .frame(width: 82)
    }

    @ViewBuilder private func chip(enabled: Bool, connected: Bool) -> some View {
        if connected {
            Text(desktop?.managed == true ? "已连接 · 已接管" : "已连接 · 未接管")
                .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.deep)
                .padding(.horizontal, 9).frame(height: 20)
                .background(Palette.wash, in: Capsule())
                .help(desktop?.message ?? "")
        } else if enabled {
            Text(desktop?.message.isEmpty == false ? "没连上 · \(desktop?.message ?? "")" : "没连上")
                .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.attentionText)
                .lineLimit(1).truncationMode(.tail)
                .padding(.horizontal, 9).frame(height: 20)
                .background(Palette.attentionFill, in: Capsule())
                .help(desktop?.message ?? "")
        } else {
            Text("热切换关着").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                .padding(.horizontal, 9).frame(height: 20)
                .background(Palette.chip, in: Capsule())
        }
    }

    private var advanced: some View {
        HStack(spacing: 9) {
            Text("本机调试端口").font(.system(size: 11)).foregroundStyle(Palette.ink2)
            LoopField(placeholder: "9522", text: $port).frame(width: 77).monospacedDigit()
            Button("保存并连接") {
                if let number = UInt16(port), number >= 1024 {
                    Task { await service.accountOperation("accounts.desktop", params: LoginParams(port: number, enabled: true)) }
                }
            }
            .buttonStyle(.pill(.secondary, height: 27))
            .disabled(disabled || (UInt16(port).map { $0 < 1024 } ?? true))
            Text("1024–65535；改已接管桌面的端口前，要先退出桌面。").font(.system(size: 11)).foregroundStyle(Palette.ink3)
        }
    }

    // MARK: 步骤

    private var steps: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("点「以调试模式重新打开」后会这样做").font(.system(size: 13, weight: .semibold))
            HStack(alignment: .top, spacing: 14) {
                step("01", "退出 Codex 桌面", "只在桌面里没有任务在运行时退出；AuthBaton 不会替你结束任务。")
                step("02", "用调试模式打开", "AuthBaton 用调试端口 \(desktop.map { String($0.port) } ?? "9522") 重新打开桌面。")
                step("03", "连上后自动核对", "桌面账号和默认文件一致才算连上，之后就能热切换。")
            }
        }
        .padding(.horizontal, 17).padding(.top, 15).padding(.bottom, 17)
        .frame(maxWidth: .infinity, alignment: .leading)
        .loopCard()
    }

    private func step(_ number: String, _ title: String, _ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(number).font(.system(size: 12, weight: .bold, design: .rounded)).foregroundStyle(Palette.solid)
            Text(title).font(.system(size: 12, weight: .semibold)).padding(.top, 3.5)
            Text(detail).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true).padding(.top, 2)
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    // MARK: 热切换

    private var hotSwitch: some View {
        let enabled = desktop?.enabled == true
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("热切换").font(.system(size: 13, weight: .semibold))
                    Text("开着时，Codex 只通过桌面换号。关掉后改用普通切换：要先退出 Codex 才能换号，自动换号只会重置。")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 10)
                Toggle("热切换", isOn: Binding(get: { enabled }, set: { value in
                    Task { await service.accountOperation("accounts.desktop", params: LoginParams(enabled: value)) }
                }))
                .labelsHidden().toggleStyle(.switch).controlSize(.small)
                .disabled(disabled || desktop?.busy == true)
            }
            Text("热切换开着时，你照常打开的桌面会在启动完成、还没有任务时，自动以调试模式重开，不会停在退出确认上。AuthBaton 启动前就在用的桌面不会被重启，需要时在上面以调试模式打开一次。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                .padding(.top, 10)
                .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
                .padding(.top, 10)
        }
        .padding(.horizontal, 17).padding(.vertical, 15)
        .loopCard()
    }
}
