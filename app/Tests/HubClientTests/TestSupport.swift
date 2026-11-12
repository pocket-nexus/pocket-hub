import Foundation
import HubClient
import Testing

/// A fresh, short home folder (socket paths must stay under 104 bytes).
func temporaryHome() throws -> URL {
    let url = URL(fileURLWithPath: "/tmp/phc-\(UUID().uuidString.prefix(8))", isDirectory: true)
    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

/// Waits until `condition` holds, polling; fails the test after `timeout`.
func eventually(_ what: String = "condition", timeout: Duration = .seconds(5),
                sourceLocation: SourceLocation = #_sourceLocation,
                _ condition: @Sendable () async -> Bool) async
{
    let deadline = ContinuousClock.now + timeout
    while ContinuousClock.now < deadline {
        if await condition() { return }
        try? await Task.sleep(for: .milliseconds(20))
    }
    Issue.record("timed out waiting for \(what)", sourceLocation: sourceLocation)
}

/// A raw server for wire-level tests: hands each accepted connection to the
/// test, which reads lines and writes bytes exactly as it wants.
final class RawServer: @unchecked Sendable {
    let path: String
    private let listener: UnixSocketListener
    private let lock = NSLock()
    private var waiting: [CheckedContinuation<RawPeer, Never>] = []
    private var ready: [RawPeer] = []

    init(home: URL) throws {
        path = home.appending(path: "hubd.sock").path
        listener = try UnixSocketListener(path: path)
        listener.run { [weak self] socket in self?.arrived(RawPeer(socket: socket)) }
    }

    private func arrived(_ peer: RawPeer) {
        lock.lock()
        if waiting.isEmpty {
            ready.append(peer)
            lock.unlock()
        } else {
            let continuation = waiting.removeFirst()
            lock.unlock()
            continuation.resume(returning: peer)
        }
    }

    func nextPeer() async -> RawPeer {
        await withCheckedContinuation { continuation in
            lock.lock()
            if ready.isEmpty {
                waiting.append(continuation)
                lock.unlock()
            } else {
                let peer = ready.removeFirst()
                lock.unlock()
                continuation.resume(returning: peer)
            }
        }
    }

    func stop() { listener.stop() }
}

/// One accepted connection seen from the server side.
actor RawPeer {
    let socket: UnixSocketStream
    private var framer = LineFramer()
    private var lines: [JSONValue] = []
    private var iterator: AsyncStream<Data>.Iterator

    init(socket: UnixSocketStream) {
        self.socket = socket
        iterator = socket.chunks.makeAsyncIterator()
    }

    /// The next JSON object the client wrote.
    func next() async throws -> JSONValue {
        while lines.isEmpty {
            // AsyncStream's iterator shares its state between copies, so a local copy is safe.
            var local = iterator
            let chunk = await local.next(isolation: self)
            iterator = local
            guard let chunk else { throw HubError.disconnected }
            for line in try framer.append(chunk) {
                lines.append(try JSONDecoder.hub.decode(JSONValue.self, from: line))
            }
        }
        return lines.removeFirst()
    }

    nonisolated func write(_ text: String) {
        socket.send(Data(text.utf8))
    }

    nonisolated func write(_ bytes: Data) {
        socket.send(bytes)
    }

    nonisolated func close() { socket.close() }
}
