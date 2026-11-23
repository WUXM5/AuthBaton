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
    // 导入: the name, filled with the CLI's sign-in email until the person types their own.
    @State private var importName = ""
    @State private var importNameTyped = false
    // A new login once saved: its name, filled with the login email.
    @State private var savedName = ""
    @State private var savedNameTyped = false
    @State private var authorizationCode = ""
    @State private var copiedLink: String?
    @State private var manualOpen = false

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
        // A new account needs no name up front: it is named after its login email.
        .onAppear { if accountLabel.isEmpty { accountLabel = account?.label ?? "" } }
        .task(id: importing && adding ? provider : "") {
            if importing && adding { await service.readClientSignIns() }
        }
        .onChange(of: signInEmail) { _, email in if !importNameTyped { importName = email ?? "" } }
        .onChange(of: provider) { _, _ in importNameTyped = false; importName = signInEmail ?? "" }
        .onChange(of: newAccountLabel) { _, label in if !savedNameTyped { savedName = label ?? "" } }
        .interactiveDismissDisabled(attempt?.isActive == true)
    }

    // The email of the CLI's default sign-in, as the service last read it.
    private var signInEmail: String? {
        service.directory?.clients?.first { $0.id == provider }?.identity.flatMap { $0.contains("@") ? $0 : nil }
    }

    // The account a new login just made, and the name the service gave it.
    private var newAccountID: String? { adding && attempt?.phase == "succeeded" ? attempt?.accountId : nil }
    private var newAccountLabel: String? {
        newAccountID.flatMap { id in service.directory?.accounts.first { $0.id == id }?.label }
    }

    private func typed(_ text: Binding<String>, _ flag: Binding<Bool>) -> Binding<String> {
        Binding(get: { text.wrappedValue }, set: { text.wrappedValue = $0; flag.wrappedValue = true })
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
                    // Claude 桌面 users read 「这台 Mac 上已有的登录」 as their desktop sign-in, which can't be imported.
                    MethodOption(title: provider == "codex" ? "导入这台 Mac 上已有的登录" : "导入 Claude Code CLI 的登录",
                                 detail: provider == "codex" ? "把 \(name) 现在的登录交给 AuthBaton 一起管。"
                                     : "把 Claude Code CLI 现在的登录交给 AuthBaton 一起管。\nClaude 桌面的登录不能导入。",
                                 selected: importing, signIn: importing ? signInEmail : nil) { importing = true }
                }
                .padding(.top, 12)
            } else if account?.native == true {
                Banner(text: "这会更新在用账号的默认登录。先结束任务并退出官方客户端，完成后再打开。").padding(.top, 15)
            } else if cloudAccountID != nil {
                Text("授权完成后自动发到个人云端，更新同一个云端账号。请在浏览器里选这个账号；本机客户端保持现在的登录。")
                    .font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true).padding(.top, 15)
            } else {
                Text("授权完成后，更新这个账号保存的登录；现在的默认登录不变。")
                    .font(.system(size: 12)).foregroundStyle(Palette.ink2).fixedSize(horizontal: false, vertical: true).padding(.top, 15)
            }
            if importing && adding {
                NameField(text: typed($importName, $importNameTyped), note: "默认用登录邮箱，可以改。现在的默认登录不变。")
                    .padding(.top, 14)
            } else if adding {
                Text("名称默认用登录邮箱，完成后可以改。现在的默认登录不变。")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 14)
            } else {
                NameField(text: $accountLabel, placeholder: "例如：个人 / 工作").padding(.top, 14)
            }
            if let error = service.loginError ?? (importing ? service.accountError : nil) { Banner(text: error).padding(.top, 12) }
            HStack(spacing: 9) {
                Spacer()
                Button("取消") { dismiss() }.buttonStyle(.pill(.secondary, height: 27)).keyboardShortcut(.cancelAction)
                if importing && adding {
                    Button(service.accountBusy ? "正在导入…" : "导入") {
                        Task {
                            // A blank name lets the service name the account after its login email.
                            let label = importName.trimmingCharacters(in: .whitespacesAndNewlines)
                            await service.accountOperation("accounts.import", params: LoginParams(
                                provider: provider, label: label.isEmpty || label == signInEmail ? nil : label))
                            if service.accountError == nil { dismiss() }
                        }
                    }
                    .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                    .disabled(service.accountBusy)
                } else {
                    Button(cloudAccountID == nil ? "开始官方登录" : "登录并更新云端") {
                        Task { await service.startLogin(provider: provider, label: adding ? "" : accountLabel, accountID: account?.id, cloudAccountID: cloudAccountID) }
                    }
                    .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                    .disabled((!adding && accountLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty) || service.loginBusy)
                }
            }
            .padding(.top, 19)
        }
    }

    // 完成: a new account keeps the email as its name unless the person typed another.
    private func finish() async {
        let name = savedName.trimmingCharacters(in: .whitespacesAndNewlines)
        if let id = newAccountID, !name.isEmpty, name != newAccountLabel {
            await service.accountOperation("accounts.update", params: LoginParams(id: id, label: name))
            guard service.accountError == nil else { return }
        }
        service.clearLogin()
        dismiss()
    }

    // MARK: 登录中

    // The CLI does not open a browser: the person opens the link back to this Mac, or copies it
    // into another browser or browser profile. The manual page and its code are the fallback.
    @ViewBuilder
    private func claudeLinks(_ attempt: LoginAttempt) -> some View {
        if let link = attempt.browserUrl, let url = URL(string: link) {
            let copied = copiedLink == link
            VStack(spacing: 10) {
                HStack(spacing: 9) {
                    Button { NSWorkspace.shared.open(url) } label: {
                        HStack(spacing: 7) {
                            Text("打开浏览器")
                            Icon(.external, size: 11, color: .white)
                        }
                    }
                    .buttonStyle(.pill(.primary, height: 32))
                    Button {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(link, forType: .string)
                        copiedLink = link
                    } label: {
                        HStack(spacing: 6) {
                            Icon(copied ? .check : .copy, size: 12, color: copied ? Palette.solid : Palette.ink2)
                            Text(copied ? "已复制" : "复制链接").foregroundStyle(copied ? Palette.deep : Palette.ink)
                        }
                    }
                    .buttonStyle(.pill(.secondary, height: 32))
                }
                Text("复制后可以在别的浏览器或浏览器身份里登录")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3)
            }
            .frame(maxWidth: .infinity)
            .padding(17)
            .loopPanel(radius: 14)
            .padding(.top, 15)
        }
        Button { manualOpen.toggle() } label: {
            HStack(spacing: 4) {
                Text("浏览器回不到这里？用手动授权")
                Icon(.chevronDown, size: 9, color: Palette.solid, weight: 2.4)
                    .rotationEffect(.degrees(manualOpen ? 180 : 0))
            }
        }
        .buttonStyle(.pill(.link))
        .padding(.top, 12)
        if manualOpen {
            VStack(alignment: .leading, spacing: 9) {
                if let link = attempt.verificationUrl, let url = URL(string: link) {
                    Button { NSWorkspace.shared.open(url) } label: {
                        HStack(spacing: 6) {
                            Text("打开手动授权页面")
                            Icon(.external, size: 10, color: Palette.ink2)
                        }
                    }
                    .buttonStyle(.pill(.secondary, height: 27))
                }
                HStack(spacing: 7) {
                    LoopField(placeholder: "手动授权码（只用这一次）", text: $authorizationCode, secure: true)
                    Button("提交") {
                        let code = authorizationCode.trimmingCharacters(in: .whitespacesAndNewlines)
                        authorizationCode = ""
                        Task { await service.submitLoginCode(code) }
                    }
                    .buttonStyle(.pill(.secondary, height: 27))
                    .disabled(authorizationCode.isEmpty || service.loginBusy)
                }
            }
            .padding(.top, 9)
        }
    }

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
            if provider == "codex", let link = attempt.verificationUrl, let url = URL(string: link) {
                Button { NSWorkspace.shared.open(url) } label: {
                    HStack(spacing: 7) {
                        Text("打开官方授权页面")
                        Icon(.external, size: 12, color: .white)
                    }
                }
                .buttonStyle(.pill(.primary, height: 32))
                .frame(maxWidth: .infinity)
                .padding(.top, 14)
            }
            if provider == "claude" && attempt.isActive {
                claudeLinks(attempt)
            }
            if newAccountID != nil {
                NameField(text: typed($savedName, $savedNameTyped), note: "默认用登录邮箱；改了名字，邮箱照样显示在名字下面。")
                    .padding(.top, 16)
            }
            if let error = service.loginError ?? (newAccountID != nil ? service.accountError : nil) { Banner(text: error).padding(.top, 12) }
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
                        .help("关掉窗口不影响登录；之后可以在账号页重新打开进度")
                } else {
                    Button("重试") { service.clearLogin() }.buttonStyle(.pill(.secondary, height: 27))
                    Spacer()
                    Button("完成") { Task { await finish() } }
                        .buttonStyle(.pill(.primary, height: 27)).keyboardShortcut(.defaultAction)
                        .disabled(service.accountBusy)
                }
            }
            .padding(.top, 12)
            .overlay(alignment: .top) { Rectangle().fill(Palette.divider).frame(height: 1) }
            .padding(.top, 19)
        }
    }
}

// The account's name, as typed or as the service will name it.
private struct NameField: View {
    let text: Binding<String>
    var placeholder = "登录邮箱"
    var note: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("账号名称").font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
            LoopField(placeholder: placeholder, text: text)
            if let note { Text(note).font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.top, 2) }
        }
    }
}

// One way to add an account, as a radio card.
private struct MethodOption: View {
    let title: String
    let detail: String
    let selected: Bool
    // 导入: whose sign-in it would bring in, once the service has read it.
    var signIn: String? = nil
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
                    if let signIn {
                        HStack(spacing: 7) {
                            Seal(signIn, size: 18)
                            Text(accountName: signIn).font(.system(size: 11.5, weight: .semibold)).foregroundStyle(Palette.ink).lineLimit(1)
                        }
                        .padding(.horizontal, 8).frame(maxWidth: .infinity, minHeight: 28, alignment: .leading)
                        .background(Palette.panel, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                        .padding(.top, 6)
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel("当前登录 \(signIn)")
                    }
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
