import Foundation

// Production models only. No AppKit, IPC, credential or desktop access.
@main struct StrategyModelSmoke {
    static func main() throws {
        let source = Data(#"{"enabled":true,"accounts":[{"id":"a","order":"reset_first","participate":true,"below":2},{"id":"b","order":"switch_first","participate":true,"below":3},{"id":"c","order":"reset_first","participate":false,"below":2}]}"#.utf8)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        var draft = try decoder.decode(CodexStrategyConfig.self, from: source)
        precondition(draft.accounts.filter { $0.order == .resetFirst }.map(\.id) == ["a", "c"])
        precondition(draft.accounts.filter { $0.order == .switchFirst }.map(\.id) == ["b"])
        draft.accounts[0].order = .switchFirst
        precondition(draft.accounts.map(\.id) == ["a", "b", "c"])
        precondition(draft.accounts.filter { $0.order == .resetFirst }.map(\.id) == ["c"])
        precondition(draft.accounts.filter { $0.order == .switchFirst }.map(\.id) == ["a", "b"])
        precondition(draft.accounts[0].below == 2 && draft.accounts[0].participate)
        draft.move(id: "b", by: -1)
        precondition(draft.accounts.map(\.id) == ["b", "a", "c"])
        precondition(draft.rank(of: "b") == 1 && draft.rank(of: "a") == 2)
        draft.move(id: "c", by: -1)
        precondition(draft.accounts.map(\.id) == ["b", "c", "a"])
        precondition(!draft.accounts[1].participate && draft.accounts[1].order == .resetFirst)
        draft.move(id: "b", by: -1)
        precondition(draft.accounts.map(\.id) == ["b", "c", "a"])
        precondition(Set(draft.accounts.map(\.id)).count == 3)
        let encoded = try JSONEncoder().encode(draft.input)
        let wire = try JSONSerialization.jsonObject(with: encoded) as! [String: Any]
        precondition(Set(wire.keys) == ["enabled", "accounts"])
        let rows = wire["accounts"] as! [[String: Any]]
        precondition(rows[0]["order"] as? String == "switch_first")
        precondition(rows[1]["order"] as? String == "reset_first")
        precondition(rows[0]["below"] as? Int == 3)
        precondition(Set(rows[0].keys) == ["id", "order", "participate", "below"])
        precondition(CodexStrategyOrder.allCases.map(\.rawValue) == ["reset_first", "switch_first", "reset_only", "switch_only"])
        precondition(CodexStrategyOrder.allCases.filter(\.resets) == [.resetFirst, .switchFirst, .resetOnly])
        precondition(CodexStrategyOrder.allCases.filter(\.switches) == [.resetFirst, .switchFirst, .switchOnly])
        // A newly listed account never spends credits by default.
        let fresh = CodexStrategyConfig.Rule(id: "d", participate: true)
        precondition(fresh.order == .switchOnly && fresh.below == 2)
        let roundTrip = try decoder.decode(CodexStrategyConfig.self, from: encoded)
        precondition(roundTrip.accounts.map(\.id) == draft.accounts.map(\.id))
        precondition(roundTrip.accounts.map(\.order) == draft.accounts.map(\.order))
        print("Strategy orders, per-account line, global priority and IPC round-trip passed; live operations: 0")
    }
}
