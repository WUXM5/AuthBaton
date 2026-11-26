import AppKit
import Combine
import SwiftUI

// Which tool the menu bar shows (11-25, Gemini 稿): by default the page last picked in the window, Codex or
// Claude; the right-click menu can fix it to one.
enum MenuBarShows: String {
    case follow, codex, claude

    static let key = "menubar.shows"
    // The tool page last open in the window.
    static let lastKey = "menubar.lastPage"

    static func provider(_ setting: String, last: String) -> String {
        switch MenuBarShows(rawValue: setting) ?? .follow {
        case .codex: "codex"
        case .claude: "claude"
        case .follow: last == "claude" ? "claude" : "codex"
        }
    }

    @MainActor static var provider: String {
        provider(UserDefaults.standard.string(forKey: key) ?? "", last: UserDefaults.standard.string(forKey: lastKey) ?? "")
    }
}

// The menu bar item. Hovering opens the panel and moving away closes it; a click keeps it open until a click
// elsewhere; a right click offers what it shows, refresh, the window and quit.
@MainActor
final class MenuBarController: NSObject, ObservableObject, NSPopoverDelegate {
    static let shared = MenuBarController()
    // 退出 from the right-click menu while cloud accounts are in use: the panel asks, as its own 退出 does.
    @Published var quitAsked = false
    private var item: NSStatusItem?
    private let popover = NSPopover()
    private var pinned = false
    private var overButton = false
    private var overPanel = false
    private var pending: DispatchWorkItem?
    private var watching: Set<AnyCancellable> = []
    private var openWindow: () -> Void = {}

    func start(openWindow: @escaping () -> Void) {
        guard item == nil else { return }
        self.openWindow = openWindow
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        self.item = item
        if let button = item.button {
            button.target = self
            button.action = #selector(clicked)
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
            button.imagePosition = .imageLeading
            button.font = .monospacedDigitSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
            let hover = HoverArea { [weak self] inside in
                self?.overButton = inside
                self?.settle()
            }
            hover.frame = button.bounds
            hover.autoresizingMask = [.width, .height]
            button.addSubview(hover)
        }
        let panel = MenuBarPanel(openWindow: { [weak self] in self?.showWindow() })
            .onHover { [weak self] inside in
                self?.overPanel = inside
                self?.settle()
            }
            .environmentObject(ServiceController.shared)
        let host = NSHostingController(rootView: panel)
        // The panel grows and shrinks as rows open; the popover follows its height.
        host.sizingOptions = [.preferredContentSize]
        popover.contentViewController = host
        popover.behavior = .applicationDefined
        popover.delegate = self
        watch()
        refresh()
    }

    // The label redraws when the service publishes, the window changes page, or the setting changes.
    private func watch() {
        let service = ServiceController.shared
        AppNavigation.shared.$page
            .sink { page in
                if page == .codex || page == .claude { UserDefaults.standard.set(page == .claude ? "claude" : "codex", forKey: MenuBarShows.lastKey) }
            }
            .store(in: &watching)
        Publishers.MergeMany(
            service.objectWillChange.map { _ in () }.eraseToAnyPublisher(),
            NotificationCenter.default.publisher(for: UserDefaults.didChangeNotification).map { _ in () }.eraseToAnyPublisher()
        )
        // objectWillChange fires before the change: read it on the next turn of the run loop.
        .receive(on: RunLoop.main)
        .sink { [weak self] in self?.refresh() }
        .store(in: &watching)
    }

    private func refresh() {
        guard let button = item?.button else { return }
        let service = ServiceController.shared
        let directory = service.directory
        let provider = MenuBarShows.provider
        let lowest = directory.flatMap { directory in
            provider == "claude"
                ? directory.claudeLowest(ClaudeUse.resolve(UserDefaults.standard.string(forKey: ClaudeUse.key) ?? "auto", directory))
                : directory.current("codex")?.lowestPrimary
        }
        let urgent = (directory?.attentionCount ?? 0) > 0
        let reminded = !ExpiryReminder.all(directory?.accounts ?? [], dismissed: ReminderStore.dismissed()).isEmpty
        let offline = !service.isConnected && !service.isConnecting
        let image = BrandArtwork.menuIcon(badge: offline || urgent ? .systemRed : reminded ? .systemOrange : nil)
        let title = lowest.map { " \($0)%" } ?? ""
        if button.image !== image { button.image = image }
        if button.title != title { button.title = title }
        button.setAccessibilityLabel(offline ? "AuthBaton，后台服务未连接" : urgent ? "AuthBaton，有待处理的事项"
                                     : "AuthBaton" + (lowest.map { "，\(GlyphTile.name(provider)) 剩 \($0)%" } ?? ""))
    }

