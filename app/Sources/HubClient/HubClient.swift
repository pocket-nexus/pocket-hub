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
    /// The core speaks a lower protocol and this client has no core of its own
    /// to start in its place (no launcher), so it replaces nothing.
    case incompatible(hubProtocol: Int)
    /// The core speaks a higher protocol: a newer Pocket Hub runs on this Mac.
    /// It is never replaced (CONTROL-API.md §1, "Starting and stopping the
    /// core"); this client keeps looking and connects once it is gone.
    case newerVersionRunning
    /// An older core was asked to stop and was not gone within
    /// `replaceWait`. Nothing more is done (no signal, no removing the
    /// socket) until `retry()`.
    case olderVersionDidNotStop
    /// Anything else: the socket refused us, the hello timed out, a resync call failed.
    case failed(String)
}

public enum HubConnectionState: Sendable, Equatable {
    case idle
    case connecting
    /// The core was not running and is being started.
    case startingCore
    /// A core of an earlier version was found: it is being asked to stop, and
    /// this client's own core starts after it.
    case replacingCore
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
    /// nil when `services.list` failed; the devices still work.
    public var services: [Service]?
    /// nil when `services.runtime` failed.
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
    public static let protocolVersion = 2
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
        /// How long an older core has to be gone after `hub.shutdown` before
        /// this client says it did not stop: the command line's wait, which
        /// fits a core's links' close drain and its services' stop
        /// (5 s, then SIGTERM, 5 s, then SIGKILL).
        public var replaceWait: Duration

