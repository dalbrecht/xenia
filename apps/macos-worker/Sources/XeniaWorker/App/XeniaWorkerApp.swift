import SwiftUI
import AppKit

@main
struct XeniaWorkerApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var worker = WorkerManager()
    var body: some Scene {
        WindowGroup("Xenia Worker") { ContentView(worker: worker).frame(minWidth: 520, minHeight: 580).onAppear { appDelegate.worker = worker } }
        MenuBarExtra("Xenia Worker", systemImage: worker.running ? "circle.fill" : "circle") {
            Text(worker.status)
            Button(worker.running ? "Stop worker" : "Start worker") { worker.running ? worker.stop() : worker.start() }
                .disabled(worker.stopping || worker.pairing || worker.accessingKeychain)
            Divider()
            Button("Quit Xenia Worker") { worker.quit() }
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    weak var worker: WorkerManager?
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let worker, worker.running || worker.pairing else { return .terminateNow }
        worker.quit()
        return .terminateCancel
    }
}
