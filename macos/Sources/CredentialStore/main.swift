import Foundation
import Security
import SQLite3

// Private pipe protocol used by Rust, never exposed to the UI or logging system.
struct Input: Decodable {
    let operation: String
    let account: String?
    let value: String?
    let path: String?
}
struct Output: Encodable {
    let status: String
    var value: String? = nil
}
// Never requests access to item data or unlocks a keychain.
func keychainStatus() -> Output {
    var keychain: SecKeychain?
    guard SecKeychainCopyDefault(&keychain) == errSecSuccess, let keychain else { return Output(status: "locked") }
    var status: SecKeychainStatus = 0
    guard SecKeychainGetStatus(keychain, &status) == errSecSuccess,
          status & UInt32(kSecUnlockStateStatus) != 0 else { return Output(status: "locked") }
    return Output(status: "ok")
}
// The installed v1 helper is immutable across app rebuilds, preserving its
// Keychain identity. This route cannot address official clients' keychain items.
func vault(_ input: Input) -> Output {
    guard let account = input.account, !account.isEmpty, account.count <= 80,
          account.utf8.allSatisfy({ (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 }) else { return Output(status: "invalid") }
    let query: [String: Any] = [kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "io.github.wuxm5.authbaton.vault", kSecAttrAccount as String: account]
    if input.operation == "vault.delete" {
        let status = SecItemDelete(query as CFDictionary)
        return Output(status: status == errSecSuccess || status == errSecItemNotFound ? "ok" : "failed")
    }
    if input.operation == "vault.read" {
        var read = query
        read[kSecReturnData as String] = true
        var result: CFTypeRef?
        let status = SecItemCopyMatching(read as CFDictionary, &result)
        if status == errSecItemNotFound { return Output(status: "missing") }
        guard status == errSecSuccess, let data = result as? Data else { return Output(status: "failed") }
        return Output(status: "ok", value: data.base64EncodedString())
    }
    guard input.operation == "vault.write", let value = input.value,
          let data = Data(base64Encoded: value), data.count <= 1024 * 1024 else { return Output(status: "invalid") }
    var status = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
    if status == errSecItemNotFound {
        var item = query
        item[kSecValueData as String] = data
        status = SecItemAdd(item as CFDictionary, nil)
    }
    return Output(status: status == errSecSuccess ? "ok" : "failed")
}
func database(_ input: Input) -> Output {
    guard let path = input.path, path.hasPrefix("/"), input.operation == "db.read" || input.operation == "db.write" else { return Output(status: "invalid") }
    let manager = FileManager.default
    if !manager.fileExists(atPath: path) {
        guard manager.createFile(atPath: path, contents: Data(), attributes: [.posixPermissions: 0o600]) else { return Output(status: "failed") }
    }
    var db: OpaquePointer?
    guard sqlite3_open_v2(path, &db, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, nil) == SQLITE_OK else {
        if let db { sqlite3_close(db) }; return Output(status: "failed")
    }
    defer { sqlite3_close(db) }
    sqlite3_busy_timeout(db, 2000)
    guard sqlite3_exec(db, "CREATE TABLE IF NOT EXISTS state(id INTEGER PRIMARY KEY CHECK(id=1),body TEXT NOT NULL)", nil, nil, nil) == SQLITE_OK else { return Output(status: "failed") }
    var statement: OpaquePointer?
    if input.operation == "db.read" {
        guard sqlite3_prepare_v2(db, "SELECT body FROM state WHERE id=1", -1, &statement, nil) == SQLITE_OK else { return Output(status: "failed") }
        defer { sqlite3_finalize(statement) }
        let result = sqlite3_step(statement)
        if result == SQLITE_DONE { return Output(status: "missing") }
        guard result == SQLITE_ROW, let text = sqlite3_column_text(statement, 0) else { return Output(status: "failed") }
        return Output(status: "ok", value: String(cString: text))
    }
    guard let value = input.value, value.utf8.count <= 4 * 1024 * 1024 else { return Output(status: "invalid") }
    guard sqlite3_prepare_v2(db, "INSERT INTO state(id,body) VALUES(1,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body", -1, &statement, nil) == SQLITE_OK else { return Output(status: "failed") }
    defer { sqlite3_finalize(statement) }
    let result = value.withCString { text in
        guard sqlite3_bind_text(statement, 1, text, -1, nil) == SQLITE_OK else { return SQLITE_ERROR }
        return sqlite3_step(statement)
    }
    return Output(status: result == SQLITE_DONE ? "ok" : "failed")
}
func run() -> Output {
    // Status checks must never trigger a keychain unlock dialog.
    guard SecKeychainSetUserInteractionAllowed(false) == errSecSuccess else { return Output(status: "failed") }
    let input = FileHandle.standardInput.readDataToEndOfFile()
    guard input.count <= 5 * 1024 * 1024, let request = try? JSONDecoder().decode(Input.self, from: input) else { return Output(status: "invalid") }
    if request.operation == "keychain.status" { return keychainStatus() }
    if request.operation.hasPrefix("vault.") { return vault(request) }
    return request.operation.hasPrefix("db.") ? database(request) : Output(status: "invalid")
}
if let data = try? JSONEncoder().encode(run()) { FileHandle.standardOutput.write(data) }
