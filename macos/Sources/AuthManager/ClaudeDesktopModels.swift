import Foundation

// Claude desktop: every account's list of local Code sessions kept the same, and switching the
// desktop's sign-in without signing out. The service owns all of it; this is what it publishes.
struct ClaudeDesktopStatus: Decodable, Equatable, Sendable {
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
    // Quota read through Claude 桌面's own sign-in, for accounts only signed in there. Off until the user turns it on.
    var tokenQuota: TokenQuotaStatus? = nil
    // Seamless same-Team account switching for Claude 桌面's Code tab via vouched Mod in-place swap. Off by default.
    var codeRelay: CodeRelayStatus? = nil
    // Optional Chrome companion extension: swaps claude.ai cookies, resets Claude in Chrome, arms OAuth auto-approve.
    var chromeSync: ChromeSyncStatus? = nil

    struct ChromeSyncStatus: Decodable, Equatable, Sendable {
        let enabled: Bool
        // not_installed, installed, needs_update
        let installState: String
        let extensionId: String
        var extensionDir: String? = nil
        let connected: Bool
        let claudeExtInstalled: Bool
        var claudeExtEnabled: Bool = false
        var claudeExtVersion: String? = nil
        var claudeExtMayDisable: Bool = false
        // off, not_installed, waiting_extension, syncing, awaiting_login, synced, mismatch, missing_session, error
        let state: String
        let message: String
        var busy: Bool = false
        var chromeEmail: String? = nil
        var chromeHandle: String? = nil
        var desktopMatches: Bool = false
        var desktopSaved: Bool = false
        var oauthArmedUntil: Double? = nil
        var savedAccounts: [SavedAccount] = []
        var lastSyncAt: Double? = nil
        var lastError: String? = nil

        var isInstalled: Bool { installState == "installed" || installState == "needs_update" }

        struct SavedAccount: Decodable, Equatable, Sendable, Identifiable {
            let handle: String
            let email: String
            var orgs: [Org] = []
            var cookieCount: Int = 0
            let savedAt: Double
            let updatedAt: Double
            let inChrome: Bool
            let inDesktop: Bool

            var id: String { handle }

            struct Org: Decodable, Equatable, Sendable {
                let name: String
            }
        }
    }

    struct CodeRelayStatus: Decodable, Equatable, Sendable {
        let enabled: Bool
        // not_installed, installed, needs_update, blocked_remote
        let installState: String
        // off, active, waiting_account, team_mismatch, blocked_remote, installing, uninstalling, error
        let state: String
        let message: String
        var activeLabel: String? = nil
        var sameTeam: Bool? = nil
        var busy: Bool = false
        var lastError: String? = nil
        var relayedCount: Int = 0
        var lastSwitchAt: Double? = nil

        var isInstalled: Bool { installState == "installed" || installState == "needs_update" }
    }

    struct TokenQuotaStatus: Decodable, Equatable, Sendable {
        let enabled: Bool
        // off, waiting_key (macOS is asking), key_denied, ready, error.
        let state: String
        let message: String
    }

    // One account's quota read with Claude 桌面's cached access: ready, checking, no_token, unauthorized, limited, error.
    struct DeskQuota: Decodable, Equatable, Sendable {
        let state: String
        let message: String
        var checkedAt: Double? = nil
        var windows: [AccountQuota] = []

        // The 5-hour and weekly windows, as AuthBaton's own quota shows them.
        var primary: [AccountQuota] { Array(windows.filter { $0.kind == nil }.prefix(2)) }
    }

    struct Picks: Decodable, Equatable, Sendable {
        var next: String? = nil
        var nextAvailable: String? = nil
        var best: String? = nil
    }

    struct LastSync: Decodable, Equatable, Sendable {
        let at: Double
        let ok: Bool
        let message: String
    }

    struct Account: Decodable, Equatable, Sendable, Identifiable {
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
        // Claude allows bypass permissions mode for this account (its Settings → Claude Code).
        var bypass: Bool? = nil
        // Present only while quota through the desktop sign-in is on and this account has no AuthBaton login.
        var tokenQuota: DeskQuota? = nil

        var id: String { handle }
        var canSwitch: Bool { known && loginSaved && !inUse }
        // "#2 work": how sheets and the menu bar name an account.
        var title: String { number.map { "#\($0) \(display)" } ?? display }
    }

    struct Usage: Decodable, Equatable, Sendable {
        let remaining: Int
        let at: Double
        // Older than the window it reads, so it says little about now.
        let stale: Bool
    }

    struct Snapshot: Decodable, Equatable, Sendable, Identifiable {
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

    struct Retention: Decodable, Equatable, Sendable {
        let atRisk: Bool
        var days: Int? = nil
        let kept: Bool
        let canUndo: Bool
        // false when the organization sets the limit.
        let canKeep: Bool
    }

    struct Operation: Decodable, Equatable, Sendable {
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
    // Switching to `target` turns bypass permissions mode on for it: the account in use allows it, `target` does not.
    func turnsOnBypass(_ target: Account?) -> Bool { current?.bypass == true && target.map { $0.bypass != true } == true }
}
