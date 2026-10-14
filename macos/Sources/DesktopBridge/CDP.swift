import Foundation

// Private stdin/stdout transport. No Keychain access, logging, or general-purpose RPC endpoint.
@MainActor
final class CDP {
    let socket: URLSessionWebSocketTask
    var serial=0
    // Resolving the adapter lists a whole module scope: 16.8 MB for Codex desktop 26.1007's main module.
    static let maximumMessage=64*1024*1024
    init(url: URL) { socket=URLSession(configuration: .ephemeral).webSocketTask(with:url); socket.maximumMessageSize=CDP.maximumMessage; socket.resume() }
    func call(_ method:String, _ params:[String:Any]) async throws -> [String:Any] {
        serial += 1
        let id=serial
        let bytes=try JSONSerialization.data(withJSONObject:["id":id,"method":method,"params":params])
        let timeout=Task { try await Task.sleep(for:.seconds(40)); socket.cancel(with:.goingAway,reason:nil) }
        defer { timeout.cancel() }
        try await socket.send(.string(String(decoding:bytes,as:UTF8.self)))
        while true {
            let message=try await socket.receive()
            let data:Data
            switch message { case .data(let d):data=d; case .string(let s):data=Data(s.utf8); @unknown default:throw Failure.invalid }
            guard data.count<CDP.maximumMessage, let value=try JSONSerialization.jsonObject(with:data) as? [String:Any] else {throw Failure.invalid}
            if value["id"] as? Int == id {
                guard value["error"]==nil, let result=value["result"] as? [String:Any] else {throw Failure.invalid}
                return result
            }
        }
    }
    func evaluate(_ expression:String) async throws -> Any {
        let response=try await call("Runtime.evaluate",["expression":expression,"awaitPromise":true,"returnByValue":true])
        guard response["exceptionDetails"]==nil, let result=response["result"] as? [String:Any], let value=result["value"] else {throw Failure.invalid}
        return value
    }
    func close() {socket.cancel(with:.normalClosure,reason:nil)}
}
enum Failure:Error {case invalid}
