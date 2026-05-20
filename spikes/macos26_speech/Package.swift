// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "macos26_speech",
    platforms: [.macOS("26.0")],
    targets: [
        .executableTarget(
            name: "macos26_speech",
            path: "Sources/macos26_speech"
        )
    ]
)
