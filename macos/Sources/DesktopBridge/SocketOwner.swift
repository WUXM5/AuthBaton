import Foundation

enum ConnectionFailure: String, Error {
    case portUnavailable = "debug_port_unavailable"
    case ownerMismatch = "port_owner_mismatch"
}

// lsof lists inherited descriptors once per process, not once per listening socket.
func desktopSocketOwner(listing: String, processes: String, port: Int) throws -> String {
    struct Listener {
        let pid: Int
        let device: String
        let type: String
        let address: String
    }
    var listeners: [Listener] = []
    var pid: Int?, fields: [Character: String] = [:]
    func appendListener() throws {
        guard !fields.isEmpty else { return }
        guard let pid, let device = fields["d"], !device.isEmpty,
              let type = fields["t"], let address = fields["n"] else { throw ConnectionFailure.ownerMismatch }
        listeners.append(Listener(pid: pid, device: device, type: type, address: address))
    }
    for line in listing.split(separator: "\n") {
        guard let field = line.first else { continue }
        let value = String(line.dropFirst())
        if field == "p" {
            try appendListener(); fields = [:]
            guard let number = Int(value), number > 1 else { throw ConnectionFailure.ownerMismatch }
            pid = number
        } else if field == "f" {
            try appendListener(); fields = [:]
        } else {
            fields[field] = value
        }
    }
    try appendListener()
    guard !listeners.isEmpty else { throw ConnectionFailure.portUnavailable }
    guard Set(listeners.map(\.device)).count == 1,
          listeners.allSatisfy({ $0.type == "IPv4" && $0.address == "127.0.0.1:\(port)" }) else { throw ConnectionFailure.ownerMismatch }

    var rows: [Int: (parent: Int, executable: String)] = [:]
    for line in processes.split(separator: "\n") {
        let parts = line.split(maxSplits: 2, omittingEmptySubsequences: true, whereSeparator: \.isWhitespace)
        guard parts.count == 3, let pid = Int(parts[0]), let parent = Int(parts[1]), rows[pid] == nil else { throw ConnectionFailure.ownerMismatch }
        rows[pid] = (parent, String(parts[2]))
    }
    let apps = ["/Applications/ChatGPT.app/Contents/MacOS/ChatGPT": "/Applications/ChatGPT.app",
                "/Applications/Codex.app/Contents/MacOS/Codex": "/Applications/Codex.app"]
    let holders = Set(listeners.map(\.pid))
    let roots = holders.filter { rows[$0].flatMap { apps[$0.executable] } != nil }
    guard roots.count == 1, let root = roots.first, let executable = rows[root]?.executable, let app = apps[executable] else { throw ConnectionFailure.ownerMismatch }
    for holder in holders {
        var current = holder, seen = Set<Int>()
        while current != root {
            guard seen.insert(current).inserted, let parent = rows[current]?.parent, parent > 1 else { throw ConnectionFailure.ownerMismatch }
            current = parent
        }
    }
    return app
}
