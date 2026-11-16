import Foundation

/// The methods of CONTROL-API.md §3, typed. Each builds the params the
/// contract names and decodes the result it documents.
extension HubCalling {
    // MARK: Hub

    public func hello(protocol version: Int = HubClient.protocolVersion, client: ClientIdentity) async throws -> HelloResult {
        try await call("hub.hello", params: ["protocol": .number(Double(version)), "client": JSONValue(encoding: client)]).decoded()
    }

    public func status() async throws -> HubStatus {
        try await call("hub.status").decoded()
    }

    @discardableResult
    public func setName(_ name: String) async throws -> String {
        let result = try await call("hub.setName", params: ["name": .string(name)])
        return result["name"]?.stringValue ?? name
    }

    /// Subscribes to `topics`, or to every topic when nil.
    public func subscribe(topics: [String]? = nil) async throws {
        var params: [String: JSONValue] = [:]
        if let topics { params["topics"] = .array(topics.map(JSONValue.string)) }
        _ = try await call("hub.subscribe", params: .object(params))
    }

    public func shutdown() async throws {
        _ = try await call("hub.shutdown")
    }

    // MARK: Devices

    public func devices() async throws -> [Device] {
        try await call("devices.list")["devices"].orThrow("devices").decoded()
    }

    public func device(_ device: String) async throws -> Device {
        try await call("devices.get", params: ["device": .string(device)]).decoded()
    }

    public func forget(device: String) async throws {
        _ = try await call("devices.forget", params: ["device": .string(device)])
    }

    /// The device's fresh `runtime.status`.
    public func runtimeStatus(device: String) async throws -> RuntimeStatus {
        try await call("devices.status", params: ["device": .string(device)]).decoded()
    }

    public func push(device: String, path: String,
                     progress: (@Sendable (TransferProgress) -> Void)? = nil) async throws -> PushResult
    {
        try await call("devices.push", params: ["device": .string(device), "path": .string(path)],
                       progress: typed(progress)).decoded()
    }

    /// Launches an installed applet by output name or id.
    public func launch(device: String, app: String) async throws -> LaunchResult {
        try await call("devices.launch", params: ["device": .string(device), "app": .string(app)]).decoded()
    }

    /// Deletes an installed applet, and its data, on the device
    /// (`devices.remove`); `app` is its id or output name.
    public func remove(device: String, app: String) async throws {
        _ = try await call("devices.remove", params: ["device": .string(device), "app": .string(app)])
    }

    /// Takes a screenshot; without `path` Hub writes it into its cache folder.
    public func screenshot(device: String, path: String? = nil) async throws -> ScreenshotResult {
        var params: [String: JSONValue] = ["device": .string(device)]
        if let path { params["path"] = .string(path) }
        return try await call("devices.screenshot", params: .object(params)).decoded()
    }

    public func setLogs(device: String, on: Bool) async throws {
        _ = try await call("devices.logs", params: ["device": .string(device), "on": .bool(on)])
    }

    /// Replaces the device's own program over the link. Without `program` Hub
    /// fetches the latest release; without `launch` (true by default on Hub's
    /// side) the device restarts into it and links again by itself, held
    /// (`busy.launching`) meanwhile. Progress: `downloading` when Hub fetches
    /// a release, then the phases of `devices.push`. A release that cannot be
    /// used is the error `release`.
    public func installProgram(device: String, program: ProgramSource? = nil, launch: Bool? = nil,
                               progress: (@Sendable (TransferProgress) -> Void)? = nil) async throws -> InstallProgramResult
    {
        var params: [String: JSONValue] = ["device": .string(device)]
        if let program { params["program"] = try JSONValue(encoding: program) }
        if let launch { params["launch"] = .bool(launch) }
        return try await call("devices.installProgram", params: .object(params), progress: typed(progress)).decoded()
    }

    // MARK: Setup

    /// Hosts answering on ftpd's port, ftpd's own first. Answers within about 2 s.
    public func discover(kind: String = "3ds") async throws -> [SetupCandidate] {
        let result: DiscoverResult = try await call("setup.discover", params: ["kind": .string(kind)]).decoded()
        return result.ordered
    }

    public func install(kind: String = "3ds", host: String, port: Int? = nil, program: ProgramSource = .latestRelease,
                        progress: (@Sendable (SetupProgress) -> Void)? = nil) async throws -> InstallReport
    {
        var params: [String: JSONValue] = ["kind": .string(kind), "host": .string(host),
                                           "program": try JSONValue(encoding: program)]
        if let port { params["port"] = .number(Double(port)) }
        return try await call("setup.install", params: .object(params), progress: typed(progress)).decoded()
    }

