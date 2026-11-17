import Foundation
import MockHubCore
import Testing
@testable import HubClient

/// A launcher whose "core" is a script that records each start, and that
/// says it starts a core of `version`. The test plays the started core itself
/// with a mock core, once the script has run.
private func standIn(home: URL, version: String?) throws -> (CoreLauncher, URL) {
    let starts = home.appending(path: "starts")
    let script = home.appending(path: "own-hubd")
    try "#!/bin/sh\necho started >> '\(starts.path)'\n".write(to: script, atomically: true, encoding: .utf8)
    try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
    return (CoreLauncher(executable: { script }, environment: [:], version: { version }), starts)
}

private func startCount(_ starts: URL) -> Int {
    ((try? String(contentsOf: starts, encoding: .utf8)) ?? "").split(separator: "\n").count
}

private func replacingClient(home: URL, launcher: CoreLauncher?, replaceWait: Duration = .seconds(5)) -> HubClient {
    HubClient(configuration: .init(
        identity: ClientIdentity(kind: .gui, name: "Pocket Hub Tests", version: "0.0.1"),
        socketPath: { HubPaths(home: home).socketPath() },
        launcher: launcher,
        retryDelays: [.milliseconds(50), .milliseconds(100)],
        helloTimeout: .milliseconds(500),
        launchGrace: .seconds(30),
        replaceWait: replaceWait))
}

/// A mock core of `version`, speaking `protocolVersion`.
private func core(home: URL, protocolVersion: Int = 2, version: String, stops: Bool = true) async throws -> MockHubCore {
    var scenario = MockScenario.demo
    scenario.speaksProtocol = protocolVersion
    scenario.hubVersion = version
    scenario.stopsOnShutdown = stops
    let core = MockHubCore(home: home, scenario: scenario)
    try await core.start()
    return core
}

/// The calls a core received, a hello with the protocol it named.
private func calls(_ core: MockHubCore) async -> [String] {
    await core.received.map { $0.method == "hub.hello" ? "hub.hello \($0.params["protocol"]?.intValue ?? 0)" : $0.method }
}

private func socketExists(_ home: URL) -> Bool {
    FileManager.default.fileExists(atPath: HubPaths(home: home).socketPath())
}

