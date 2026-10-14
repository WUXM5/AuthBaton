import AppKit
import Combine
import Foundation

// Compiles with the production controller and models. No real IPC, credential access, network
// calls, or application windows are available here. The service the controller launches is
// ServiceStub, built next to this binary as auth-service; it never opens the socket path.

// Synthetic accounts only; the controller never reads past the list itself.
private let fixture = #"""
{"accounts":[
 {"id":"synthetic-a","provider":"codex","label":"Codex · a","identity":"a@example.invalid","plan":"pro","expires_at":null,"refresh_expires_at":null,"native":true,"automatic":true,"state":"healthy","message":"","last_local":null,"last_online":null,"last_refresh":null,"next_check":0,"quota":[],"usage":{"state":"ready","message":"","windows":[],"checked_at":1,"attempted_at":1,"next_update":0,"refreshing":false,"reset_credits":null,"claude_resets":null},"reset":null,"renewal_mode":null,"login_notice":null,"check_status":null,"check_online":null,"warmup":null},
 {"id":"synthetic-b","provider":"codex","label":"Codex · b","identity":"b@example.invalid","plan":"pro","expires_at":null,"refresh_expires_at":null,"native":false,"automatic":true,"state":"healthy","message":"","last_local":null,"last_online":null,"last_refresh":null,"next_check":0,"quota":[],"usage":{"state":"ready","message":"","windows":[],"checked_at":1,"attempted_at":1,"next_update":0,"refreshing":false,"reset_credits":null,"claude_resets":null},"reset":null,"renewal_mode":null,"login_notice":null,"check_status":null,"check_online":null,"warmup":null}],
 "busy":false,"checks_blocked":false,"recovery_needed":false,"notice":null,"maintenance_running":true,"desktop":null}
