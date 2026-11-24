import AppKit
import SwiftUI

// Optional Chrome companion extension sync: folded to a quiet strip inside the Claude 桌面 slot.
struct ChromeSyncStrip: View {
    @EnvironmentObject private var service: ServiceController
    let chrome: ClaudeDesktopStatus.ChromeSyncStatus
    let locked: Bool
    var embedded = false
    let open: (ClaudeSheet) -> Void

    var body: some View {
        if embedded {
            VStack(alignment: .leading, spacing: 3) {
                controls
                stateLine.padding(.leading, 32)
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .modifier(StripChrome(embedded: true))
        } else {
            controls
                .padding(.horizontal, 16).frame(minHeight: 52)
                .modifier(StripChrome(embedded: false))
        }
    }

    private var activeVisual: Bool { chrome.enabled && chrome.connected }
    private var attentionVisual: Bool {
        chrome.enabled && ["awaiting_login", "error"].contains(chrome.state)
    }

    private var controls: some View {
        HStack(spacing: 12) {
            ZStack {
                Circle()
                    .fill(attentionVisual ? Palette.attentionWash : activeVisual ? Palette.wash : Palette.chip)
                    .frame(width: embedded ? 20 : 28, height: embedded ? 20 : 28)
                Icon(
                    .globe,
                    size: embedded ? 11 : 14,
                    color: attentionVisual ? Palette.attention : activeVisual ? Palette.brand : Palette.ink3,
                    weight: 1.9
                )
            }
            .accessibilityHidden(true)

            Text("Chrome 登录同步")
                .font(.system(size: embedded ? 12 : 13, weight: .semibold))
                .fixedSize()

            if embedded { Spacer(minLength: 6) } else { stateLine.frame(maxWidth: .infinity, alignment: .leading) }

            if !chrome.isInstalled {
                Button("安装扩展…") { open(.chromeSync) }
                    .buttonStyle(.pill(.secondary))
                    .disabled(locked)
                    .help("可选扩展：切换 Claude 桌面账号时自动同步 Chrome 里的 claude.ai 登录")
            } else if chrome.enabled && !chrome.connected {
                Button("安装指引…") { open(.chromeSync) }
                    .buttonStyle(.pill(.secondary))
                    .disabled(locked)
            } else if chrome.enabled && chrome.connected && chrome.state == "mismatch" && chrome.desktopSaved {
                Button("立即同步") {
                    Task { await service.claudeDesktop("chrome_sync_now") }
                }
                .buttonStyle(.pill(.secondary))
                .disabled(locked || chrome.busy)
                Button("管理 ›") { open(.chromeSync) }.buttonStyle(.pill(.link))
            } else if chrome.enabled && chrome.connected && chrome.state == "missing_session" {
                Button("登新号") {
                    Task { await service.claudeDesktop("chrome_sync_clear_login") }
                }
                .buttonStyle(.pill(.secondary))
                .disabled(locked || chrome.busy)
                .help("仅删除 Chrome 本地 claude.ai Cookie（不注销服务端），方便登录当前桌面账号并保存")
                Button("管理 ›") { open(.chromeSync) }.buttonStyle(.pill(.link))
            } else {
                Toggle("Chrome 登录同步", isOn: Binding(get: { chrome.enabled }, set: { value in
                    Task { await service.claudeDesktop("chrome_sync", enabled: value, confirm: value ? true : nil) }
                }))
                .labelsHidden().toggleStyle(.switch).controlSize(.small)
                .disabled(locked || chrome.busy)
                .help("切换 Claude 桌面账号时自动同步 Chrome 里的 claude.ai 登录")
                Button("管理 ›") { open(.chromeSync) }.buttonStyle(.pill(.link))
            }
        }
    }

    @ViewBuilder private var stateLine: some View {
        Group {
            if chrome.busy || chrome.state == "syncing" {
                HStack(spacing: 5) {
                    ProgressView().controlSize(.mini)
                    Text(chrome.message)
                }.foregroundStyle(Palette.ink2)
            } else if chrome.state == "awaiting_login" {
                Text(chrome.message).foregroundStyle(Palette.deep).help(chrome.message)
            } else if chrome.state == "error" {
                Text(chrome.message).foregroundStyle(Palette.attentionText).help(chrome.message)
            } else if chrome.state == "synced" {
                Text(chrome.message).foregroundStyle(Palette.ink2).help(chrome.message)
            } else {
                Text(chrome.message).foregroundStyle(Palette.ink3).help(chrome.message)
            }
        }
        .font(.system(size: 12)).lineLimit(1)
    }
}

struct ChromeSyncSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void
    let openUninstall: () -> Void

