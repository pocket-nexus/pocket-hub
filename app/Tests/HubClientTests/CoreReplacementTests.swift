import Foundation
import MockHubCore
import Testing
@testable import HubClient

/// An updated app finds the core of the version before it still running (the
/// core outlives the app) and replaces it with its own (CONTROL-API.md §1).
@Suite("Client: an older core makes way")
struct CoreReplacementTests {
    /// A client of version `version` whose launcher is a script that records
    /// each start in `launches` (the test then brings the "core" up itself).
    private func client(home: URL, version: String, launcher: Bool = true) throws -> (HubClient, marker: URL) {
        let marker = home.appending(path: "launches")
        let script = home.appending(path: "fake-hubd")
        try "#!/bin/sh\necho started >> '\(marker.path)'\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        let client = HubClient(configuration: .init(
            identity: ClientIdentity(kind: .gui, name: "Pocket Hub Tests", version: version),
            socketPath: { HubPaths(home: home).socketPath() },
            launcher: launcher ? CoreLauncher(executable: { script }, environment: [:]) : nil,
            retryDelays: [.milliseconds(50), .milliseconds(100)],
            helloTimeout: .milliseconds(500),
            launchGrace: .seconds(30)))
        return (client, marker)
    }

    private func launches(_ marker: URL) -> Int {
        ((try? String(contentsOf: marker, encoding: .utf8)) ?? "").split(separator: "\n").count
    }

    private func core(home: URL, version: String, scenario: MockScenario = .demo) async throws -> (MockHubCore, Locked<Bool>) {
        var scenario = scenario
        scenario.hubVersion = version
        let core = MockHubCore(home: home, scenario: scenario)
        try await core.start()
        let shutDown = Locked(false)
        await core.setOnShutdown { shutDown.mutate { $0 = true } }
        return (core, shutDown)
    }

    @Test func anOlderCoreIsShutDownAndTheAppsOwnStarted() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let (old, oldShutDown) = try await core(home: home, version: "0.2.1")
        defer { Task { await old.stop() } }
        let (client, marker) = try client(home: home, version: "0.2.2")
        let log = UpdateLog(client)
        await client.start()
        await eventually("the older core asked to shut down") { oldShutDown.value }
        await eventually("the app's own core started") { launches(marker) == 1 }
        let (new, newShutDown) = try await core(home: home, version: "0.2.2")
        defer { Task { await new.stop() } }
        await eventually("connected") { await log.isConnected }
        // No device was held, so the older core was never shown: the first
        // state a UI gets is the new core's.
        #expect(await log.snapshots.map(\.hello.hub.version) == ["0.2.2"])
        try await Task.sleep(for: .milliseconds(300))
        #expect(!newShutDown.value)
        #expect(launches(marker) == 1)
        await client.stop()
    }

    /// A device operation in progress (another client's: the app's own end
    /// when it disconnects) would end with the core, so the client waits.
    @Test func itWaitsUntilNoDeviceIsHeld() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let (old, oldShutDown) = try await core(home: home, version: "0.2.1")
        defer { Task { await old.stop() } }
        let held = "89abcdef01234567"
        await old.modify(held) {
            $0.busy = ["holder": ["kind": "cli", "name": "pocket-hub", "method": "devices.installProgram"],
                       "since": 1, "launching": false]
        }
        let (client, marker) = try client(home: home, version: "0.2.2")
        let log = UpdateLog(client)
        await client.start()
        await eventually("connected to the older core") { await log.isConnected }
        #expect(await log.snapshots.first?.hello.hub.version == "0.2.1")
        try await Task.sleep(for: .milliseconds(300))
        #expect(!oldShutDown.value, "the core was shut down under a held device")
        // Another device changing is not the held one coming free.
        await old.modify("0123456789abcdef") { $0.name = "Renamed" }
        try await Task.sleep(for: .milliseconds(200))
        #expect(!oldShutDown.value)
        await old.modify(held) { $0.busy = .null }
        await eventually("the older core asked to shut down") { oldShutDown.value }
        await eventually("the app's own core started") { launches(marker) == 1 }
        let (new, _) = try await core(home: home, version: "0.2.2")
        defer { Task { await new.stop() } }
        await eventually("connected to the new core") { await log.snapshots.last?.hello.hub.version == "0.2.2" }
        // The core left because it was asked to: no failure in between.
        #expect(!(await log.states.contains { if case .unreachable(.failed, _) = $0 { true } else { false } }))
        await client.stop()
    }

    @Test func aCoreOfTheSameOrANewerVersionStays() async throws {
        for version in ["0.2.2", "0.3.0", "0.10.0"] {
            let home = try temporaryHome()
            defer { try? FileManager.default.removeItem(at: home) }
            let (core, shutDown) = try await core(home: home, version: version)
            defer { Task { await core.stop() } }
            let (client, marker) = try client(home: home, version: "0.2.2")
            let log = UpdateLog(client)
            await client.start()
            await eventually("connected") { await log.isConnected }
            try await Task.sleep(for: .milliseconds(200))
            #expect(!shutDown.value, "a core of \(version) was replaced by a client of 0.2.2")
            #expect(launches(marker) == 0)
            await client.stop()
        }
    }

    /// The mock's own version, a build's: no release, never older.
    @Test func aCoreWhoseVersionIsNoReleaseStays() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let (core, shutDown) = try await core(home: home, version: "0.1.0-mock")
        defer { Task { await core.stop() } }
        let (client, _) = try client(home: home, version: "0.2.2")
        let log = UpdateLog(client)
        await client.start()
        await eventually("connected") { await log.isConnected }
        try await Task.sleep(for: .milliseconds(200))
        #expect(!shutDown.value)
        await client.stop()
    }

    /// A client that starts no core (the app with POCKET_HUB_NO_LAUNCH=1, a
    /// tool) has no core of its own to put in its place.
    @Test func aClientThatStartsNoCoreLeavesIt() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let (core, shutDown) = try await core(home: home, version: "0.2.1")
        defer { Task { await core.stop() } }
        let (client, _) = try client(home: home, version: "0.2.2", launcher: false)
        let log = UpdateLog(client)
        await client.start()
        await eventually("connected") { await log.isConnected }
        try await Task.sleep(for: .milliseconds(200))
        #expect(!shutDown.value)
        await client.stop()
    }

    /// The core a client started is its own whatever version it reports (a
    /// `$POCKET_HUBD` of another build): replacing it would start it again.
    @Test func aCoreTheClientStartedStays() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let (client, marker) = try client(home: home, version: "0.2.2")
        let log = UpdateLog(client)
        await client.start()
        await eventually("started") { launches(marker) == 1 }
        let (core, shutDown) = try await core(home: home, version: "0.2.1")
        defer { Task { await core.stop() } }
        await eventually("connected") { await log.isConnected }
        try await Task.sleep(for: .milliseconds(200))
        #expect(!shutDown.value)
        #expect(launches(marker) == 1)
        await client.stop()
    }

    @Test func releaseVersions() {
        #expect(ReleaseVersion("0.2.1") != nil)
        #expect(ReleaseVersion("0.2.1")! < ReleaseVersion("0.2.2")!)
        #expect(ReleaseVersion("0.9.0")! < ReleaseVersion("0.10.0")!)
        #expect(ReleaseVersion("1.0.0")! > ReleaseVersion("0.99.99")!)
        #expect(ReleaseVersion("0.2.1") == ReleaseVersion("0.2.1"))
        for text in ["0.1.0-mock", "0.2", "0.2.1.4", "v0.2.1", "0..1", "+1.0.0", " 0.2.1", ""] {
            #expect(ReleaseVersion(text) == nil, "\(text) parsed as a release")
        }
    }
}
