// Launched in a disposable app bundle by macos_worker_settings.py.
// No Keychain access: the synthetic token is passed to WorkerManager directly.
import Foundation

@main struct SettingsProbe {
    @MainActor static func main() async {
        let environment = ProcessInfo.processInfo.environment
        UserDefaults.standard.register(defaults: [
            "node": "synthetic-node", "capacity": 1, "port": "8080",
            "controllerURL": environment["TEST_HTTPS"] == "1" ? "https://controller.example:8443" : "",
            "sshHost": environment["TEST_HTTPS"] == "1" ? "legacy-must-not-run" : "",
            "codexInteractive": environment["TEST_CODEX"] == "1",
            "claudeInteractive": environment["TEST_CLAUDE"] == "1"
        ])
        let worker = WorkerManager(sshExecutable: URL(fileURLWithPath: environment["TEST_SSH"]!))
        worker.start(token: "synthetic-worker-credential")
        for _ in 0..<100 {
            if !worker.running { break }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        if worker.running || worker.status != "Stopped" {
            fputs("Worker launch probe failed: \(worker.status)\n", stderr)
            exit(1)
        }
    }
}
