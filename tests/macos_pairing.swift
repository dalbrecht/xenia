import Foundation
@main struct PairingProbe {
    @MainActor static func main() async {
        let worker = WorkerManager()
        let cancel = ProcessInfo.processInfo.environment["TEST_CANCEL"] == "1"
        let task = Task { try await worker.requestAccess(url: "https://controller.example:8443", node: "example-node", name: "Example node") }
        for _ in 0..<100 {
            if worker.pairingStatus.contains("ABC12345") { break }
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
        guard worker.pairingStatus.contains("ABC12345") else { fatalError("Comparison code was not displayed") }
        if cancel { worker.cancelPairing() }
        do {
            let credential = try await task.value
            guard !cancel, credential == String(repeating: "c", count: 64), !worker.pairing else { fatalError("Unexpected completion") }
        } catch { if !cancel { fatalError("Unexpected pairing failure: \(error)") } }
        guard !worker.pairing else { fatalError("Pairing state was not released") }
    }
}
