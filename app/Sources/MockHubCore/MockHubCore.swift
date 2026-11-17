import Foundation
import HubClient

/// An implementation of the control API (docs/CONTROL-API.md) on a Unix
/// socket, with scriptable devices, services, progress sequences and failures.
///
/// It exists so the client and the app can be tested and developed before the
/// real `pocket-hubd` runs, and so failures that a real device rarely
/// produces can be produced on demand. It keeps the contract's rules that a
/// client can observe: hello first, progress before the answer, `cancel`,
/// events only after `hub.subscribe`, whole Devices in `device.changed`, the
/// device mutex with `busy` and its holder.
public actor MockHubCore {
    /// A call or notification the mock received, for assertions.
    public struct Received: Sendable, Equatable {
        public var session: Int
        public var method: String
        public var params: JSONValue
    }

    /// A failure to answer the next call of a method with.
    public struct Failure: Sendable {
        public var kind: String
        public var message: String
        public var data: [String: JSONValue]

        public init(kind: String, message: String, data: [String: JSONValue] = [:]) {
            self.kind = kind
            self.message = message
            self.data = data
        }
    }

    private struct Session {
        var socket: UnixSocketStream
        var framer = LineFramer()
        var helloed = false
        var client: JSONValue = .null
        /// nil: not subscribed. Empty: every topic.
        var topics: Set<String>?
        var tasks: [String: Task<Void, Never>] = [:]
    }

    public nonisolated let socketPath: String
    public nonisolated let home: URL
    public private(set) var scenario: MockScenario
    public private(set) var received: [Received] = []
    /// Called after `hub.shutdown` has been answered and every session closed.
    private var onShutdown: (@Sendable () -> Void)?
    private var listener: UnixSocketListener?
    private var sessions: [Int: Session] = [:]
    private var nextSession = 1
    private var failures: [String: [Failure]] = [:]
    private var logTasks: [String: Task<Void, Never>] = [:]
    /// Devices whose logs some client turned on; lines flow while they are linked.
    private var logsOn: Set<String> = []
    private var discoverCalls = 0
    private var storeFetchedAt = MockClock.now()
    /// Services removed with `purge: true`, for assertions.
    public private(set) var purged: Set<String> = []
    private var frame = 4096

    public init(home: URL, scenario: MockScenario, socketPath: String? = nil) {
        self.home = home
        self.scenario = scenario
        self.socketPath = socketPath ?? HubPaths(home: home).socketPath()
    }

    // MARK: - Lifecycle

    public func setOnShutdown(_ handler: @escaping @Sendable () -> Void) {
        onShutdown = handler
    }

    /// Starts listening. Creates the home folder (and the socket's folder).
    public func start() throws {
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: URL(fileURLWithPath: socketPath).deletingLastPathComponent(),
                                                withIntermediateDirectories: true)
        writeStoreIcons()
        let listener = try UnixSocketListener(path: socketPath)
        self.listener = listener
        listener.run { [weak self] socket in
            guard let self else { socket.close(); return }
            Task { await self.accept(socket) }
        }
    }

    /// Stops listening and drops every connection.
    public func stop() {
        listener?.stop()
        listener = nil
        dropConnections()
        for task in logTasks.values { task.cancel() }
        logTasks.removeAll()
    }

    /// Closes every client connection, as a crashing core would.
    public func dropConnections() {
        for (id, session) in sessions {
            for task in session.tasks.values { task.cancel() }
            session.socket.close()
            sessions[id] = nil
        }
    }

    /// Stops accepting connections (the core is "down") without dropping existing ones.
    public func stopListening() {
        listener?.stop()
        listener = nil
    }

    public var connectionCount: Int { sessions.count }

    // MARK: - Scripting

    public func failNext(_ method: String, with failure: Failure) {
        failures[method, default: []].append(failure)
    }

    public func setSetupBehavior(_ behavior: MockSetupBehavior) {
        scenario.setup = behavior
    }

    public func setCandidates(_ candidates: [SetupCandidate]) {
        scenario.candidates = candidates
    }

    public func setStoreReachable(_ reachable: Bool) {
        scenario.storeReachable = reachable
    }

    public func setStoreRejected(_ ids: Set<String>) {
        scenario.storeRejected = ids
    }

    public func setStoreLinkLoss(_ ids: Set<String>) {
        scenario.storeLinkLoss = ids
    }

    /// Replaces the catalog (a new version published, say) and announces it.
    public func setStore(_ apps: [MockStoreApp]) {
        scenario.store = apps
        storeFetchedAt = MockClock.now()
        emit("store.changed", ["fetchedAt": .number(Double(storeFetchedAt))])
    }

    public func setProgramBehavior(_ behavior: MockProgramBehavior) {
        scenario.program = behavior
    }

    /// A new release of the devices' program (nil: the source stops answering).
    /// Hub learns it at a device's next link, as the real one does.
    public func setProgramRelease(version: String?, bytes: Int = 310_000) {
        scenario.programRelease = version.map { ($0, bytes) }
    }

    public func setResponseDelay(_ method: String, _ delay: Duration?) {
        scenario.responseDelays[method] = delay
    }

    public func device(_ id: String) -> MockDevice? {
        scenario.devices.first { $0.id == id }
    }

    // MARK: Pairing by a code

    public func setPairingBehavior(_ behavior: MockPairingBehavior) {
        scenario.pairing = behavior
    }

    /// A device asks to be paired: one request a device id, a later one
    /// replacing the one before, announced with `pairing.changed`.
    public func askToPair(_ request: MockPairRequest) {
        scenario.pairRequests.removeAll { $0.device == request.device }
        scenario.pairRequests.append(request)
        emitPairRequests()
    }

    /// The device left, or its request ran out.
    public func withdrawPairRequest(_ device: String) {
        guard scenario.pairRequests.contains(where: { $0.device == device }) else { return }
        scenario.pairRequests.removeAll { $0.device == device }
        emitPairRequests()
    }

    private func emitPairRequests() {
        emit("pairing.changed", ["requests": .array(scenario.pairRequests.map(\.json))])
    }

    /// A request by its device id, or `"*"` for the only one that waits.
    private func resolvePairRequest(_ params: JSONValue) throws -> MockPairRequest {
        guard let id = params["device"]?.stringValue else { throw MockError.invalidParams("device") }
        if id == "*" {
            switch scenario.pairRequests.count {
            case 0: throw MockError.operation("not_found", "No device asks to be paired.", [:])
            case 1: return scenario.pairRequests[0]
            default: throw MockError.operation("invalid", "More than one device asks to be paired; name one.", [:])
            }
        }
        guard let request = scenario.pairRequests.first(where: { $0.device == id }) else {
            throw MockError.operation("not_found", "No device with this id asks to be paired.", [:])
        }
        return request
    }

    /// `pairing.answer`: while the code is on its way the request is not in
    /// the list. The device's code pairs it (a record, then its link); any
    /// other costs one of its tries, stores nothing, and the device asks
    /// again while it has tries left.
    private func answerPairRequest(_ params: JSONValue) async throws -> JSONValue {
        var request = try resolvePairRequest(params)
        guard let code = params["code"]?.stringValue, code.utf8.count == 6, code.utf8.allSatisfy({ (48...57).contains($0) }) else {
            throw MockError.operation("invalid", "A pairing code is the 6 digits the device shows.", [:])
        }
        let behavior = scenario.pairing
        scenario.pairRequests.removeAll { $0.device == request.device }
        emitPairRequests()
        try? await Task.sleep(for: behavior.answerDelay)
        guard code == request.code else {
            request.tries -= 1
            if request.tries > 0 {
                let again = request
                Task {
                    try? await Task.sleep(for: behavior.askAgainAfter)
                    var asking = again
                    asking.since = MockClock.now()
                    self.askToPair(asking)
                }
            }
            throw MockError.operation("pairing", "The code is not the one the device shows. Nothing was stored.", ["reason": "wrong_code"])
        }
        var record = device(request.device) ?? MockDevice(id: request.device)
        record.kind = request.kind
        record.name = request.name
        record.model = request.model
        record.target = request.target
        record.lastAddress = request.address
        record.setupUnconfirmed = false
        upsert(record)
        if let delay = behavior.linkAfter {
            let (id, address) = (request.device, request.address)
            Task {
                try? await Task.sleep(for: delay)
                self.modify(id) { $0.linkUp(address: address) }
            }
        }
        return record.json
    }

    /// Adds or replaces a device and announces it.
    public func upsert(_ device: MockDevice) {
        if let index = scenario.devices.firstIndex(where: { $0.id == device.id }) {
            scenario.devices[index] = device
        } else {
            scenario.devices.append(device)
        }
        emit("device.changed", ["device": device.json])
    }

    /// Replaces a device without announcing it: the state a later
    /// `devices.list` reports but no event carried.
    public func replaceSilently(_ device: MockDevice) {
        if let index = scenario.devices.firstIndex(where: { $0.id == device.id }) {
            scenario.devices[index] = device
        } else {
            scenario.devices.append(device)
        }
    }

    /// Changes a device in place and announces it.
    public func modify(_ id: String, _ change: @Sendable (inout MockDevice) -> Void) {
        guard let index = scenario.devices.firstIndex(where: { $0.id == id }) else { return }
        change(&scenario.devices[index])
        emit("device.changed", ["device": scenario.devices[index].json])
    }

    public func link(_ id: String, carrier: String = "tcp") {
        // Hub looks the newest release up when a device links; a failed look
        // (no release source) leaves `latest` as it was. A link ends a launch
        // hold (link specification §8.3).
        let latest = scenario.programRelease?.version
        modify(id) {
            $0.linkUp(carrier: carrier)
            if $0.busy["launching"]?.boolValue == true { $0.busy = .null }
            if let latest { $0.latestProgram = latest }
        }
        // Hub turns a device's logs on again after it links (§3 devices.logs).
        if logsOn.contains(id) { startLogLines(id) }
    }

    /// Ends a device's link: `device.unlinked` with the close code, then `device.changed`.
    public func unlink(_ id: String, code: String = "going_away", reason: String = "the device went to sleep") {
        guard device(id) != nil else { return }
        logTasks.removeValue(forKey: id)?.cancel()
        emit("device.unlinked", ["id": .string(id), "code": .string(code), "reason": .string(reason)])
        modify(id) { $0.linkDown() }
    }

    public func remove(_ id: String) {
        scenario.devices.removeAll { $0.id == id }
        emit("device.removed", ["id": .string(id)])
    }

    public func emitDeviceLog(_ id: String, level: String = "info", source: String = "guest", text: String) {
        frame += 1
        emit("device.log", ["id": .string(id), "level": .string(level), "source": .string(source),
                            "text": .string(text), "frame": .number(Double(frame))])
    }

    public func emitServiceLog(_ id: String, stream: String = "stdout", text: String) {
        let line: JSONValue = ["at": .number(Double(MockClock.now())), "stream": .string(stream), "text": .string(text)]
        if let index = scenario.services.firstIndex(where: { $0.id == id }) { scenario.services[index].log.append(line) }
        emit("service.log", ["id": .string(id), "at": line["at"]!, "stream": .string(stream), "text": .string(text)])
    }

    /// Sends an event with arbitrary members to every subscribed session.
    public func emit(_ topic: String, _ members: [String: JSONValue]) {
        var params = members
        params["topic"] = .string(topic)
        let message: JSONValue = ["jsonrpc": "2.0", "method": "event", "params": .object(params)]
        for session in sessions.values {
            guard let topics = session.topics, topics.isEmpty || topics.contains(topic) else { continue }
            send(message, on: session.socket)
        }
    }

    // MARK: - Connections

    private func accept(_ socket: UnixSocketStream) {
        let id = nextSession
        nextSession += 1
        sessions[id] = Session(socket: socket)
        Task { await self.read(session: id, socket: socket) }
    }

    private func read(session id: Int, socket: UnixSocketStream) async {
        for await chunk in socket.chunks {
            guard var session = sessions[id] else { break }
            let lines: [Data]
            do {
                lines = try session.framer.append(chunk)
            } catch {
                socket.close()
                break
            }
            sessions[id]?.framer = session.framer
            for line in lines { handle(line, session: id) }
        }
        // A client that disconnects cancels its open calls and releases what it held (§1).
        if let session = sessions.removeValue(forKey: id) {
            for task in session.tasks.values { task.cancel() }
        }
    }

    private func handle(_ line: Data, session id: Int) {
        guard let session = sessions[id] else { return }
        guard let message = try? JSONDecoder.hub.decode(JSONValue.self, from: line), case .object = message else {
            send(["jsonrpc": "2.0", "id": .null, "error": ["code": -32700, "message": "Parse error."]], on: session.socket)
            return
        }
        guard let method = message["method"]?.stringValue else { return }
        let params = message["params"] ?? .null
        received.append(Received(session: id, method: method, params: params))
        guard let requestId = message["id"] else {
            if method == "cancel", let target = params["id"] {
                sessions[id]?.tasks[key(target)]?.cancel()
            }
            return
        }
        if !session.helloed && method != "hub.hello" {
            reply(id, requestId, error: ["code": -32600, "message": "The first request must be hub.hello."])
            return
        }
        if method == "hub.hello" {
            // A hello refused for its protocol leaves the session without one,
            // as the real core does.
            if params["protocol"]?.intValue == scenario.speaksProtocol {
                sessions[id]?.helloed = true
                sessions[id]?.client = params["client"] ?? .null
            }
        }
        let task = Task {
            if let delay = self.scenario.responseDelays[method] { try? await Task.sleep(for: delay) }
            await self.dispatch(method, params: params, requestId: requestId, session: id)
        }
        sessions[id]?.tasks[key(requestId)] = task
    }

    private func dispatch(_ method: String, params: JSONValue, requestId: JSONValue, session id: Int) async {
        let progress: @Sendable (JSONValue) async -> Void = { [weak self] members in
            await self?.progress(id, requestId, members)
        }
        do {
            if var queued = failures[method], !queued.isEmpty {
                let failure = queued.removeFirst()
                failures[method] = queued
                throw MockError.operation(failure.kind, failure.message, failure.data)
            }
            let result = try await perform(method, params: params, session: id, progress: progress)
            reply(id, requestId, result: result)
        } catch let error as MockError {
            reply(id, requestId, error: error.json)
        } catch is CancellationError {
            reply(id, requestId, error: MockError.operation("cancelled", "The operation was cancelled.", [:]).json)
        } catch {
            reply(id, requestId, error: MockError.operation("internal", "\(error)", [:]).json)
        }
        sessions[id]?.tasks[key(requestId)] = nil
    }

    private func progress(_ session: Int, _ requestId: JSONValue, _ members: JSONValue) {
        guard let socket = sessions[session]?.socket, case .object(var params) = members else { return }
        params["id"] = requestId
        send(["jsonrpc": "2.0", "method": "progress", "params": .object(params)], on: socket)
    }

    private func reply(_ session: Int, _ requestId: JSONValue, result: JSONValue) {
        guard let socket = sessions[session]?.socket else { return }
        send(["jsonrpc": "2.0", "id": requestId, "result": result], on: socket)
    }

    private func reply(_ session: Int, _ requestId: JSONValue, error: JSONValue) {
        guard let socket = sessions[session]?.socket else { return }
        send(["jsonrpc": "2.0", "id": requestId, "error": error], on: socket)
    }

    private func send(_ message: JSONValue, on socket: UnixSocketStream) {
        guard let line = try? message.encodedLine() else { return }
        socket.send(line + Data([0x0A]))
    }

    private func key(_ id: JSONValue) -> String {
        (try? String(decoding: id.encodedLine(), as: UTF8.self)) ?? "?"
    }

    // MARK: - Methods

    typealias Progress = @Sendable (JSONValue) async -> Void

    private func perform(_ method: String, params: JSONValue, session id: Int, progress: Progress) async throws -> JSONValue {
        switch method {
        case "hub.hello":
            let asked = params["protocol"]?.intValue ?? 0
            guard asked == scenario.speaksProtocol else {
                throw MockError.otherProtocol(core: scenario.speaksProtocol, client: asked)
            }
            return ["protocol": .number(Double(scenario.speaksProtocol)), "hub": hubIdentity]
        case "hub.status":
            return [
                "id": .string(scenario.hubId), "name": .string(scenario.hubName), "version": .string(scenario.hubVersion),
                "home": .string(home.path), "startedAt": .number(Double(MockClock.now() - 3_600_000)),
                "carriers": [["kind": "tcp", "port": 8628]],
                "bun": ["version": scenario.bunInstalled ? "1.3.2" : .null, "installed": .bool(scenario.bunInstalled)],
            ]
        case "hub.setName":
            guard let name = params["name"]?.stringValue?.trimmingCharacters(in: .whitespaces), !name.isEmpty else {
                throw MockError.operation("invalid", "A name cannot be empty.", [:])
            }
            scenario.hubName = name
            emit("hub.changed", ["hub": hubIdentity])
            return ["name": .string(name)]
        case "hub.subscribe":
            if case .array(let topics)? = params["topics"] {
                sessions[id]?.topics = Set(topics.compactMap(\.stringValue))
            } else {
                sessions[id]?.topics = []
            }
            return [:]
        case "hub.shutdown":
            guard scenario.stopsOnShutdown else { return [:] }
            Task {
                try? await Task.sleep(for: .milliseconds(50))
                self.shutdownNow()
            }
            return [:]

        case "devices.list":
            return ["devices": .array(scenario.devices.map(\.json))]
        case "devices.get":
            return try resolve(params).json
        case "devices.forget":
            let device = try resolve(params)
            if device.isLinked {
                emit("device.unlinked", ["id": .string(device.id), "code": "forgotten", "reason": "forgotten by the owner"])
            }
            remove(device.id)
            return [:]
        case "devices.status":
            let device = try linked(params)
            return device.status
        case "devices.push":
            try supporting("push", params)
            let device = try linked(params)
            guard let path = params["path"]?.stringValue else { throw MockError.invalidParams("path") }
            guard path.hasSuffix(".pocket") else {
                throw MockError.operation("invalid", "This file is not a Pocket applet.", [:])
            }
            guard FileManager.default.fileExists(atPath: path) else {
                throw MockError.operation("not_found", "The file does not exist.", [:])
            }
            let hold = scenario.mutexHold
            return try await holdingMutex(device.id, session: id, method: method) {
                let total = 200_000
                for step in 0...4 {
                    try await Task.sleep(for: hold / 4)
                    let phase: JSONValue = step == 0 ? "sending" : step < 4 ? "receiving" : "staged"
                    await progress(["phase": phase, "done": .number(Double(total * step / 4)), "total": .number(Double(total))])
                }
                let output = URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent
                let appId = "nexus.pocket.\(output)"
                await self.installApp(device.id, id: appId, output: output)
                return ["app": ["id": .string(appId), "output": .string(output), "title": .string(output.capitalized)],
                        "hash": "abcdef0123456789", "generation": 4]
            }
        case "devices.launch":
            let device = try linked(params)
            guard let app = params["app"]?.stringValue else { throw MockError.invalidParams("app") }
            guard let row = device.installedApps.first(where: { $0["output"]?.stringValue == app || $0["id"]?.stringValue == app }) else {
                throw MockError.operation("not_found", "No applet \(app) on this device.", [:])
            }
            let hold = scenario.mutexHold
            return try await holdingMutex(device.id, session: id, method: method) {
                try await Task.sleep(for: hold)
                await self.setRunning(device.id, row: row)
                return ["output": row["output"] ?? .null]
            }
        case "devices.remove":
            // CONTROL-API.md `devices.remove`: the device refuses its home
            // screen and the running applet, as §6.2 `runtime.remove` does.
            let device = try linked(params)
            guard let app = params["app"]?.stringValue else { throw MockError.invalidParams("app") }
            guard case .array(let features)? = device.link["features"], features.contains(.string("runtime.remove")) else {
                throw MockError.operation("link", "The device does not support runtime.remove.", ["code": "unsupported"])
            }
            guard let row = device.installedApps.first(where: { $0["id"]?.stringValue == app || $0["output"]?.stringValue == app }),
                  let appId = row["id"]?.stringValue
            else {
                throw MockError.operation("not_found", "The device has no such app: \(app).", [:])
            }
            if row["home"]?.boolValue == true {
                throw MockError.operation("link", "The device answered rejected: the launcher is never removed.",
                                          ["code": "rejected", "reason": "home"])
            }
            if device.status["app"]?["id"]?.stringValue == appId {
                throw MockError.operation("link", "The device answered rejected: the applet is running.",
                                          ["code": "rejected", "reason": "running"])
            }
            let hold = scenario.mutexHold
            return try await holdingMutex(device.id, session: id, method: method) {
                try await Task.sleep(for: hold)
                await self.removeApp(device.id, id: appId)
                return [:]
            }
        case "devices.screenshot":
            try supporting("screenshot", params)
            let device = try linked(params)
            frame += 60
            let path = params["path"]?.stringValue
                ?? home.appending(path: "cache/screenshot-\(device.id)-\(frame).png").path
            try MockScreenshot.write(to: URL(fileURLWithPath: path), frame: frame)
            return ["path": .string(path), "width": 400, "height": 480, "frame": .number(Double(frame))]
        case "devices.logs":
            // Accepted while the device is not linked: the lines start when it links.
            try supporting("logs", params)
            let device = try resolve(params)
            setLogs(device.id, on: params["on"]?.boolValue ?? false)
            return [:]
        case "devices.installProgram":
            try supporting("program", params)
            return try await installProgram(params, session: id, progress: progress)

        case "setup.discover":
            discoverCalls += 1
            try await Task.sleep(for: .milliseconds(300))
            let candidates = discoverCalls <= scenario.setup.discoverEmptyCalls ? [] : scenario.candidates
            return ["candidates": .array(candidates.map {
                ["address": .string($0.address), "port": .number(Double($0.port)),
                 "banner": $0.banner.map(JSONValue.string) ?? .null, "ftpd": .bool($0.ftpd)]
            })]
        case "setup.install":
            guard let host = params["host"]?.stringValue, !host.isEmpty else { throw MockError.invalidParams("host") }
            return try await runSetup(host: host, progress: progress)
        case "setup.card":
            guard let path = params["path"]?.stringValue else { throw MockError.invalidParams("path") }
            guard FileManager.default.fileExists(atPath: path) else {
                throw MockError.operation("not_found", "No SD card is mounted there.", [:])
            }
            return try await runSetup(host: nil, progress: progress)

        case "pairing.list":
            return ["requests": .array(scenario.pairRequests.map(\.json))]
        case "pairing.answer":
            return try await answerPairRequest(params)
        case "pairing.decline":
            let request = try resolvePairRequest(params)
            withdrawPairRequest(request.device)
            return [:]

        case "store.apps":
            guard scenario.storeReachable else { throw storeUnreachable() }
            if params["refresh"]?.boolValue == true { storeFetchedAt = MockClock.now() }
            var device: MockDevice?
            if params["device"] != nil { device = try resolve(params) }
            let offered = scenario.store.filter { app in
                guard let device else { return true }
                // A device that never linked has no known target: nothing.
                return !device.target.isEmpty && app.targets.contains(device.target)
            }
            return ["apps": .array(offered.map { $0.json(icon: storeIconPath($0.id), device: device) }),
                    "fetchedAt": .number(Double(storeFetchedAt))]
        case "store.details":
            guard scenario.storeReachable else { throw storeUnreachable() }
            guard let appId = params["app"]?.stringValue else { throw MockError.invalidParams("app") }
            guard let app = scenario.store.first(where: { $0.id == appId }) else {
                throw MockError.operation("not_found", "The applet store lists no \(appId).", [:])
            }
            return app.detailsJSON
        case "store.sample":
            guard scenario.storeReachable else { throw storeUnreachable() }
            guard let appId = params["app"]?.stringValue else { throw MockError.invalidParams("app") }
            guard let app = scenario.store.first(where: { $0.id == appId && $0.sample }) else {
                throw MockError.operation("not_found", "The applet store has no sample data for \(appId).", [:])
            }
            await progress(["phase": "downloading", "done": .number(Double(app.bytes)), "total": .number(Double(app.bytes))])
            // The mock has no real package: the player says it cannot run this one.
            let folder = home.appending(path: "cache/store/samples")
            try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let package = folder.appending(path: "\(app.id).pocket"), mock = folder.appending(path: "\(app.id).json")
            try? Data("not a package".utf8).write(to: package)
            try? Data(#"{"mock":1,"routes":[]}"#.utf8).write(to: mock)
            return ["package": .string(package.path), "mock": .string(mock.path)]
        case "store.install":
            try supporting("push", params)
            let device = try linked(params)
            guard let appId = params["app"]?.stringValue else { throw MockError.invalidParams("app") }
            guard let index = scenario.store.firstIndex(where: { $0.id == appId }),
                  scenario.store[index].targets.contains(device.target)
            else {
                throw MockError.operation("not_found", "No applet \(appId) for this device's target.", [:])
            }
            let app = scenario.store[index]
            let delay = scenario.storeStepDelay
            let reachable = scenario.storeReachable
            let rejected = scenario.storeRejected.contains(appId)
            let linkLoss = scenario.storeLinkLoss.contains(appId)
            let relinkAfter = scenario.storeRelinkAfter
            let result = try await holdingMutex(device.id, session: id, method: method) {
                let total = app.bytes
                if app.cached {
                    await progress(["phase": "downloading", "done": .number(Double(total)), "total": .number(Double(total))])
                } else {
                    guard reachable else { throw MockError.operation("store", "The store did not answer.", ["reason": "unreachable"]) }
                    for step in 0...3 {
                        await progress(["phase": "downloading", "done": .number(Double(total * step / 3)), "total": .number(Double(total))])
                        try await Task.sleep(for: delay)
                    }
                }
                if rejected {
                    throw MockError.operation("store", "The package of \(appId) \(app.version) is rejected: the publisher's signature does not verify.",
                                              ["reason": "rejected"])
                }
                for (phase, done) in [("sending", 0), ("receiving", total / 2), ("receiving", total), ("staged", total)] {
                    await progress(["phase": .string(phase), "done": .number(Double(done)), "total": .number(Double(total))])
                    try await Task.sleep(for: delay)
                    if linkLoss && phase == "receiving" && done == total {
                        await self.loseLinkDuringInstall(device.id, app, relinkAfter: relinkAfter)
                        throw MockError.operation("link", "The device answered closed: the connection closed.", ["code": "closed"])
                    }
                }
                return ["app": ["id": .string(app.id), "output": .string(app.output), "title": .string(app.title)],
                        "hash": "5a5a5a5a5a5a5a5a", "generation": 5]
            }
            scenario.store[index].cached = true
            installStoreApp(device.id, app)
            emit("store.changed", ["fetchedAt": .number(Double(storeFetchedAt))])
            return result
        case "services.list":
            return ["services": .array(scenario.services.map(\.json))]
        case "services.install":
            guard let path = params["path"]?.stringValue else { throw MockError.invalidParams("path") }
            var isFolder: ObjCBool = false
            guard FileManager.default.fileExists(atPath: path, isDirectory: &isFolder),
                  isFolder.boolValue || path.hasSuffix(".tar.gz")
            else {
                throw MockError.operation("invalid", "This is not a service package.", [:])
            }
            let stages = scenario.bunInstalled ? ["installing"] : ["runtime", "installing"]
            for stage in stages {
                let message = stage == "runtime" ? "Downloading Bun for the service." : "Installing the service package \(path)."
                await progress(["stage": .string(stage), "message": .string(message)])
                try await Task.sleep(for: .milliseconds(150))
            }
            scenario.bunInstalled = true
            var name = URL(fileURLWithPath: path).lastPathComponent
            if name.hasSuffix(".tar.gz") { name.removeLast(7) }
            var service = MockService(id: name, version: "1.0.0")
            service.since = MockClock.now()
            scenario.services.removeAll { $0.id == service.id }
            scenario.services.append(service)
            emit("service.changed", ["service": service.json])
            return service.json
        case "services.remove":
            let service = try resolveService(params)
            if params["purge"]?.boolValue == true { purged.insert(service.id) }
            scenario.services.removeAll { $0.id == service.id }
            emit("service.removed", ["id": .string(service.id)])
            return [:]
        case "services.start", "services.stop", "services.restart":
            let service = try resolveService(params)
            return try await changeService(service.id, action: String(method.dropFirst("services.".count)))
        case "services.logs":
            let service = try resolveService(params)
            let tail = params["tail"]?.intValue ?? 200
            return ["lines": .array(Array(service.log.suffix(tail)))]
        case "services.runtime":
            if params["install"]?.boolValue == true, !scenario.bunInstalled {
                let total = 60_000_000
                for step in 1...5 {
                    try await Task.sleep(for: .milliseconds(200))
                    await progress(["done": .number(Double(total * step / 5)), "total": .number(Double(total))])
                }
                scenario.bunInstalled = true
            }
            return ["version": scenario.bunInstalled ? "1.3.2" : .null, "installed": .bool(scenario.bunInstalled),
                    "path": scenario.bunInstalled ? .string(home.appending(path: "bun/bin/bun").path) : .null]
        default:
            throw MockError.methodNotFound(method)
        }
    }

    private var hubIdentity: JSONValue {
        ["id": .string(scenario.hubId), "name": .string(scenario.hubName), "version": .string(scenario.hubVersion)]
    }

    private func resolve(_ params: JSONValue) throws -> MockDevice {
        guard let id = params["device"]?.stringValue else { throw MockError.invalidParams("device") }
        if id == "*" {
            switch scenario.devices.count {
            case 0: throw MockError.operation("not_found", "No device is paired with this Hub.", [:])
            case 1: return scenario.devices[0]
            default: throw MockError.operation("invalid", "More than one device is paired; name one.", [:])
            }
        }
        guard let device = device(id) else { throw MockError.operation("not_found", "No such device.", [:]) }
        return device
    }

    /// Refuses an operation the device's family does not take (§2
    /// `supports`): `link` with `code` `unsupported`, before anything else.
    private func supporting(_ operation: String, _ params: JSONValue) throws {
        guard try !resolve(params).supports.contains(operation) else { return }
        let message: String = switch operation {
        case "push": "This device takes no applet sent from the computer: install it from the store page on the device itself."
        case "screenshot": "This device takes no screenshot."
        case "logs": "This device sends no log."
        default: "Hub does not install this device's software."
        }
        throw MockError.operation("link", message, ["code": "unsupported"])
    }

    private func linked(_ params: JSONValue) throws -> MockDevice {
        let device = try resolve(params)
        guard device.isLinked else {
            throw MockError.operation("not_linked", "\(device.name.isEmpty ? "The device" : device.name) is not connected.", [:])
        }
        return device
    }

    private func resolveService(_ params: JSONValue) throws -> MockService {
        guard let id = params["id"]?.stringValue else { throw MockError.invalidParams("id") }
        guard let service = scenario.services.first(where: { $0.id == id }) else {
            throw MockError.operation("not_found", "No such service.", [:])
        }
        return service
    }

    /// Runs `body` holding the device mutex: `busy` with the holder `{kind, name, method}` while held.
    private func holdingMutex(_ device: String, session: Int, method: String,
                              _ body: @Sendable () async throws -> JSONValue) async throws -> JSONValue
    {
        if let current = self.device(device), !current.busy.isNull {
            throw MockError.operation("busy", "The device is busy.", ["holder": current.busy["holder"] ?? .null])
        }
        let client = sessions[session]?.client ?? .null
        let holder: JSONValue = ["kind": client["kind"] ?? "gui", "name": client["name"] ?? "unknown", "method": .string(method)]
        modify(device) { $0.busy = ["holder": holder, "since": .number(Double(MockClock.now())), "launching": false] }
        defer { Task { self.modify(device) { $0.busy = .null } } }
        return try await body()
    }

    /// `devices.installProgram`: the latest release (or a tag, or a local
    /// file), downloaded, sent, and by default launched: the device restarts
    /// (`device.unlinked` `restart`, held with `launching`) and links again on
    /// the new version, as `scenario.program` scripts it.
    private func installProgram(_ params: JSONValue, session: Int, progress: Progress) async throws -> JSONValue {
        let device = try resolve(params)
        let localFile = params["program"]?["path"]?.stringValue
        if let localFile, !FileManager.default.fileExists(atPath: localFile) {
            throw MockError.operation("not_found", "There is no file at \(localFile).", [:])
        }
        let launch = params["launch"]?.boolValue ?? true
        let behavior = scenario.program
        let release = scenario.programRelease
        // The mutex first, as Hub takes it: a restarting device is `busy`, not `not_linked`.
        if !device.busy.isNull {
            throw MockError.operation("busy", "The device is busy.", ["holder": device.busy["holder"] ?? .null])
        }
        guard device.isLinked else {
            throw MockError.operation("not_linked", "The device is not connected to this Hub now.", [:])
        }
        let client = sessions[session]?.client ?? .null
        let holder: JSONValue = ["kind": client["kind"] ?? "gui", "name": client["name"] ?? "unknown", "method": "devices.installProgram"]
        modify(device.id) { $0.busy = ["holder": holder, "since": .number(Double(MockClock.now())), "launching": false] }
        let version: String?
        do {
            var total = 70_000
            if localFile == nil {
                if let reason = behavior.releaseFailure {
                    let message = reason == "rejected"
                        ? "Pocket Shell's release does not pass its checks, so nothing was sent to the device: the checksum does not match."
                        : reason == "not_found" ? "Pocket Shell has no release v1999.01.01."
                        : "Hub could not get Pocket Shell's release, so nothing was sent to the device: HTTP 503."
                    throw MockError.operation("release", message, ["reason": .string(reason)])
                }
                guard let release else {
                    throw MockError.operation("release", "Hub could not get Pocket Shell's release, so nothing was sent to the device: no answer.",
                                              ["reason": "unreachable"])
                }
                total = release.bytes
                for step in 0...3 {
                    await progress(["phase": "downloading", "done": .number(Double(total * step / 3)), "total": .number(Double(total))])
                    try await Task.sleep(for: behavior.stepDelay)
                }
                version = release.version
            } else {
                version = nil
            }
            for (phase, done) in [("sending", 0), ("receiving", total / 2), ("receiving", total)] {
                await progress(["phase": .string(phase), "done": .number(Double(done)), "total": .number(Double(total))])
                try await Task.sleep(for: behavior.stepDelay)
                if behavior.dropLinkWhileSending && phase == "receiving" {
                    unlink(device.id, code: "lost", reason: "the connection ended without a CLOSE")
                    throw MockError.operation("link", "The device answered closed.", ["code": "closed"])
                }
            }
            if behavior.launchFails {
                throw MockError.operation("link", "The device answered launch: the program could not be started.",
                                          ["code": "launch", "installed": true])
            }
        } catch {
            modify(device.id) { $0.busy = .null }
            throw error
        }
        guard launch else {
            modify(device.id) { $0.busy = .null }
            return ["phase": "installed", "name": "pocket-shell.3dsx", "version": version.map(JSONValue.string) ?? .null]
        }
        // The answer goes out first; the restart follows it.
        let id = device.id
        Task {
            try? await Task.sleep(for: .milliseconds(50))
            self.restartIntoProgram(id, holder: holder, version: behavior.reportedAfterRestart ?? version, behavior: behavior)
        }
        return ["phase": "launching", "name": "pocket-shell.3dsx", "version": version.map(JSONValue.string) ?? .null]
    }

    /// The device closes with `restart`, Hub holds it (`launching`) until it
    /// links again, which it does with its old apps report first and the new
    /// version a moment later; or Hub lets go after its hold limit.
    private func restartIntoProgram(_ id: String, holder: JSONValue, version: String?, behavior: MockProgramBehavior) {
        logTasks.removeValue(forKey: id)?.cancel()
        emit("device.unlinked", ["id": .string(id), "code": "restart", "reason": "the device restarts into a new program"])
        modify(id) {
            $0.linkDown()
            $0.busy = ["holder": holder, "since": .number(Double(MockClock.now())), "launching": true]
        }
        Task {
            guard let relinkAfter = behavior.relinkAfter else {
                try? await Task.sleep(for: behavior.launchHoldLimit)
                self.modify(id) { $0.busy = .null }
                return
            }
            try? await Task.sleep(for: relinkAfter)
            self.link(id)
            try? await Task.sleep(for: behavior.reportDelay)
            if let version { self.modify(id) { $0.setProgramVersion(version) } }
        }
    }

    /// The device went quiet while it installed: Hub closes the link for
    /// silence; the device comes back holding the applet.
    private func loseLinkDuringInstall(_ id: String, _ app: MockStoreApp, relinkAfter: Duration) {
        unlink(id, code: "silence", reason: "10 s without a frame")
        Task {
            try? await Task.sleep(for: relinkAfter)
            self.installStoreApp(id, app)
            self.link(id)
        }
    }

    private func installApp(_ device: String, id: String, output: String) {
        modify(device) { value in
            var rows = value.installedApps.filter { $0["id"]?.stringValue != id }
            rows.append(["id": .string(id), "output": .string(output), "version": "0.0.1", "hash": "abcdef0123456789"])
            value.apps = .array(rows)
        }
    }

    private func removeApp(_ device: String, id: String) {
        modify(device) { value in
            value.apps = .array(value.installedApps.filter { $0["id"]?.stringValue != id })
        }
    }

    private func installStoreApp(_ device: String, _ app: MockStoreApp) {
        modify(device) { value in
            var rows = value.installedApps.filter { $0["id"]?.stringValue != app.id }
            rows.append(["id": .string(app.id), "output": .string(app.output), "version": .string(app.version), "hash": "5a5a5a5a5a5a5a5a"])
            value.apps = .array(rows)
        }
    }

    private func storeUnreachable() -> MockError {
        .operation("store", "The applet store at http://127.0.0.1:9 did not answer.", ["reason": "unreachable"])
    }

    private func storeIconPath(_ id: String) -> String {
        home.appending(path: "cache/store/icons/\(id).png").path
    }

    /// One generated PNG per applet, where the real core caches store icons.
    private func writeStoreIcons() {
        for (index, app) in scenario.store.enumerated() {
            try? MockIcon.write(to: URL(fileURLWithPath: storeIconPath(app.id)), hue: Double(index) / Double(max(scenario.store.count, 1)))
        }
    }

    private func setRunning(_ device: String, row: JSONValue) {
        modify(device) { value in
            guard case .object(var status) = value.status else { return }
            let output = row["output"]?.stringValue ?? ""
            status["app"] = ["id": row["id"] ?? .null, "output": .string(output), "title": .string(output.capitalized)]
            value.status = .object(status)
        }
    }

    private func setLogs(_ device: String, on: Bool) {
        logTasks.removeValue(forKey: device)?.cancel()
        if on { logsOn.insert(device) } else { logsOn.remove(device) }
        if on, self.device(device)?.isLinked == true { startLogLines(device) }
    }

    private func startLogLines(_ device: String) {
        logTasks.removeValue(forKey: device)?.cancel()
        let interval = scenario.logInterval
        logTasks[device] = Task {
            var n = 0
            let samples = [("info", "guest", "render: 60 fps"), ("debug", "host", "heap 3.1 MiB used"),
                           ("info", "guest", "weather: fetched forecast"), ("warn", "guest", "slow frame: 41 ms")]
            while !Task.isCancelled {
                let sample = samples[n % samples.count]
                self.emitDeviceLog(device, level: sample.0, source: sample.1, text: sample.2)
                n += 1
                try? await Task.sleep(for: interval)
            }
        }
    }

    private func runSetup(host: String?, progress: Progress) async throws -> JSONValue {
        let behavior = scenario.setup
        func fail(_ stage: SetupStage) throws {
            guard behavior.failAt == stage else { return }
            if let kind = behavior.failureKind {
                throw MockError.operation(kind, "Something went wrong.", [:])
            }
            var data: [String: JSONValue] = ["step": .string(stage.rawValue)]
            if let reason = behavior.failureReason { data["reason"] = .string(reason) }
            let messages: [SetupStage: String] = [
                .connecting: "Hub could not reach the console at \(host ?? "the card").",
                .reading: "Hub could not read the console's storage.",
                .downloading: "Hub could not download Pocket Shell.",
                .uploading: "The transfer to the console stopped.",
                .pairing: "Hub could not finish writing to the console.",
            ]
            throw MockError.operation("setup", messages[stage] ?? "The setup failed.", data)
        }
        let stages: [(SetupStage, String)] = [
            (.connecting, "Connecting to \(host ?? "the SD card")"), (.reading, "Reading the console's storage"),
            (.downloading, "Downloading Pocket Shell"), (.uploading, "Copying Pocket Shell to the console"),
            (.pairing, "Introducing the console to this Mac"),
        ]
        for (stage, message) in stages {
            try fail(stage)
            if stage == .downloading || stage == .uploading {
                let total = 2_400_000
                for chunk in 0...behavior.chunks {
                    await progress(["stage": .string(stage.rawValue), "done": .number(Double(total * chunk / behavior.chunks)),
                                    "total": .number(Double(total)), "message": .string(message)])
                    try await Task.sleep(for: behavior.stepDelay)
                }
            } else {
                await progress(["stage": .string(stage.rawValue), "message": .string(message)])
                try await Task.sleep(for: behavior.stepDelay)
            }
        }
        await progress(["stage": "done", "message": "Done"])

        // The record exists from here on: unlinked, named only once it links.
        let id = behavior.deviceId
        if device(id) == nil {
            var record = MockDevice(id: id)
            record.lastAddress = host
            record.setupUnconfirmed = !behavior.confirmed
            upsert(record)
        }
        if let delay = behavior.linkAfter {
            Task {
                try? await Task.sleep(for: delay)
                self.link(id)
            }
        }
        if !behavior.confirmed {
            // A key was stored and its final readback did not return: the error,
            // never a report (§3 Setup).
            throw MockError.operation("setup", "Hub wrote the pairing but could not read it back.",
                                      ["step": "pairing", "reason": "storage", "device": .string(id), "confirmed": false])
        }
        return [
            "device": .string(id), "pairing": "key",
            "program": ["action": "installed", "version": "0.4.0", "runtime": "0.4.0"],
            "confirmed": true,
            "next": "Exit ftpd, then open Pocket Shell from the Homebrew Launcher.",
        ]
    }

    private func changeService(_ id: String, action: String) async throws -> JSONValue {
        func set(_ change: (inout MockService) -> Void) {
            guard let index = scenario.services.firstIndex(where: { $0.id == id }) else { return }
            change(&scenario.services[index])
            emit("service.changed", ["service": scenario.services[index].json])
        }
        if action == "stop" || action == "restart" {
            set { $0.state = "stopping"; $0.since = MockClock.now() }
            try await Task.sleep(for: .milliseconds(150))
            set {
                $0.state = "stopped"; $0.since = MockClock.now(); $0.pid = nil; $0.error = nil
                $0.lastExit = ["code": .null, "signal": "SIGTERM", "at": .number(Double(MockClock.now()))]
            }
        }
        if action == "start" || action == "restart" {
            set { $0.state = "starting"; $0.since = MockClock.now(); $0.error = nil }
            try await Task.sleep(for: .milliseconds(150))
            set { $0.state = "running"; $0.since = MockClock.now(); $0.pid = Int.random(in: 20_000...60_000) }
            emitServiceLog(id, text: "\(id) started")
        }
        return scenario.services.first { $0.id == id }?.json ?? .null
    }

    private func shutdownNow() {
        stop()
        onShutdown?()
    }
}

enum MockError: Error {
    case operation(String, String, [String: JSONValue])
    case invalidParams(String)
    case methodNotFound(String)
    /// A hello in another protocol than the core's (CONTROL-API.md §1, Hello).
    case otherProtocol(core: Int, client: Int)

    var json: JSONValue {
        switch self {
        case .otherProtocol(let core, let client):
            var error: [String: JSONValue] = ["code": -32602, "message": .string("This Hub speaks protocol \(core), not \(client).")]
            if core >= 2 { error["data"] = ["protocol": .number(Double(core))] }
            return .object(error)
        case .operation(let kind, let message, var data):
            data["kind"] = .string(kind)
            return ["code": 1, "message": .string(message), "data": .object(data)]
        case .invalidParams(let name):
            return ["code": -32602, "message": .string("Missing or unusable parameter \(name).")]
        case .methodNotFound(let method):
            return ["code": -32601, "message": .string("No method \(method).")]
        }
    }
}
