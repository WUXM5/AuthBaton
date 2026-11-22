// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "AuthManager",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "AuthManager", targets: ["AuthManager"]), .executable(name: "credential-store", targets: ["CredentialStore"]), .executable(name: "desktop-bridge", targets: ["DesktopBridge"]), .executable(name: "hotspot-helper", targets: ["HotspotHelper"])],
    dependencies: [.package(url: "https://github.com/sparkle-project/Sparkle", exact: "2.10.1")],
    targets: [.executableTarget(name: "AuthManager", dependencies: [.product(name: "Sparkle", package: "Sparkle")], resources: [.process("Resources")]), .executableTarget(name: "CredentialStore"), .executableTarget(name: "DesktopBridge", resources: [.copy("bridge.js"), .copy("preflight.js"), .copy("automatic.js")]), .executableTarget(name: "HotspotHelper")]
)
