import Foundation
import HubClient
import MockHubCore
import Testing
@testable import HubAppModel

func temporaryHome() throws -> URL {
    let url = URL(fileURLWithPath: "/tmp/phm-\(UUID().uuidString.prefix(8))", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

@MainActor
func eventually(_ what: String = "condition", timeout: Duration = .seconds(5),
                sourceLocation: SourceLocation = #_sourceLocation,
                _ condition: @MainActor () async -> Bool) async
{
    let deadline = ContinuousClock.now + timeout
    while ContinuousClock.now < deadline {
        if await condition() { return }
        try? await Task.sleep(for: .milliseconds(20))
    }
    Issue.record("timed out waiting for \(what)", sourceLocation: sourceLocation)
}

/// A mock core and a model connected to it.
@MainActor
struct Harness {
    let home: URL
    let core: MockHubCore
    let model: HubModel

    init(_ scenario: MockScenario) async throws {
        home = try temporaryHome()
        core = MockHubCore(home: home, scenario: scenario)
        try await core.start()
        let home = self.home
        let client = HubClient(configuration: .init(
            identity: ClientIdentity(kind: .gui, name: "Pocket Hub", version: "0.0.1"),
            socketPath: { HubPaths(home: home).socketPath() },
            retryDelays: [.milliseconds(50), .milliseconds(100)]))
        model = HubModel(client: client)
        model.start()
        await eventually("connected") { model.isLive }
    }

    func tearDown() async {
        await model.quit(shutdownCore: false)
        await core.stop()
        try? FileManager.default.removeItem(at: home)
    }
}
