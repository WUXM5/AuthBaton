import AppKit
import SwiftUI

// Adding an account: log a new one in through the official flow, or take over the login this Mac
// already has. Relogin and the cloud's relogin reuse the same sheet without the choices.
struct LoginView: View {
    var account: ManagedAccount? = nil
    var cloudAccountID: String? = nil
    @EnvironmentObject private var service: ServiceController
    @Environment(\.dismiss) private var dismiss
    @State private var provider: String
    @State private var importing = false
    @State private var accountLabel = ""
    @State private var authorizationCode = ""

    init(provider: String, account: ManagedAccount? = nil, cloudAccountID: String? = nil) {
        self.account = account
        self.cloudAccountID = cloudAccountID
        _provider = State(initialValue: provider)
    }

    private var name: String { GlyphTile.name(provider) }
    private var adding: Bool { account == nil && cloudAccountID == nil }
    // Only the attempt started for this very account; another account's result is not this one's.
    private var attempt: LoginAttempt? {
        guard service.login?.provider == provider,
              service.loginTarget == LoginTarget(accountID: account?.id, cloudAccountID: cloudAccountID) else { return nil }
        return service.login
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                GlyphTile(provider: provider, size: 34)
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.ring, lineWidth: 1))
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 14, weight: .bold)).lineLimit(1)
                    Text(provider == "codex" ? "官方设备授权 · 登录信息由 Codex 保存" : "官方 Claude CLI 登录 · 认证保存在本机")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                }
            }
            if let attempt {
                progress(attempt)
            } else {
                intro
            }
        }
        .padding(20)
        .frame(width: 442, alignment: .leading)
        .background(Palette.card)
        .onAppear { if accountLabel.isEmpty { accountLabel = account?.label ?? "\(name) 账号" } }
        .onChange(of: provider) { old, new in
            if account == nil, accountLabel == "\(GlyphTile.name(old)) 账号" { accountLabel = "\(GlyphTile.name(new)) 账号" }
        }
        .interactiveDismissDisabled(attempt?.isActive == true)
    }

    private var title: String {
        if attempt != nil { return "登录 \(name)" }
        if let account { return "重新登录「\(account.displayName)」" }
        return cloudAccountID != nil ? "重新登录并更新云端" : "添加账号"
    }

    // MARK: 开始之前

    private var intro: some View {
        VStack(alignment: .leading, spacing: 0) {
            if adding {
                PillSegments(segments: [.init(value: "codex", title: "Codex", glyph: .codex), .init(value: "claude", title: "Claude Code", glyph: .claude)],
                             selection: provider, select: { provider = $0 }, fill: true)
                    .padding(.top, 15)
                VStack(spacing: 9) {
                    MethodOption(title: "登录新账号",
                                 detail: provider == "codex" ? "拿到授权码后，在浏览器里选账号并确认。" : "Claude CLI 会打开浏览器，登录后保存在本机。",
                                 selected: !importing) { importing = false }
                    MethodOption(title: "导入这台 Mac 上已有的登录",
                                 detail: "把 \(name) 现在的登录交给 BatonPass 一起管。",
                                 selected: importing) { importing = true }
                }
                .padding(.top, 12)
            } else if account?.native == true {
                Banner(text: "这会更新在用账号的默认登录。先结束任务并退出官方客户端，完成后再打开。").padding(.top, 15)
            } else if cloudAccountID != nil {
                Text("授权完成后自动发到你的个人认证中心，更新同一个云端账号。请在浏览器里选这个账号；本机客户端保持现在的登录。")
                    .font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true).padding(.top, 15)
            } else {
                Text("授权完成后，更新这个账号保存的登录；现在的默认登录不变。")
                    .font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true).padding(.top, 15)
            }
            if importing && adding {
                Text("名称用登录邮箱，之后可以重命名；现在的默认登录不变。")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 14)
            } else {
                VStack(alignment: .leading, spacing: 5) {
                    Text("账号名称").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                    LoopField(placeholder: "例如：个人 / 工作", text: $accountLabel)
                    if adding {
                        Text("授权完成后单独保存这个账号，现在的默认登录不变。").font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 2)
                    }
                }
                .padding(.top, 14)
            }
            if let error = service.loginError ?? (importing ? service.accountError : nil) { Banner(text: error).padding(.top, 12) }
            HStack(spacing: 9) {
                Spacer()
                Button("取消") { dismiss() }.buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
                if importing && adding {
                    Button(service.accountBusy ? "正在导入…" : "导入") {
                        Task {
                            // A blank name lets the service name the account after its login email.
                            await service.accountOperation("accounts.import", params: LoginParams(provider: provider))
                            if service.accountError == nil { dismiss() }
                        }
                    }
                    .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                    .disabled(service.accountBusy)
                } else {
                    Button(cloudAccountID == nil ? "开始官方登录" : "登录并更新云端") {
                        Task { await service.startLogin(provider: provider, label: accountLabel, accountID: account?.id, cloudAccountID: cloudAccountID) }
                    }
                    .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                    .disabled(accountLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || service.loginBusy)
                }
            }
            .padding(.top, 19)
        }
    }

    // MARK: 登录中

    private func progress(_ attempt: LoginAttempt) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 9) {
                if attempt.isActive {
                    ProgressView().controlSize(.small).frame(width: 15, height: 15)
                } else {
                    Icon(attempt.phase == "succeeded" ? .check : .info, size: 15, color: attempt.phase == "succeeded" ? Palette.solid : Palette.attention)
                }
                Text(attempt.phaseLabel).font(.system(size: 12, weight: .semibold))
            }
            .padding(.top, 19)
            Text(attempt.message).font(.system(size: 11)).foregroundStyle(Palette.ink3)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.leading, 24).padding(.top, 5)
            if let code = attempt.userCode {
                VStack(spacing: 7) {
                    Text("在浏览器中输入这个授权码").font(.system(size: 11)).foregroundStyle(Palette.ink3)
                    Text(code).font(.system(size: 26, weight: .semibold, design: .monospaced)).kerning(3.6).textSelection(.enabled)
                }
                .frame(maxWidth: .infinity).padding(17)
                .loopPanel(radius: 14)
                .padding(.top, 15)
            }
            if let link = attempt.verificationUrl, let url = URL(string: link) {
                Button { NSWorkspace.shared.open(url) } label: {
                    HStack(spacing: 7) {
                        Text(provider == "codex" ? "打开官方授权页面" : "浏览器没打开？用手动授权")
                        Icon(.external, size: 12, color: .white)
                    }
                }
                .buttonStyle(.pill(.primary, height: 32))
                .frame(maxWidth: .infinity)
                .padding(.top, 14)
            }
            if provider == "claude" && attempt.isActive {
                VStack(alignment: .leading, spacing: 7) {
                    Text("浏览器一般会自动完成回调。用了手动授权页面的话，把页面上完整的 code#state 填到下面。")
                        .font(.system(size: 11)).foregroundStyle(Palette.ink3).fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 7) {
                        LoopField(placeholder: "手动授权代码（只用这一次）", text: $authorizationCode, secure: true)
                        Button("提交") {
                            let code = authorizationCode.trimmingCharacters(in: .whitespacesAndNewlines)
                            authorizationCode = ""
                            Task { await service.submitLoginCode(code) }
                        }
                        .buttonStyle(.pill(.secondary, height: 27))
                        .disabled(authorizationCode.isEmpty || service.loginBusy)
                    }
                }
                .padding(.top, 14)
            }
            if let error = service.loginError { Banner(text: error).padding(.top, 12) }
            HStack(spacing: 9) {
                if attempt.isActive {
                    Button(attempt.phase == "cancelling" ? "正在停止…" : "停止这次登录", role: .destructive) {
                        authorizationCode = ""
                        Task { await service.cancelLogin() }
                    }
                    .buttonStyle(.pill(.danger))
                    .disabled(service.loginBusy || attempt.phase == "cancelling")
                    Spacer()
                    Button("在后台等待") { dismiss() }
                        .buttonStyle(.pill(.secondary, height: 27))
                        .help("关掉窗口，登录照常进行；之后可以在账号页重新打开进度")
                } else {
                    Button("重新尝试") { service.clearLogin() }.buttonStyle(.pill(.secondary, height: 27))
                    Spacer()
                    Button("完成") { service.clearLogin(); dismiss() }
                        .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                }
            }
            .padding(.top, 12)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
            .padding(.top, 19)
        }
    }
}

// One way to add an account, as a radio card.
private struct MethodOption: View {
    let title: String
    let detail: String
    let selected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 10) {
                Circle()
                    .strokeBorder(selected ? Palette.solid : Palette.dashStrong, lineWidth: selected ? 4.5 : 1.5)
                    .background(Circle().fill(selected ? Color.clear : Palette.card))
                    .frame(width: 15, height: 15)
                    .padding(.top, 1)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
                    Text(detail).font(.system(size: 11)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 12).padding(.vertical, 10)
            .background(selected ? Palette.card : Palette.panel, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay { if selected { RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Palette.brand, lineWidth: 1.5) } }
            .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
