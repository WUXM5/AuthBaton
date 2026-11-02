import Foundation

// Claude desktop: every account's list of local Code sessions kept the same, and switching the
// desktop's sign-in without signing out. The service owns all of it; this is what it publishes.
struct ClaudeDesktopStatus: Decodable, Sendable {
    let installed: Bool
    let running: Bool
    var version: String? = nil
    let switchSupported: Bool
    let background: Bool
    let syncing: Bool
    let headline: String
    // Why the sidebar shows a dot; nil when nothing needs a look.
    var attention: String? = nil
    let sessions: Int
    let pending: Int
    let restartNeeded: Bool
    var lastSync: LastSync? = nil
    let accounts: [Account]
    let snapshots: [Snapshot]
    var retention: Retention? = nil
    let symlinks: Int
    var operation: Operation? = nil
    // Who each way of picking the next account would switch to right now.
    var picks: Picks? = nil

    struct Picks: Decodable, Sendable {
        var next: String? = nil
        var nextAvailable: String? = nil
        var best: String? = nil
    }

    struct LastSync: Decodable, Sendable {
        let at: Double
        let ok: Bool
        let message: String
    }

    struct Account: Decodable, Sendable, Identifiable {
        // An opaque handle the service maps back to the account; never its ID.
        let handle: String
        var number: Int? = nil
        let display: String
        var email: String? = nil
        var alias: String? = nil
        let inUse: Bool
        let loginSaved: Bool
        let participates: Bool
        let sessions: Int
        let behind: Int
        var fiveHour: Usage? = nil
        var weekly: Usage? = nil
        // The AuthBaton Claude Code account signed in as the same person, when there is one.
        var linkedAccount: String? = nil
        // false: only session folders, from before AuthBaton kept this account's sign-in.
        let known: Bool

        var id: String { handle }
        var canSwitch: Bool { known && loginSaved && !inUse }
        // "#2 work": how sheets and the menu bar name an account.
        var title: String { number.map { "#\($0) \(display)" } ?? display }
    }

    struct Usage: Decodable, Sendable {
        let remaining: Int
        let at: Double
        // Older than the window it reads, so it says little about now.
        let stale: Bool
    }

    struct Snapshot: Decodable, Sendable, Identifiable {
        let id: String
        let at: Double
        let kind: String
        // Session lists it holds (one per account and organization).
        let folders: Int

        var kindLabel: String {
            switch kind {
            case "baseline": "首次"
            case "pre-restore": "恢复前"
            default: "每小时"
            }
        }
    }

    struct Retention: Decodable, Sendable {
        let atRisk: Bool
        var days: Int? = nil
        let kept: Bool
        let canUndo: Bool
        // false when the organization sets the limit.
        let canKeep: Bool
    }

    struct Operation: Decodable, Sendable {
        // switch, add, sync, restore, fix, retention
        let kind: String
        var target: String? = nil
        // quitting, asking, after_work, updating, moving, launching, waiting_sign_in, working, done, failed, stopped
        let step: String
        var message: String? = nil
        let running: Bool
        var ok: Bool? = nil

        var waitsOnClaude: Bool { running && ["quitting", "asking", "after_work"].contains(step) }
    }

    var current: Account? { accounts.first { $0.inUse } }
    var known: [Account] { accounts.filter(\.known) }
    var onlyFolders: [Account] { accounts.filter { !$0.known } }
    var busy: Bool { operation?.running == true }
}
