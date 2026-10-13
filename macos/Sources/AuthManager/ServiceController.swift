import AppKit
import Foundation

@MainActor
final class ServiceController: ObservableObject {
    static let shared = ServiceController()
    @Published private(set) var snapshot: Snapshot?
    @Published private(set) var isRefreshing = false
    @Published private(set) var errorText: String?
    @Published private(set) var isConnected = false
    @Published private(set) var login: LoginAttempt?
    @Published private(set) var loginBusy = false
    @Published private(set) var loginError: String?
    @Published private(set) var directory: AccountDirectory?
    @Published private(set) var accountError: String?
    @Published private(set) var accountErrorID: String?
    @Published private(set) var directoryError: String?
    @Published private(set) var accountBusy = false
    @Published private(set) var quotaRequests: Set<String> = []
    @Published private(set) var quotaErrors: [String: String] = [:]
    private var quotaSweepPending = false
    private var directoryTask: Task<Void, Never>?
    private var process: Process?
    private var lifetimePipe: Pipe?
    private var loginTask: Task<Void, Never>?
    private var started = false

    var socketPath: String {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/AuthManager/service.sock").path
    }

    func start() async {
        guard !started else { return }
        started = true
        do {
            try launchService()
            let path = socketPath
            var ready = false
            // A stale socket can remain after SIGTERM; readiness is an RPC fact.
            for _ in 0..<30 {
                if (try? await Task.detached { try IPCClient.ping(socketPath: path) }.value) != nil {
                    ready = true
                    break
                }
                if let process, !process.isRunning { throw ServiceError.connection }
                try await Task.sleep(for: .milliseconds(50))
            }
            guard ready else { throw ServiceError.connection }
            await refresh()
            await reloadAccounts()
            watchAccounts()
            await pollLogin()
            if login?.isActive == true { watchLogin() }
        } catch {
            errorText = error.localizedDescription
        }
    }

