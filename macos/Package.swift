// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "AuthManager",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "AuthManager", targets: ["AuthManager"]), .executable(name: "credential-store", targets: ["CredentialStore"]), .executable(name: "desktop-bridge", targets: ["DesktopBridge"])],
    targets: [.executableTarget(name: "AuthManager", resources: [.process("Resources")]), .executableTarget(name: "CredentialStore"), .executableTarget(name: "DesktopBridge", resources: [.copy("bridge.js"), .copy("preflight.js"), .copy("selective-ui.js"), .copy("automatic.js")])]
)
