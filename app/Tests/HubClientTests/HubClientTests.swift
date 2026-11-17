import Foundation
import MockHubCore
import Testing
@testable import HubClient

/// Collects a client's updates for assertions.
actor UpdateLog {
    private(set) var updates: [HubClientUpdate] = []

    init(_ client: HubClient) {
        let stream = client.updates
        Task { [weak self] in
            for await update in stream { await self?.append(update) }
        }
    }

    private func append(_ update: HubClientUpdate) { updates.append(update) }

    var snapshots: [HubSnapshot] {
        updates.compactMap { if case .synced(let s) = $0 { s } else { nil } }
    }

    var events: [HubEvent] {
        updates.compactMap { if case .event(let e) = $0 { e } else { nil } }
    }

    var states: [HubConnectionState] {
        updates.compactMap { if case .state(let s) = $0 { s } else { nil } }
    }

    var isConnected: Bool { states.last?.isConnected ?? false }
}

func makeClient(home: URL, launcher: CoreLauncher? = nil) -> HubClient {
    HubClient(configuration: .init(
        identity: ClientIdentity(kind: .gui, name: "Pocket Hub Tests", version: "0.0.1"),
        socketPath: { HubPaths(home: home).socketPath() },
        launcher: launcher,
        retryDelays: [.milliseconds(50), .milliseconds(100)],
        helloTimeout: .milliseconds(500),
        launchGrace: .seconds(30)))
}

@Suite("Client: hello, resync, events, reconnection")
struct HubClientTests {
    @Test func connectsSubscribesAndResyncs() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let core = MockHubCore(home: home, scenario: .demo)
        try await core.start()
        defer { Task { await core.stop() } }
        let client = makeClient(home: home)
        let log = UpdateLog(client)
        await client.start()
        await eventually("a snapshot") { await log.snapshots.count == 1 }
        let snapshot = try #require(await log.snapshots.first)
        #expect(snapshot.hello.hub.name == "Studio Mac")
        #expect(snapshot.status.carriers.first?.port == 8628)
        #expect(snapshot.devices.map(\.id).sorted() == ["0123456789abcdef", "89abcdef01234567"])
        #expect(snapshot.services?.count == 2)
        #expect(snapshot.bun?.installed == true)
        let methods = await core.received.map(\.method)
        #expect(Array(methods.prefix(4)) == ["hub.hello", "hub.subscribe", "hub.status", "devices.list"])
        #expect(await core.received.first?.params["client"] == ["kind": "gui", "name": "Pocket Hub Tests", "version": "0.0.1"])

