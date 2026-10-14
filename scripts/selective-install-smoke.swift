import Foundation

// Production discovery/installer; synthetic CDP peer. Optional bundle read is static only.
@MainActor final class CDP {
    var missingReference = false
    var duplicateReference = false
    var attached = false
    var released = false
    let mapping: SelectiveSymbols.Mapping
    init(_ mapping: SelectiveSymbols.Mapping) { self.mapping = mapping }
    func evaluate(_ expression: String) async throws -> Any {
        if expression == "synthetic-adapter-script" { return true }
        if expression == "globalThis.__authManagerSelectiveUIV1.ready()" { return false }
        if expression.hasPrefix("globalThis.__authManagerSelectiveUIV1.configure(") { return true }
        throw FixtureFailure.unexpectedCall
    }
    func call(_ method: String, _ params: [String: Any]) async throws -> [String: Any] {
        switch method {
        case "Runtime.evaluate":
            precondition(params["expression"] as? String == "globalThis.__authManagerSelectiveUIV1.discover()")
            return ["result": ["objectId": "function"]]
        case "Runtime.getProperties":
            switch params["objectId"] as? String {
            case "function": return ["internalProperties": [["name": "[[Scopes]]", "value": ["objectId": "main-scopes"]]]]
            case "ref-gf": return ["internalProperties": [["name": "[[Scopes]]", "value": ["objectId": "provider-scopes"]]]]
            case "main-scopes": return ["result": [["value": ["description": "Module (synthetic-main)", "objectId": "main"]]]]
            case "provider-scopes": return ["result": [["value": ["description": "Module (synthetic-shared)", "objectId": "shared"]]]]
            case "main", "shared":
                let shared = params["objectId"] as? String == "shared"
                var values: [[String: Any]] = mapping.references.filter { $0.value.providerScope == shared }.compactMap { role, ref in
                    if missingReference && role == "Qd" { return nil }
                    return ["name": ref.name, "value": ["objectId": "ref-" + role]]
                }
                if duplicateReference && shared, let first = values.first { values.append(first) }
                return ["result": values]
            default: throw FixtureFailure.unexpectedCall
            }
        case "Runtime.callFunctionOn":
            precondition((params["arguments"] as? [[String: String]])?.count == SelectiveUI.symbols.count)
            attached = true
            return ["result": ["value": ["ok": true]]]
        case "Runtime.releaseObjectGroup": released = true; return [:]
        default: throw FixtureFailure.unexpectedCall
        }
    }
}
enum FixtureFailure: Error { case unexpectedCall }

@main struct SelectiveInstallSmoke {
    @MainActor static func main() async throws {
        let refs = Dictionary(uniqueKeysWithValues: SelectiveSymbols.roles.enumerated().map { index, role in
            (role, SelectiveSymbols.Reference(name: "renamed_" + String(index), providerScope: role == "Qd"))
        })
        let mapping = SelectiveSymbols.Mapping(boundary: "RenamedBoundary", references: refs)
        let supported = CDP(mapping)
        try await SelectiveUI.install(on: supported, script: "synthetic-adapter-script", mapping: mapping)
        precondition(supported.attached && supported.released)
        for duplicate in [false, true] {
            let unsupported = CDP(mapping)
            unsupported.missingReference = !duplicate
            unsupported.duplicateReference = duplicate
            do {
                try await SelectiveUI.install(on: unsupported, script: "synthetic-adapter-script", mapping: mapping)
                preconditionFailure("missing/ambiguous interface must stop installation")
            } catch UIAdapterFailure.unsupported {}
            precondition(!unsupported.attached && unsupported.released)
        }
        do { _ = try SelectiveSymbols.discover(modules: [:]); preconditionFailure("missing source accepted") }
        catch UIAdapterFailure.unsupported {}
        if CommandLine.arguments.count > 1 {
            let current = try SelectiveSymbols.read(app: CommandLine.arguments[1])
            precondition(current.references.count == SelectiveSymbols.roles.count)
            print("Static installed bundle: \(current.references.count) unique roles; shared-module scope: \(current.references.filter { $0.value.providerScope }.keys.sorted())")
        }
        print("Synthetic renamed/shared-module install and missing/ambiguous rejection passed; live CDP: 0")
    }
}
