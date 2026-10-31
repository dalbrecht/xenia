import AppKit
import Foundation

@MainActor final class WorkerManager: ObservableObject {
    @Published var running = false
    @Published var stopping = false
    @Published var accessingKeychain = false
    @Published var status = "Stopped"
    private var process: Process?
    private var tunnel: Process?
    private var stopFile: URL?
    private var logHandle: FileHandle?
    private let sshExecutable: URL
    private let retryBaseSeconds: Double
    private var tunnelArguments: [String]?
    private var tunnelRetry: Task<Void, Never>?
    private var tunnelFailures = 0
    private var tunnelStartedAt: TimeInterval = 0

    @Published var pairing = false
    @Published var pairingStatus = ""
    private var pairProcess: Process?
    private var pairStop: URL?

    static func validController(_ value: String) -> Bool {
        guard let url = URLComponents(string: value), url.scheme == "https", url.host != nil,
              url.user == nil, url.password == nil, url.query == nil, url.fragment == nil,
              url.path.isEmpty || url.path == "/" else { return false }
        return true
    }
    func cancelPairing() {
        guard let pairStop else { return }
        do { try Data().write(to: pairStop, options: .atomic); pairingStatus = "Cancelling request…" }
        catch { pairingStatus = "Could not cancel request: \(error.localizedDescription)" }
    }
    func requestAccess(url: String, node: String, name: String) async throws -> String {
        guard !running, !pairing, Self.validController(url), !node.isEmpty else { throw NSError(domain: "XeniaWorker", code: 1, userInfo: [NSLocalizedDescriptionKey: "Enter an HTTPS controller origin and node ID."]) }
        guard let engine = Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent("xenia-engine") else { throw NSError(domain: "XeniaWorker", code: 1) }
        let process = Process(); process.executableURL = engine; process.arguments = ["pair"]
        let output = Pipe(), errors = Pipe(); process.standardOutput = output; process.standardError = errors
        let stop = FileManager.default.temporaryDirectory.appendingPathComponent("xenia-pair-stop-\(UUID().uuidString)")
        var environment = ProcessInfo.processInfo.environment.filter { !$0.key.uppercased().hasPrefix("XENIA_") }
        environment["XENIA_CONTROLLER"] = url; environment["XENIA_NODE_ID"] = node
        environment["XENIA_NODE_NAME"] = name.isEmpty ? node : name; environment["XENIA_PAIR_STOP_FILE"] = stop.path
        process.environment = environment
        try process.run(); pairProcess = process; pairStop = stop; pairing = true; pairingStatus = "Requesting access…"
        defer { if process.isRunning { process.terminate() }; pairing = false; pairProcess = nil; pairStop = nil; try? FileManager.default.removeItem(at: stop) }
        var credential: String?
        for try await line in output.fileHandleForReading.bytes.lines {
            guard let data = line.data(using: .utf8), let value = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { continue }
            if value["state"] as? String == "completed" { credential = value["token"] as? String }
            else if let code = value["code"] as? String { pairingStatus = "Compare code \(code) in the controller’s Nodes view and approve this worker. Expires in five minutes." }
        }
        let code = await Task.detached { process.waitUntilExit(); return process.terminationStatus }.value
        if code == 0, let credential { return credential }
        let error = String(data: errors.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? "Request ended"
        throw NSError(domain: "XeniaWorker", code: Int(code), userInfo: [NSLocalizedDescriptionKey: error.trimmingCharacters(in: .whitespacesAndNewlines)])
    }

    init(sshExecutable: URL = URL(fileURLWithPath: "/usr/bin/ssh"), retryBaseSeconds: Double = 1) {
        self.sshExecutable = sshExecutable
        self.retryBaseSeconds = retryBaseSeconds
    }

    private func launchTunnel() throws {
        guard let arguments = tunnelArguments, let handle = logHandle else { throw NSError(domain: "XeniaWorker", code: 1) }
        let ssh = Process(); ssh.executableURL = sshExecutable; ssh.arguments = arguments
        ssh.standardOutput = handle; ssh.standardError = handle
        ssh.terminationHandler = { [weak self] process in Task { @MainActor in self?.tunnelExited(process) } }
        try ssh.run(); tunnel = ssh; tunnelStartedAt = ProcessInfo.processInfo.systemUptime
    }
    private func tunnelExited(_ finished: Process) {
        guard tunnel === finished else { return }
        tunnel = nil
        guard running else { return }
        if ProcessInfo.processInfo.systemUptime - tunnelStartedAt >= 30 { tunnelFailures = 0 }
        status = stopping ? "Stopping; reconnecting SSH to deliver reports…" : "SSH tunnel disconnected; reconnecting…"
        scheduleTunnelRetry()
    }
    private func scheduleTunnelRetry() {
        guard running, tunnelRetry == nil, tunnelArguments != nil else { return }
        let delay = min(retryBaseSeconds * pow(2.0, Double(min(tunnelFailures, 5))), 30)
        tunnelFailures += 1
        tunnelRetry = Task { [weak self] in
            do { try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000)) } catch { return }
            guard let self else { return }
            self.tunnelRetry = nil
            guard self.running else { return }
            do {
                try self.launchTunnel()
                self.status = self.stopping ? "Stopping after active work finishes…" : "SSH tunnel restarting (check controller for connectivity)"
            } catch {
                self.status = "Could not restart SSH tunnel; retrying…"
                self.scheduleTunnelRetry()
            }
        }
    }

    func start() {
        guard !running, !pairing, !accessingKeychain else { return }
        accessingKeychain = true
        status = "Waiting for Keychain…"
        Task {
            do {
                let token = try await Task.detached { try SecretStore.load() }.value
                accessingKeychain = false
                start(token: token)
            } catch {
                accessingKeychain = false
                status = "Could not read worker credential from Keychain: \(error.localizedDescription)"
            }
        }
    }

    func start(token: String) {
        guard !running, !pairing else { return }
        let defaults = UserDefaults.standard
        let node = defaults.string(forKey: "node") ?? ""
        let root = defaults.string(forKey: "root") ?? "~/projects"
        guard !node.isEmpty, !root.isEmpty, !token.isEmpty else { status = "Fill in node ID, projects directory and worker token."; return }
        guard let executable = Bundle.main.executableURL?.deletingLastPathComponent().appendingPathComponent("xenia-engine"), FileManager.default.isExecutableFile(atPath: executable.path) else { status = "The bundled worker engine is missing."; return }
        do {
            let directory: URL
            if let override = ProcessInfo.processInfo.environment["XENIA_WORKER_DATA_DIR"], !override.isEmpty { directory = URL(fileURLWithPath: override, isDirectory: true) }
            else { directory = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("XeniaWorker", isDirectory: true) }
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            let log = directory.appendingPathComponent("worker.log")
            if !FileManager.default.fileExists(atPath: log.path) { FileManager.default.createFile(atPath: log.path, contents: nil, attributes: [.posixPermissions: 0o600]) }
            let handle = try FileHandle(forWritingTo: log); try handle.seekToEnd(); logHandle = handle
            let stop = directory.appendingPathComponent("stop-\(UUID().uuidString)"); stopFile = stop
            let sshHost = defaults.string(forKey: "sshHost") ?? ""
            let port = defaults.string(forKey: "port") ?? "8080"
            guard let number = Int(port), (1...65535).contains(number) else { status = "Controller port must be 1–65535."; return }
            let controllerURL = defaults.string(forKey: "controllerURL") ?? ""
            guard controllerURL.isEmpty || Self.validController(controllerURL) else { status = "Enter an HTTPS controller origin without a path, query or credentials."; return }
            if controllerURL.isEmpty && !sshHost.isEmpty {
                let remotePort = defaults.string(forKey: "remotePort") ?? "8080"
                guard let remoteNumber = Int(remotePort), (1...65535).contains(remoteNumber) else { status = "Remote controller port must be 1–65535."; return }
                tunnelArguments = ["-N", "-o", "BatchMode=yes", "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3", "-L", "127.0.0.1:\(port):127.0.0.1:\(remotePort)", "--", sshHost]
                tunnelFailures = 0
                try launchTunnel()
            }
            let worker = Process(); worker.executableURL = executable; worker.arguments = ["worker"]
            var env = ProcessInfo.processInfo.environment
            let extra = defaults.string(forKey: "runnerPath") ?? ""
            env["PATH"] = [extra, NSHomeDirectory() + "/.local/bin", "/opt/homebrew/bin", "/usr/local/bin", env["PATH"] ?? "/usr/bin:/bin"].filter { !$0.isEmpty }.joined(separator: ":")
            env.removeValue(forKey: "XENIA_CODEX_APP_SERVER")
            env.removeValue(forKey: "XENIA_CLAUDE_CONTROL")
            if defaults.bool(forKey: "codexInteractive") { env["XENIA_CODEX_APP_SERVER"] = "1" }
            if defaults.bool(forKey: "claudeInteractive") { env["XENIA_CLAUDE_CONTROL"] = "1" }
            env["XENIA_ALLOW_CODEX"] = (defaults.object(forKey: "allowCodex") as? Bool ?? true) ? "1" : "0"
            env["XENIA_ALLOW_CLAUDE"] = (defaults.object(forKey: "allowClaude") as? Bool ?? true) ? "1" : "0"
            env["XENIA_ALLOW_PROJECT_CHECKOUT"] = defaults.bool(forKey: "allowCheckout") ? "1" : "0"
            env["XENIA_ALLOW_SETUP_SCRIPTS"] = defaults.bool(forKey: "allowSetup") ? "1" : "0"
            env["XENIA_ALLOW_CONTAINERS"] = defaults.bool(forKey: "allowContainers") ? "1" : "0"
            env["XENIA_ALLOWED_PROJECTS"] = defaults.string(forKey: "allowedProjects") ?? ""
            env["XENIA_NODE_ID"] = node
            let name = defaults.string(forKey: "name") ?? ""
            env["XENIA_NODE_NAME"] = name.isEmpty ? node : name
            env["XENIA_PROJECTS_ROOT"] = NSString(string: root).expandingTildeInPath
            env["XENIA_CAPACITY"] = String(max(1, defaults.integer(forKey: "capacity")))
            env["XENIA_CONTROLLER"] = controllerURL.isEmpty ? "http://127.0.0.1:\(port)" : controllerURL; env["XENIA_WORKER_TOKEN"] = token
            env["XENIA_WORKER_DB"] = directory.appendingPathComponent("worker.db").path; env["XENIA_STOP_FILE"] = stop.path
            worker.environment = env; worker.standardOutput = handle; worker.standardError = handle
            worker.terminationHandler = { [weak self] process in Task { @MainActor in self?.finished(process.terminationStatus) } }
            try worker.run(); process = worker; running = true; stopping = false; status = "Running (check the controller for connectivity)"
        } catch {
            if let tunnel, tunnel.isRunning { tunnel.terminate() }; tunnel = nil
            tunnelArguments = nil; try? logHandle?.close(); logHandle = nil
            status = "Could not start: \(error.localizedDescription)"
        }
    }
    func stop() {
        guard running, let stopFile else { return }
        do { try Data().write(to: stopFile, options: .atomic); stopping = true; status = "Stopping after active work finishes…" }
        catch { status = "Could not request stop: \(error.localizedDescription)" }
    }
    private func finished(_ code: Int32) {
        tunnelRetry?.cancel(); tunnelRetry = nil; tunnelArguments = nil
        if let tunnel, tunnel.isRunning { tunnel.terminate() }; tunnel = nil
        if let stopFile { try? FileManager.default.removeItem(at: stopFile) }
        try? logHandle?.close(); logHandle = nil; process = nil; running = false; stopping = false
        status = code == 0 ? "Stopped" : "Worker exited (\(code)); see worker.log in Application Support/XeniaWorker."
    }
    func quit() { if pairing { cancelPairing(); status = "Cancelling pairing; quit once it finishes." } else if running { stop(); status = "Stopping after active work finishes; quit once stopped." } else { NSApplication.shared.terminate(nil) } }
}
