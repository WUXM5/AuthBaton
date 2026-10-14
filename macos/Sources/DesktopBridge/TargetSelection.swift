import Foundation

// The primary page and auxiliary windows share index.html and electronBridge.
// Focus cannot identify the primary page while the user clicks AuthManager.
enum DesktopTargets {
    static func isPrimaryPage(_ target: [String: Any]) -> Bool {
        guard target["type"] as? String == "page", let address = target["url"] as? String,
              let url = URLComponents(string: address), url.scheme == "app", url.host == "-",
              url.path == "/index.html" else { return false }
        let route = url.queryItems?.first { $0.name == "initialRoute" }?.value ?? ""
        // Codex keeps a hidden page window warm (`/space/…?window=page&prewarm=1`) and rebuilds it after
        // every account change; counting it leaves two candidates whenever no target is bound yet.
        if URLComponents(string: route)?.queryItems?.contains(where: { $0.name == "prewarm" && $0.value == "1" }) == true {
            return false
        }
        return !["/avatar-overlay", "/global-dictation", "/hotkey-window",
                 "/chatgpt/quick-chat-prewarm", "/chatgpt/quick-chat", "/debug"].contains {
            route == $0 || route.hasPrefix($0 + "/") || route.hasPrefix($0 + "?")
        }
    }

    // While the earlier target is still listed only it is probed. Once it is gone (the main window was
    // reopened or the desktop restarted) every primary page is, so select can choose afresh.
    static func probed(_ candidates: [[String: Any]], previous: String?) -> [[String: Any]] {
        let bound = candidates.filter { previous != nil && $0["id"] as? String == previous }
        return bound.isEmpty ? candidates : bound
    }

    struct Observation {
        let id: String
        let supported: Bool
        let compact: Bool
        let focused: Bool
    }
    static func select(_ observations: [Observation], previous: String?) -> String? {
        let primary = observations.filter { $0.supported && !$0.compact }
        let choices: [Observation]
        // A target from an earlier desktop run is gone, not ambiguous: choose afresh.
        if let previous, observations.contains(where: { $0.id == previous }) { choices = primary.filter { $0.id == previous } }
        else if primary.count == 1 { choices = primary }
        else { choices = primary.filter(\.focused) }
        return choices.count == 1 ? choices[0].id : nil
    }
}
