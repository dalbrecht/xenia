// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "XeniaWorker", platforms: [.macOS(.v13)], products: [.executable(name: "XeniaWorker", targets: ["XeniaWorker"])], targets: [.executableTarget(name: "XeniaWorker")])