    // Opening waits a moment so passing over the icon does nothing; closing waits so the pointer can cross
    // the gap between the icon and the panel.
    private func settle() {
        pending?.cancel()
        guard !pinned else { return }
        let inside = overButton || overPanel
        guard inside != popover.isShown else { return }
        let work = DispatchWorkItem { [weak self] in
            guard let self, !self.pinned else { return }
            if self.overButton || self.overPanel { self.show() } else { self.popover.performClose(nil) }
        }
        pending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + (inside ? 0.18 : 0.25), execute: work)
    }

    private func show() {
        guard let button = item?.button, !popover.isShown else { return }
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
    }

    @objc private func clicked() {
        if let event = NSApp.currentEvent, event.type == .rightMouseUp || event.modifierFlags.contains(.control) {
            showMenu()
            return
        }
        pending?.cancel()
        if popover.isShown && pinned {
            popover.performClose(nil)
            return
        }
        // Kept open: a click anywhere else closes it.
        pinned = true
        popover.behavior = .transient
        show()
    }

    func popoverDidClose(_ notification: Notification) {
        pinned = false
        popover.behavior = .applicationDefined
    }

    private func showMenu() {
        pending?.cancel()
        popover.performClose(nil)
        let menu = NSMenu()
        let heading = NSMenuItem(title: "菜单栏显示", action: nil, keyEquivalent: "")
        heading.isEnabled = false
        menu.addItem(heading)
        let setting = MenuBarShows(rawValue: UserDefaults.standard.string(forKey: MenuBarShows.key) ?? "") ?? .follow
        for (shows, title) in [(MenuBarShows.follow, "跟随主窗口"), (.codex, "Codex"), (.claude, "Claude")] {
            let entry = NSMenuItem(title: title, action: #selector(pick(_:)), keyEquivalent: "")
            entry.target = self
            entry.representedObject = shows.rawValue
            entry.state = setting == shows ? .on : .off
            entry.indentationLevel = 1
            menu.addItem(entry)
        }
        menu.addItem(.separator())
        menu.addItem(entry("刷新额度", key: "r", action: #selector(refreshQuota)))
        menu.addItem(entry("打开 AuthBaton", key: "", action: #selector(openFromMenu)))
        menu.addItem(.separator())
        menu.addItem(entry("退出 AuthBaton", key: "q", action: #selector(quit)))
        item?.menu = menu
        item?.button?.performClick(nil)
        // Cleared again so the next left click opens the panel, not the menu.
        item?.menu = nil
    }

    private func entry(_ title: String, key: String, action: Selector) -> NSMenuItem {
        let entry = NSMenuItem(title: title, action: action, keyEquivalent: key)
        entry.target = self
        return entry
    }

    @objc private func pick(_ sender: NSMenuItem) {
        UserDefaults.standard.set(sender.representedObject as? String ?? MenuBarShows.follow.rawValue, forKey: MenuBarShows.key)
    }

    @objc private func refreshQuota() {
        let service = ServiceController.shared
        Task {
            await service.refresh()
            await service.refreshQuotaIfNeeded()
        }
    }

    @objc private func openFromMenu() { showWindow() }

    @objc private func quit() {
        guard ServiceController.shared.directory?.cloudInUse.isEmpty == false else {
            NSApplication.shared.terminate(nil)
            return
        }
        pinned = true
        popover.behavior = .transient
        show()
        quitAsked = true
    }

    private func showWindow() {
        popover.performClose(nil)
        openWindow()
        NSApplication.shared.activate(ignoringOtherApps: true)
    }
}

// Tells the controller when the pointer enters or leaves the menu bar icon; clicks pass through to it.
private final class HoverArea: NSView {
    private let changed: (Bool) -> Void

    init(changed: @escaping (Bool) -> Void) {
        self.changed = changed
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { nil }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
    }

    override func mouseEntered(with event: NSEvent) { changed(true) }
    override func mouseExited(with event: NSEvent) { changed(false) }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