    func refresh() async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        let path = socketPath
        do {
            let result = try await Task.detached(priority: .userInitiated) {
                try IPCClient.snapshot(socketPath: path)
            }.value
            snapshot = result
            errorText = nil
            isConnected = true
            await reloadAccounts()
        } catch {
            errorText = error.localizedDescription
            isConnected = false
        }
    }

    func restart() async {
        guard !isRefreshing, login?.isActive != true, !loginBusy, directory?.busy != true, !accountBusy else { return }
        stop()
        started = false
        // Wait for our own child to exit before starting another lock owner.
        if let process, process.isRunning {
            await Task.detached { process.waitUntilExit() }.value
        }
        self.process = nil
        await start()
    }

    func stop() {
        loginTask?.cancel()
        directoryTask?.cancel()
        // EOF tells the Rust owner to cancel and reap its official login child.
        try? lifetimePipe?.fileHandleForWriting.close()
        lifetimePipe = nil
        isConnected = false
    }
    func resetHistory(accountID: String?, before: String? = nil) async throws -> ResetHistoryPage {
        let path = socketPath
        return try await Task.detached {
            try IPCClient.resetHistory(accountID: accountID, before: before, socketPath: path)
        }.value
    }

    func startLogin(provider: String, label: String, accountID: String? = nil, cloudAccountID: String? = nil) async {
        loginBusy = true
        await accountOperation("accounts.login", params: LoginParams(provider: provider, id: accountID, label: label, cloud_account_id: cloudAccountID))
        loginBusy = false
        loginError = accountError
        watchLogin()
    }

    func cancelLogin() async {
        guard let login else { return }
        await loginOperation("login.cancel", params: LoginParams(attempt_id: login.attemptId))
        if self.login?.isActive != true { await refresh() }
    }

    func submitLoginCode(_ code: String) async {
        guard let login else { return }
        await loginOperation("login.code", params: LoginParams(attempt_id: login.attemptId, code: code))
    }

    func clearLogin() {
        guard login?.isActive != true else { return }
        login = nil
        loginError = nil
    }

    private func loginOperation(_ method: String, params: LoginParams) async {
        guard !loginBusy else { return }
        loginBusy = true
        defer { loginBusy = false }
        let path = socketPath
        do {
            login = try await Task.detached { try IPCClient.login(method: method, params: params, socketPath: path) }.value.attempt
            loginError = nil
        } catch { loginError = error.localizedDescription }
    }

    private func pollLogin() async {
        let path = socketPath
        do {
            login = try await Task.detached { try IPCClient.login(method: "login.status", socketPath: path) }.value.attempt
            loginError = nil
        } catch { loginError = error.localizedDescription }
    }

    private func watchLogin() {
        loginTask?.cancel()
        loginTask = Task {
            while !Task.isCancelled && login?.isActive == true {
                do { try await Task.sleep(for: .milliseconds(500)) } catch { return }
                if !loginBusy { await pollLogin() }
            }
            if !Task.isCancelled { await refresh() }
        }
    }

    func clearAccountError() {
        accountError = nil
        accountErrorID = nil
    }

    func accountOperation(_ method: String, params: LoginParams = LoginParams()) async {
        guard !accountBusy else {
            accountError = "上一项操作尚未完成，请稍后重试"
            accountErrorID = params.id
            return
        }
        accountBusy = true
        defer { accountBusy = false }
        let path = socketPath
        do {
            let result = try await Task.detached { try IPCClient.accounts(method: method, params: params, socketPath: path) }.value
            directory = result.directory
            if method == "accounts.login" { login = result.attempt }
            accountError = nil
            accountErrorID = nil
        } catch { accountError = error.localizedDescription; accountErrorID = params.id }
    }

    func reloadAccounts() async {
        guard !accountBusy else { return }
        let path = socketPath
        do {
            let result = try await Task.detached { try IPCClient.accounts(socketPath: path) }.value
            directory = result.directory
            directoryError = nil
            if result.attempt?.isActive == true || (login != nil && login?.attemptId == result.attempt?.attemptId) { login = result.attempt }
        } catch { directoryError = error.localizedDescription }
    }

    func refreshQuotaIfNeeded() async {
        guard isConnected else { return }
        if directory?.cloud?.mode == "cloud" {
            if directory?.cloud?.paired == true && directory?.cloud?.busy != true { await cloudAction("refresh") }
            return
        }
        await checkQuota()
    }

    func cloudAction(_ action: String, id: String? = nil, mode: String? = nil, endpoint: String? = nil, label: String? = nil, confirm: Bool? = nil, automatic: Bool? = nil) async {
        await accountOperation("accounts.cloud", params: LoginParams(id: id, label: label, confirm: confirm, automatic: automatic, cloud_action: action, mode: mode, endpoint: endpoint))
    }

    func checkQuota(id: String? = nil) async {
        if let id {
            guard quotaRequests.insert(id).inserted else { return }
        } else {
            guard !quotaSweepPending else { return }
            quotaSweepPending = true
        }
        defer {
            if let id { quotaRequests.remove(id) }
            else { quotaSweepPending = false }
        }
        let path = socketPath
        do {
            let result = try await Task.detached {
                try IPCClient.accounts(method: "accounts.quota", params: LoginParams(id: id), socketPath: path)
            }.value
            directory = result.directory
            if let id { quotaErrors[id] = nil }
            else { directoryError = nil }
        } catch {
            if let id { quotaErrors[id] = error.localizedDescription }
            else { directoryError = error.localizedDescription }
        }
    }

    private func watchAccounts() {
        directoryTask?.cancel()
        directoryTask = Task {
            while !Task.isCancelled {
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
                await reloadAccounts()
            }
        }
    }

    private func launchService() throws {
        guard let directory = Bundle.main.executableURL?.deletingLastPathComponent() else {
            throw ServiceError.missingExecutable
        }
        let executable = directory.appendingPathComponent("auth-service")
        guard FileManager.default.isExecutableFile(atPath: executable.path) else {
            throw ServiceError.missingExecutable
        }
        let child = Process()
        child.executableURL = executable
        child.arguments = ["serve", "--socket", socketPath, "--managed"]
        let pipe = Pipe()
        child.standardInput = pipe
        child.standardOutput = FileHandle.nullDevice
        child.standardError = FileHandle.nullDevice
        try child.run()
        lifetimePipe = pipe
        process = child
    }
}