    @State private var working = false
    @State private var copiedURL = false
    @State private var copiedID = false

    private var chrome: ClaudeDesktopStatus.ChromeSyncStatus? { service.directory?.claudeDesktop?.chromeSync }

    var body: some View {
        if let chrome, chrome.isInstalled {
            if chrome.connected {
                manageView(chrome)
            } else {
                installGuideView(chrome)
            }
        } else {
            onboardingView
        }
    }

    // First-time boundary disclosure & local deployment.
    private var onboardingView: some View {
        SheetScaffold(
            title: "安装 Chrome 登录同步扩展？",
            lead: "切换 Claude 桌面账号时，自动同步同一 Chrome 资料里的 claude.ai 登录，并重置官方 Claude 插件准备自动授权。",
            width: 472
        ) {
            VStack(alignment: .leading, spacing: 8) {
                CheckItem(kind: .system, text: "无需管理员密码：AuthBaton 会在本地释放扩展目录并注册 Chrome Native Messaging，首次需在 Chrome 扩展页拖入文件夹一次（后续更新自动同步）")
                CheckItem(kind: .you, text: "新安全边界：仅在 AuthBaton 本机钥匙串 Vault 保管该扩展从 Chrome 读到的 claude.ai 网页会话 Cookie，不解密 Claude 桌面文件、不上传云端")
                CheckItem(kind: .info, text: "切号时自动替换 claude.ai Cookie、清站点缓存、重载官方 Claude 插件清掉旧登录并武装自动点 Authorize；你只需在 Chrome 的 Claude 插件里点一下 Log in")
                CheckItem(kind: .info, text: "随时可以关闭开关或一键卸载扩展并清除已保存的网页会话")
            }
            errorLine
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button(working ? "正在准备…" : "安装并开启") {
                working = true
                Task {
                    await service.claudeDesktop("chrome_sync", enabled: true, confirm: true)
                    working = false
                }
            }
            .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            .disabled(working || service.accountBusy)
        }
        .onAppear { service.clearAccountError() }
    }

