// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Sugrob",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(name: "Sugrob", path: "Sources/Sugrob")
    ]
)
