import Foundation
import os

/// Why Hub cannot be reached right now.
public enum UnreachableReason: Sendable, Equatable {
    /// Nothing listens on the socket.
    case notRunning
    /// Nothing listens and no `pocket-hubd` was found to start.
    case coreNotFound
    /// `pocket-hubd` could not be started, or started and exited before it
    /// answered: the core's own one-line reason (English), or the spawn error.
    /// The client does not start it again until `retry()`.
    case coreFailedToStart(String)
    /// The core answered with another protocol version.
    case incompatible(hubProtocol: Int)
    /// Anything else: the socket refused us, the hello timed out, a resync call failed.
    case failed(String)
}

public enum HubConnectionState: Sendable, Equatable {
    case idle
    case connecting
    /// The core was not running and is being started.
    case startingCore
    /// Connected, subscribed and resynced.
    case connected(HelloResult)
    /// Not connected; another attempt follows at `retryAt`.
    case unreachable(UnreachableReason, retryAt: Date)
    /// `stop()` or `shutdownHub()` ended the client.
    case stopped

    public var isConnected: Bool {
        if case .connected = self { return true }
        return false
    }
}

/// The state a client holds after a (re)connection, read after subscribing so
/// that it plus the events that follow it are the truth (§4, last paragraph).
public struct HubSnapshot: Sendable {
    public var hello: HelloResult
    public var status: HubStatus
    public var devices: [Device]
    /// The devices that ask to be paired by the code they show.
    public var pairRequests: [PairRequest]
    /// nil when `daemons.list` failed; the devices still work.
    public var daemons: [Daemon]?
    /// nil when `daemons.runtime` failed.
    public var bun: BunRuntime?
}

public enum HubClientUpdate: Sendable {
    case state(HubConnectionState)
    /// A connection was established and resynced. Replaces all state; the
    /// events that follow apply on top of it.
    case synced(HubSnapshot)
    case event(HubEvent)
}

