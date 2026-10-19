// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "LiteMail",
    platforms: [.macOS(.v26)],
    targets: [
        // Rust core (static library + C header), built by scripts/build-core.sh.
        .binaryTarget(name: "litemail_coreFFI", path: "Frameworks/litemail_coreFFI.xcframework"),
        // UniFFI-generated Swift bindings for the core.
        .target(
            name: "LiteMailCore",
            dependencies: ["litemail_coreFFI"],
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
                .linkedFramework("CoreFoundation"),
            ]
        ),
        .executableTarget(
            name: "LiteMail",
            dependencies: ["LiteMailCore"],
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [.linkedFramework("WebKit"), .linkedFramework("UserNotifications")]
        ),
        .testTarget(name: "LiteMailTests", dependencies: ["LiteMail"]),
    ]
)
