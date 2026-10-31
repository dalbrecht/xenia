// Launched in a disposable app bundle by macos_worker_settings.py.
// No Keychain access: the synthetic token is passed to WorkerManager directly.
import Foundation

@main struct SettingsProbe {
    @MainActor static func main() async {
        let environment = ProcessInfo.processInfo.environment
        UserDefaults.standard.register(defaults: [
            "node": "synthetic-node", "capacity": 1, "port": "8080",
            "codexInteractive": environment["TEST_CODEX"] == "1",
            "claudeInteractive": environment["TEST_CLAUDE"] == "1"
        ])
        let worker = WorkerManager()
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