        public init(identity: ClientIdentity,
                    socketPath: @escaping @Sendable () -> String = { HubPaths.current().socketPath() },
                    launcher: CoreLauncher? = nil,
                    retryDelays: [Duration] = [.milliseconds(250), .milliseconds(500), .seconds(1), .seconds(2), .seconds(3)],
                    helloTimeout: Duration = .seconds(5),
                    launchGrace: Duration = .seconds(15),
                    replaceWait: Duration = .seconds(20))
        {
            self.identity = identity
            self.socketPath = socketPath
            self.launcher = launcher
            self.retryDelays = retryDelays
            self.helloTimeout = helloTimeout
            self.launchGrace = launchGrace
            self.replaceWait = replaceWait
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
    /// Set when an older core did not stop within `replaceWait`: it is not
    /// asked again until the owner asks (`retry()`).
    private var replaceBlocked = false
    /// The version of the core this client starts (`CoreLauncher.version`),
    /// asked once.
    private var ownCoreVersion: SemanticVersion??

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

    /// Asks the core to shut down, then stops ("Quit and Stop"). Reconnection
    /// is disabled first, so the client cannot start the core again while it
    /// exits. Whatever core answers is stopped, of this version or another:
    /// without a synced connection the client meets it through the two frozen
    /// methods (CONTROL-API.md §1, Hello). Waits at most `timeout`, so a core
    /// that does not answer cannot hold the quitting app.
    public func shutdownHub(timeout: Duration = .seconds(3)) async throws {
        stopped = true
        launched?.discardOutput()
        launched = nil
        runTask?.cancel()
        wakeRetry()
        defer { updateContinuation.yield(.state(.stopped)) }
        let current = connection
        connection = nil
        _ = await stopCore(introduced: current, wait: timeout)
    }

    /// Skips the wait before the next attempt (a "Try again" button), and
    /// allows starting the core again after it failed to start, and asking an
    /// older core to stop again after it did not.
    public func retry() {
        launchBlocked = nil
        lastLaunch = nil
        replaceBlocked = false
        wakeRetry()
    }

    public func call(_ method: String, params: JSONValue?, progress: (@Sendable (JSONValue) -> Void)?) async throws -> JSONValue {
        guard let connection else { throw HubError.notConnected }
        return try await connection.call(method, params: params, progress: progress)
    }

    // MARK: - The connection loop

    private func run() async {
        var failures = 0
        attempts: while !Task.isCancelled && !stopped {
            updateContinuation.yield(.state(.connecting))
            let reason: UnreachableReason
            do {
                let connection = try HubConnection.open(path: configuration.socketPath())
                do {
                    switch try await meet(connection) {
                    case .use(let hello):
                        let (snapshot, thresholds) = try await resync(connection, hello: hello)
                        if stopped { await connection.close(); break attempts }
                        self.connection = connection
                        failures = 0
                        lastLaunch = nil
                        launchBlocked = nil
                        launched?.discardOutput()
                        launched = nil
                        Self.log.notice("connected to hub \(snapshot.hello.hub.id, privacy: .public): \(snapshot.devices.count) devices")
                        updateContinuation.yield(.synced(snapshot))
                        updateContinuation.yield(.state(.connected(snapshot.hello)))
                        await forwardEvents(of: connection, after: thresholds)
                        self.connection = nil
                        if stopped { break attempts }
                        reason = .failed(await connection.closeReason ?? "the connection closed")
                    case .replace(let introduced, let coreProtocol):
                        if !introduced { await connection.close() }
                        if await replace(introduced ? connection : nil, coreProtocol: coreProtocol) {
                            // Nobody listens now: the next attempt starts this
                            // client's own core, at once.
                            failures = 0
                            continue attempts
                        }
                        reason = .olderVersionDidNotStop
                    case .standAside(let why):
                        await connection.close()
                        reason = why
                    }
                } catch {
                    await connection.close()
                    reason = .failed(String(describing: error))
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

    // MARK: - Meeting the core (CONTROL-API.md §1, Hello; "Starting and stopping the core")

    private enum Meeting {
        /// A core of this client's protocol, to be used: the hello is said.
        case use(HelloResult)
        /// A core of an earlier version, to be replaced. `introduced`: the
        /// connection already said hello in the core's protocol (the same as
        /// this client's), and asks it to stop itself.
        case replace(introduced: Bool, coreProtocol: Int)
        /// A core this client neither uses nor replaces.
        case standAside(UnreachableReason)
    }

    /// Says hello in this client's protocol and decides what to do with the
    /// core that answers. A lower protocol, or the same one with a lower
    /// `hub.version` than this client's own core (SemVer precedence), is an
    /// earlier version: replaced when this client can start its own core. A
    /// higher protocol is a newer Pocket Hub: never replaced. The same protocol
    /// with an equal or higher version is used as it is.
    private func meet(_ connection: HubConnection) async throws -> Meeting {
        let coreProtocol: Int
        switch try await introduce(connection, protocol: Self.protocolVersion) {
        case .accepted(let hello) where hello.protocol == Self.protocolVersion:
            if isOlderThanOwnCore(hello.hub.version), cannotReplace(coreProtocol: hello.protocol) == nil {
                return .replace(introduced: true, coreProtocol: hello.protocol)
            }
            return .use(hello)
        case .accepted(let hello):
            // Accepted, yet answered in another protocol: its own decides.
            coreProtocol = hello.protocol
        case .refused(let theirs, _):
            coreProtocol = theirs
        }
        if coreProtocol > Self.protocolVersion { return .standAside(.newerVersionRunning) }
        if coreProtocol == Self.protocolVersion {
            return .standAside(.failed("the core refused protocol \(coreProtocol) and says it speaks it"))
        }
        if let reason = cannotReplace(coreProtocol: coreProtocol) { return .standAside(reason) }
        return .replace(introduced: false, coreProtocol: coreProtocol)
    }

    /// `hub.hello` in `version`, bounded: a socket that accepts and never
    /// answers (a hung core) is closed after `helloTimeout`, which fails the
    /// hello with `.disconnected`.
    private func introduce(_ connection: HubConnection, protocol version: Int) async throws -> HelloAnswer {
        let watchdog = Task { [helloTimeout = configuration.helloTimeout] in
            try await Task.sleep(for: helloTimeout)
            await connection.close()
        }
        defer { watchdog.cancel() }
        return try await connection.introduce(protocol: version, client: configuration.identity)
    }

    /// Why an earlier core cannot be replaced now; nil when it can.
    private func cannotReplace(coreProtocol: Int) -> UnreachableReason? {
        guard let launcher = configuration.launcher else { return .incompatible(hubProtocol: coreProtocol) }
        // Stopping it without a core to start after it would leave nothing.
        guard launcher.locate() != nil else { return .coreNotFound }
        if replaceBlocked { return .olderVersionDidNotStop }
        return nil
    }

    /// Whether `version` orders below the version of the core this client
    /// starts. False when either is unknown or not a SemVer version.
    private func isOlderThanOwnCore(_ version: String) -> Bool {
        guard let launcher = configuration.launcher else { return false }
        if ownCoreVersion == nil {
            ownCoreVersion = .some(launcher.version().flatMap(SemanticVersion.init))
        }
        guard let own = ownCoreVersion ?? nil, let found = SemanticVersion(version) else { return false }
        return found < own
    }

    /// Asks an earlier core to stop and waits until it is gone. True when it
    /// is (or nobody answers any more); false after `replaceWait`, and then
    /// nothing more is done until `retry()`.
    private func replace(_ introduced: HubConnection?, coreProtocol: Int) async -> Bool {
        Self.log.notice("replacing the core of an earlier version (protocol \(coreProtocol))")
        updateContinuation.yield(.state(.replacingCore))
        switch await stopCore(introduced: introduced, firstProtocol: coreProtocol, wait: configuration.replaceWait) {
        case .gone, .nobody:
            lastLaunch = nil
            launchBlocked = nil
            return true
        case .notGone:
            Self.log.error("the core of an earlier version did not stop within \(self.configuration.replaceWait)")
            replaceBlocked = true
            return false
        }
    }

    private enum StopOutcome {
        case gone
        case nobody
        case notGone
    }

    /// Stops the core on the socket, of any version, through the two methods
    /// every protocol keeps: `hub.hello` in the core's own protocol, then
    /// `hub.shutdown` (CONTROL-API.md §1, Hello). Nothing else is called in
    /// another version's dialect. `introduced`: a connection that already said
    /// hello. The core is gone when the connection has ended and `hubd.sock`
    /// no longer exists (§1); both are waited for within `wait`. No signal is
    /// sent and the socket is never removed here.
    private func stopCore(introduced: HubConnection?, firstProtocol: Int = HubClient.protocolVersion, wait: Duration) async -> StopOutcome {
        let deadline = ContinuousClock.now + wait
        let socket = configuration.socketPath()
        var connection = introduced
        if connection == nil {
            var version = firstProtocol
            // At most two hellos: in `version`, then in the protocol its refusal names.
            for _ in 0..<2 where connection == nil {
                let attempt: HubConnection
                do {
                    attempt = try HubConnection.open(path: socket)
                } catch UnixSocketStream.SocketError.nobodyListening {
                    return .nobody
                } catch {
                    return .notGone
                }
                switch try? await introduce(attempt, protocol: version) {
                case .accepted?:
                    connection = attempt
                case .refused(let theirs, _)? where theirs != version:
                    await attempt.close()
                    version = theirs
                default:
                    await attempt.close()
                    return .notGone
                }
            }
        }
        guard let connection else { return .notGone }
        let watchdog = Task {
            try await Task.sleep(until: deadline)
            await connection.close()
        }
        defer { watchdog.cancel() }
        do {
            try await connection.shutdown()
        } catch HubError.disconnected {
            // The core closed the socket as it exited, or the wait ran out.
        } catch {
            await connection.close()
            return .notGone
        }
        await connection.waitUntilClosed()
        while FileManager.default.fileExists(atPath: socket), ContinuousClock.now < deadline {
            try? await Task.sleep(for: .milliseconds(20))
        }
        return FileManager.default.fileExists(atPath: socket) ? .notGone : .gone
    }

    /// The subscription and the reads that rebuild a client's state, after
    /// the hello. Subscribing before reading means no change can fall between
    /// the read and the first event; the returned wire positions let events
    /// that Hub sent before an answer (and that the answer already includes)
    /// be dropped.
    private func resync(_ connection: HubConnection, hello: HelloResult) async throws -> (HubSnapshot, Thresholds) {
        try await connection.subscribe()
        let (statusValue, statusSequence) = try await connection.callSequenced("hub.status")
        let (devicesValue, devicesSequence) = try await connection.callSequenced("devices.list")
        let (pairingValue, pairingSequence) = try await connection.callSequenced("pairing.list")
        let status: HubStatus = try statusValue.decoded()
        let devices: [Device] = try devicesValue["devices"].orThrow("devices").decoded()
        let pairRequests: [PairRequest] = try pairingValue["requests"].orThrow("requests").decoded()
        var services: [Service]?
        var servicesSequence: UInt64 = 0
        if let (value, sequence) = try? await connection.callSequenced("services.list") {
            services = try? value["services"].orThrow("services").decoded()
            servicesSequence = sequence
        }
        let bun = try? await connection.bunRuntime()
        let snapshot = HubSnapshot(hello: hello, status: status, devices: devices, pairRequests: pairRequests, services: services, bun: bun)
        return (snapshot, Thresholds(devices: devicesSequence, pairing: pairingSequence, services: servicesSequence, hub: statusSequence))
    }

    struct Thresholds {
        var devices: UInt64
        var pairing: UInt64
        var services: UInt64
        var hub: UInt64
    }

    private func forwardEvents(of connection: HubConnection, after thresholds: Thresholds) async {
        for await item in connection.events {
            let threshold: UInt64 = switch item.event.family {
            case .devices: thresholds.devices
            case .pairing: thresholds.pairing
            case .services: thresholds.services
            case .hub: thresholds.hub
            case .other: 0
            }
            // A state event that preceded the answer is already in the snapshot,
            // and applying it after the snapshot would roll state back. Log lines
            // are not state: they are kept whatever their position.
            switch item.event {
            case .deviceLog, .serviceLog: break
            default: if item.sequence < threshold { continue }
            }
            updateContinuation.yield(.event(item.event))
        }
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
