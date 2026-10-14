import SwiftUI

struct DesktopPage: View {
    @EnvironmentObject private var service: ServiceController
    @State private var port = "9522"
    @State private var showAdvanced = false

    private var directory: AccountDirectory? { service.directory }
    private var desktop: DesktopConnection? { directory?.desktop }
    private var disabled: Bool { service.accountBusy || directory?.busy == true || directory?.recoveryNeeded == true }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Codex 桌面热切换").font(.system(size: 20, weight: .semibold))
                        Text("AuthManager 以调试模式启动 Codex 桌面并连上后，换号不用退出桌面窗口，自动换号也要靠它来暂停和接续会话。AuthManager 运行时，你照常打开的桌面，或更新后自行重启的桌面，会在启动完成、还没有任务时自动以调试模式重新打开，不会停在退出确认上；AuthManager 打开的桌面，你自己退出时照常确认。AuthManager 启动前就在用的桌面不会被重启，需要时在这里以调试模式启动一次。")
                            .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                    if let error = service.accountError {
                        Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: error) {
                            Button("知道了") { service.clearAccountError() }.controlSize(.small)
                        }
                    }
                    statusCard
                    if desktop?.enabled == true {
                        Label("热切换开着时，Codex 的「设为当前」只通过桌面进行；桌面没连上就切换不了。不想用桌面时，可以关闭热切换改用普通切换（需要先退出 Codex）。",
                              systemImage: "info.circle")
                            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(24).frame(maxWidth: 900, alignment: .leading).frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .pageBar("Codex 桌面", subtitle: desktop.map { $0.enabled ? ($0.connected ? "实验性 · 已连接" : "实验性 · 未连接") : "实验性 · 已关闭" })
        .onAppear { port = String(desktop?.port ?? 9522) }
    }

    private var statusCard: some View {
        let enabled = desktop?.enabled == true
        let connected = desktop?.connected == true
        return VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(!enabled ? "热切换已关闭" : connected ? "已连接" : "未连接").font(.headline)
                    Text(desktop?.message ?? "").font(.callout).foregroundStyle(enabled && !connected ? Palette.warning : Palette.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
                if desktop?.busy == true { ProgressView().controlSize(.small) }
                Toggle("启用热切换", isOn: Binding(get: { enabled }, set: { value in
                    Task { await service.accountOperation("accounts.desktop", params: LoginParams(enabled: value)) }
                }))
                .toggleStyle(.switch).disabled(disabled || desktop?.busy == true)
            }
            if enabled {
                HStack(alignment: .top, spacing: 10) {
                    step(1, "退出 Codex 桌面", "按 ⌘Q 退出普通模式启动的桌面；AuthManager 不会替你结束任务。", state: connected ? .done : .current)
                    step(2, "以调试模式启动", "AuthManager 以调试端口 \(desktop.map { String($0.port) } ?? "9522") 重新打开桌面。", state: connected ? .done : .current)
                    step(3, "连上后自动核对", "桌面账号与默认文件一致才算连接成功，之后就能热切换。", state: connected ? .done : .todo)
                }
                HStack(spacing: 8) {
                    Button(connected ? "重新以调试模式启动" : "以调试模式启动 Codex 桌面", systemImage: "play.fill") {
                        Task { await service.accountOperation("accounts.desktop_launch") }
                    }
                    .buttonStyle(.borderedProminent).disabled(disabled)
                    Button(showAdvanced ? "收起高级设置" : "高级：端口 \(desktop.map { String($0.port) } ?? "9522")") { showAdvanced.toggle() }
                        .buttonStyle(.borderless)
                    Spacer()
                }
                if showAdvanced {
                    HStack(spacing: 8) {
                        Text("本机调试端口").font(.callout)
                        TextField("9522", text: $port).textFieldStyle(.roundedBorder).frame(width: 80).monospacedDigit()
                        Button("保存并连接") {
                            if let number = UInt16(port), number >= 1024 {
                                Task { await service.accountOperation("accounts.desktop", params: LoginParams(port: number, enabled: true)) }
                            }
                        }
                        .disabled(disabled || (UInt16(port).map { $0 < 1024 } ?? true))
                        Text("1024–65535；更改已接管桌面的端口前需先退出桌面。").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .card(padding: 16)
    }

    private enum StepState { case done, current, todo }

    private func step(_ number: Int, _ title: String, _ detail: String, state: StepState) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Group {
                if state == .done { Image(systemName: "checkmark").font(.caption2.weight(.bold)) } else { Text("\(number)").font(.caption.weight(.bold)) }
            }
            .foregroundStyle(state == .todo ? Palette.muted : .white)
            .frame(width: 20, height: 20)
            .background(state == .done ? Palette.success : state == .current ? Palette.accent : Palette.track, in: Circle())
            Text(title).font(.callout.weight(.semibold))
            Text(detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .padding(12).frame(maxWidth: .infinity, alignment: .topLeading)
        .background(Palette.raised, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(state == .current ? Palette.accent.opacity(0.5) : .clear))
    }
}