/// CONTROL-API.md §1, "Starting and stopping the core": an updated app takes
/// over from a resident core of an earlier version, never from a newer one,
/// and speaks no method but the two frozen ones in another version's dialect.
@Suite("Client: an earlier core is replaced, a newer one never")
struct ReplacementTests {
    /// Pocket Hub 0.2.1's core (protocol 1) met by this app: the hello in 2 is
    /// refused without `data`, the client says hello in 1, asks it to stop,
    /// waits until it is gone, and starts its own.
    @Test func aCoreOfAnEarlierProtocolIsStoppedAndThisOneStarted() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let earlier = try await core(home: home, protocolVersion: 1, version: "0.2.1")
        let (launcher, starts) = try standIn(home: home, version: "0.3.0")
        let client = replacingClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("its own core started") { startCount(starts) == 1 }
        #expect(await log.states.contains(.replacingCore))
        #expect(await calls(earlier) == ["hub.hello 2", "hub.hello 1", "hub.shutdown"])
        let own = try await core(home: home, version: "0.3.0")
        defer { Task { await own.stop() } }
        await eventually("connected to its own core") { await log.snapshots.last?.hello.hub.version == "0.3.0" }
        #expect(startCount(starts) == 1)
        await client.stop()
    }

    /// The same protocol and a lower version (0.2.0 → 0.2.1 met the same
    /// way, silently): the hello is said, and the same connection asks it to stop.
    @Test func aCoreOfTheSameProtocolAndAnEarlierVersionIsReplaced() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let earlier = try await core(home: home, version: "0.3.0-beta.2")
        let (launcher, starts) = try standIn(home: home, version: "0.3.0")
        let client = replacingClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("its own core started") { startCount(starts) == 1 }
        #expect(await calls(earlier) == ["hub.hello 2", "hub.shutdown"])
        #expect(await log.snapshots.isEmpty, "nothing was read from the earlier core")
        await client.stop()
    }

    @Test func anEqualVersionIsUsedAsItIs() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let same = try await core(home: home, version: "0.3.0+build.7")
        defer { Task { await same.stop() } }
        let (launcher, starts) = try standIn(home: home, version: "0.3.0")
        let client = replacingClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("connected") { await log.isConnected }
        #expect(await !calls(same).contains("hub.shutdown"))
        #expect(await !log.states.contains(.replacingCore))
        #expect(startCount(starts) == 0)
        await client.stop()
    }

    /// A newer core of the same protocol speaks this client's API: it is used,
    /// never replaced, so two installed copies do not end each other's core.
    @Test func aNewerVersionOfTheSameProtocolIsUsedNotReplaced() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let newer = try await core(home: home, version: "0.4.0")
        defer { Task { await newer.stop() } }
        let (launcher, starts) = try standIn(home: home, version: "0.3.0")
        let client = replacingClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("connected") { await log.isConnected }
        #expect(await log.snapshots.last?.hello.hub.version == "0.4.0")
        #expect(await !calls(newer).contains("hub.shutdown"))
        #expect(startCount(starts) == 0)
        await client.stop()
    }

    /// A higher protocol is a newer Pocket Hub: left running, said, and only
    /// ever met with a hello in this client's own protocol.
    @Test func aCoreOfAHigherProtocolIsLeftAlone() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let newer = try await core(home: home, protocolVersion: 3, version: "1.0.0")
        defer { Task { await newer.stop() } }
        let (launcher, starts) = try standIn(home: home, version: "0.3.0")
        let client = replacingClient(home: home, launcher: launcher)
        let log = UpdateLog(client)
        await client.start()
        await eventually("a newer one runs") {
            await log.states.contains { if case .unreachable(.newerVersionRunning, _) = $0 { true } else { false } }
        }
        try await Task.sleep(for: .milliseconds(300))
        let seen = await calls(newer)
        #expect(!seen.isEmpty && seen.allSatisfy { $0 == "hub.hello 2" }, "\(seen)")
        #expect(startCount(starts) == 0)
        #expect(socketExists(home))
        await client.stop()
    }

    /// An earlier core that answers `hub.shutdown` and does not stop: after
    /// `replaceWait` the client says so once and does nothing else (no
    /// signal, no removing the socket, no second request) until `retry()`.
    @Test func anEarlierCoreThatDoesNotStopIsSaidAndLeft() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let stubborn = try await core(home: home, protocolVersion: 1, version: "0.2.1", stops: false)
        defer { Task { await stubborn.stop() } }
        let (launcher, starts) = try standIn(home: home, version: "0.3.0")
        let client = replacingClient(home: home, launcher: launcher, replaceWait: .milliseconds(300))
        let log = UpdateLog(client)
        await client.start()
        await eventually("said it did not stop") {
            await log.states.contains { if case .unreachable(.olderVersionDidNotStop, _) = $0 { true } else { false } }
        }
        try await Task.sleep(for: .milliseconds(500))
        #expect(await calls(stubborn).filter { $0 == "hub.shutdown" }.count == 1)
        #expect(await calls(stubborn).allSatisfy { ["hub.hello 2", "hub.hello 1", "hub.shutdown"].contains($0) })
        #expect(startCount(starts) == 0)
        #expect(socketExists(home), "the socket is never removed by the client")
        if case .unreachable(.olderVersionDidNotStop, _)? = await log.states.last {} else {
            Issue.record("the sentence stays: \(String(describing: await log.states.last))")
        }
        // "Try Again" is the owner's request to ask once more.
        await client.retry()
        await eventually("asked again") { await calls(stubborn).filter { $0 == "hub.shutdown" }.count == 2 }
        await client.stop()
    }

    /// Quit and Stop with no synced connection: whatever core is there stops,
    /// an earlier one and a newer one alike, through the frozen pair.
    @Test(arguments: [(1, "0.2.1"), (3, "1.0.0")])
    func quitAndStopStopsACoreOfAnyVersion(protocolVersion: Int, version: String) async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let other = try await core(home: home, protocolVersion: protocolVersion, version: version)
        let client = replacingClient(home: home, launcher: nil)
        let log = UpdateLog(client)
        await client.start()
        await eventually("not used") { await log.states.contains { if case .unreachable = $0 { true } else { false } } }
        try await client.shutdownHub()
        #expect(!socketExists(home), "it stopped")
        let seen = await calls(other)
        #expect(Array(seen.suffix(3)) == ["hub.hello 2", "hub.hello \(protocolVersion)", "hub.shutdown"], "\(seen)")
        #expect(seen.allSatisfy { $0.hasPrefix("hub.hello") || $0 == "hub.shutdown" })
        await eventually("stopped") { await log.states.last == .stopped }
    }
}

@Suite("SemVer precedence")
struct SemanticVersionTests {
    @Test func ordersAsSemVerSays() throws {
        let ordered = ["0.2.0", "0.2.1", "0.3.0-alpha", "0.3.0-alpha.1", "0.3.0-alpha.beta", "0.3.0-beta.2", "0.3.0-beta.11", "0.3.0-rc.1", "0.3.0", "0.10.0", "1.0.0"]
        let versions = try ordered.map { try #require(SemanticVersion($0)) }
        for (a, b) in zip(versions, versions.dropFirst()) { #expect(a < b, "\(a) < \(b)") }
        #expect(SemanticVersion("0.3.0+build.7") == SemanticVersion("0.3.0"))
        for bad in ["", "1", "1.2", "01.2.3", "1.2.3-", "1.2.3-01", "v1.2.3", "1.2.3.4", "1.2.x"] {
            #expect(SemanticVersion(bad) == nil, "\(bad)")
        }
    }

    /// The version of the core a launcher starts, as `pocket-hubd --version` says it.
    @Test func theCoreSaysItsVersion() throws {
        let folder = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: folder) }
        let core = folder.appending(path: "pocket-hubd")
        try "#!/bin/sh\n[ \"$1\" = --version ] && echo 'pocket-hubd 0.3.0' && exit 0\nexit 9\n".write(to: core, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: core.path)
        #expect(CoreLauncher.reportedVersion(of: core) == "0.3.0")
        #expect(CoreLauncher.reportedVersion(of: folder.appending(path: "nothing")) == nil)
        let liar = folder.appending(path: "other")
        try "#!/bin/sh\necho 'something 1.0'\n".write(to: liar, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: liar.path)
        #expect(CoreLauncher.reportedVersion(of: liar) == nil)
    }
}