"""#

private let decoder: JSONDecoder = {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return decoder
}()

private final class FixtureIPC: @unchecked Sendable {
    private let condition = NSCondition()
    private var directory: AccountDirectory
    private var calls: [String: Int] = [:]
    private var failures: Set<String> = []
    private var held = false
    private var active = 0
    private var peak = 0
    private var connectionLost = false
    private var pingBlocked = false

    init() throws {
        directory = try decoder.decode(AccountDirectory.self, from: Data(fixture.utf8))
        precondition(directory.accounts.allSatisfy { $0.id.hasPrefix("synthetic-") })
    }

    // Cloud mode, paired, with the directory last fetched `age` seconds ago.
    func useCloud(checkedAgo age: Double) throws {
        let checked = Date().timeIntervalSince1970 - age
        let cloud = try decoder.decode(CloudStatus.self, from: Data(#"""
            {"mode":"cloud","endpoint":"https://cloud.example.invalid","label":"合成设备","paired":true,"pair":null,"accounts":[],"devices":[],
             "selected":{},"busy":false,"error":null,"checked_at":\#(checked),"pending_activation":null,"transfers":[]}
            """#.utf8))
        condition.lock()
        directory.cloud = cloud
        condition.unlock()
    }

    // The socket stops answering: every request fails as a lost connection.
    func setConnectionLost(_ lost: Bool) {
        condition.lock()
        connectionLost = lost
        condition.unlock()
    }

    // The service process runs but does not answer pings yet.
    func setPingBlocked(_ blocked: Bool) {
        condition.lock()
        pingBlocked = blocked
        condition.unlock()
    }

    func answersPing() -> Bool {
        condition.lock()
        defer { condition.unlock() }
        return !pingBlocked && !connectionLost && !Stub.live().isEmpty
    }

    func reachable() throws {
        condition.lock()
        defer { condition.unlock() }
        if connectionLost { throw ServiceError.connection }
    }

    func configure(failures: Set<String> = [], hold: Bool = false) {
        condition.lock()
        defer { condition.unlock() }
        precondition(active == 0)
        calls = [:]
        self.failures = failures
        held = hold
        peak = 0
    }

    func accounts(method: String, id: String?) throws -> AccountResult {
        let key = "\(method):\(id ?? "all")"
        condition.lock()
        calls[key, default: 0] += 1
        active += 1
        peak = max(peak, active)
        condition.broadcast()
        while held { condition.wait() }
        let fail = failures.contains(key)
        let lost = connectionLost
        let current = directory
        active -= 1
        condition.unlock()
        if lost { throw ServiceError.connection }
        if fail { throw ServiceError.requestFailed("合成请求失败：\(key)") }
        let attempt = method == "accounts.login"
            ? LoginAttempt(attemptId: "synthetic-attempt", provider: "codex", phase: "succeeded", message: "合成登录已完成", verificationUrl: nil, userCode: nil)
            : nil
        return AccountResult(directory: current, attempt: attempt)
    }

    func count(_ key: String) -> Int {
        condition.lock()
        defer { condition.unlock() }
        return calls[key, default: 0]
    }

    func maximumActive() -> Int {
        condition.lock()
        defer { condition.unlock() }
        return peak
    }

    func waitForCall(_ key: String) -> Bool {
        condition.lock()
        defer { condition.unlock() }
        let deadline = Date(timeIntervalSinceNow: 2)
        while calls[key, default: 0] == 0 {
            if !condition.wait(until: deadline) { return false }
        }
        return true
    }

    func release() {
        condition.lock()
        held = false
        condition.broadcast()
        condition.unlock()
    }
}

enum IPCClient {
    fileprivate static let fixture = try! FixtureIPC()
    static func resetHistory(accountID: String?, before: String?, socketPath: String) throws -> ResetHistoryPage {
        fatalError("history is not requested by quota checks")
    }

    static func accounts(method: String = "accounts.list", params: LoginParams? = nil, socketPath: String) throws -> AccountResult {
        try fixture.accounts(method: method, id: params?.id)
    }

    static func snapshot(socketPath: String) throws -> Snapshot {
        try fixture.reachable()
        return Snapshot(checkedAt: 0, serviceVersion: "synthetic", providers: [], verification: "synthetic")
    }

    // The socket answers once a stand-in service holds its lock, as the real one binds after locking.
    static func ping(socketPath: String) throws -> ServiceInfo {
        guard fixture.answersPing() else { throw ServiceError.connection }
        return ServiceInfo(serviceVersion: "synthetic")
    }

    static func login(method: String, params: LoginParams? = nil, socketPath: String) throws -> LoginResult {
        try fixture.reachable()
        return LoginResult(attempt: nil)
    }
}

// What the stand-in services wrote next to this binary: "ready|refused|exit <pid>" per line.
enum Stub {
    static let directory = Bundle.main.executableURL!.deletingLastPathComponent().path

    static func events() -> [(event: String, pid: Int32)] {
        let text = (try? String(contentsOfFile: directory + "/stub.log", encoding: .utf8)) ?? ""
        return text.split(separator: "\n").compactMap { line in
            let parts = line.split(separator: " ")
            return parts.count == 2 ? (String(parts[0]), Int32(parts[1])!) : nil
        }
    }

    // Lock owners that have not exited, ignoring any that were killed.
    static func live() -> [Int32] {
        let all = events()
        return all.filter { $0.event == "ready" }.map(\.pid)
            .filter { pid in !all.contains { $0.event == "exit" && $0.pid == pid } && kill(pid, 0) == 0 }
    }

    static func refusals() -> Int { events().filter { $0.event == "refused" }.count }

    static func reset() {
        try? FileManager.default.removeItem(atPath: directory + "/stub.log")
    }

    // Plays a service that is still shutting down: holds the lock until released.
    static func holdLock() -> Int32 {
        let file = open(directory + "/stub.lock", O_RDWR | O_CREAT | O_CLOEXEC, 0o600)
        precondition(file >= 0 && flock(file, LOCK_EX) == 0)
        return file
    }

    static func release(_ file: Int32) {
        flock(file, LOCK_UN)
        close(file)
    }
}

@main struct QuotaControllerTests {
    @MainActor static func waitForCall(_ key: String) async {
        let arrived = await Task.detached { IPCClient.fixture.waitForCall(key) }.value
        precondition(arrived, "synthetic request did not arrive: \(key)")
    }

    @MainActor static func until(_ condition: () -> Bool) async throws -> Bool {
        for _ in 0..<60 where !condition() { try await Task.sleep(for: .milliseconds(50)) }
        return condition()
    }

    @MainActor static func main() async throws {
        let controller = ServiceController()
        await controller.refresh()
        let ids = controller.directory!.accounts.map(\.id)
        precondition(ids.count >= 2)
        let first = ids[0]
        let second = ids[1]
        let firstKey = "accounts.quota:\(first)"
        let secondKey = "accounts.quota:\(second)"
        let sweepKey = "accounts.quota:all"

        IPCClient.fixture.configure(failures: ["accounts.update:\(first)"])
        await controller.accountOperation("accounts.update", params: LoginParams(id: first))
        let existingAccountError = controller.accountError
        precondition(existingAccountError != nil && !controller.accountBusy)
        precondition(controller.accountErrorID == first)

        var busyChanges = 0
        var directoriesCleared = 0
        var identitiesChanged = 0
        var cancellables = Set<AnyCancellable>()
        controller.$accountBusy.dropFirst().sink { _ in busyChanges += 1 }.store(in: &cancellables)
        controller.$directory.dropFirst().sink { directory in
            if directory == nil { directoriesCleared += 1 }
            if directory?.accounts.map(\.id) != ids { identitiesChanged += 1 }
        }.store(in: &cancellables)

        IPCClient.fixture.configure()
        for _ in 0..<10 { await controller.checkQuota(id: first) }
        precondition(IPCClient.fixture.count(firstKey) == 10)
        precondition(busyChanges == 0 && controller.quotaRequests.isEmpty)
        precondition(controller.accountError == existingAccountError)

        IPCClient.fixture.configure(hold: true)
        let firstRequest = Task { await controller.checkQuota(id: first) }
        await waitForCall(firstKey)
        precondition(controller.quotaRequests == [first])
        precondition(!controller.accountBusy)
        await controller.checkQuota(id: first)
        precondition(IPCClient.fixture.count(firstKey) == 1)
        let secondRequest = Task { await controller.checkQuota(id: second) }
        await waitForCall(secondKey)
        precondition(controller.quotaRequests == [first, second])
        precondition(IPCClient.fixture.maximumActive() == 2)
        IPCClient.fixture.release()
        await firstRequest.value
        await secondRequest.value
        precondition(controller.quotaRequests.isEmpty)

        IPCClient.fixture.configure(failures: [firstKey])
        await controller.checkQuota(id: first)
        precondition(Set(controller.quotaErrors.keys) == [first])
        precondition(controller.accountError == existingAccountError)
        precondition(controller.directoryError == nil && controller.quotaRequests.isEmpty)

        IPCClient.fixture.configure(failures: [secondKey])
        await controller.checkQuota(id: second)
        let secondError = controller.quotaErrors[second]
        precondition(secondError != nil)
        IPCClient.fixture.configure()
        await controller.checkQuota(id: first)
        precondition(controller.quotaErrors[first] == nil && controller.quotaErrors[second] == secondError)
        precondition(controller.accountError == existingAccountError)

        IPCClient.fixture.configure(failures: [sweepKey])
        await controller.refreshQuotaIfNeeded()
        precondition(controller.directoryError != nil)
        precondition(controller.quotaErrors[second] == secondError)
        precondition(controller.accountError == existingAccountError)

        IPCClient.fixture.configure(hold: true)
        let sweep = Task { await controller.refreshQuotaIfNeeded() }
        await waitForCall(sweepKey)
        await controller.refreshQuotaIfNeeded()
        precondition(IPCClient.fixture.count(sweepKey) == 1)
        precondition(controller.quotaRequests.isEmpty && !controller.accountBusy)
        IPCClient.fixture.release()
        await sweep.value
        precondition(controller.directoryError == nil)
        precondition(controller.accountError == existingAccountError)
        precondition(controller.quotaErrors[second] == secondError)

        IPCClient.fixture.configure()
        await controller.checkQuota(id: second)
        precondition(controller.quotaErrors.isEmpty && controller.quotaRequests.isEmpty)
        precondition(busyChanges == 0 && directoriesCleared == 0 && identitiesChanged == 0)

        // A login result belongs to the account it was started for (R25).
        let logins = ServiceController()
        await logins.refresh()
        IPCClient.fixture.configure()
        await logins.startLogin(provider: "codex", label: "合成", accountID: second)
        precondition(logins.login != nil && logins.loginTarget == LoginTarget(accountID: second), "login target not recorded")
        IPCClient.fixture.configure(failures: ["accounts.login:\(first)"])
        await logins.startLogin(provider: "codex", label: "合成", accountID: first)
        precondition(logins.loginTarget == LoginTarget(accountID: second), "a refused login took over the earlier result")
        logins.clearLogin()
        precondition(logins.login == nil && logins.loginTarget == nil)

        // Cloud mode: activation refreshes at most once a minute, and never while an operation runs (R26).
        try IPCClient.fixture.useCloud(checkedAgo: 600)
        let cloud = ServiceController()
        await cloud.refresh()
        IPCClient.fixture.configure(hold: true)
        let running = Task { await cloud.accountOperation("accounts.update", params: LoginParams(id: first)) }
        await waitForCall("accounts.update:\(first)")
        await cloud.refreshQuotaIfNeeded()
        precondition(IPCClient.fixture.count("accounts.cloud:all") == 0, "cloud refresh sent while an operation was running")
        precondition(cloud.accountError == nil, "activation reported an error for an operation it did not start")
        IPCClient.fixture.release()
        await running.value
        for _ in 0..<5 { await cloud.refreshQuotaIfNeeded() }
        let cloudRefreshes = IPCClient.fixture.count("accounts.cloud:all")
        precondition(cloudRefreshes == 1, "activation sent \(cloudRefreshes) cloud refreshes within a minute")
        try IPCClient.fixture.useCloud(checkedAgo: 5)
        let fresh = ServiceController()
        await fresh.refresh()
        IPCClient.fixture.configure()
        await fresh.refreshQuotaIfNeeded()
        precondition(IPCClient.fixture.count("accounts.cloud:all") == 0, "refreshed a directory the service fetched seconds ago")

        // Service lifecycle against the stand-in auth-service next to this binary.
        Stub.reset()
        let service = ServiceController()

        // R23: a caller that stops waiting (a closed window) must not abandon the first handshake.
        IPCClient.fixture.setPingBlocked(true)
        let opening = Task { await service.start() }
        try await Task.sleep(for: .milliseconds(150))
        opening.cancel()
        IPCClient.fixture.setPingBlocked(false)
        await opening.value
        precondition(service.isConnected && service.directory != nil && service.errorText == nil, "cancelled first start: \(service.errorText ?? "not connected")")
        precondition(Stub.live().count == 1)

        // R24: a second click while the first reconnect waits for the old service must not start
        // a lock owner of its own, nor lose the handle of the one it starts.
        let click = Task { await service.restart() }
        let again = Task { await service.restart() }
        await click.value
        await again.value
        await service.restart()
        precondition(service.isConnected && Stub.live().count == 1, "reconnect left \(Stub.live().count) services")
        precondition(Stub.refusals() == 0, "a new service was started while ours still held the lock")

        // R24: reopening right after quitting finds the previous service still stopping, lock held.
        service.stop()
        let previous = await Task.detached { Stub.holdLock() }.value
        let reopened = ServiceController()
        let reopening = Task { await reopened.start() }
        try await Task.sleep(for: .milliseconds(1500))
        Stub.release(previous)
        await reopening.value
        precondition(reopened.isConnected && reopened.errorText == nil, "reopen failed: \(reopened.errorText ?? "not connected")")
        let lockRetries = Stub.refusals()
        precondition(lockRetries >= 1)

        // R10: the service exits on its own. Nothing it published stays on screen as current,
        // and it is not restarted behind the user's back.
        kill(Stub.live()[0], SIGKILL)
        let dropped = try await until { !reopened.isConnected }
        precondition(dropped, "still connected after the service exited")
        precondition(reopened.directory == nil && reopened.login == nil && reopened.errorText != nil)
        try await Task.sleep(for: .milliseconds(1200))
        precondition(Stub.live().isEmpty && !reopened.isConnected, "service restarted without the user")
        await reopened.restart()
        precondition(reopened.isConnected && reopened.directory != nil && Stub.live().count == 1)

        // R10: requests fail in a row. One miss is not a lost service; three are, and it comes back on its own.
        IPCClient.fixture.setConnectionLost(true)
        await reopened.reloadAccounts()
        precondition(reopened.isConnected && reopened.directory != nil, "one missed answer dropped the connection")
        await reopened.reloadAccounts()
        await reopened.reloadAccounts()
        precondition(!reopened.isConnected && reopened.directory == nil && reopened.errorText != nil, "still connected after three failures")
        IPCClient.fixture.setConnectionLost(false)
        await reopened.reloadAccounts()
        precondition(reopened.isConnected && reopened.directory != nil && reopened.errorText == nil)
        reopened.stop()
        let stopped = try await until { Stub.live().isEmpty }
        precondition(stopped, "the stand-in service outlived stop()")

        let report: [String: Any] = [
            "assertionsPassed": true,
            "sequentialQuotaRequests": 10,
            "globalBusyPublications": busyChanges,
            "sameAccountRequestsDeduplicated": true,
            "differentAccountsConcurrent": 2,
            "quotaFailureScopedToTarget": true,
            "existingAccountErrorPreserved": true,
            "successClearsOnlyTargetError": true,
            "sweepDeduplicatedAndIndependent": true,
            "directoryClearedToNil": directoriesCleared,
            "accountIdentityChanges": identitiesChanged,
            "loginResultScopedToAccount": true,
            "cloudRefreshesPerMinute": cloudRefreshes,
            "cancelledStartStillConnects": true,
            "concurrentReconnectsRefusedLock": 0,
            "reopenLockRetries": lockRetries,
            "exitedServiceClearsState": true,
            "missedAnswersBeforeDisconnect": 3
        ]
        print(String(decoding: try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]), as: UTF8.self))
        withExtendedLifetime(cancellables) {}
    }
}
