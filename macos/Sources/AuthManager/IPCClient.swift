import Darwin
import Foundation

enum IPCClient {
    static func snapshot(socketPath: String) throws -> Snapshot {
        try request(method: "snapshot", socketPath: socketPath, responseType: Snapshot.self)
    }

    static func ping(socketPath: String) throws -> ServiceInfo {
        try request(method: "ping", socketPath: socketPath, responseType: ServiceInfo.self)
    }

    static func login(method: String, params: LoginParams? = nil, socketPath: String) throws -> LoginResult {
        try request(method: method, params: params, socketPath: socketPath, responseType: LoginResult.self)
    }

    static func accounts(method: String = "accounts.list", params: LoginParams? = nil, socketPath: String) throws -> AccountResult {
        try request(method: method, params: params, socketPath: socketPath, responseType: AccountResult.self)
    }
    static func resetHistory(accountID: String?, before: String?, socketPath: String) throws -> ResetHistoryPage {
        try request(method: "accounts.reset_history", params: LoginParams(id: accountID, before: before, limit: 30),
                    socketPath: socketPath, responseType: ResetHistoryPage.self)
    }

    private static func request<Result: Decodable & Sendable>(method: String, params: LoginParams? = nil, socketPath: String, responseType: Result.Type) throws -> Result {
        var address = sockaddr_un()
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard socketPath.utf8.count < capacity else { throw ServiceError.pathTooLong }
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        withUnsafeMutablePointer(to: &address.sun_path) { pointer in
            pointer.withMemoryRebound(to: CChar.self, capacity: capacity) { buffer in
                socketPath.withCString { source in
                    _ = strncpy(buffer, source, capacity - 1)
                }
            }
        }
        let descriptor = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else { throw ServiceError.connection }
        defer { Darwin.close(descriptor) }
        var timeout = timeval(tv_sec: 8, tv_usec: 0)
        var noSignal: Int32 = 1
        _ = setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        _ = setsockopt(descriptor, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        _ = setsockopt(descriptor, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size))
        let connected = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard connected == 0 else { throw ServiceError.connection }
        let id = UUID().uuidString
        var request = try JSONEncoder().encode(RPCRequest(id: id, method: method, params: params))
        request.append(0x0A)
        try request.withUnsafeBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                let count = Darwin.send(descriptor, buffer.baseAddress!.advanced(by: offset), buffer.count - offset, 0)
                guard count > 0 else { throw ServiceError.connection }
                offset += count
            }
        }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while data.last != 0x0A {
            let count = Darwin.recv(descriptor, &buffer, buffer.count, 0)
            guard count > 0 else { throw ServiceError.connection }
            data.append(contentsOf: buffer.prefix(count))
            guard data.count <= 512 * 1024 else { throw ServiceError.invalidResponse }
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let response: RPCResponse<Result>
        do { response = try decoder.decode(RPCResponse<Result>.self, from: data) }
        catch { throw ServiceError.invalidResponse }
        guard response.version == 1, response.id == id else { throw ServiceError.invalidResponse }
        if let error = response.error { throw ServiceError.requestFailed(error.message) }
        guard let snapshot = response.result else { throw ServiceError.invalidResponse }
        return snapshot
    }
}
