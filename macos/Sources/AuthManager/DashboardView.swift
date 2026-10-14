import AppKit
import SwiftUI

enum Page: Hashable {
    case accounts, codex, claude, automation, desktop, clients, cloud
}

// Shared so the menu bar panel can open the window on a specific page or account.
@MainActor
final class AppNavigation: ObservableObject {
    static let shared = AppNavigation()
    @Published var page: Page = .accounts
    // The account a page should point at: the Claude page shows it, the Codex page rings its card.
    @Published var selection: String?

    func show(_ page: Page, select: String? = nil) {
        self.page = page
        if let select { selection = select }
    }

    // The page an account lives on.
    func show(account: ManagedAccount) {
        show(account.provider == "claude" ? .claude : .codex, select: account.id)
    }
}

struct DashboardView: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared

    init(initialPage: Page? = nil) {
        if let initialPage { AppNavigation.shared.page = initialPage }
    }

    private var cloudMode: Bool { service.directory?.cloud?.mode == "cloud" }

    var body: some View {
        HStack(spacing: 0) {
            Sidebar()
                .frame(width: 197)
                .frame(maxHeight: .infinity)
                .background(Palette.rail)
                .overlay(alignment: .trailing) { Rectangle().fill(Palette.railEdge).frame(width: 1) }
            VStack(spacing: 0) {
                if let error = service.errorText {
                    Banner(glyph: .server, text: error) {
                        if !service.isConnected { ReconnectButton() }
                    }
                    // Clear of the hidden title bar, where the window is dragged.
                    .padding(.horizontal, 27).padding(.top, 34)
                    .frame(maxWidth: 816)
                }
                page
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(Palette.ground)
        }
        .ignoresSafeArea(.container, edges: .top)
        .frame(minWidth: 833, minHeight: 527)
        .foregroundStyle(Palette.ink)
        .tint(Palette.solid)
    }

    @ViewBuilder private var page: some View {
        switch nav.page {
        case .accounts:
            if cloudMode {
                PageScaffold(title: "云端账号", subtitle: Text("账号保存在你的个人认证中心，设备按需取得可用凭据。"), spacing: 14) {
                    CloudAccountsView(openSettings: { nav.show(.cloud) })
                }
            } else {
                AccountsView()
            }
        case .codex: cloudMode ? AnyView(AccountsView()) : AnyView(CodexPage())
        case .claude: cloudMode ? AnyView(AccountsView()) : AnyView(ClaudePage())
        case .automation: AutomationPage()
        case .desktop: DesktopPage()
        case .clients: ClientsPage()
        case .cloud: CloudPage()
        }
    }
}

// MARK: 侧边栏

