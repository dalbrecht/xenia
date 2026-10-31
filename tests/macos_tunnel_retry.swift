import Foundation

@main struct TunnelProbe {
    @MainActor static func main() async {
        let env = ProcessInfo.processInfo.environment
        let data = URL(fileURLWithPath: env["XENIA_WORKER_DATA_DIR"]!)
        UserDefaults.standard.register(defaults: ["node":"synthetic", "root":"/tmp", "capacity":1, "port":"18080", "remotePort":"8080", "sshHost":"synthetic-host"])
        let worker = WorkerManager(sshExecutable: URL(fileURLWithPath: env["TEST_SSH"]!), retryBaseSeconds: 0.05)
        worker.start(token: "synthetic-token")
        guard worker.running else { fputs("worker failed to start\n", stderr); exit(1) }
        func starts() -> Int { (try? String(contentsOf: data.appendingPathComponent("ssh-starts"), encoding: .utf8).split(separator: "\n").count) ?? 0 }
        for _ in 0..<200 {
            if starts() >= 2 { break }
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
        guard worker.running && starts() >= 2 else { fputs("SSH tunnel did not reconnect\n", stderr); exit(1) }
        worker.stop()
        for _ in 0..<200 {
            if !worker.running { break }
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
        guard !worker.running else { fputs("worker failed to stop\n", stderr); exit(1) }
        let afterStop = starts()
        try? await Task.sleep(nanoseconds: 300_000_000)
        guard starts() == afterStop else { fputs("SSH restarted after worker exit\n", stderr); exit(1) }
    }
}
