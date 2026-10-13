import Foundation

@main struct TransportSmoke {
    @MainActor static func main() async throws {
        guard CommandLine.arguments.count==3, let port=UInt16(CommandLine.arguments[1]) else {throw Failure.invalid}
        let cdp=CDP(url:URL(string:"ws://127.0.0.1:\(port)/devtools/page/synthetic")!)
        defer {cdp.close()}
        let result=try await cdp.evaluate("synthetic fixture expression") as? [String:Bool]
        guard result?["fixture"]==true else {throw Failure.invalid}
        var rejected=false
        do {_=try await cdp.call("synthetic/error",[:])}
        catch {rejected=true;precondition(!String(describing:error).contains("SYNTHETIC_PRIVATE_ERROR"))}
        precondition(rejected)
        let installer=try String(contentsOfFile:CommandLine.arguments[2],encoding:.utf8)
        for _ in 0..<2 {
            let installed=try await cdp.evaluate(installer) as? Bool
            precondition(installed == true, "installer must acknowledge both first and repeated calls")
        }
        print("Foundation CDP text transport and error redaction passed")
    }
}
