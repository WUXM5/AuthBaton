import Foundation

// Static discovery from the already verified official listener's bundle. Names/hashes are
// not a version allowlist; the renderer still validates the actual objects before installing.
enum SelectiveSymbols {
    struct Reference { let name: String; let providerScope: Bool }
    struct Mapping { let boundary: String; let references: [String: Reference] }
    static let roles = ["SSl", "_Sl", "qwl", "gf", "$", "Qd", "Up", "Csn", "O_", "X3", "loi", "OS"]
    private static let id = "[A-Za-z_$][\\w$]{0,63}"
    private static func matches(_ pattern: String, _ source: String) throws -> [[String]] {
        let anchored = pattern.hasPrefix("(" + id + ")=") ? "(?<![\\w$])" + pattern : pattern
        let regex = try NSRegularExpression(pattern: anchored)
        let text = source as NSString
        return regex.matches(in: source, range: NSRange(location: 0, length: text.length)).map { match in
            (0..<match.numberOfRanges).map { match.range(at: $0).location == NSNotFound ? "" : text.substring(with: match.range(at: $0)) }
        }
    }
    static func discover(modules: [String: String]) throws -> Mapping {
        guard let main = modules.keys.filter({ $0.hasPrefix("app-initial-") }).only else { throw UIAdapterFailure.unsupported }
        var found: [String: [(String, String)]] = [:]
        func bind(_ role: String, _ module: String, _ symbol: String) { found[role, default: []].append((module, symbol)) }
        for (name, source) in modules {
            let regex = try NSRegularExpression(pattern: "function (" + id + ")\\(")
            let text = source as NSString
            let functions = regex.matches(in: source, range: NSRange(location: 0, length: text.length))
            for (index, function) in functions.enumerated() {
                let end = index + 1 < functions.count ? functions[index + 1].range.location : text.length
                let body = text.substring(with: NSRange(location: function.range.location, length: end - function.range.location))
                let symbol = text.substring(with: function.range(at: 1))
                if body.contains(".useSyncExternalStore)") && body.contains(".identityKey,") && body.contains(".Fragment,") {
                    bind("_Sl", name, symbol)
                    for m in try matches("(" + id + ")\\.subscribeStatus,\\1\\.identityKey,", body) { bind("Up", name, m[1]) }
                    for m in try matches("\\(0,(" + id + ")\\.jsx\\)\\(" + id + "\\.Fragment,", body) { bind("SSl", name, m[1]) }
                }
                if body.contains("`toggle-query-devtools`,") && body.contains(".useState)") { bind("qwl", name, symbol) }
                if body.contains("`Missing parent scope`") { bind("gf", name, symbol) }
                if body.contains("getOwnValue(") && body.contains("Missing query client") && body.contains("s={chain:t,get:") { bind("Qd", name, symbol) }
            }
            let anchors: [(String, String)] = [
                ("$", "(" + id + ")=(" + id + ")\\(`AppScope`\\)"),
                ("OS", "(" + id + ")=(" + id + ")\\(`RouteScope`,"),
                // The effect reads a retry counter and sets `loading` before it reads accessInputs.
                ("Csn", "(" + id + ")=(" + id + ")\\(" + id + ",\\(\\)=>`loading`,\\(\\{get:" + id + ",scope:" + id + ",set:" + id + "\\}\\)=>\\{[^{};]{0,80};let " + id + "=" + id + "\\?\\.accessInputs;"),
                // The host registry, and the manager factory its registerHost calls before addManager.
                ("X3", "(" + id + ")=class\\{scope;constructor\\(e\\)\\{this.scope=e\\}registerHost\\(e\\)\\{[^{}]*\\}refreshRendererManager\\(e\\)\\{"),
                ("loi", "registerHost\\(e\\)\\{[^{}]*let t=(" + id + ")\\(this\\.scope,e\\);t!=null&&this\\.addManager\\(t\\)\\}"),
                ("O_", "\\)\\),(" + id + ")=" + id + "\\(" + id + ",\\(e,\\{get:t\\}\\)=>t\\(" + id + ",e\\)\\.client\\)"),
            ]
            for (role, pattern) in anchors { for m in try matches(pattern, source) { bind(role, name, m[1]) } }
        }
        guard roles.allSatisfy({ found[$0]?.count == 1 }), found["_Sl"]?.first?.0 == main else { throw UIAdapterFailure.unsupported }
        var references: [String: Reference] = [:]
        for role in roles {
            let (module, symbol) = found[role]![0]
            if module == main { references[role] = Reference(name: symbol, providerScope: false); continue }
            let exports = modules[module]!.components(separatedBy: "export{").last!
            let aliases = try matches("(?:^|,)" + NSRegularExpression.escapedPattern(for: symbol) + " as (" + id + ")", exports)
            var names: [String] = []
            if let exported = aliases.only?[1] {
                for clause in try matches("import\\{([^}]+)\\}from\"\\./" + NSRegularExpression.escapedPattern(for: module) + "\"", modules[main]!) {
                    names += try matches("(?:^|,)" + NSRegularExpression.escapedPattern(for: exported) + " as (" + id + ")(?:,|$)", clause[1]).map { $0[1] }
                }
            }
            if let alias = names.only { references[role] = Reference(name: alias, providerScope: false) }
            else { references[role] = Reference(name: symbol, providerScope: true) }
        }
        guard references["gf"]?.providerScope == false else { throw UIAdapterFailure.unsupported }
        return Mapping(boundary: found["_Sl"]![0].1, references: references)
    }
    static func read(app: String) throws -> Mapping {
        let file = try FileHandle(forReadingFrom: URL(fileURLWithPath: app + "/Contents/Resources/app.asar"))
        defer { try? file.close() }
        let prefix = Array(try file.read(upToCount: 16) ?? Data())
        guard prefix.count == 16 else { throw UIAdapterFailure.unsupported }
        func u32(_ i: Int) -> UInt64 { (0..<4).reduce(0) { $0 | UInt64(prefix[i + $1]) << (8 * $1) } }
        let size = u32(12), base = 8 + u32(4)
        guard size > 0, size < 16 * 1024 * 1024, base >= size + 16 else { throw UIAdapterFailure.unsupported }
        let headerData = try file.read(upToCount: Int(size)) ?? Data()
        guard headerData.count == size,
              let header = try JSONSerialization.jsonObject(with: headerData) as? [String: Any],
              let root = header["files"] as? [String: Any], let webview = root["webview"] as? [String: Any],
              let webFiles = webview["files"] as? [String: Any], let assets = webFiles["assets"] as? [String: Any],
              let entries = assets["files"] as? [String: Any] else { throw UIAdapterFailure.unsupported }
        var modules: [String: String] = [:]
        for (name, entry) in entries where name.range(of: "^app-(initial|shared)-[a-f0-9]+\\.js$", options: .regularExpression) != nil {
            guard let entry = entry as? [String: Any], let count = entry["size"] as? Int, count > 0, count < 32 * 1024 * 1024,
                  let offset = entry["offset"] as? String, let offset = UInt64(offset), offset < 1024 * 1024 * 1024 else { throw UIAdapterFailure.unsupported }
            try file.seek(toOffset: base + offset)
            let data = try file.read(upToCount: count) ?? Data()
            guard data.count == count, let source = String(data: data, encoding: .utf8) else { throw UIAdapterFailure.unsupported }
            modules[name] = source
        }
        return try discover(modules: modules)
    }
}
private extension Array { var only: Element? { count == 1 ? first : nil } }
