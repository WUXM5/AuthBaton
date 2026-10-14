import AppKit
import SwiftUI

enum Page: Hashable {
    case accounts, automation, desktop, clients, cloud
}

// Shared so the menu bar panel can open the window on a specific page or account.
@MainActor
final class AppNavigation: ObservableObject {
    static let shared = AppNavigation()
    @Published var page: Page = .accounts
    @Published var filter: String?
    @Published var selection: String?

    func show(_ page: Page, filter: String? = nil, select: String? = nil) {
        self.page = page
        self.filter = filter
        if let select { selection = select }
    }
}

private enum SidebarItem: Hashable {
    case accounts(String?)
    case page(Page)
}

struct DashboardView: View {
    @EnvironmentObject private var service: ServiceController
    @ObservedObject private var nav = AppNavigation.shared

    init(initialPage: Page? = nil) {
        if let initialPage { AppNavigation.shared.page = initialPage }
    }

    private var cloudMode: Bool { service.directory?.cloud?.mode == "cloud" }

    var body: some View {
        NavigationSplitView {
            sidebar.navigationSplitViewColumnWidth(min: 200, ideal: 216, max: 250)
        } detail: {
            VStack(spacing: 0) {
                if let error = service.errorText {
                    Callout(icon: "bolt.horizontal.circle.fill", tone: .danger, message: error) {
                        if !service.isConnected { ReconnectButton().controlSize(.small) }
                    }
                    .padding(.horizontal, 20).padding(.top, 12)
                }
                switch nav.page {
                case .accounts:
                    if cloudMode {
                        LegacyPage(title: "云端账号") { CloudAccountsView(openSettings: { nav.show(.cloud) }) }
                    } else {
                        AccountsView()
                    }
                case .automation: AutomationPage()
                case .desktop: DesktopPage()
                case .clients: ClientsPage()
                case .cloud: LegacyPage(title: "个人云端", subtitle: cloudMode ? "云端模式" : "本地模式") { CloudConnectionView() }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .background(Palette.card)
        }
        .toolbar(removing: .sidebarToggle)
        .frame(minWidth: 980, minHeight: 620)
        .tint(Palette.accent)
    }

    private var selectionBinding: Binding<SidebarItem?> {
        Binding(get: {
            nav.page == .accounts ? .accounts(nav.filter) : .page(nav.page)
        }, set: { item in
            switch item {
            case .accounts(let filter): nav.show(.accounts, filter: filter)
            case .page(let page): nav.show(page)
            case nil: break
            }
        })
    }

    private var sidebar: some View {
        let directory = service.directory
        let accounts = directory?.accounts ?? []
        let desktop = directory?.desktop
        let strategyOn = directory?.strategy?.config.enabled == true
        return List(selection: selectionBinding) {
            Section {
                Label("账号", systemImage: "person.2")
                    .badge(directory?.attentionCount ?? 0)
                    .tag(SidebarItem.accounts(nil))
            }
            if !cloudMode {
                Section("按工具") {
                    ForEach(["codex", "claude"], id: \.self) { provider in
                        Label(ProviderGlyph.name(provider), systemImage: provider == "codex" ? "terminal" : "sparkle")
                            .badge(Text("\(accounts.filter { $0.provider == provider }.count)").foregroundStyle(.tertiary))
                            .tag(SidebarItem.accounts(provider))
                    }
                }
            }
            Section("自动化") {
                sidebarRow("换号与重置", icon: "bolt", tone: strategyOn ? .success : nil, text: strategyOn ? nil : "关")
                    .tag(SidebarItem.page(.automation))
            }
            Section("连接") {
                sidebarRow("Codex 桌面", icon: "macwindow",
                           tone: desktop?.enabled == true ? (desktop?.managed == true ? .success : .warning) : nil,
                           text: desktop?.enabled == true ? nil : "关")
                    .tag(SidebarItem.page(.desktop))
                Label("客户端与服务", systemImage: "square.grid.2x2").tag(SidebarItem.page(.clients))
                sidebarRow("个人云端", icon: "cloud", tone: cloudMode ? .accent : nil,
                           text: cloudMode ? nil : (directory?.cloud?.paired == true ? "已配对" : "未配对"))
                    .tag(SidebarItem.page(.cloud))
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .top, spacing: 0) {
            HStack(spacing: 10) {
                BrandMark().padding(5).frame(width: 28, height: 28)
                    .background(Color(white: 0.13), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                VStack(alignment: .leading, spacing: 1) {
                    Text("AuthManager").font(.system(size: 13, weight: .semibold))
                    Text(cloudMode ? "个人云端认证" : "本地认证工作台").font(.caption2).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16).padding(.top, 34).padding(.bottom, 8)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Divider().padding(.bottom, 6)
                HStack(spacing: 7) {
                    Dot(tone: service.isConnected ? .success : service.isConnecting ? .warning : .danger)
                    Text(service.isConnected ? "后台服务正常" : service.isConnecting ? "正在连接后台服务" : "后台服务未连接")
                        .font(.caption.weight(.medium))
                    Text("· \(cloudMode ? "云端模式" : "本地模式")").font(.caption).foregroundStyle(.secondary)
                }
                Text("v\(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "开发版") · 关窗后驻留菜单栏")
                    .font(.caption2).foregroundStyle(.tertiary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16).padding(.bottom, 12)
        }
    }

    private func sidebarRow(_ title: String, icon: String, tone: Tone?, text: String?) -> some View {
        let badge: Text? = text.map { Text($0).foregroundStyle(.tertiary) } ?? tone.map { Text("●").foregroundStyle($0.color) }
        return Label(title, systemImage: icon).badge(badge)
    }
}

// One reconnect at a time: a second click while the first waits for the old service would
// otherwise start a lock owner of its own.
struct ReconnectButton: View {
    @EnvironmentObject private var service: ServiceController

    var body: some View {
        Button(service.isConnecting ? "正在重新连接…" : "重新连接后台服务") { Task { await service.restart() } }
            .disabled(!service.canRestart)
    }
}

// Pages that keep their original content under the toolbar title.
struct LegacyPage<Content: View>: View {
    let title: String
    var subtitle: String? = nil
    @ViewBuilder var content: Content

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) { content }
                .padding(24)
                .frame(maxWidth: 900, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .pageBar(title, subtitle: subtitle)
    }
}

// MARK: 客户端与服务

struct ClientsPage: View {
    @EnvironmentObject private var service: ServiceController

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if let snapshot = service.snapshot {
                        VStack(spacing: 0) {
                            ForEach(Array(snapshot.providers.enumerated()), id: \.element.id) { index, provider in
                                if index > 0 { Divider() }
                                providerRow(provider)
                            }
                        }
                        .card(padding: 16)
                        Label("\(snapshot.verification) 最近检查 \(When.text(snapshot.checkedAt))。", systemImage: "info.circle")
                            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    } else {
                        ProgressView("正在检查本地认证状态…").frame(maxWidth: .infinity).padding(.vertical, 60)
                    }
                    serviceCard
                }
                .padding(24)
                .frame(maxWidth: 900, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .pageBar("客户端与服务", subtitle: "官方客户端现在从哪里读登录") {
            Button(service.isRefreshing ? "刷新中" : "刷新", systemImage: "arrow.clockwise") {
                Task { await service.refresh() }
            }
            .disabled(service.isRefreshing)
        }
    }

    private func providerRow(_ provider: ProviderStatus) -> some View {
        let tone: Tone = provider.state == "configured" ? .success : ["missing", "unavailable"].contains(provider.state) ? .neutral : .warning
        return HStack(alignment: .top, spacing: 12) {
            ProviderGlyph(provider: provider.id, size: 30)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 8) {
                    Text(provider.name).font(.callout.weight(.semibold))
                    Text(provider.authKind).font(.caption).foregroundStyle(.secondary)
                }
                Text(provider.summary).font(.callout).foregroundStyle(.secondary)
                if let expiry = provider.expiresAt {
                    Text("本地 access 到期 \(When.text(expiry))").font(.caption).foregroundStyle(.secondary).help(When.exact(expiry))
                }
                Text(provider.source.replacingOccurrences(of: FileManager.default.homeDirectoryForCurrentUser.path, with: "~"))
                    .font(.system(size: 11, design: .monospaced)).foregroundStyle(.tertiary).lineLimit(1).truncationMode(.middle)
            }
            Spacer()
            HealthLabel(health: Health(tone: tone, title: provider.stateLabel, detail: nil))
        }
        .padding(.vertical, 10)
    }

    private var serviceCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 10) {
                Image(systemName: service.isConnected ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                    .font(.system(size: 20)).foregroundStyle(service.isConnected ? Palette.success : Palette.warning)
                VStack(alignment: .leading, spacing: 2) {
                    Text(service.isConnected ? "后台服务已连接" : "后台服务不可用").font(.callout.weight(.semibold))
                    Text("退出整个应用时，本应用启动的后台服务也会停止。").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                ReconnectButton()
            }
            Divider()
            Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 8) {
                GridRow { Text("服务版本").foregroundStyle(.secondary); Text(service.snapshot?.serviceVersion ?? "—").monospacedDigit() }
                GridRow { Text("连接方式").foregroundStyle(.secondary); Text("本地 Unix socket · 协议 v1") }
                GridRow {
                    Text("服务地址").foregroundStyle(.secondary)
                    Text(service.socketPath.replacingOccurrences(of: FileManager.default.homeDirectoryForCurrentUser.path, with: "~"))
                        .font(.system(size: 11.5, design: .monospaced)).textSelection(.enabled)
                }
            }
            .font(.callout)
        }
        .card(padding: 16)
    }
}
