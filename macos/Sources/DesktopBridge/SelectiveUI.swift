import Foundation

// Resolve current semantic names, then check actual renderer references and structure.
@MainActor
enum SelectiveUI {
    static let symbols = SelectiveSymbols.roles
    static func install(on cdp: CDP, script: String, mapping: SelectiveSymbols.Mapping) async throws {
        _ = try await cdp.evaluate(script)
        if (try await cdp.evaluate("globalThis.__authManagerSelectiveUIV1.ready()") as? Bool) == true { return }
        let name = String(decoding: try JSONSerialization.data(withJSONObject: [mapping.boundary]), as: UTF8.self)
        _ = try await cdp.evaluate("globalThis.__authManagerSelectiveUIV1.configure(" + name + "[0])")
        do {
            let function = try await cdp.call("Runtime.evaluate", ["expression":"globalThis.__authManagerSelectiveUIV1.discover()", "objectGroup":"authmanager-ui-adapter"])
            guard let result = function["result"] as? [String:Any], let functionID = result["objectId"] as? String else { throw UIAdapterFailure.unsupported }
            var references = try await resolve(on: cdp, function: functionID, names: mapping.references.filter { !$0.value.providerScope })
            let shared = mapping.references.filter { $0.value.providerScope }
            if !shared.isEmpty {
                guard let provider = references["gf"] else { throw UIAdapterFailure.unsupported }
                references.merge(try await resolve(on: cdp, function: provider, names: shared)) { _, next in next }
            }
            guard symbols.allSatisfy({references[$0] != nil}) else { throw UIAdapterFailure.unsupported }
            let names = String(decoding:try JSONSerialization.data(withJSONObject:symbols),as:UTF8.self)
            let attached = try await cdp.call("Runtime.callFunctionOn",[
                "objectId":functionID,
                "functionDeclaration":"function(...values){return globalThis.__authManagerSelectiveUIV1.attach(Object.fromEntries(" + names + ".map((name,i)=>[name,values[i]])))}",
                "arguments":symbols.map{["objectId":references[$0]!]},
                "returnByValue":true,
            ])
            guard attached["exceptionDetails"] == nil,
                  let answer = attached["result"] as? [String:Any], let value = answer["value"] as? [String:Any], value["ok"] as? Bool == true else {throw UIAdapterFailure.unsupported}
            _ = try await cdp.call("Runtime.releaseObjectGroup",["objectGroup":"authmanager-ui-adapter"])
        } catch {
            _ = try? await cdp.call("Runtime.releaseObjectGroup",["objectGroup":"authmanager-ui-adapter"])
            throw UIAdapterFailure.unsupported
        }
    }
    private static func resolve(on cdp: CDP, function: String, names: [String: SelectiveSymbols.Reference]) async throws -> [String: String] {
        let properties = try await cdp.call("Runtime.getProperties",["objectId":function, "ownProperties":true])
        guard let internals = properties["internalProperties"] as? [[String:Any]],
              let scopes = internals.first(where:{$0["name"] as? String == "[[Scopes]]"})?["value"] as? [String:Any],
              let scopesID = scopes["objectId"] as? String else {throw UIAdapterFailure.unsupported}
        let entries = try await cdp.call("Runtime.getProperties",["objectId":scopesID, "ownProperties":true])
        guard let list = entries["result"] as? [[String:Any]] else {throw UIAdapterFailure.unsupported}
        var found: [String: [String]] = [:]
        for entry in list {
            guard let value = entry["value"] as? [String:Any],
                  let description = value["description"] as? String, description.hasPrefix("Module"),
                  let objectID = value["objectId"] as? String else {continue}
            let members = try await cdp.call("Runtime.getProperties",["objectId":objectID, "ownProperties":true])
            for member in members["result"] as? [[String:Any]] ?? [] {
                guard let name = member["name"] as? String,
                      let reference = member["value"] as? [String:Any], let id = reference["objectId"] as? String else {continue}
                for (role, wanted) in names where wanted.name == name { found[role, default: []].append(id) }
            }
        }
        guard names.keys.allSatisfy({found[$0]?.count == 1}) else {throw UIAdapterFailure.unsupported}
        return found.mapValues { $0[0] }
    }
}
enum UIAdapterFailure: String, Error {case unsupported = "ui_adapter_unavailable"}
