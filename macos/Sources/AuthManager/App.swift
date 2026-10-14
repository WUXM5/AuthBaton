import AppKit
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    var reopenWindow: (() -> Void)?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApplication.shared.applicationIconImage = BrandArtwork.appIcon
        // The service runs with the app, not with the window: closing the window must not cut its start short.
        Task { await ServiceController.shared.start() }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationDidBecomeActive(_ notification: Notification) {
        Task { await ServiceController.shared.refreshQuotaIfNeeded() }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { reopenWindow?() }
        return true
    }

    func applicationWillTerminate(_ notification: Notification) {
        ServiceController.shared.stop()
    }
}

@main
struct AuthManagerApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @StateObject private var service = ServiceController.shared

    var body: some Scene {
        Window("AuthManager", id: "main") {
            ManagedWindow(delegate: delegate).environmentObject(service)
        }
        // The sidebar runs up under the traffic lights; pages carry their own titles.
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 1020, height: 660)
        .commands {
            CommandGroup(after: .newItem) {
                Button("刷新状态与额度") { Task { await service.refresh(); await service.refreshQuotaIfNeeded() } }
                    .keyboardShortcut("r")
                    .disabled(service.isRefreshing)
            }
        }

        MenuBarExtra {
            MenuBarContent().environmentObject(service)
        } label: {
            MenuBarLabel().environmentObject(service)
        }
        .menuBarExtraStyle(.window)
    }
}

private struct MenuBarContent: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        MenuBarPanel(openWindow: {
            openWindow(id: "main")
            NSApplication.shared.activate(ignoringOtherApps: true)
        })
    }
}

private struct ManagedWindow: View {
    let delegate: AppDelegate
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        DashboardView().onAppear {
            delegate.reopenWindow = {
                openWindow(id: "main")
                Task { await ServiceController.shared.refreshQuotaIfNeeded() }
            }
        }
    }
}
