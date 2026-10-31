import SwiftUI

struct ContentView: View {
    @ObservedObject var worker: WorkerManager
    @AppStorage("node") private var node = ""
    @AppStorage("name") private var name = ""
    @AppStorage("root") private var root = "~/projects"
    @AppStorage("port") private var port = "8080"
    @AppStorage("remotePort") private var remotePort = "8080"
    @AppStorage("sshHost") private var sshHost = ""
    @AppStorage("runnerPath") private var runnerPath = ""
    @AppStorage("capacity") private var capacity = 1
    @AppStorage("codexInteractive") private var codexInteractive = false
    @AppStorage("claudeInteractive") private var claudeInteractive = false
    @State private var token = ""
    @State private var savedToken = ""
    @State private var message = ""
    @State private var loadingToken = true
    @State private var savingToken = false
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack { Image(systemName: worker.running ? "circle.fill" : "circle").foregroundStyle(worker.running ? .green : .secondary); Text(worker.status); Spacer(); Button(worker.running ? "Stop worker" : "Start worker") { worker.running ? worker.stop() : saveAndStart() }.disabled(worker.stopping || worker.accessingKeychain || loadingToken || savingToken) }
            Text("Stopping waits for active jobs to finish.").font(.caption).foregroundStyle(.secondary)
            Form {
                TextField("Node ID", text: $node)
                TextField("Node name", text: $name)
                TextField("Projects directory", text: $root)
                Stepper("Initial capacity: \(capacity)", value: $capacity, in: 1...64)
                TextField("Local controller port", text: $port)
                TextField("SSH host (blank for local controller)", text: $sshHost)
                if !sshHost.isEmpty { TextField("Controller port on SSH host", text: $remotePort) }
                TextField("Additional runner PATH (optional)", text: $runnerPath)
                Toggle("Interactive Codex approvals (experimental)", isOn: $codexInteractive)
                Toggle("Interactive Claude questions and approvals (experimental)", isOn: $claudeInteractive)
                SecureField("Worker credential", text: $token)
            }.disabled(worker.running || worker.accessingKeychain || loadingToken || savingToken)
            Text("Credential is stored in macOS Keychain. For SSH, configure key-based authentication and trust the controller host in Terminal first.").font(.caption).foregroundStyle(.secondary)
            if loadingToken || savingToken { Text("Waiting for Keychain…").font(.caption).foregroundStyle(.secondary) }
            if !message.isEmpty { Text(message).foregroundStyle(.red) }
        }.padding(24).task {
            worker.accessingKeychain = true
            do {
                let stored = try await Task.detached { try SecretStore.load() }.value
                token = stored
                savedToken = stored
            } catch {
                message = "Could not read worker credential from Keychain: \(error.localizedDescription)"
            }
            loadingToken = false
            worker.accessingKeychain = false
        }
    }
    private func saveAndStart() {
        guard !node.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, !root.isEmpty, !token.isEmpty else { message = "Enter a node ID, projects directory and worker credential."; return }
        let credential = token
        if credential == savedToken {
            message = ""
            worker.start(token: credential)
            return
        }
        savingToken = true
        worker.accessingKeychain = true
        Task {
            do {
                try await Task.detached { try SecretStore.save(credential) }.value
                savedToken = credential; savingToken = false; worker.accessingKeychain = false; message = ""; worker.start(token: credential)
            } catch {
                savingToken = false; worker.accessingKeychain = false; message = "Could not save credential: \(error.localizedDescription)"
            }
        }
    }
}