        // Events arrive after the snapshot.
        await core.unlink("0123456789abcdef", code: "silence", reason: "no frame for 10 s")
        await eventually("unlink events") { await log.events.count >= 2 }
        let events = await log.events
        #expect(events.first == .deviceUnlinked(DeviceUnlinked(id: "0123456789abcdef", code: "silence", reason: "no frame for 10 s")))
        guard case .deviceChanged(let device)? = events.dropFirst().first else { Issue.record("device.changed"); return }
        #expect(device.id == "0123456789abcdef" && !device.isLinked)
        await client.stop()
    }

    @Test func callsGoThroughTheCurrentConnection() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let core = MockHubCore(home: home, scenario: .demo)
        try await core.start()
        defer { Task { await core.stop() } }
        let client = makeClient(home: home)
        let log = UpdateLog(client)
        await #expect(throws: HubError.notConnected) { try await client.devices() }
        await client.start()
        await eventually("connected") { await log.isConnected }
        let shot = try await client.screenshot(device: "0123456789abcdef")
        #expect(shot.width == 400 && shot.height == 480)
        #expect(FileManager.default.fileExists(atPath: shot.path))
        await #expect(throws: HubError.self) { try await client.launch(device: "89abcdef01234567", app: "weather") }
        do {
            _ = try await client.launch(device: "89abcdef01234567", app: "weather")
        } catch let error as HubError {
            #expect(error.kind == .notLinked)
        }
        await client.stop()
    }

    @Test func reconnectsAndResyncsAfterTheCoreDrops() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let core = MockHubCore(home: home, scenario: .demo)
        try await core.start()
        defer { Task { await core.stop() } }
        let client = makeClient(home: home)
        let log = UpdateLog(client)
        await client.start()
        await eventually("first snapshot") { await log.snapshots.count == 1 }

        // The core goes down; while it is down a device changes and nobody is told.
        await core.stopListening()
        await core.dropConnections()
        await eventually("unreachable") {
            if case .unreachable? = await log.states.last { return true }
            return false
        }
        var renamed = try #require(await core.device("89abcdef01234567"))
        renamed.name = "Renamed While Away"
        await core.replaceSilently(renamed)
        try await core.start()

        await eventually("second snapshot") { await log.snapshots.count == 2 }
        let second = try #require(await log.snapshots.last)
        #expect(second.devices.first { $0.id == "89abcdef01234567" }?.name == "Renamed While Away")
        // A fresh hello and subscription on the new connection.
        let sessions = await core.received.filter { $0.method == "hub.hello" }.map(\.session)
        #expect(Set(sessions).count == 2)
        await eventually("connected again") { await log.isConnected }
        _ = try await client.devices()
        await client.stop()
    }

    @Test func eventsOlderThanTheSnapshotAreDropped() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        var scenario = MockScenario.demo
        scenario.responseDelays["devices.list"] = .milliseconds(400)
        let core = MockHubCore(home: home, scenario: scenario)
        try await core.start()
        defer { Task { await core.stop() } }
        let client = makeClient(home: home)
        let log = UpdateLog(client)
        await client.start()
        // While devices.list is being answered: an event announces state A, then
        // the device silently moves to state B. The list answers B; A must not
        // be applied on top of it.
        await eventually("devices.list requested") { await core.received.contains { $0.method == "devices.list" } }
        var a = try #require(await core.device("89abcdef01234567"))
        a.name = "State A"
        await core.upsert(a)
        a.name = "State B"
        await core.replaceSilently(a)
        await eventually("snapshot") { await log.snapshots.count == 1 }
        try await Task.sleep(for: .milliseconds(100))
        #expect(await log.snapshots[0].devices.first { $0.id == "89abcdef01234567" }?.name == "State B")
        #expect(await log.events.isEmpty, "the stale device.changed was forwarded")
        await client.stop()
    }

    @Test func startsTheCoreWhenNobodyListens() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        // A stand-in core: a script that records that it ran.
        let marker = home.appending(path: "launched")
        let script = home.appending(path: "fake-hubd")
        try "#!/bin/sh\necho \"$POCKET_HUB_HOME\" > '\(marker.path)'\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        let launcher = CoreLauncher(executable: { script }, environment: ["POCKET_HUB_HOME": home.path])
        let client = makeClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("launch") { FileManager.default.fileExists(atPath: marker.path) }
        #expect(await log.states.contains(.startingCore))
        #expect(try String(contentsOf: marker, encoding: .utf8).trimmingCharacters(in: .newlines) == home.path)
        // The "core" comes up afterwards; the client connects without launching again.
        let core = MockHubCore(home: home, scenario: .firstRun)
        try await core.start()
        defer { Task { await core.stop() } }
        await eventually("connected") { await log.isConnected }
        #expect(await log.states.filter { $0 == .startingCore }.count == 1)
        await client.stop()
    }

    @Test func aCoreThatCannotStartIsReportedAndNotRestartedInALoop() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let line = "The link port 8628 is in use by another program, so Pocket Hub cannot start. Quit that program, or start Pocket Hub with --link-port <another port>."
        let script = home.appending(path: "failing-hubd")
        try "#!/bin/sh\necho 'pocket-hubd: \(line)' >&2\nexit 1\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        let launches = Locked(0)
        let launcher = CoreLauncher(executable: { launches.mutate { $0 += 1 }; return script })
        let client = makeClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("the core's own line") {
            await log.states.contains { if case .unreachable(.coreFailedToStart(let shown), _) = $0 { shown == line } else { false } }
        }
        // Retries continue (another core may appear) but the failing one is not started again.
        try await Task.sleep(for: .milliseconds(600))
        #expect(launches.value == 1)
        if case .unreachable(.coreFailedToStart, _)? = await log.states.last {} else {
            Issue.record("the failure must stay on screen, not turn into a generic retry: \(String(describing: await log.states.last))")
        }
        // "Try Again" is the owner's request to start it once more.
        await client.retry()
        await eventually("started again") { launches.value == 2 }
        await client.stop()
    }

    @Test func reportsAMissingCore() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let launcher = CoreLauncher(executable: { home.appending(path: "nope") })
        let client = makeClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("core not found") {
            await log.states.contains { if case .unreachable(.coreNotFound, _) = $0 { true } else { false } }
        }
        await client.stop()
        await eventually("stopped") { await log.states.last == .stopped }
    }

    @Test func aHungCoreIsAbandonedAfterTheHelloTimeout() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let client = makeClient(home: home)
        let log = UpdateLog(client)
        await client.start()
        let peer = await server.nextPeer()
        #expect(try await peer.next()["method"] == "hub.hello")
        await eventually("unreachable after timeout") {
            await log.states.contains { if case .unreachable(.failed, _) = $0 { true } else { false } }
        }
        await client.stop()
    }

    /// A core that answers in a higher protocol is a newer Pocket Hub: left alone.
    @Test func aCoreAnsweringInAHigherProtocolIsNewer() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let client = makeClient(home: home)
        let log = UpdateLog(client)
        await client.start()
        let peer = await server.nextPeer()
        let hello = try await peer.next()
        peer.write(#"{"jsonrpc":"2.0","id":\#(hello["id"]!.intValue!),"result":{"protocol":4,"hub":{"id":"h","name":"n","version":"9"}}}"# + "\n")
        await eventually("newer") {
            await log.states.contains(where: { if case .unreachable(.newerVersionRunning, _) = $0 { true } else { false } })
        }
        await client.stop()
    }

    /// A resident core of Pocket Hub 0.2.2 or earlier speaks protocol 1 and
    /// refuses this client's hello with `-32602`, its sentence word for word
    /// and no `data` (measured against the binaries built from `v0.2.1` and
    /// `v0.2.2`). A client with no core of its own to start replaces nothing:
    /// it says which protocol it met.
    @Test func aRefusalWithoutDataIsProtocolOne() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let client = makeClient(home: home)
        let log = UpdateLog(client)
        await client.start()
        let peer = await server.nextPeer()
        let hello = try await peer.next()
        #expect(hello["params"]?["protocol"] == 3)
        peer.write(#"{"jsonrpc":"2.0","id":\#(hello["id"]!.intValue!),"error":{"code":-32602,"message":"This Hub speaks protocol 1, not 2."}}"# + "\n")
        await eventually("incompatible") {
            await log.states.contains(where: { if case .unreachable(.incompatible(hubProtocol: 1), _) = $0 { true } else { false } })
        }
        await client.stop()
    }

    @Test func helloMustComeFirst() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let core = MockHubCore(home: home, scenario: .demo)
        try await core.start()
        defer { Task { await core.stop() } }
        let connection = try HubConnection.open(path: core.socketPath)
        await #expect(throws: HubError.rpc(code: -32600, message: "The first request must be hub.hello.")) {
            try await connection.devices()
        }
        await connection.close()
    }

    @Test func shutdownStopsTheCoreAndDoesNotRestartIt() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let core = MockHubCore(home: home, scenario: .demo)
        try await core.start()
        let shutDown = Locked(false)
        await core.setOnShutdown { shutDown.mutate { $0 = true } }
        let launches = Locked(0)
        let script = home.appending(path: "fake-hubd")
        try "#!/bin/sh\nexit 0\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        let launcher = CoreLauncher(executable: { launches.mutate { $0 += 1 }; return script })
        let client = makeClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("connected") { await log.isConnected }
        try await client.shutdownHub()
        await eventually("core shut down") { shutDown.value }
        try await Task.sleep(for: .milliseconds(300))
        #expect(launches.value == 0, "the client started the core again after shutting it down")
        await eventually("stopped") { await log.states.last == .stopped }
    }

    @Test func retryWakesTheWaitingLoop() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let client = HubClient(configuration: .init(
            identity: ClientIdentity(kind: .gui, name: "t", version: "0"),
            socketPath: { HubPaths(home: home).socketPath() },
            retryDelays: [.seconds(60)]))
        let log = UpdateLog(client)
        await client.start()
        await eventually("unreachable") { await log.states.contains { if case .unreachable = $0 { true } else { false } } }
        let core = MockHubCore(home: home, scenario: .demo)
        try await core.start()
        defer { Task { await core.stop() } }
        await client.retry()
        await eventually("connected after retry", timeout: .seconds(2)) { await log.isConnected }
        await client.stop()
    }
}
