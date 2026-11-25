// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "LocalNotebook",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "LocalNotebook", targets: ["LocalNotebook"]),
    ],
    targets: [
        .target(
            name: "CLnaFfi",
            path: "Sources/CLnaFfi",
            publicHeadersPath: "include"
        ),
        .executableTarget(
            name: "LocalNotebook",
            dependencies: ["CLnaFfi"],
            path: "Sources/LocalNotebook",
            resources: [.copy("Resources/deck_player.html")],
            linkerSettings: [
                .unsafeFlags(["-L../../../target/release", "-llna_ffi"])
            ]
        ),
    ]
)
