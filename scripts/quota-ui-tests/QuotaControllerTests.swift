import AppKit
import Combine
import Foundation

// Compiles with the production controller and models. No real IPC, service,
// credential access, network calls, or application windows are available here.
private final class FixtureIPC: @unchecked Sendable {
    private let condition = NSCondition()
    private let directory: AccountDirectory
    private var calls: [String: Int] = [:]
    private var failures: Set<String> = []
    private var held = false
    private var active = 0
    private var peak = 0

    init() throws {
        precondition(CommandLine.arguments.count == 2, "synthetic fixture path required")
        let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        directory = try decoder.decode(AccountDirectory.self, from: data)
        precondition(directory.accounts.allSatisfy { $0.id.hasPrefix("synthetic-") })
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
        active -= 1
        condition.unlock()
        if fail { throw ServiceError.requestFailed("合成请求失败：\(key)") }
        return AccountResult(directory: directory, attempt: nil)
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
        Snapshot(checkedAt: 0, serviceVersion: "synthetic", providers: [], verification: "synthetic")
    }

    static func ping(socketPath: String) throws -> ServiceInfo { fatalError("service launch is forbidden in this test") }
    static func login(method: String, params: LoginParams? = nil, socketPath: String) throws -> LoginResult { fatalError("login is forbidden in this test") }
}

@main struct QuotaControllerTests {
    @MainActor static func waitForCall(_ key: String) async {
        let arrived = await Task.detached { IPCClient.fixture.waitForCall(key) }.value
        precondition(arrived, "synthetic request did not arrive: \(key)")
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
            "accountIdentityChanges": identitiesChanged
        ]
        print(String(decoding: try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]), as: UTF8.self))
        withExtendedLifetime(cancellables) {}
    }
}