    // Step 2: Extension files and Native Messaging manifest are ready; guide one-time drag into Chrome.
    private func installGuideView(_ chrome: ClaudeDesktopStatus.ChromeSyncStatus) -> some View {
        SheetScaffold(
            title: "把扩展拖进 Chrome（仅首次）",
            lead: "本机扩展目录与 Native Messaging 已自动装好。拖进 Chrome 后这里会自动变为「已连接」。",
            width: 476
        ) {
            VStack(alignment: .leading, spacing: 10) {
                CheckItem(kind: .you, text: "1. 打开 Chrome 扩展管理页 chrome://extensions，打开右上角「开发者模式」")
                CheckItem(kind: .you, text: "2. 点下面「在 Finder 中显示扩展目录」，把 chrome-extension 文件夹直接拖进 Chrome 扩展页")
                CheckItem(kind: .info, text: "扩展 ID 已固定为 \(chrome.extensionId)，以后 AuthBaton 更新时原地覆盖文件，不需要重新拖入")

                HStack(spacing: 8) {
                    if let dir = chrome.extensionDir {
                        Button("在 Finder 中显示扩展目录") {
                            revealInFinder(dir)
                        }
                        .buttonStyle(.pill(.primary, height: 27))
                    }
                    Button(copiedURL ? "已复制 chrome://extensions" : "打开 Chrome 扩展页") {
                        openChromeExtensionsPage()
                        copiedURL = true
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { copiedURL = false }
                    }
                    .buttonStyle(.pill(.secondary, height: 27))
                    .help("若 Chrome 未直接跳转，地址已复制到剪贴板，可在 Chrome 地址栏粘贴回车")
                }

                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("等待 Chrome 扩展连接… 拖入并启用后会自动识别。")
                        .font(.system(size: 12)).foregroundStyle(Palette.ink2)
                    Spacer(minLength: 4)
                    Button(copiedID ? "已复制 ID" : "复制 ID") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(chrome.extensionId, forType: .string)
                        copiedID = true
                        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { copiedID = false }
                    }
                    .buttonStyle(.pill(.link))
                }
                .padding(.top, 2)
            }
            errorLine
        } actions: {
            Button("卸载扩展…", role: .destructive) {
                openUninstall()
            }
            .buttonStyle(.pill(.link))
            Spacer()
            Button("稍后再说", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
        }
        .onAppear { service.clearAccountError() }
    }

    // Connected management view.
    private func manageView(_ chrome: ClaudeDesktopStatus.ChromeSyncStatus) -> some View {
        SheetScaffold(
            title: "Chrome 登录同步",
            lead: "已连上 Chrome 扩展；切 Claude 桌面账号时自动同步 claude.ai 网页登录并重置官方插件。",
            width: 500
        ) {
            VStack(alignment: .leading, spacing: 12) {
                // Master enable toggle & live Chrome status
                VStack(alignment: .leading, spacing: 8) {
                    HStack {
                        Text("切桌面账号时自动同步到 Chrome")
                            .font(.system(size: 12, weight: .semibold)).foregroundStyle(Palette.ink)
                        Spacer()
                        Toggle("自动同步", isOn: Binding(get: { chrome.enabled }, set: { value in
                            Task { await service.claudeDesktop("chrome_sync", enabled: value, confirm: value ? true : nil) }
                        }))
                        .labelsHidden().toggleStyle(.switch).controlSize(.small)
                        .disabled(service.accountBusy || chrome.busy)
                    }
                    HStack(spacing: 12) {
                        Label {
                            Text("Chrome 网页：\(chrome.chromeEmail ?? "未登录")")
                        } icon: {
                            Icon(.globe, size: 11, color: Palette.ink3)
                        }
                        Spacer(minLength: 8)
                        Text(chrome.claudeExtInstalled
                             ? "Claude 插件 v\(chrome.claudeExtVersion ?? "")"
                             : "未检测到 Claude 插件")
                            .foregroundStyle(Palette.ink3)
                    }
                    .font(.system(size: 11)).foregroundStyle(Palette.ink2)
                }
                .padding(11)
                .background(Palette.panel, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Palette.divider, lineWidth: 1))

                if chrome.state == "awaiting_login" {
                    Banner(glyph: .info, text: chrome.message) { EmptyView() }
                } else if let err = chrome.lastError {
                    Banner(glyph: .alert, text: err) { EmptyView() }
                }

                // Quick actions
                HStack(spacing: 8) {
                    Button("同步当前桌面账号") {
                        Task { await service.claudeDesktop("chrome_sync_now") }
                    }
                    .buttonStyle(.pill(.primary, height: 26))
                    .disabled(service.accountBusy || chrome.busy || !chrome.desktopSaved)
                    .help(chrome.desktopSaved ? "把 Chrome 切到当前 Claude 桌面账号并重置插件" : "请先在 Chrome 登录一次当前桌面账号")

                    Button("保存当前 Chrome 登录") {
                        Task { await service.claudeDesktop("chrome_sync_capture") }
                    }
                    .buttonStyle(.pill(.secondary, height: 26))
                    .disabled(service.accountBusy || chrome.busy)

                    Button("本地切空登新号") {
                        Task { await service.claudeDesktop("chrome_sync_clear_login") }
                    }
                    .buttonStyle(.pill(.secondary, height: 26))
                    .disabled(service.accountBusy || chrome.busy)
                    .help("仅删除 Chrome 本地 claude.ai 认证 Cookie（不会注销服务端会话），方便登录新账号并自动保存")
                }

                // Saved Chrome sessions list
                VStack(alignment: .leading, spacing: 6) {
                    Text("已保存的 Chrome 网页会话（\(chrome.savedAccounts.count)）")
                        .font(.system(size: 11, weight: .semibold)).foregroundStyle(Palette.ink2)
                    if chrome.savedAccounts.isEmpty {
                        Text("还没有保存任何账号。在 Chrome 登录 claude.ai 后会自动保存，也可以点上方「保存当前 Chrome 登录」。")
                            .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        ForEach(chrome.savedAccounts) { item in
                            HStack(spacing: 8) {
                                VStack(alignment: .leading, spacing: 2) {
                                    HStack(spacing: 5) {
                                        Text(item.email.isEmpty ? "已保存账号" : item.email)
                                            .font(.system(size: 12, weight: .medium)).foregroundStyle(Palette.ink)
                                            .lineLimit(1)
                                        if item.inDesktop { Pill("桌面在用", style: .brand, small: true) }
                                        if item.inChrome { Pill("Chrome 当前", style: .neutral, small: true) }
                                    }
                                    Text("更新于 \(When.text(item.updatedAt)) · \(item.cookieCount) 项会话 Cookie")
                                        .font(.system(size: 10.5)).foregroundStyle(Palette.ink3)
                                }
                                Spacer(minLength: 6)
                                Button("切到 Chrome") {
                                    Task { await service.claudeDesktop("chrome_sync_now", account: item.handle) }
                                }
                                .buttonStyle(.pill(.secondary, height: 22))
                                .disabled(service.accountBusy || chrome.busy)
                                Button("移除") {
                                    Task { await service.claudeDesktop("chrome_sync_forget", account: item.handle, confirm: true) }
                                }
                                .buttonStyle(.pill(.link))
                                .disabled(service.accountBusy || chrome.busy)
                            }
                            .padding(.horizontal, 10).padding(.vertical, 7)
                            .background(Palette.card, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Palette.divider, lineWidth: 1))
                        }
                    }
                }
            }
            errorLine
        } actions: {
            if let dir = chrome.extensionDir {
                Button("扩展目录") { revealInFinder(dir) }.buttonStyle(.pill(.link))
            }
            Button("卸载扩展…", role: .destructive) { openUninstall() }.buttonStyle(.pill(.link))
            Spacer()
            Button("完成", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.primary, height: 27))
        }
        .onAppear { service.clearAccountError() }
    }

    @ViewBuilder private var errorLine: some View {
        if let error = service.accountError {
            Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func revealInFinder(_ path: String) {
        let url = URL(fileURLWithPath: path)
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    private func openChromeExtensionsPage() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString("chrome://extensions", forType: .string)
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        proc.arguments = ["-a", "Google Chrome", "chrome://extensions"]
        try? proc.run()
    }
}

struct ChromeSyncUninstallSheet: View {
    @EnvironmentObject private var service: ServiceController
    let close: () -> Void
    @State private var working = false

    var body: some View {
        SheetScaffold(
            title: "卸载 Chrome 登录同步扩展？",
            lead: "会关闭同步，并删除 AuthBaton 部署的扩展文件、Native Messaging 注册项及 Vault 中保存的 Chrome 网页 Cookie。",
            width: 446
        ) {
            VStack(alignment: .leading, spacing: 7) {
                CheckItem(kind: .system, text: "自动移除 NativeMessagingHosts 配置、chrome-extension 目录以及 Vault 里的 claude.ai 网页会话")
                CheckItem(kind: .info, text: "不会影响你 Chrome 当前的登录状态，也不会影响 Claude 桌面或 CLI 账号")
                CheckItem(kind: .you, text: "卸载后可在 Chrome 的 chrome://extensions 页面顺手移除「AuthBaton Chrome Sync」条目")
            }
            if let error = service.accountError {
                Text(error).font(.system(size: 11)).foregroundStyle(Palette.attentionText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } actions: {
            Button("取消", action: close).keyboardShortcut(.cancelAction).buttonStyle(.pill(.secondary, height: 27))
            Button(working ? "正在卸载…" : "确认卸载") {
                working = true
                Task {
                    await service.claudeDesktop("chrome_sync_uninstall", confirm: true)
                    working = false
                    if service.accountError == nil { close() }
                }
            }
            .keyboardShortcut(.defaultAction).buttonStyle(.pill(.primary, height: 27))
            .disabled(working || service.accountBusy)
        }
        .onAppear { service.clearAccountError() }
    }
}
