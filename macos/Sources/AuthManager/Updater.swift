import AppKit
import Sparkle

// Sparkle runs only in builds that name a feed: scripts/build.sh writes SUFeedURL when AUTHMANAGER_UPDATE_FEED is set.
// Updates download on their own and install when the app quits; Sparkle asks only if the app stays open for a long time.
@MainActor
final class AppUpdater: NSObject, SPUUpdaterDelegate {
    static let shared = AppUpdater()
    static let enabled = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") != nil

    private var controller: SPUStandardUpdaterController?
    private var pendingRelaunch: (() -> Void)?

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

    func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem, untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        guard !canRelaunch else { return false }
        pendingRelaunch = installHandler
        Task { await relaunchWhenIdle() }
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
}
