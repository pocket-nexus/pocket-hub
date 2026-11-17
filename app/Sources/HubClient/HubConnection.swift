import Foundation

/// Anything that can make a control API call: one connection, or the
/// reconnecting `HubClient` in front of it. The typed methods of
/// `HubMethods.swift` are written once against this.
public protocol HubCalling: Sendable {
    /// Calls `method` and returns its `result`. `progress` receives the
    /// `params` of each `progress` notification for this call, in order.
    ///
    /// Cancelling the calling task sends `cancel` for the call; the call then
    /// ends as Hub ends it, normally with `HubError.operation(kind: .cancelled)`.
    func call(_ method: String, params: JSONValue?, progress: (@Sendable (JSONValue) -> Void)?) async throws -> JSONValue
}

extension HubCalling {
    public func call(_ method: String, params: JSONValue? = nil) async throws -> JSONValue {
        try await call(method, params: params, progress: nil)
    }
}

/// An event with its position on the wire. `sequence` counts every line read
/// on the connection, so an event can be ordered against a call's answer.
public struct SequencedEvent: Sendable {
    public var sequence: UInt64
    public var event: HubEvent
}

/// One connection to `pocket-hubd`: line framing, request/response
/// correlation, progress routing, cancellation and the event stream.
///
/// It does not say hello or reconnect; `HubClient` does both.
public actor HubConnection: HubCalling {
    /// A call's answer as it came: its `result`, or its `error` object.
    private enum Answer {
        case result(JSONValue)
        case error(JSONValue)
    }

    private struct Pending {
        var continuation: CheckedContinuation<(Answer, UInt64), any Error>
        var progress: (@Sendable (JSONValue) -> Void)?
    }

    /// Events in wire order. Finishes when the connection closes.
    public nonisolated let events: AsyncStream<SequencedEvent>
    private let eventContinuation: AsyncStream<SequencedEvent>.Continuation
    private let socket: UnixSocketStream
    private var framer = LineFramer()
    private var nextId = 1
    private var pending: [Int: Pending] = [:]
    private var sequence: UInt64 = 0
    private var closeWaiters: [CheckedContinuation<Void, Never>] = []
    public private(set) var isClosed = false
    /// Why the connection ended, when it ended for a reason other than the peer closing.
    public private(set) var closeReason: String?

    public init(socket: UnixSocketStream) {
        self.socket = socket
        (events, eventContinuation) = AsyncStream.makeStream(of: SequencedEvent.self, bufferingPolicy: .unbounded)
        Task { await self.readLoop() }
    }

    /// Connects to the socket at `path`.
    public static func open(path: String) throws -> HubConnection {
        HubConnection(socket: try UnixSocketStream.connect(path: path))
    }

    public func call(_ method: String, params: JSONValue?, progress: (@Sendable (JSONValue) -> Void)?) async throws -> JSONValue {
        try await callSequenced(method, params: params, progress: progress).result
    }

    /// Like `call`, also returning the wire position of the answer.
    public func callSequenced(_ method: String, params: JSONValue? = nil,
                              progress: (@Sendable (JSONValue) -> Void)? = nil) async throws -> (result: JSONValue, sequence: UInt64)
    {
        let (answer, sequence) = try await exchange(method, params: params, progress: progress)
        switch answer {
        case .result(let result): return (result, sequence)
        case .error(let error): throw HubError(rpcError: error)
        }
    }

    /// Says hello in `protocol` and returns what the core answered, its refusal
    /// for the protocol included: `-32602` with `data.protocol`, the core's, or
    /// without `data` from a core of Pocket Hub 0.2.1 and earlier, which speaks
    /// protocol 1 (CONTROL-API.md §1, Hello). `hub.hello` is one of the two
    /// methods every protocol keeps, so this is how a client meets a core of
    /// any version. Any other error is thrown.
    public func introduce(protocol version: Int, client: ClientIdentity) async throws -> HelloAnswer {
        let params: JSONValue = ["protocol": .number(Double(version)), "client": try JSONValue(encoding: client)]
        switch try await exchange("hub.hello", params: params, progress: nil).0 {
        case .result(let result):
            return .accepted(try result.decoded())
        case .error(let error) where error["code"]?.intValue == -32602:
            return .refused(coreProtocol: error["data"]?["protocol"]?.intValue ?? 1,
                            message: error["message"]?.stringValue ?? "")
        case .error(let error):
            throw HubError(rpcError: error)
        }
    }

    private func exchange(_ method: String, params: JSONValue?,
                          progress: (@Sendable (JSONValue) -> Void)?) async throws -> (Answer, UInt64)
    {
        if isClosed { throw HubError.disconnected }
        if Task.isCancelled { throw HubError.operation(.init(kind: .cancelled, message: "Cancelled.")) }
        let id = nextId
        nextId += 1
        var request: [String: JSONValue] = ["jsonrpc": "2.0", "id": .number(Double(id)), "method": .string(method)]
        if let params { request["params"] = params }
        let line = try JSONValue.object(request).encodedLine() + Data([0x0A])
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<(Answer, UInt64), any Error>) in
                if isClosed {
                    continuation.resume(throwing: HubError.disconnected)
                    return
                }
                pending[id] = Pending(continuation: continuation, progress: progress)
                socket.send(line)
            }
        } onCancel: {
            Task { await self.sendCancel(for: id) }
        }
    }

    /// Sends a notification (no answer).
    public func notify(_ method: String, params: JSONValue?) throws {
        guard !isClosed else { return }
        var message: [String: JSONValue] = ["jsonrpc": "2.0", "method": .string(method)]
        if let params { message["params"] = params }
        socket.send(try JSONValue.object(message).encodedLine() + Data([0x0A]))
    }

    /// Closes the connection. Open calls fail with `.disconnected`; Hub
    /// cancels them and releases what this client held (§1).
    public func close() {
        socket.close()
    }

    /// Returns once the connection has closed.
    public func waitUntilClosed() async {
        if isClosed { return }
        await withCheckedContinuation { closeWaiters.append($0) }
    }

    private func sendCancel(for id: Int) {
        guard pending[id] != nil else { return }
        try? notify("cancel", params: ["id": .number(Double(id))])
    }

    private func readLoop() async {
        for await chunk in socket.chunks {
            let lines: [Data]
            do {
                lines = try framer.append(chunk)
            } catch {
                closeReason = "framing: \(error)"
                socket.close()
                break
            }
            for line in lines { handle(line: line) }
        }
        finish()
    }

    private func handle(line: Data) {
        sequence += 1
        guard let message = try? JSONDecoder.hub.decode(JSONValue.self, from: line), case .object = message else {
            // A line that is not a JSON object cannot be attributed to a call;
            // it is dropped rather than closing a connection that otherwise works.
            return
        }
        if let method = message["method"]?.stringValue {
            let params = message["params"] ?? .null
            switch method {
            case "progress":
                if let id = params["id"]?.intValue, let entry = pending[id] {
                    entry.progress?(params)
                }
            case "event":
                if let event = try? HubEvent(params: params) {
                    eventContinuation.yield(SequencedEvent(sequence: sequence, event: event))
                }
            default:
                break // Hub sends no other notification to a client; ignore what a newer one might.
            }
            return
        }
        guard let id = message["id"]?.intValue, let entry = pending.removeValue(forKey: id) else { return }
        if let error = message["error"] {
            entry.continuation.resume(returning: (.error(error), sequence))
        } else {
            entry.continuation.resume(returning: (.result(message["result"] ?? .null), sequence))
        }
    }

    private func finish() {
        isClosed = true
        for entry in pending.values { entry.continuation.resume(throwing: HubError.disconnected) }
        pending.removeAll()
        eventContinuation.finish()
        for waiter in closeWaiters { waiter.resume() }
        closeWaiters.removeAll()
    }
}
