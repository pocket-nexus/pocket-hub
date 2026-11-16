import Foundation
import HubClient
import Observation

/// The app's view of Hub: connection state, the paired devices, the daemons,
/// logs and screenshots, and the actions a UI offers. Main-actor state fed by
/// `HubClient`'s updates; every call to Hub runs off the main actor.
///
/// While the core is unreachable, `isLive` is false and the device and daemon
/// lists are emptied: a UI never shows a link or a state it cannot vouch for.
@MainActor
@Observable
public final class HubModel {
    public enum Connection: Equatable, Sendable {
        case connecting
        case startingCore
        case connected
        case unreachable(UnreachableReason, retryAt: Date)
        case stopped
    }

    public private(set) var connection: Connection = .connecting
    /// Hub's identity; kept across outages so the window can still say whose Hub it is.
    public private(set) var hub: HubIdentity?
    public private(set) var status: HubStatus?
    /// Paired devices, linked first, then by name.
    public private(set) var devices: [Device] = []
    /// The devices that ask to be paired by the code they show and wait for
    /// it, oldest first (CONTROL-API.md §3 Pairing).
    public private(set) var pairRequests: [PairRequest] = []
    /// nil while unknown (not connected, or `daemons.list` failed).
    public private(set) var daemons: [Daemon]?
    public private(set) var bun: BunRuntime?
    /// The last way each device's link ended, until it links again.
    public private(set) var lastUnlink: [String: DeviceUnlinked] = [:]
    /// The latest screenshot of each device, asked for or sent unasked.
    public private(set) var screenshots: [String: ScreenshotResult] = [:]
    public private(set) var deviceLogs: [String: LogBuffer<DeviceLogLine>] = [:]
    /// Devices whose log this app has turned on.
    public private(set) var logsOn: Set<String> = []
    public private(set) var daemonLogs: [String: LogBuffer<DaemonLogLine>] = [:]
    /// Push progress per device (0...1, nil fraction while unknown).
    public private(set) var pushes: [String: PushActivity] = [:]
    public private(set) var takingScreenshot: Set<String> = []
    /// The applet being launched per device.
    public private(set) var launching: [String: String] = [:]
    /// The applet being deleted per device.
    public private(set) var removing: [String: String] = [:]
    public private(set) var daemonInstall: DaemonInstallActivity?
    public private(set) var bunInstallProgress: Double??
    /// Software updates per device, from the click until it is dismissed.
    public internal(set) var programUpdates: [String: ProgramUpdate] = [:]
    /// How long an update waits for the device to come back; shorter in tests.
    public var programUpdateTimeouts = ProgramUpdateTimeouts()
    @ObservationIgnored var programTracking: [String: ProgramUpdateTracking] = [:]

    public struct PushActivity: Equatable, Sendable {
        public var fileName: String
        public var fraction: Double?
        /// `sending`, then the device's own `receiving`, `received`, `staged`.
        public var phase: String?
    }

    public struct DaemonInstallActivity: Equatable, Sendable {
        public var source: String
        /// `runtime` while Hub downloads Bun for it, then `installing`; empty before the first progress.
        public var stage: String
    }

    public nonisolated let client: HubClient
    /// The applet store page and the catalog by id.
    public let store: StoreModel
    /// The setup flow that is open, told about device changes so it can finish by itself.
    weak var activeSetup: SetupFlowModel?
    private var updatesTask: Task<Void, Never>?

    public init(client: HubClient) {
        self.client = client
        store = StoreModel(client: client, model: nil)
        store.model = self
    }

    /// A model for this app: the standard socket path, a `gui` identity, and
    /// the standard core launcher (unless `launchCore` is false).
    public convenience init(clientName: String, version: String, launchCore: Bool = true) {
        let configuration = HubClient.Configuration(
            identity: ClientIdentity(kind: .gui, name: clientName, version: version),
            launcher: launchCore ? .standard() : nil)
        self.init(client: HubClient(configuration: configuration))
    }

    public var isLive: Bool { connection == .connected }

    public func device(_ id: String) -> Device? {
        devices.first { $0.id == id }
    }

    public var linkedDevices: [Device] { devices.filter(\.isLinked) }

    /// Returns once the core answers and its lists are here (`isLive`). What
    /// arrives from outside while the core is still starting, a link the app
    /// was opened with (docs/DEEP-LINKS.md §2), waits here for it.
    public func whenLive() async {
        while !isLive {
            await withCheckedContinuation { continuation in
                // Told once, just before `connection` next changes; the loop
                // then reads the new value.
                withObservationTracking { _ = connection } onChange: { continuation.resume() }
            }
        }
    }

    /// Starts connecting and applying updates. Idempotent.
    public func start() {
        guard updatesTask == nil else { return }
        let updates = client.updates
        updatesTask = Task { [weak self] in
            for await update in updates {
                guard let self else { return }
                self.apply(update)
            }
        }
        Task { await client.start() }
    }

