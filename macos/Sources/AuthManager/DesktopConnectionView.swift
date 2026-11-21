import SwiftUI

// Codex 桌面连接: everything the old Codex 桌面 page had, in a sheet behind the Codex page's header capsule.
// AuthBaton and the desktop joined by a line that says whether hot switch can work, the one action that fixes
// it, the debug port, the three steps a relaunch takes, and the hot switch itself.
struct CodexConnectSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void
    @State private var port = "9522"
    @State private var editingPort = false
    @State private var takingOver: ManagedAccount?

    private var directory: AccountDirectory? { service.directory }
    private var desktop: DesktopConnection? { directory?.desktop }
    private var current: ManagedAccount? { directory?.current("codex") }
    private var disabled: Bool { service.accountBusy || directory?.busy == true || directory?.recoveryNeeded == true }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Text("Codex 桌面连接").font(.system(size: 15, weight: .bold))
                Spacer(minLength: 10)
                if desktop?.busy == true { ProgressView().controlSize(.small) }
                Button(action: close) {
                    Icon(.close, size: 9, color: Palette.ink2, weight: 1.8)
                        .frame(width: 22, height: 22)
                        .background(Palette.chip, in: Circle())
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .keyboardShortcut(.cancelAction)
                .accessibilityLabel("关闭")
            }
            if let error = service.accountError { Banner(text: error).padding(.top, 12) }
            if let desktop {
                bridge(desktop).padding(.top, 14)
                if desktop.enabled && !desktop.connected { steps.padding(.top, 14) }
                hotSwitch(desktop).padding(.top, 16)
            } else {
                ProgressView("正在读取桌面状态…").frame(maxWidth: .infinity).padding(.vertical, 34)
            }
        }
        .padding(20)
        .frame(width: 476, alignment: .leading)
        .background(Palette.card)
        .onAppear { port = String(desktop?.port ?? 9522) }
        .sheet(item: $takingOver) { account in
            SwitchSheet(account: account, close: { takingOver = nil }).environmentObject(service)
        }
    }

    // MARK: 连接桥

    private func bridge(_ desktop: DesktopConnection) -> some View {
        let wiring = DesktopWiring(desktop, current: current)
        return VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 0) {
                end(caption: "AuthBaton") { BrandTile(size: 40).shadow(color: .black.opacity(0.12), radius: 6, y: 5) }
                VStack(spacing: 7) {
                    RelayLine(style: wiring.plain ? .idle : wiring.attention ? .blocked : .open, thickness: 2, arrow: false)
                    chip(wiring)
                }
                .padding(.top, 13)
                .frame(maxWidth: .infinity)
                end(caption: "Codex 桌面") {
                    Icon(.codex, size: 22)
                        .frame(width: 40, height: 40)
                        .background(Palette.card, in: RoundedRectangle(cornerRadius: 11, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 11, style: .continuous).strokeBorder(Palette.edge, lineWidth: 1))
                        .shadow(color: .black.opacity(0.08), radius: 6, y: 5)
                }
            }
            Text(wiring.plain ? "热切换关着：换号前要先退出 Codex 桌面。打开下面的热切换后再连接。"
                 : wiring.ownLogin ? "桌面还在用自己的登录；接管后账号不变，改由 AuthBaton 提供登录。"
                 : wiring.lost && !desktop.message.isEmpty ? desktop.message
                 : "连上后，换号不用退出桌面；自动换号也靠它暂停和接续会话。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink2).multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: 360)
                .padding(.top, 12)
            if desktop.enabled {
                HStack(spacing: 7) {
                    if wiring.lost {
                        Button(desktop.busy ? "正在重开…" : "以调试模式重新打开") {
                            Task { await service.accountOperation("accounts.desktop_launch") }
                        }
                        .buttonStyle(.pill(.primary, height: 26))
                        .disabled(disabled || desktop.busy)
                    } else if wiring.ownLogin, let current {
                        Button("接管桌面") { takingOver = current }
                            .buttonStyle(.pill(.primary, height: 26))
                            .disabled(service.accountBusy || directory?.switchDisabled(for: current) == true)
                    }
                    Button {
                        editingPort.toggle()
                    } label: {
                        HStack(spacing: 4) {
                            Text("端口 \(String(desktop.port))")
                            Icon(.chevronDown, size: 8, color: Palette.ink3, weight: 2.4).rotationEffect(.degrees(editingPort ? 180 : 0))
                        }
                    }
                    .buttonStyle(.pill(.secondary, height: 26))
                    .accessibilityLabel(editingPort ? "收起调试端口" : "调试端口 \(String(desktop.port))")
                }
                .padding(.top, 12)
                if editingPort { portEditor(desktop).padding(.top, 12) }
            }
        }
        .padding(.horizontal, 20).padding(.top, 17).padding(.bottom, 15)
        .frame(maxWidth: .infinity)
        .loopPanel(radius: 14)
    }

    private func end<Tile: View>(caption: String, @ViewBuilder tile: () -> Tile) -> some View {
        VStack(spacing: 6) {
            tile()
            Text(caption).font(.system(size: 11, weight: .medium)).foregroundStyle(Palette.ink2)
        }
        .frame(width: 76)
    }

    private func chip(_ wiring: DesktopWiring) -> some View {
        let text = wiring.plain ? "热切换关着"
            : wiring.lost ? "没连上"
            : wiring.ownLogin ? "已连接 · 未接管" : "已连接 · 已接管"
        return Text(text)
            .font(.system(size: 11, weight: wiring.plain ? .regular : .semibold))
            .foregroundStyle(wiring.plain ? Palette.ink3 : wiring.attention ? Palette.attentionText : Palette.deep)
            .lineLimit(1).truncationMode(.tail)
            .padding(.horizontal, 9).frame(height: 20)
            .background(wiring.plain ? Palette.chip : wiring.attention ? Palette.attentionFill : Palette.wash, in: Capsule())
    }

    private func portEditor(_ desktop: DesktopConnection) -> some View {
        let number = UInt16(port).flatMap { $0 >= 1024 ? $0 : nil }
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text("调试端口").font(.system(size: 11)).foregroundStyle(Palette.ink2)
                LoopField(placeholder: "9522", text: $port).frame(width: 72).monospacedDigit()
                Button("保存") {
                    if let number { Task { await service.accountOperation("accounts.desktop", params: LoginParams(port: number, enabled: true)) } }
                }
                .buttonStyle(.pill(.primary, height: 26))
                .disabled(disabled || number == nil || desktop.managed)
                Button("恢复默认") { port = "9522" }.buttonStyle(.pill(.link)).disabled(desktop.managed)
                Spacer(minLength: 0)
            }
            .disabled(desktop.managed)
            Text(desktop.managed ? "当前已接管，改端口前请先退出 Codex 桌面。" : "1024–65535")
                .font(.system(size: 11)).foregroundStyle(desktop.managed ? Palette.attentionText : Palette.ink3)
        }
        .padding(.top, 12)
        .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
    }

    // MARK: 步骤

    private var steps: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("用调试模式重开，分三步").font(.system(size: 12, weight: .medium))
            HStack(alignment: .top, spacing: 14) {
                step("01", "先退出 Codex 桌面", "桌面还开着时要你自己退出；AuthBaton 不会替你结束任务。")
                step("02", "以调试模式打开", "用调试端口 \(desktop.map { String($0.port) } ?? "9522") 重新打开桌面。")
                step("03", "连上后自动核对", "桌面账号与 auth.json 一致就能热切换；第一次连上还要点一次「接管桌面」。")
            }
        }
    }

    private func step(_ number: String, _ title: String, _ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(number).font(.system(size: 11, weight: .bold, design: .rounded)).foregroundStyle(Palette.brand)
            Text(title).font(.system(size: 12, weight: .semibold)).padding(.top, 3)
            Text(detail).font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true).padding(.top, 2)
        }
        .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    // MARK: 热切换

    private func hotSwitch(_ desktop: DesktopConnection) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 10) {
                Text("热切换").font(.system(size: 13, weight: .semibold))
                Spacer(minLength: 10)
                Toggle("热切换", isOn: Binding(get: { desktop.enabled }, set: { value in
                    // Remembered for coming back from 只用 CLI.
                    UserDefaults.standard.set(value, forKey: CodexUse.hotKey)
                    Task { await service.accountOperation("accounts.desktop", params: LoginParams(enabled: value)) }
                }))
                .labelsHidden().toggleStyle(.switch).controlSize(.small)
                .disabled(disabled || desktop.busy)
            }
            Text("开启时通过 Codex 桌面换号并支持自动换号；关掉后改用普通切换，换号前要自己退出桌面，见底时不再自动换号。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
            if desktop.enabled {
                Text("照常打开的桌面在刚启动且没有任务时，会自动以调试模式重开；AuthBaton 启动前就在用的桌面不会重启，需要时在上方重开一次。")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 2)
            }
        }
        .padding(.top, 14)
        .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
    }
}