/// The long-lived client: connects, says hello, subscribes, resyncs, forwards
/// events, and reconnects when the connection drops, starting the core when
/// nothing answers. Calls go to the current connection.
public actor HubClient: HubCalling {
    public static let protocolVersion = 1
    static let log = Logger(subsystem: "nexus.pocket.hub", category: "client")

    public struct Configuration: Sendable {
        public var identity: ClientIdentity
        /// Re-evaluated before every attempt.
        public var socketPath: @Sendable () -> String
        /// Starts the core when nothing listens. nil: never start it.
        public var launcher: CoreLauncher?
        /// Delay before the n-th consecutive retry; the last value repeats.
        public var retryDelays: [Duration]
        /// How long an unanswered hello may take before the attempt is abandoned.
        public var helloTimeout: Duration
        /// After a launch, how long to keep retrying quickly before launching again.
        public var launchGrace: Duration

        public init(identity: ClientIdentity,
                    socketPath: @escaping @Sendable () -> String = { HubPaths.current().socketPath() },
                    launcher: CoreLauncher? = nil,
                    retryDelays: [Duration] = [.milliseconds(250), .milliseconds(500), .seconds(1), .seconds(2), .seconds(3)],
                    helloTimeout: Duration = .seconds(5),
                    launchGrace: Duration = .seconds(15))
        {
            self.identity = identity
            self.socketPath = socketPath
            self.launcher = launcher
            self.retryDelays = retryDelays
            self.helloTimeout = helloTimeout
            self.launchGrace = launchGrace
        }
    }

    public nonisolated let updates: AsyncStream<HubClientUpdate>
    private let updateContinuation: AsyncStream<HubClientUpdate>.Continuation
    private let configuration: Configuration
    private var connection: HubConnection?
    private var runTask: Task<Void, Never>?
    private var stopped = false
    private var retryNow: CheckedContinuation<Void, Never>?
    private var sleepToken: UInt64 = 0
    /// The core this client started last, while it may still come up.
    private var launched: LaunchedCore?
    private var lastLaunch: ContinuousClock.Instant?
    private var launchReason = UnreachableReason.notRunning
    /// Set when a core this client started exited with a failure: no further
    /// launch until the owner asks (`retry()`), so a core that cannot start is
    /// not restarted in a loop and its reason stays on screen.
    private var launchBlocked: UnreachableReason?
    /// Set once this client has asked an older core to make way for its own
    /// (`replacesOlderCore`).
    private var replacedOlderCore = false

    public init(configuration: Configuration) {
        self.configuration = configuration
        (updates, updateContinuation) = AsyncStream.makeStream(of: HubClientUpdate.self, bufferingPolicy: .unbounded)
    }

    /// Starts connecting. Idempotent.
    public func start() {
        guard runTask == nil, !stopped else { return }
        runTask = Task { await self.run() }
    }

    /// Disconnects and stops reconnecting; the core keeps running.
    public func stop() async {
        stopped = true
        launched?.discardOutput()
        launched = nil
        runTask?.cancel()
        wakeRetry()
        await connection?.close()
        connection = nil
        updateContinuation.yield(.state(.stopped))
    }

    /// Asks the core to shut down, then stops. Reconnection is disabled first,
    /// so the client cannot start the core again while it exits.
    public func shutdownHub(timeout: Duration = .seconds(3)) async throws {
        stopped = true
        launched?.discardOutput()
        launched = nil
        runTask?.cancel()
        wakeRetry()
        defer { updateContinuation.yield(.state(.stopped)) }
        guard let connection else { return }
        self.connection = nil
        try await shutDown(connection, timeout: timeout)
    }

    /// Asks the core on `connection` to shut down and waits until it is gone.
    /// A core that does not answer must not hold the caller: closing the
    /// connection after `timeout` fails the call with `.disconnected`.
    private func shutDown(_ connection: HubConnection, timeout: Duration) async throws {
        let watchdog = Task {
            try await Task.sleep(for: timeout)
            await connection.close()
        }
        defer { watchdog.cancel() }
        do {
            try await connection.shutdown()
        } catch HubError.disconnected {
            // The core closed the socket as it exited, or the watchdog fired.
        }
        // The core is gone when the connection has ended and `hubd.sock` no
        // longer exists (§1); wait for both, within the same bound.
        await connection.waitUntilClosed()
        let socket = configuration.socketPath()
        let deadline = ContinuousClock.now + timeout
        while FileManager.default.fileExists(atPath: socket), ContinuousClock.now < deadline {
            try? await Task.sleep(for: .milliseconds(50))
        }
    }

    /// Skips the wait before the next attempt (a "Try again" button), and
    /// allows starting the core again after it failed to start.
    public func retry() {
        launchBlocked = nil
        lastLaunch = nil
        wakeRetry()
    }

    public func call(_ method: String, params: JSONValue?, progress: (@Sendable (JSONValue) -> Void)?) async throws -> JSONValue {
        guard let connection else { throw HubError.notConnected }
        return try await connection.call(method, params: params, progress: progress)
    }

    // MARK: - The connection loop

    private func run() async {
        var failures = 0
        while !Task.isCancelled && !stopped {
            updateContinuation.yield(.state(.connecting))
            let reason: UnreachableReason
            do {
                let connection = try HubConnection.open(path: configuration.socketPath())
                do {
                    let (snapshot, thresholds) = try await resync(connection)
                    if stopped { await connection.close(); break }
                    // Read before `launched` is cleared below: whether this
                    // core is one this client started.
                    let replacing = replacesOlderCore(snapshot.hello.hub, startedHere: launched != nil)
                    let held = Set(snapshot.devices.filter { $0.busy != nil }.map(\.id))
                    if replacing, held.isEmpty {
                        await makeWay(connection)
                        continue
                    }
                    self.connection = connection
                    failures = 0
                    lastLaunch = nil
                    launchBlocked = nil
                    launched?.discardOutput()
                    launched = nil
                    Self.log.notice("connected to hub \(snapshot.hello.hub.id, privacy: .public): \(snapshot.devices.count) devices")
                    updateContinuation.yield(.synced(snapshot))
                    updateContinuation.yield(.state(.connected(snapshot.hello)))
                    let madeWay = await forwardEvents(of: connection, after: thresholds,
                                                      makingWayOnceFree: replacing ? held : nil)
                    self.connection = nil
                    if stopped { break }
                    // The core left because this client asked it to: start
                    // this client's own now, with no wait and no failure shown.
                    if madeWay { continue }
                    reason = .failed(await connection.closeReason ?? "the connection closed")
                } catch {
                    await connection.close()
                    if case HubError.incompatibleProtocol(let hub, _) = error {
                        reason = .incompatible(hubProtocol: hub)
                    } else {
                        reason = .failed(String(describing: error))
                    }
                }
            } catch UnixSocketStream.SocketError.nobodyListening {
                reason = nobodyListening(&failures)
            } catch UnixSocketStream.SocketError.pathTooLong {
                // The core refuses such a home and says so; starting it is how
                // the owner gets that sentence.
                reason = nobodyListening(&failures)
            } catch {
                reason = .failed(String(describing: error))
            }
            if stopped || Task.isCancelled { break }
            Self.log.notice("hub unreachable: \(String(describing: reason), privacy: .public)")
            let delays = configuration.retryDelays
            let delay = delays.isEmpty ? .seconds(1) : delays[min(failures, delays.count - 1)]
            failures += 1
            updateContinuation.yield(.state(.unreachable(reason, retryAt: Date().addingTimeInterval(delay.timeInterval))))
            await sleep(delay)
        }
    }

    /// Nothing listens: start the core, or say why it is not running.
    private func nobodyListening(_ failures: inout Int) -> UnreachableReason {
        guard let launcher = configuration.launcher else { return .notRunning }
        // A core this client started has exited: 0 means another core won the
        // home (keep connecting); anything else is a failure to report.
        if let core = launched, let status = core.exitStatus {
            launched = nil
            if status != 0 {
                let line = core.failureLine() ?? "pocket-hubd exited with status \(status)."
                core.discardOutput()
                Self.log.error("the core exited before answering: \(line, privacy: .public)")
                launchBlocked = .coreFailedToStart(line)
            } else {
                core.discardOutput()
            }
        }
        if let blocked = launchBlocked { return blocked }
        if let lastLaunch, ContinuousClock.now - lastLaunch < configuration.launchGrace {
            // Still coming up, or the launch could not happen; say which.
            return launchReason
        }
        updateContinuation.yield(.state(.startingCore))
        lastLaunch = .now
        let outcome = launcher.launch()
        switch outcome {
        case .started(let core):
            Self.log.notice("started the core \(core.executable, privacy: .public) (pid \(core.pid))")
            launched = core
            failures = 0
            launchReason = .notRunning
        case .notFound:
            Self.log.notice("no pocket-hubd to start")
            launchReason = .coreNotFound
        case .failed(let message):
            Self.log.error("starting the core failed: \(message, privacy: .public)")
            launchReason = .coreFailedToStart(message)
            launchBlocked = launchReason
        }
        return launchReason
    }

    /// The hello, the subscription and the reads that rebuild a client's state.
    /// Subscribing before reading means no change can fall between the read and
    /// the first event; the returned wire positions let events that Hub sent
    /// before an answer (and that the answer already includes) be dropped.
    private func resync(_ connection: HubConnection) async throws -> (HubSnapshot, Thresholds) {
        let identity = configuration.identity
        // A socket that accepts and never answers (a hung core) is closed after
        // `helloTimeout`, which fails the hello with `.disconnected`.
        let watchdog = Task { [helloTimeout = configuration.helloTimeout] in
            try await Task.sleep(for: helloTimeout)
            await connection.close()
        }
        let hello: HelloResult
        do {
            hello = try await connection.hello(client: identity)
            watchdog.cancel()
        } catch {
            watchdog.cancel()
            throw error
        }
        guard hello.protocol == Self.protocolVersion else {
            throw HubError.incompatibleProtocol(hub: hello.protocol, client: Self.protocolVersion)
        }
        try await connection.subscribe()
        let (statusValue, statusSequence) = try await connection.callSequenced("hub.status")
        let (devicesValue, devicesSequence) = try await connection.callSequenced("devices.list")
        let (pairingValue, pairingSequence) = try await connection.callSequenced("pairing.list")
        let status: HubStatus = try statusValue.decoded()
        let devices: [Device] = try devicesValue["devices"].orThrow("devices").decoded()
        let pairRequests: [PairRequest] = try pairingValue["requests"].orThrow("requests").decoded()
        var daemons: [Daemon]?
        var daemonsSequence: UInt64 = 0
        if let (value, sequence) = try? await connection.callSequenced("daemons.list") {
            daemons = try? value["daemons"].orThrow("daemons").decoded()
            daemonsSequence = sequence
        }
        let bun = try? await connection.bunRuntime()
        let snapshot = HubSnapshot(hello: hello, status: status, devices: devices, pairRequests: pairRequests, daemons: daemons, bun: bun)
        return (snapshot, Thresholds(devices: devicesSequence, pairing: pairingSequence, daemons: daemonsSequence, hub: statusSequence))
    }

    struct Thresholds {
        var devices: UInt64
        var pairing: UInt64
        var daemons: UInt64
        var hub: UInt64
    }

    /// Forwards the connection's events until it ends. With `held`, the core
    /// is an older one to replace and those are the devices it held when this
    /// client connected: the events keep that set current, and once it is
    /// empty the core is asked to make way and this returns true.
    private func forwardEvents(of connection: HubConnection, after thresholds: Thresholds,
                               makingWayOnceFree held: Set<String>?) async -> Bool
    {
        var held = held
        for await item in connection.events {
            let threshold: UInt64 = switch item.event.family {
            case .devices: thresholds.devices
            case .pairing: thresholds.pairing
            case .daemons: thresholds.daemons
            case .hub: thresholds.hub
            case .other: 0
            }
            // A state event that preceded the answer is already in the snapshot,
            // and applying it after the snapshot would roll state back. Log lines
            // are not state: they are kept whatever their position.
            switch item.event {
            case .deviceLog, .daemonLog: break
            default: if item.sequence < threshold { continue }
            }
            updateContinuation.yield(.event(item.event))
            guard var devices = held else { continue }
            switch item.event {
            case .deviceChanged(let device):
                if device.busy == nil { devices.remove(device.id) } else { devices.insert(device.id) }
            case .deviceRemoved(let id):
                devices.remove(id)
            default:
                break
            }
            held = devices
            // A client that is stopping (the app quits) leaves the core as it is.
            if devices.isEmpty, !stopped {
                self.connection = nil
                await makeWay(connection)
                return true
            }
        }
        return false
    }

    // MARK: - An older core makes way (CONTROL-API.md §1)

    /// Whether the core that answered is to make way for this client's own.
    /// The core outlives the app (it keeps devices connected when the app
    /// quits), so an update of the app does not reach a core that was
    /// already running: the updated app replaces it. Only a client that
    /// starts cores does, only a core it found running (one it started is its
    /// own, whatever it says), only one older than itself (a newer one stays:
    /// two builds side by side do not take turns replacing each other), and
    /// once: a core that did not go when asked is not asked in a loop.
    private func replacesOlderCore(_ hub: HubIdentity, startedHere: Bool) -> Bool {
        guard configuration.launcher != nil, !startedHere, !replacedOlderCore,
              let core = ReleaseVersion(hub.version),
              let mine = ReleaseVersion(configuration.identity.version)
        else { return false }
        return core < mine
    }

    /// Asks an older core to shut down, so that the connection loop finds
    /// nobody listening and starts this client's own. A device operation in
    /// progress would end with it, so this waits until no device is held
    /// (`forwardEvents`); a client's own calls end when it disconnects anyway
    /// (§1), so a held device belongs to another client.
    private func makeWay(_ connection: HubConnection) async {
        replacedOlderCore = true
        Self.log.notice("the running core is older than this app: asking it to shut down and starting this app's own")
        try? await shutDown(connection, timeout: .seconds(3))
        await connection.close()
        lastLaunch = nil
    }

    /// Waits `duration`, or less when `retry()` or `stop()` wakes it. Each wait
    /// has its own token so a timer of an earlier wait cannot cut a later one short.
    private func sleep(_ duration: Duration) async {
        sleepToken &+= 1
        let token = sleepToken
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            retryNow = continuation
            Task {
                try? await Task.sleep(for: duration)
                self.wakeRetry(token: token)
            }
        }
    }

    private func wakeRetry(token: UInt64? = nil) {
        if let token, token != sleepToken { return }
        retryNow?.resume()
        retryNow = nil
    }
}

extension Duration {
    var timeInterval: TimeInterval {
        let (seconds, attoseconds) = components
        return TimeInterval(seconds) + TimeInterval(attoseconds) / 1e18
    }
}