    /// Ends the app's session. Without `shutdownCore` (the default way to quit)
    /// Hub keeps running and devices stay connected; with it Hub itself is
    /// asked to shut down (links close, daemons stop) and the call returns
    /// once the core is gone or the bound has passed.
    public func quit(shutdownCore: Bool) async {
        if shutdownCore {
            try? await client.shutdownHub()
        } else {
            await client.stop()
        }
    }

    public func retryNow() {
        Task { await client.retry() }
    }

    // MARK: - Updates

    func apply(_ update: HubClientUpdate) {
        switch update {
        case .state(let state):
            switch state {
            case .idle, .connecting:
                // A reconnect after a live session keeps showing "unreachable"
                // until it succeeds, so the window does not flicker.
                if case .unreachable = connection {} else { connection = .connecting }
                dropLiveState()
            case .startingCore:
                connection = .startingCore
                dropLiveState()
            case .connected:
                connection = .connected
            case .unreachable(let reason, let retryAt):
                connection = .unreachable(reason, retryAt: retryAt)
                dropLiveState()
            case .stopped:
                connection = .stopped
                dropLiveState()
            }
        case .synced(let snapshot):
            // A snapshot is only ever delivered on a live connection; becoming
            // live together with the lists means no view sees one without the other.
            connection = .connected
            hub = snapshot.hello.hub
            status = snapshot.status
            devices = Self.sorted(snapshot.devices)
            pairRequests = snapshot.pairRequests
            daemons = snapshot.daemons.map(Self.sorted)
            bun = snapshot.bun
            let known = Set(snapshot.devices.map(\.id))
            screenshots = screenshots.filter { known.contains($0.key) }
            // Hub keeps logs on per client and per device, and re-enables them
            // after a relink by itself; a new connection is a new client, so
            // this client asks again for the devices it was listening to.
            logsOn.formIntersection(known)
            for id in logsOn { resendLogs(id) }
            for device in snapshot.devices { activeSetup?.deviceDidChange(device) }
            activeSetup?.pairRequestsDidChange(pairRequests)
            store.didSync()
        case .event(let event):
            apply(event)
        }
    }

    private func apply(_ event: HubEvent) {
        switch event {
        case .deviceChanged(let device):
            let old = self.device(device.id)
            defer {
                if old == nil { store.devicesDidChange() } else { store.deviceDidChange(from: old, to: device) }
            }
            if let index = devices.firstIndex(where: { $0.id == device.id }) {
                devices[index] = device
            } else {
                devices.append(device)
            }
            devices = Self.sorted(devices)
            if device.isLinked { lastUnlink[device.id] = nil }
            activeSetup?.deviceDidChange(device)
            programDeviceDidChange(device)
        case .deviceRemoved(let id):
            devices.removeAll { $0.id == id }
            store.devicesDidChange()
            screenshots[id] = nil
            deviceLogs[id] = nil
            logsOn.remove(id)
            lastUnlink[id] = nil
            forgetProgramUpdate(of: id)
        case .deviceUnlinked(let unlinked):
            lastUnlink[unlinked.id] = unlinked
            // The `device.changed` that follows carries the truth; until it
            // arrives the device must not look linked.
            if let index = devices.firstIndex(where: { $0.id == unlinked.id }) {
                devices[index].link = nil
                devices[index].busy = nil
                devices[index].channels = []
                devices = Self.sorted(devices)
            }
            if let device = device(unlinked.id) { programDeviceDidChange(device, fromHub: false) }
            takingScreenshot.remove(unlinked.id)
        case .deviceLog(let line):
            deviceLogs[line.id, default: LogBuffer()].append(line)
        case .deviceScreenshot(let shot):
            screenshots[shot.id] = shot.result
        case .pairingChanged(let requests):
            pairRequests = requests
            activeSetup?.pairRequestsDidChange(requests)
        case .daemonChanged(let daemon):
            var list = daemons ?? []
            if let index = list.firstIndex(where: { $0.id == daemon.id }) {
                list[index] = daemon
            } else {
                list.append(daemon)
            }
            daemons = Self.sorted(list)
        case .daemonRemoved(let id):
            daemons?.removeAll { $0.id == id }
            daemonLogs[id] = nil
        case .daemonLog(let id, let line):
            daemonLogs[id, default: LogBuffer(capacity: 1000)].append(line)
        case .storeChanged:
            store.catalogDidChange()
        case .hubChanged(let identity):
            hub = identity
            status?.name = identity.name
        case .unknown:
            break
        }
    }

    /// Forgets everything that described the live state of Hub. Logs and
    /// screenshots are history and stay; links, applets and daemons do not.
    private func dropLiveState() {
        store.dropLiveState()
        devices = []
        pairRequests = []
        daemons = nil
        bun = nil
        pushes = [:]
        takingScreenshot = []
        launching = [:]
        daemonInstall = nil
        bunInstallProgress = nil
        dropProgramUpdates()
    }

    private func resendLogs(_ id: String) {
        Task { try? await client.setLogs(device: id, on: true) }
    }