private struct Sidebar: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared
    @ObservedObject private var reminders = ReminderModel.shared

    var body: some View {
        let directory = service.directory
        let cloudMode = directory?.cloud?.mode == "cloud"
        let issues = directory?.issues(dismissed: reminders.dismissed).count ?? 0
        let desktop = directory?.desktop
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 9) {
                BrandTile(size: 26)
                Text("AuthManager").font(.system(size: 12, weight: .semibold)).kerning(-0.14)
            }
            .padding(.horizontal, 7)
            VStack(spacing: 2) {
                item(.accounts, "账号", glyph: .accounts) {
                    if issues > 0 {
                        Text("\(issues)").font(.system(size: 10, weight: .semibold)).monospacedDigit()
                            .foregroundStyle(Palette.attentionText)
                            .padding(.horizontal, 5).frame(minWidth: 17, minHeight: 15)
                            .background(Palette.attentionFill, in: Capsule())
                    }
                }
                .accessibilityLabel(issues > 0 ? "账号，\(issues) 件事要处理" : "账号")
                if !cloudMode {
                    ForEach(["codex", "claude"], id: \.self) { provider in
                        item(provider == "codex" ? .codex : .claude, GlyphTile.name(provider), glyph: .provider(provider)) {
                            if let lowest = directory?.current(provider)?.lowestPrimary { trailingText("\(lowest)%") }
                        }
                    }
                }
                Color.clear.frame(height: 12)
                item(.automation, "换号与重置", glyph: .relay) {
                    if let strategy = directory?.strategy { trailingText(strategy.config.enabled ? "开" : "关") }
                }
                item(.desktop, "Codex 桌面", glyph: .desktop) {
                    if directory?.desktopBlocked == true {
                        Circle().fill(Palette.attentionDot).frame(width: 6, height: 6)
                    } else if desktop?.enabled == false {
                        trailingText("关")
                    }
                }
                .accessibilityLabel(directory?.desktopBlocked == true ? "Codex 桌面，没连上" : "Codex 桌面")
                item(.clients, "客户端与服务", glyph: .clients) {
                    if !service.isConnected && !service.isConnecting {
                        Circle().fill(Palette.attentionDot).frame(width: 6, height: 6)
                    }
                }
                item(.cloud, "个人云端", glyph: .cloud) {
                    if cloudMode { trailingText("云端") } else if directory?.cloud?.paired == true { trailingText("已配对") }
                }
            }
            .padding(.top, 17)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 9)
        .padding(.top, 43)
        .padding(.bottom, 10)
    }

    private func trailingText(_ text: String) -> some View {
        Text(text).font(.system(size: 11)).foregroundStyle(Palette.ink3).monospacedDigit()
    }

    private func item<Trailing: View>(_ page: Page, _ title: String, glyph: Glyph, @ViewBuilder trailing: () -> Trailing) -> some View {
        let on = nav.page == page
        return Button { nav.show(page) } label: {
            HStack(spacing: 9) {
                Icon(glyph, size: 15, color: glyph.tint ?? (on ? Palette.solid : Palette.ink2))
                Text(title).font(.system(size: 12, weight: on ? .semibold : .regular)).foregroundStyle(Palette.ink)
                Spacer(minLength: 3.5)
                trailing()
            }
            .padding(.horizontal, 9)
            .frame(height: 29)
            .background {
                if on {
                    RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Palette.card)
                        .shadow(color: .black.opacity(0.08), radius: 1, y: 1)
                        .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(Palette.ring, lineWidth: 0.5))
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

// One reconnect at a time: a second click while the first waits for the old service would
// otherwise start a lock owner of its own.
struct ReconnectButton: View {
    @EnvironmentObject private var service: ServiceController
    // On an amber banner by default; the menu bar puts it on a plain card.
    var kind: PillButtonStyle.Kind = .onAttention

    var body: some View {
        Button(service.isConnecting ? "正在重新连接…" : "重新连接后台服务") { Task { await service.restart() } }
            .buttonStyle(.pill(kind, height: kind == .onAttention ? 20 : 24))
            .disabled(!service.canRestart)
    }
}

// MARK: 客户端与服务

struct ClientsPage: View {
    @EnvironmentObject private var service: ServiceController

    var body: some View {
        PageScaffold(title: "客户端与服务", subtitle: Text("官方客户端现在从哪里读登录")) {
            Button { Task { await service.refresh() } } label: {
                HStack(spacing: 5) {
                    Icon(.refresh, size: 12, color: Palette.ink2)
                    Text(service.isRefreshing ? "检查中" : "重新检查")
                }
            }
            .buttonStyle(.pill(.secondary, height: 27))
            .help("只检查本地凭据和 Claude CLI 的登录状态，不请求模型")
            .disabled(service.isRefreshing)
        } content: {
            if let snapshot = service.snapshot {
                VStack(spacing: 0) {
                    ForEach(Array(snapshot.providers.enumerated()), id: \.element.id) { index, provider in
                        providerRow(provider)
                            .overlay(alignment: .top) { if index > 0 { Rectangle().fill(Palette.divider).frame(height: 1) } }
                    }
                }
                .loopCard()
                Text("\(snapshot.verification) 最近检查 \(When.text(snapshot.checkedAt))。")
                    .font(.system(size: 11)).foregroundStyle(Palette.ink3).padding(.horizontal, 3.5).padding(.top, -8)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                ProgressView("正在检查本地认证状态…").frame(maxWidth: .infinity).padding(.vertical, 51)
            }
            serviceCard
        }
    }

    private func providerRow(_ provider: ProviderStatus) -> some View {
        let configured = provider.state == "configured"
        let quiet = ["missing", "unavailable"].contains(provider.state)
        let codex = service.directory?.current("codex")
        let title = provider.name.components(separatedBy: " · ").first ?? provider.name
        return HStack(spacing: 12) {
            GlyphTile(provider: provider.id, size: 31)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 7) {
                    Text(title).font(.system(size: 13, weight: .semibold))
                    Pill(provider.authKind, small: true)
                }
                Group {
                    if provider.id == "pi", configured, let codex {
                        HStack(spacing: 5) {
                            Text("跟着 Codex 在用的账号：")
                            Seal(codex.displayName, style: .inUse, size: 14)
                            Text(codex.displayName).fontWeight(.semibold).foregroundStyle(Palette.ink)
                        }
                    } else {
                        Text(provider.summary)
                    }
                }
                .font(.system(size: 11)).foregroundStyle(Palette.ink3).lineLimit(1)
                .help(provider.expiresAt.map { "本地 access 到期 \(When.exact($0))" } ?? "")
            }
            Spacer(minLength: 7)
            Text(provider.source.replacingOccurrences(of: FileManager.default.homeDirectoryForCurrentUser.path, with: "~"))
                .font(.system(size: 10, design: .monospaced)).foregroundStyle(Palette.ink2)
                .lineLimit(1).truncationMode(.middle)
                .padding(.horizontal, 7.5).frame(height: 20)
                .background(Palette.field, in: RoundedRectangle(cornerRadius: 5, style: .continuous))
            HStack(spacing: 3.5) {
                if configured { Icon(.check, size: 11, color: Palette.ink2) }
                Text(provider.stateLabel)
            }
            .font(.system(size: 11, weight: configured || quiet ? .regular : .semibold))
            .foregroundStyle(configured ? Palette.ink2 : quiet ? Palette.ink3 : Palette.attentionText)
            .frame(width: 54, alignment: .trailing)
        }
        .padding(.horizontal, 17).padding(.vertical, 12)
        .frame(minHeight: 65)
    }

    private var serviceCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Icon(.server, size: 16, color: Palette.solid)
                    .frame(width: 31, height: 31)
                    .background(Palette.wash, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text("后台服务").font(.system(size: 13, weight: .semibold))
                    Text(service.isConnected ? "正常 · \(service.directory?.cloud?.mode == "cloud" ? "云端模式" : "本地模式")"
                         : service.isConnecting ? "正在连接" : "没连上")
                        .font(.system(size: 11)).foregroundStyle(service.isConnected ? Palette.ink3 : Palette.attentionText)
                }
                Spacer()
                Button(service.isConnecting ? "正在重新连接…" : "重新连接") { Task { await service.restart() } }
                    .buttonStyle(.pill(.secondary))
                    .disabled(!service.canRestart)
            }
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                GridRow { Text("服务版本").foregroundStyle(Palette.ink3); Text(service.snapshot?.serviceVersion ?? "—").monospacedDigit() }
                GridRow { Text("连接方式").foregroundStyle(Palette.ink3); Text("本地 Unix socket · 协议 v1") }
                GridRow {
                    Text("服务地址").foregroundStyle(Palette.ink3)
                    Text(service.socketPath.replacingOccurrences(of: FileManager.default.homeDirectoryForCurrentUser.path, with: "~"))
                        .font(.system(size: 10, design: .monospaced)).foregroundStyle(Palette.ink2).textSelection(.enabled)
                }
            }
            .font(.system(size: 11))
            .padding(.top, 12)
            Rectangle().fill(Palette.divider).frame(height: 1).padding(.top, 12)
            Text("退出整个应用时，AuthManager 启动的后台服务也会停止。")
                .font(.system(size: 11)).foregroundStyle(Palette.ink3)
                .padding(.top, 10)
        }
        .padding(.horizontal, 17).padding(.top, 15).padding(.bottom, 15)
        .loopCard()
    }
}