    public func installFromCard(kind: String = "3ds", path: String, program: ProgramSource = .latestRelease,
                                progress: (@Sendable (SetupProgress) -> Void)? = nil) async throws -> InstallReport
    {
        try await call("setup.card", params: ["kind": .string(kind), "path": .string(path),
                                              "program": try JSONValue(encoding: program)],
                       progress: typed(progress)).decoded()
    }

    // MARK: Pairing by a code

    /// The devices that ask to be paired and wait for their code, oldest first.
    public func pairRequests() async throws -> [PairRequest] {
        try await call("pairing.list")["requests"].orThrow("requests").decoded()
    }

    /// Answers a device's request with the 6 digits it shows. The answer is
    /// the device, now paired; a wrong code is the error `pairing` with
    /// `reason` `wrong_code`, and stores nothing.
    public func answerPairing(device: String, code: String) async throws -> Device {
        try await call("pairing.answer", params: ["device": .string(device), "code": .string(code)]).decoded()
    }

    /// Dismisses a device's request: the device says that this computer's
    /// owner declined.
    public func declinePairing(device: String) async throws {
        _ = try await call("pairing.decline", params: ["device": .string(device)])
    }

    // MARK: Store

    /// The store's applets; with `device`, those that have a variant for its
    /// target, each with `installed` and `update`. A failed catalog fetch is the
    /// error `store`, never the older catalog.
    public func storeApps(device: String? = nil, refresh: Bool = false) async throws -> StoreCatalog {
        var params: [String: JSONValue] = [:]
        if let device { params["device"] = .string(device) }
        if refresh { params["refresh"] = true }
        return try await call("store.apps", params: .object(params)).decoded()
    }

    /// Downloads (when Hub's cache lacks it), checks and installs an applet on
    /// a device. Progress: `downloading`, then the phases of `devices.push`.
    public func storeInstall(device: String, app: String,
                             progress: (@Sendable (TransferProgress) -> Void)? = nil) async throws -> PushResult
    {
        try await call("store.install", params: ["device": .string(device), "app": .string(app)],
                       progress: typed(progress)).decoded()
    }

    // MARK: Daemons

    public func daemons() async throws -> [Daemon] {
        try await call("daemons.list")["daemons"].orThrow("daemons").decoded()
    }

    /// Installs a daemon from a folder or a `.tar.gz`.
    public func installDaemon(path: String, progress: (@Sendable (DaemonInstallProgress) -> Void)? = nil) async throws -> Daemon {
        try await call("daemons.install", params: ["path": .string(path)], progress: typed(progress)).decoded()
    }

    /// Removes a daemon; its data folder stays unless `purge`.
    public func removeDaemon(id: String, purge: Bool = false) async throws {
        var params: [String: JSONValue] = ["id": .string(id)]
        if purge { params["purge"] = true }
        _ = try await call("daemons.remove", params: .object(params))
    }

    public func daemon(_ action: DaemonAction, id: String) async throws -> Daemon {
        try await call("daemons.\(action.rawValue)", params: ["id": .string(id)]).decoded()
    }

    public func daemonLogs(id: String, tail: Int? = nil) async throws -> [DaemonLogLine] {
        var params: [String: JSONValue] = ["id": .string(id)]
        if let tail { params["tail"] = .number(Double(tail)) }
        return try await call("daemons.logs", params: .object(params))["lines"].orThrow("lines").decoded()
    }

    /// The Bun runtime's state; with `install` Hub downloads Bun now.
    public func bunRuntime(install: Bool = false, progress: (@Sendable (ByteProgress) -> Void)? = nil) async throws -> BunRuntime {
        let params: JSONValue = install ? ["install": true] : [:]
        return try await call("daemons.runtime", params: params, progress: typed(progress)).decoded()
    }
}

public enum DaemonAction: String, Sendable { case start, stop, restart }

/// Adapts a typed progress handler to the raw one; a notification that does
/// not decode is skipped rather than failing the call.
private func typed<P: Decodable & Sendable>(_ handler: (@Sendable (P) -> Void)?) -> (@Sendable (JSONValue) -> Void)? {
    guard let handler else { return nil }
    return { raw in
        if let value = try? raw.decode(P.self) { handler(value) }
    }
}

extension Optional where Wrapped == JSONValue {
    func orThrow(_ member: String) throws -> JSONValue {
        guard let value = self else { throw HubError.invalidResponse("result without \(member)") }
        return value
    }
}

extension JSONValue {
    /// Decodes this value, reporting a mismatch as `HubError.invalidResponse`.
    func decoded<T: Decodable>() throws -> T {
        do {
            return try decode(T.self)
        } catch {
            throw HubError.invalidResponse("\(T.self): \(error)")
        }
    }
}
