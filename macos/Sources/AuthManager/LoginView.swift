import AppKit
import SwiftUI

struct LoginView: View {
    let provider: String
    var account: ManagedAccount? = nil
    var cloudAccountID: String? = nil
    @EnvironmentObject private var service: ServiceController
    @Environment(\.dismiss) private var dismiss
    @State private var accountLabel = ""
    @State private var authorizationCode = ""

    private var name: String { provider == "codex" ? "Codex" : "Claude Code" }
    // Only the attempt started for this very account; another account's result is not this one's.
    private var attempt: LoginAttempt? {
        guard service.login?.provider == provider,
              service.loginTarget == LoginTarget(accountID: account?.id, cloudAccountID: cloudAccountID) else { return nil }
        return service.login
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 12) {
                ProviderGlyph(provider: provider, size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(account == nil ? "登录 \(name)" : "重新登录「\(account?.label ?? "")」").font(.title3.weight(.semibold))
                    Text(provider == "codex" ? "官方设备授权" : "官方 Claude CLI 登录").font(.callout).foregroundStyle(.secondary)
                }
            }
            if let attempt {
                progress(attempt)
            } else {
                intro
            }
        }
        .padding(24).frame(width: 520)
        .onAppear { if accountLabel.isEmpty { accountLabel = account?.label ?? "\(name) 账号" } }
        .interactiveDismissDisabled(attempt?.isActive == true)
    }

    private var intro: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(provider == "codex"
                 ? "使用官方设备授权。取得授权码后，在浏览器中选择账号并确认。登录信息由 Codex 保存。"
                 : "使用官方 Claude CLI 登录订阅账号。CLI 会打开浏览器，并将认证保存在本机，供 Claude Code 和官方 Agent SDK 使用。")
                .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            Callout(icon: account?.native == true ? "exclamationmark.triangle.fill" : "info.circle.fill",
                    tone: account?.native == true ? .warning : .accent,
                    message: cloudAccountID != nil ? "授权完成后会自动发送到已配置的个人认证中心，更新同一云端账号。请在浏览器中选择该账号；原生客户端继续保持当前页面。" : account?.native == true ? "这会更新当前账号的默认登录。请先结束任务并退出官方客户端，完成后再重新打开。" : "授权完成后独立保存此账号。当前默认登录保持原样，你可以稍后从账号列表中切换。")
            VStack(alignment: .leading, spacing: 6) {
                Text("账号名称").font(.callout.weight(.medium))
                TextField("例如：个人 / 工作", text: $accountLabel).textFieldStyle(.roundedBorder)
            }
            if provider == "claude" {
                Text("长期 setup-token 面向独立运行环境，后续单独管理；本入口使用原生登录。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if let error = service.loginError { Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: error) }
            HStack {
                Spacer()
                Button("取消") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(cloudAccountID == nil ? "开始官方登录" : "登录并更新云端") { Task { await service.startLogin(provider: provider, label: accountLabel, accountID: account?.id, cloudAccountID: cloudAccountID) } }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                    .disabled(accountLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || service.loginBusy)
            }.padding(.top, 4)
        }
    }

    private func progress(_ attempt: LoginAttempt) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 10) {
                if attempt.isActive { ProgressView().controlSize(.small) }
                else {
                    Image(systemName: attempt.phase == "succeeded" ? "checkmark.circle.fill" : "info.circle.fill")
                        .foregroundStyle(attempt.phase == "succeeded" ? Palette.success : Palette.warning)
                }
                Text(attempt.phaseLabel).font(.headline)
            }
            Text(attempt.message).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if let code = attempt.userCode {
                VStack(spacing: 6) {
                    Text("在浏览器中输入此授权码").font(.caption).foregroundStyle(.secondary)
                    Text(code).font(.system(size: 30, weight: .semibold, design: .monospaced))
                        .kerning(2)
                        .textSelection(.enabled)
                }
                .frame(maxWidth: .infinity).padding(.vertical, 18)
                .background(Palette.raised, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.hairline))
            }
            if let link = attempt.verificationUrl, let url = URL(string: link) {
                Button(provider == "codex" ? "打开官方授权页面" : "浏览器未打开？使用手动授权", systemImage: "arrow.up.forward.app") {
                    NSWorkspace.shared.open(url)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
            }
            if provider == "claude" && attempt.isActive {
                VStack(alignment: .leading, spacing: 8) {
                    Text("通常浏览器会自动完成回调。若使用手动授权页面，请将页面显示的完整 code#state 填入下方。")
                        .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    HStack {
                        SecureField("手动授权代码（仅本次使用）", text: $authorizationCode)
                            .textFieldStyle(.roundedBorder)
                        Button("提交") {
                            let code = authorizationCode.trimmingCharacters(in: .whitespacesAndNewlines)
                            authorizationCode = ""
                            Task { await service.submitLoginCode(code) }
                        }.disabled(authorizationCode.isEmpty || service.loginBusy)
                    }
                }
            }
            if let error = service.loginError { Callout(icon: "exclamationmark.triangle.fill", tone: .warning, message: error) }
            Divider()
            HStack {
                if attempt.isActive {
                    Button(attempt.phase == "cancelling" ? "正在停止…" : "停止本次登录", role: .destructive) {
                        authorizationCode = ""
                        Task { await service.cancelLogin() }
                    }.disabled(service.loginBusy || attempt.phase == "cancelling")
                    Spacer()
                    Button("在后台等待") { dismiss() }
                        .help("关闭此窗口，登录继续进行；可从账号页重新打开进度")
                } else {
                    Button("重新尝试") { service.clearLogin() }
                    Spacer()
                    Button("完成") { service.clearLogin(); dismiss() }
                        .keyboardShortcut(.defaultAction)
                        .buttonStyle(.borderedProminent)
                }
            }
        }
    }
}
