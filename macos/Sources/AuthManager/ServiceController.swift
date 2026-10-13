import AppKit
import Foundation

@MainActor
final class ServiceController: ObservableObject {
    static let shared = ServiceController()
    @Published private(set) var snapshot: Snapshot?
    @Published private(set) var isRefreshing = false
    @Published private(set) var errorText: String?
    @Published private(set) var isConnected = false
    @Published private(set) var isConnecting = false
    @Published private(set) var login: LoginAttempt?
    // The attempt only names its provider; this is the account the app started it for.
    @Published private(set) var loginTarget: LoginTarget?
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
    private var connectionFailures = 0
    private var cloudRefreshAt: Double = 0

    var socketPath: String {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/AuthManager/service.sock").path
    }

    // Restarting cancels an official login and would cut off an operation in flight.
    var canRestart: Bool {
        !isConnecting && !isRefreshing && login?.isActive != true && !loginBusy && directory?.busy != true && !accountBusy
    }

    func start() async {
        guard !isConnecting, !isConnected, process == nil else { return }
        isConnecting = true
        defer { isConnecting = false }
        // The service belongs to the app: a caller that stops waiting must not abandon the handshake.
        await Task { await connect() }.value
    }

    private func connect() async {
        do {
            try await launchUntilReady()
            await refresh()
            await reloadAccounts()
            watchAccounts()
            await pollLogin()
            if login?.isActive == true { watchLogin() }
        } catch {
            errorText = error.localizedDescription
        }
    }

    // A service that was just told to quit keeps the lock while it stops its jobs, and a new one
    // exits at once until then. Each retry starts only after our previous child has exited.
    private func launchUntilReady() async throws {
        let path = socketPath
        for attempt in 0..<8 {
            if attempt > 0 { try await Task.sleep(for: .seconds(1)) }
            let child = try launchService()
            // A stale socket can remain after SIGTERM; readiness is an RPC fact.
            for _ in 0..<30 {
                if (try? await Task.detached { try IPCClient.ping(socketPath: path) }.value) != nil { return }
                if !child.isRunning { break }
                try await Task.sleep(for: .milliseconds(50))
            }
            if child.isRunning { throw ServiceError.connection }
        }
        throw ServiceError.connection
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
            reached(nil)
            await reloadAccounts()
        } catch {
            errorText = error.localizedDescription
            reached(error)
        }
    }

    func restart() async {
        guard canRestart else { return }
        isConnecting = true
        defer { isConnecting = false }
        stop()
        // Wait for our own child to exit before starting another lock owner.
        if let process, process.isRunning {
            await Task.detached { process.waitUntilExit() }.value
        }
        self.process = nil
        await connect()
    }

    func stop() {
        loginTask?.cancel()
        directoryTask?.cancel()
        // EOF tells the Rust owner to cancel and reap its official login child.
        try? lifetimePipe?.fileHandleForWriting.close()
        lifetimePipe = nil
        disconnect(nil)
    }

    // Every answer shows the service is there. A few connection failures in a row mean it is not,
    // and what it published earlier must not stay on screen as if it were current.
    private func reached(_ error: Error?) {
        guard let error else {
            connectionFailures = 0
            if !isConnected { isConnected = true; errorText = nil }
            return
        }
        guard case ServiceError.connection = error else { return }
        connectionFailures += 1
        if connectionFailures >= 3, isConnected {
            disconnect("后台服务没有响应，账号和额度暂停显示；恢复后会自动刷新，也可以重新连接后台服务。")
        }
    }

    // Accounts, quota and login attempts all live in the service; without it none of them is current.
    private func disconnect(_ message: String?) {
        isConnected = false
        directory = nil
        login = nil
        errorText = message
    }

    // A child that exits without stop() having closed its pipe has crashed or quit on its own.
    // It is not restarted behind the user's back: whatever made it exit may need a look first.
    // While connecting, the handshake judges its own child.
    private func exited(_ child: Process) {
        guard child === process else { return }
        process = nil
        guard lifetimePipe != nil, !isConnecting else { return }
        stop()
        errorText = "后台服务已退出，账号和额度不再更新。重新连接后台服务后恢复。"
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
        if accountError == nil { loginTarget = LoginTarget(accountID: accountID, cloudAccountID: cloudAccountID) }
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
        loginTarget = nil
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
            // A poll that stop() cancelled must not bring back the state of the service it left.
            guard !Task.isCancelled else { return }
            reached(nil)
            directory = result.directory
            directoryError = nil
            if result.attempt?.isActive == true || (login != nil && login?.attemptId == result.attempt?.attemptId) { login = result.attempt }
        } catch {
            guard !Task.isCancelled else { return }
            directoryError = error.localizedDescription
            reached(error)
        }
    }

    func refreshQuotaIfNeeded() async {
        guard isConnected else { return }
        if let cloud = directory?.cloud, cloud.mode == "cloud" {
            // The service refreshes the cloud directory itself every 30 s while idle, and a refresh holds
            // its writer slot. App activation only fills a longer gap, and never while an operation runs.
            let now = Date().timeIntervalSince1970
            guard cloud.paired, !cloud.busy, directory?.busy != true, !accountBusy,
                  now - max(cloud.checkedAt ?? 0, cloudRefreshAt) >= 60 else { return }
            cloudRefreshAt = now
            await cloudAction("refresh")
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

    private func launchService() throws -> Process {
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
        child.terminationHandler = { [weak self] child in
            Task { @MainActor in self?.exited(child) }
        }
        try child.run()
        lifetimePipe = pipe
        process = child
        return child
    }
}
