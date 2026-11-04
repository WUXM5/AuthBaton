import Foundation

// Safe network switching (v0.22): what the service publishes. It never carries a password: a
// Wi-Fi only says whether AuthBaton remembers one for it.
struct NetworkStatus: Decodable, Sendable {
    let available: Bool
    let current: Current
    // No model reply in flight anywhere right now.
    let safe: Bool
    // The service is reading traffic (a page or the menu bar asked, or a switch runs).
    let watching: Bool
    var monitor: String? = nil
    let sources: [Source]
    let phones: [Phone]
    let networks: [Network]
    var operation: Operation? = nil
    let history: [Record]

    struct Current: Decodable, Sendable {
        // wifi, hotspot, none
        let kind: String
        // Only known when AuthBaton joined it: macOS gives the name to no app without Location Services.
        var name: String? = nil

        var title: String {
            switch kind {
            case "hotspot": name ?? "iPhone 热点"
            case "wifi": name.map { "Wi-Fi · \($0)" } ?? "Wi-Fi"
            default: "没有网络"
            }
        }
    }

    struct Source: Decodable, Sendable, Identifiable {
        let name: String
        // live, held, idle
        let state: String
        // A Claude Code session the mod can hold; the others can only be waited for.
        let gated: Bool
        var id: String { name }
    }

    struct Phone: Decodable, Sendable, Identifiable {
        let name: String
        var battery: Int? = nil
        var network: String? = nil
        var signal: Int? = nil
        var id: String { name }
    }

    struct Network: Decodable, Sendable, Identifiable {
        let ssid: String
        let remembered: Bool
        let current: Bool
        var id: String { ssid }
    }

    struct Operation: Decodable, Sendable {
        // phone, wifi
        let kind: String
        let target: String
        let from: String
        // waiting, waking, joining, checking, done, timeout, cancelled, wrong_password, not_found, failed, unreachable
        let step: String
        let running: Bool
        var ok: Bool? = nil
        var message: String? = nil
        let startedAt: Double
        var waited: Int? = nil
        var remaining: Int? = nil
    }

    struct Record: Decodable, Sendable, Identifiable {
        let at: Double
        let from: String
        let to: String
        let result: String
        var waited: Int? = nil
        var message: String? = nil
        var id: String { "\(at)-\(to)-\(result)" }
    }

    var live: [Source] { sources.filter { $0.state == "live" } }
    var switching: Bool { operation?.running == true }
}

enum NetworkTarget: Hashable, Identifiable, Sendable {
    case phone(String)
    case wifi(String)

    var id: String {
        switch self {
        case .phone(let name): "phone:\(name)"
        case .wifi(let ssid): "wifi:\(ssid)"
        }
    }
    var name: String {
        switch self {
        case .phone(let name), .wifi(let name): name
        }
    }
}
