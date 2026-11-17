import Foundation
import Testing
@testable import HubClient

/// The wire behaviour of one connection, against a raw server that writes
/// exactly the bytes each test wants.
@Suite("Connection")
struct ConnectionTests {
    @Test func responsesAreCorrelatedById() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let connection = try HubConnection.open(path: server.path)
        async let peerTask = server.nextPeer()
        async let first = connection.call("devices.get", params: ["device": "a"])
        async let second = connection.call("devices.get", params: ["device": "b"])
        let peer = await peerTask
        let requestA = try await peer.next()
        let requestB = try await peer.next()
        #expect(requestA["jsonrpc"] == "2.0")
        // Answer in the opposite order, both in one write.
        let byDevice = Dictionary(uniqueKeysWithValues: [requestA, requestB].map { ($0["params"]!["device"]!.stringValue!, $0["id"]!) })
        let idA = try String(decoding: byDevice["a"]!.encodedLine(), as: UTF8.self)
        let idB = try String(decoding: byDevice["b"]!.encodedLine(), as: UTF8.self)
        peer.write("{\"jsonrpc\":\"2.0\",\"id\":\(idB),\"result\":{\"who\":\"b\"}}\n{\"jsonrpc\":\"2.0\",\"id\":\(idA),\"result\":{\"who\":\"a\"}}\n")
        #expect(try await first == ["who": "a"])
        #expect(try await second == ["who": "b"])
        await connection.close()
    }

    @Test func requestsFollowTheContract() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let connection = try HubConnection.open(path: server.path)
        let call = Task { try await connection.hello(client: ClientIdentity(kind: .gui, name: "Pocket Hub", version: "0.1.0")) }
        let peer = await server.nextPeer()
        let request = try await peer.next()
        #expect(request["method"] == "hub.hello")
        #expect(request["params"] == ["protocol": 2, "client": ["kind": "gui", "name": "Pocket Hub", "version": "0.1.0"]])
        #expect(request["id"]?.intValue != nil)
        peer.write(#"{"jsonrpc":"2.0","id":\#(request["id"]!.intValue!),"result":{"protocol":2,"hub":{"id":"fedcba9876543210","name":"Studio Mac","version":"0.1.0"}}}"# + "\n")
        let hello = try await call.value
        #expect(hello.hub.id == "fedcba9876543210")
        await connection.close()
    }

    @Test func splitAndCoalescedServerWrites() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let connection = try HubConnection.open(path: server.path)
        let call = Task { try await connection.call("hub.status") }
        let peer = await server.nextPeer()
        let id = try await peer.next()["id"]!.intValue!
        let events = connection.events
        // An event, a progress for an unknown id, and the answer whose name holds
        // multi-byte characters, written a few bytes at a time.
        let text = "{\"jsonrpc\":\"2.0\",\"method\":\"event\",\"params\":{\"topic\":\"device.removed\",\"id\":\"x\"}}\n"
            + "{\"jsonrpc\":\"2.0\",\"method\":\"progress\",\"params\":{\"id\":999}}\n"
            + "{\"jsonrpc\":\"2.0\",\"id\":\(id),\"result\":{\"name\":\"书房的 Mac 🖥\"}}\n"
        let bytes = Data(text.utf8)
        var offset = 0
        while offset < bytes.count {
            let size = min(3, bytes.count - offset)
            peer.write(bytes.subdata(in: offset..<offset + size))
            offset += size
        }
        let result = try await call.value
        #expect(result["name"] == "书房的 Mac 🖥")
        var iterator = events.makeAsyncIterator()
        let first = await iterator.next()
        #expect(first?.event == .deviceRemoved(id: "x"))
        await connection.close()
    }

    @Test func operationErrorsAreTyped() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let connection = try HubConnection.open(path: server.path)
        let call = Task { try await connection.launch(device: "a", app: "weather") }
        let peer = await server.nextPeer()
        let id = try await peer.next()["id"]!.intValue!
        peer.write(#"{"jsonrpc":"2.0","id":\#(id),"error":{"code":1,"message":"The device is busy.","data":{"kind":"busy","holder":{"kind":"cli","name":"pocket-hub","method":"devices.push"}}}}"# + "\n")
        do {
            _ = try await call.value
            Issue.record("expected an error")
        } catch let error as HubError {
            guard case .operation(let operation) = error else { Issue.record("\(error)"); return }
            #expect(operation.kind == .busy)
            #expect(operation.holder == MutexHolder(kind: "cli", name: "pocket-hub", method: "devices.push"))
            #expect(operation.message == "The device is busy.")
        }
        await connection.close()
    }

    @Test func progressIsDeliveredInOrderBeforeTheAnswer() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let connection = try HubConnection.open(path: server.path)
        let seen = Locked<[Double]>([])
        let call = Task {
            try await connection.push(device: "a", path: "/tmp/x.pocket") { progress in
                seen.mutate { $0.append(progress.fraction ?? -1) }
            }
        }
        let peer = await server.nextPeer()
        let id = try await peer.next()["id"]!.intValue!
        var text = ""
        for done in [25, 50, 100] {
            text += #"{"jsonrpc":"2.0","method":"progress","params":{"id":\#(id),"phase":"upload","done":\#(done),"total":100}}"# + "\n"
        }
        text += #"{"jsonrpc":"2.0","id":\#(id),"result":{"app":{"id":"i","output":"o","title":"T"},"hash":"h","generation":1}}"# + "\n"
        peer.write(text)
        let result = try await call.value
        #expect(result.app.output == "o")
        #expect(seen.value == [0.25, 0.5, 1.0])
        await connection.close()
    }

    @Test func cancellingSendsCancelAndEndsWithCancelled() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let connection = try HubConnection.open(path: server.path)
        let call = Task { try await connection.install(host: "192.168.1.57") }
        let peer = await server.nextPeer()
        let request = try await peer.next()
        #expect(request["method"] == "setup.install")
        #expect(request["params"] == ["kind": "3ds", "host": "192.168.1.57", "program": ["release": "latest"]])
        let id = request["id"]!.intValue!
        call.cancel()
        let cancel = try await peer.next()
        #expect(cancel["method"] == "cancel")
        #expect(cancel["id"] == nil, "cancel is a notification")
        #expect(cancel["params"] == ["id": .number(Double(id))])
        peer.write(#"{"jsonrpc":"2.0","id":\#(id),"error":{"code":1,"message":"Cancelled.","data":{"kind":"cancelled"}}}"# + "\n")
        do {
            _ = try await call.value
            Issue.record("expected cancelled")
        } catch let error as HubError {
            #expect(error.isCancelled)
        }
        await connection.close()
    }

    @Test func disconnectFailsOpenCallsAndEndsEvents() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let server = try RawServer(home: home)
        defer { server.stop() }
        let connection = try HubConnection.open(path: server.path)
        let call = Task { try await connection.call("devices.list") }
        let peer = await server.nextPeer()
        _ = try await peer.next()
        peer.close()
        await #expect(throws: HubError.disconnected) { try await call.value }
        for await _ in connection.events {}
        #expect(await connection.isClosed)
        await #expect(throws: HubError.disconnected) { try await connection.call("devices.list") }
    }

    @Test func nobodyListening() throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        #expect(throws: UnixSocketStream.SocketError.self) {
            _ = try HubConnection.open(path: home.appending(path: "hubd.sock").path)
        }
    }

    @Test func socketPathRules() throws {
        let short = HubPaths(home: URL(fileURLWithPath: "/tmp/hub", isDirectory: true))
        #expect(short.socketPath() == "/tmp/hub/hubd.sock")
        #expect(short.socketPathIsUsable)
        #expect(short.coreLog.path == "/tmp/hub/logs/hubd.log")
        // No second location: a long home keeps its own socket path, which the core refuses.
        let long = HubPaths(home: URL(fileURLWithPath: "/tmp/" + String(repeating: "x", count: 100), isDirectory: true))
        #expect(long.socketPath().hasSuffix("/hubd.sock"))
        #expect(!long.socketPathIsUsable)
        let env = HubPaths.current(environment: ["POCKET_HUB_HOME": "/tmp/custom-home"])
        #expect(env.home.path == "/tmp/custom-home")
        let standard = HubPaths.current(environment: [:])
        #expect(standard.home.path.hasSuffix("Library/Application Support/nexus.pocket.hub"))
    }
}

/// A value shared with a `@Sendable` callback in tests.
final class Locked<Value: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Value
    init(_ value: Value) { stored = value }
    var value: Value { lock.withLock { stored } }
    func mutate(_ change: (inout Value) -> Void) { lock.withLock { change(&stored) } }
}
