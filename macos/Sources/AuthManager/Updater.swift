import AppKit
import Sparkle

// Sparkle runs only in builds that name a feed: scripts/build.sh writes SUFeedURL when AUTHMANAGER_UPDATE_FEED is set.
// Updates download on their own and install as soon as AuthBaton is left alone (11-28, 用户「同意方案A」): while one waits
// for the app to quit, Sparkle resumes it instead of reading the feed, and AuthBaton seldom quits, so releases in a row
// were installed one by one. Quitting still installs one that has not gone in yet.
@MainActor
final class AppUpdater: NSObject, SPUUpdaterDelegate {
    static let shared = AppUpdater()
    static let enabled = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil

    private var controller: SPUStandardUpdaterController?
    private var pendingRelaunch: (() -> Void)?
    private var pendingInstall: (() -> Void)?

    func start() {
        guard Self.enabled, controller == nil else { return }
        controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: self, userDriverDelegate: nil)
    }

    func checkForUpdates() {
        controller?.checkForUpdates(nil)
    }

    // "Install and Relaunch" quits the app and the service with it. Wait until no login or operation is in flight and
    // no sheet is open: AppKit refuses to quit while a window has a sheet, and Sparkle does not ask a second time.
    private var canRelaunch: Bool {
        ServiceController.shared.canRestart && NSApp.windows.allSatisfy { $0.attachedSheet == nil }
    }

    // A relaunch on its own cuts nothing short and nobody is looking: no switch, batch, transfer or network change in
    // flight, and neither the window nor the menu bar panel is open (the menu bar item's own window always is).
    private var unattended: Bool {
        guard canRelaunch else { return false }
        let working = ServiceController.shared.directory.map { d in
            d.desktop?.busy == true || d.desktop?.action?.pending == true || d.strategy?.busy == true
                || d.cloud?.busy == true || d.claudeDesktop?.busy == true || d.network?.switching == true
        } ?? false
        return !working && !NSApp.windows.contains { $0.isVisible && $0.className != "NSStatusBarWindow" }
    }

    func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem, untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        guard !canRelaunch else { return false }
        pendingRelaunch = installHandler
        Task { await relaunchWhenIdle() }
        return true
    }

    // A downloaded update goes in once AuthBaton has been left alone for 30 seconds; Sparkle relaunches it.
    func updater(_ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem, immediateInstallationBlock immediateInstallHandler: @escaping () -> Void) -> Bool {
        let waiting = pendingInstall != nil
        pendingInstall = immediateInstallHandler
        if !waiting { Task { await installWhenUnattended() } }
        return true
    }

    private func relaunchWhenIdle() async {
        while !canRelaunch {
            try? await Task.sleep(for: .seconds(2))
        }
        let relaunch = pendingRelaunch
        pendingRelaunch = nil
        relaunch?()
    }

    private func installWhenUnattended() async {
        var quietSince: Date?
        while let install = pendingInstall {
            if !unattended {
                quietSince = nil
            } else if let since = quietSince, Date().timeIntervalSince(since) >= 30 {
                // Sparkle 2.3+ takes it again if the quit is turned down; keep it until the app is gone.
                install()
                quietSince = nil
            } else if quietSince == nil {
                quietSince = Date()
            }
            try? await Task.sleep(for: .seconds(5))
        }
    }
}