    static func sorted(_ devices: [Device]) -> [Device] {
        devices.sorted { lhs, rhs in
            if lhs.isLinked != rhs.isLinked { return lhs.isLinked }
            let order = lhs.displayName.localizedStandardCompare(rhs.displayName)
            return order == .orderedSame ? lhs.id < rhs.id : order == .orderedAscending
        }
    }

    static func sorted(_ daemons: [Daemon]) -> [Daemon] {
        daemons.sorted { $0.id.localizedStandardCompare($1.id) == .orderedAscending }
    }

    // MARK: - Device actions

    public func launch(_ app: InstalledApp, on deviceId: String) async throws {
        launching[deviceId] = app.id
        defer { launching[deviceId] = nil }
        _ = try await client.launch(device: deviceId, app: app.output.isEmpty ? app.id : app.output)
    }

    /// Opens an installed applet by id (Hub sends the output name the device's
    /// last apps report gives that id).
    public func launch(appId: String, on deviceId: String) async throws {
        launching[deviceId] = appId
        defer { launching[deviceId] = nil }
        _ = try await client.launch(device: deviceId, app: appId)
    }

    /// Deletes an installed applet and its data on the device. The device's
    /// next apps report, and so `device.changed`, drops its row.
    public func remove(_ app: InstalledApp, from deviceId: String) async throws {
        removing[deviceId] = app.id
        defer { removing[deviceId] = nil }
        try await client.remove(device: deviceId, app: app.id)
    }

    @discardableResult
    public func push(_ file: URL, to deviceId: String) async throws -> PushResult {
        pushes[deviceId] = PushActivity(fileName: file.lastPathComponent, fraction: nil)
        defer { pushes[deviceId] = nil }
        return try await client.push(device: deviceId, path: file.path(percentEncoded: false)) { [weak self] progress in
            Task { @MainActor in
                guard let self, self.pushes[deviceId] != nil else { return }
                self.pushes[deviceId]?.fraction = progress.fraction
                self.pushes[deviceId]?.phase = progress.phase
            }
        }
    }

    @discardableResult
    public func takeScreenshot(of deviceId: String) async throws -> ScreenshotResult {
        takingScreenshot.insert(deviceId)
        defer { takingScreenshot.remove(deviceId) }
        let result = try await client.screenshot(device: deviceId)
        screenshots[deviceId] = result
        return result
    }

    public func setLogs(_ on: Bool, for deviceId: String) async throws {
        try await client.setLogs(device: deviceId, on: on)
        if on { logsOn.insert(deviceId) } else { logsOn.remove(deviceId) }
    }

    public func clearLog(of deviceId: String) {
        deviceLogs[deviceId] = nil
    }

    public func forget(_ deviceId: String) async throws {
        try await client.forget(device: deviceId)
    }

    // MARK: - Hub actions

    public func setHubName(_ name: String) async throws {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        let applied = try await client.setName(trimmed)
        if let hub { self.hub = HubIdentity(id: hub.id, name: applied, version: hub.version) }
        status?.name = applied
    }

    // MARK: - Daemon actions

    public func perform(_ action: DaemonAction, daemon id: String) async throws {
        let daemon = try await client.daemon(action, id: id)
        apply(.daemonChanged(daemon))
    }

    /// Removes a daemon; with `purge` its data folder goes too.
    public func removeDaemon(_ id: String, purge: Bool = false) async throws {
        try await client.removeDaemon(id: id, purge: purge)
    }

    public func installDaemon(from source: URL) async throws {
        daemonInstall = DaemonInstallActivity(source: source.lastPathComponent, stage: "")
        defer { daemonInstall = nil }
        let daemon = try await client.installDaemon(path: source.path(percentEncoded: false)) { [weak self] progress in
            Task { @MainActor in
                guard let self, self.daemonInstall != nil else { return }
                self.daemonInstall?.stage = progress.stage
            }
        }
        apply(.daemonChanged(daemon))
    }

    public func loadDaemonLog(_ id: String) async throws {
        let lines = try await client.daemonLogs(id: id)
        var buffer = LogBuffer<DaemonLogLine>(capacity: 1000)
        buffer.replace(with: lines)
        daemonLogs[id] = buffer
    }

    public func refreshBun() async throws {
        bun = try await client.bunRuntime()
    }

    public func installBun() async throws {
        bunInstallProgress = .some(nil)
        defer { bunInstallProgress = nil }
        bun = try await client.bunRuntime(install: true) { [weak self] progress in
            Task { @MainActor in
                guard let self, self.bunInstallProgress != nil else { return }
                self.bunInstallProgress = .some(progress.fraction)
            }
        }
    }

    // MARK: - Setup

    /// A new setup flow, wired to this model's device and pairing updates.
    /// With `answering`, it starts at the code of that device's request, for
    /// a device that asked while no flow was open; else at the choice of the
    /// kind of device to add.
    public func makeSetupFlow(answering request: PairRequest? = nil) -> SetupFlowModel {
        let flow = SetupFlowModel(client: client, model: self)
        activeSetup = flow
        if let request { flow.answer(request) }
        return flow
    }
}
