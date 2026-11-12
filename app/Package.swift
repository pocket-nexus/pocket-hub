// swift-tools-version: 6.0
//
// Pocket Hub for macOS: a thin client of `pocket-hubd` (docs/CONTROL-API.md).
//
//   HubClient    the control API: socket, framing, JSON-RPC, typed models, reconnection.
//                Foundation only, so another surface (a relay, a CLI) can reuse it.
//   HubAppModel  observable state for a UI, built on HubClient. No SwiftUI.
//   MockHubCore  an in-process implementation of the contract for tests and development.
//   mock-hubd    MockHubCore as a process, so the app can be run against it.
//
// The SwiftUI app itself is the PocketHub target of PocketHub.xcodeproj
// (sources in PocketHub/), which uses this package's libraries: Xcode makes
// the app bundle, its Info.plist and its localizations.

import PackageDescription

let package = Package(
    name: "PocketHub",
    defaultLocalization: "en",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "HubClient", targets: ["HubClient"]),
        .library(name: "HubAppModel", targets: ["HubAppModel"]),
        .library(name: "MockHubCore", targets: ["MockHubCore"]),
        .executable(name: "mock-hubd", targets: ["mock-hubd"]),
    ],
    targets: [
        .target(name: "HubClient"),
        .target(name: "HubAppModel", dependencies: ["HubClient"]),
        .target(name: "MockHubCore", dependencies: ["HubClient"]),
        .executableTarget(name: "mock-hubd", dependencies: ["MockHubCore"]),
        .testTarget(name: "HubClientTests", dependencies: ["HubClient", "MockHubCore"]),
        .testTarget(name: "HubAppModelTests", dependencies: ["HubAppModel", "HubClient", "MockHubCore"]),
    ]
)
