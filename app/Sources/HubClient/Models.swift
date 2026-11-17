import Foundation

// The objects of CONTROL-API.md §2 and the results of §3.
//
// Decoding follows the contract's shapes: a member typed "X or null" is an
// Optional that accepts both `null` and absence; a member the contract marks
// with `?` is an Optional. String sets the contract enumerates are open
// structs, so a value added by a newer core decodes instead of failing the
// whole Device. `RuntimeStatus` is the device's own record and is read
// leniently for the same reason.

/// Epoch milliseconds, the contract's time unit.
public typealias EpochMilliseconds = Int64

extension EpochMilliseconds {
    public var date: Date { Date(timeIntervalSince1970: TimeInterval(self) / 1000) }
}

/// A string-valued enumeration that keeps values it does not know.
public protocol OpenStringEnum: RawRepresentable, Codable, Hashable, Sendable where RawValue == String {
    init(rawValue: String)
}

extension OpenStringEnum {
    public init(from decoder: any Decoder) throws {
        self.init(rawValue: try decoder.singleValueContainer().decode(String.self))
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

// MARK: - Device

/// A device family (`kind` of a Device and of a PairRequest). Empty for a
/// family Hub does not know.
public struct DeviceKind: OpenStringEnum {
    public var rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    /// A Nintendo 3DS.
    public static let nintendo3ds = DeviceKind(rawValue: "3ds")
    /// An M5Stack Tab5.
    public static let tab5 = DeviceKind(rawValue: "tab5")
}

/// Something Hub can ask of a device of some families and not of others
/// (`supports` of a Device).
public struct DeviceOperation: OpenStringEnum {
    public var rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    /// `devices.push` and `store.install`: an applet sent from the computer.
    public static let push = DeviceOperation(rawValue: "push")
    /// `devices.screenshot`.
    public static let screenshot = DeviceOperation(rawValue: "screenshot")
    /// `devices.logs`.
    public static let logs = DeviceOperation(rawValue: "logs")
    /// `devices.installProgram`: Hub installs the device's own software.
    public static let program = DeviceOperation(rawValue: "program")

    public static let all: [DeviceOperation] = [.push, .screenshot, .logs, .program]
}

/// One paired device, linked or not.
public struct Device: Codable, Sendable, Hashable, Identifiable {
    public var id: String
    /// The device family: `3ds`, `tab5`, or empty for one Hub does not know.
    public var kind: DeviceKind
    /// What Hub can ask of a device of this family, linked or not. **A UI
    /// offers an operation only when this names it**; Hub refuses the others
    /// (`link` `unsupported`).
    public var supports: [DeviceOperation]
    /// From the last handshake; empty before the first.
    public var name: String
    public var model: String
    public var target: String
    public var hostAbi: Int
    public var lastAddress: String?
    public var lastSeen: EpochMilliseconds?
    /// A setup wrote a key and could not read it back.
    public var setupUnconfirmed: Bool
    /// Two consoles run one card image.
    public var duplicateId: Bool
    public var link: DeviceLink?
    public var status: RuntimeStatus?
    public var apps: [InstalledApp]?
    public var busy: DeviceBusy?
    /// The device's own program: its version, the newest release Hub knows,
    /// and whether that one is newer.
    public var program: DeviceProgram
    public var channels: [AppletChannel]

    public init(id: String, kind: DeviceKind = .nintendo3ds, supports: [DeviceOperation] = DeviceOperation.all,
                name: String = "", model: String = "", target: String = "",
                hostAbi: Int = 0, lastAddress: String? = nil, lastSeen: EpochMilliseconds? = nil,
                setupUnconfirmed: Bool = false, duplicateId: Bool = false, link: DeviceLink? = nil,
                status: RuntimeStatus? = nil, apps: [InstalledApp]? = nil, busy: DeviceBusy? = nil,
                program: DeviceProgram = DeviceProgram(), channels: [AppletChannel] = [])
    {
        self.id = id
        self.kind = kind
        self.supports = supports
        self.name = name
        self.model = model
        self.target = target
        self.hostAbi = hostAbi
        self.lastAddress = lastAddress
        self.lastSeen = lastSeen
        self.setupUnconfirmed = setupUnconfirmed
        self.duplicateId = duplicateId
        self.link = link
        self.status = status
        self.apps = apps
        self.busy = busy
        self.program = program
        self.channels = channels
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = try c.decode(DeviceKind.self, forKey: .kind)
        // The contract always sends `supports`; a core from before it existed
        // knew the 3DS alone, which takes every operation.
        supports = try c.decodeIfPresent([DeviceOperation].self, forKey: .supports) ?? DeviceOperation.all
        name = try c.decode(String.self, forKey: .name)
        model = try c.decode(String.self, forKey: .model)
        target = try c.decode(String.self, forKey: .target)
        hostAbi = try c.decode(Int.self, forKey: .hostAbi)
        lastAddress = try c.decodeIfPresent(String.self, forKey: .lastAddress)
        lastSeen = try c.decodeIfPresent(EpochMilliseconds.self, forKey: .lastSeen)
        setupUnconfirmed = try c.decode(Bool.self, forKey: .setupUnconfirmed)
        duplicateId = try c.decode(Bool.self, forKey: .duplicateId)
        link = try c.decodeIfPresent(DeviceLink.self, forKey: .link)
        status = try c.decodeIfPresent(RuntimeStatus.self, forKey: .status)
        apps = try c.decodeIfPresent([InstalledApp].self, forKey: .apps)
        busy = try c.decodeIfPresent(DeviceBusy.self, forKey: .busy)
        // The contract always sends `program`; a core from before it existed
        // means "nothing known", which offers no update, rather than an
        // unreadable Device.
        program = try c.decodeIfPresent(DeviceProgram.self, forKey: .program) ?? DeviceProgram()
        channels = try c.decode([AppletChannel].self, forKey: .channels)
    }

    private enum CodingKeys: String, CodingKey {
        case id, kind, supports, name, model, target, hostAbi, lastAddress, lastSeen, setupUnconfirmed, duplicateId
        case link, status, apps, busy, program, channels
    }

    public var isLinked: Bool { link != nil }

    /// Whether Hub can ask `operation` of this device (`supports`).
    public func supports(_ operation: DeviceOperation) -> Bool { supports.contains(operation) }
}

// MARK: - Pairing by a code

/// One device that asks to be paired by the code it shows, and waits for
/// that code (§2 PairRequest, §3 Pairing).
public struct PairRequest: Codable, Sendable, Hashable, Identifiable {
    /// The device id the request names. Nothing has proved it yet.
    public var device: String
    /// The family of its target; empty for one Hub does not know.
    public var kind: DeviceKind
    public var name: String
    public var model: String
    public var target: String
    /// Where the request came from.
    public var address: String
    public var since: EpochMilliseconds
    /// When Hub closes the request: the device keeps its code as long.
    public var expires: EpochMilliseconds

    public init(device: String, kind: DeviceKind = .tab5, name: String = "", model: String = "", target: String = "",
                address: String = "", since: EpochMilliseconds = 0, expires: EpochMilliseconds = 0)
    {
        self.device = device
        self.kind = kind
        self.name = name
        self.model = model
        self.target = target
        self.address = address
        self.since = since
        self.expires = expires
    }

    public var id: String { device }
}

/// `program` of a Device: `{version, latest, update}`.
///
/// `version` is what the device reports for its own program (the `home` row
/// of its apps), nil before its first report. `latest` is the newest release
/// Hub knows, nil until Hub has looked. `update` is Hub's verdict that
/// `latest` orders above `version`; the client does not compare versions itself.
public struct DeviceProgram: Codable, Sendable, Hashable {
    public var version: String?
    public var latest: String?
    public var update: Bool

    public init(version: String? = nil, latest: String? = nil, update: Bool = false) {
        self.version = version
        self.latest = latest
        self.update = update
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decodeIfPresent(String.self, forKey: .version)
        latest = try c.decodeIfPresent(String.self, forKey: .latest)
        update = try c.decodeIfPresent(Bool.self, forKey: .update) ?? false
    }

    private enum CodingKeys: String, CodingKey { case version, latest, update }
}

public struct Carrier: OpenStringEnum {
    public var rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public static let tcp = Carrier(rawValue: "tcp")
    public static let usb = Carrier(rawValue: "usb")
    public static let uart = Carrier(rawValue: "uart")
    public static let ble = Carrier(rawValue: "ble")
}

/// The live link of a device. For display only: what runs over it does not
/// depend on the carrier.
public struct DeviceLink: Codable, Sendable, Hashable {
    public var carrier: Carrier
    public var peer: String
    public var since: EpochMilliseconds
    public var payloadLimit: Int
    public var features: [String]

    public init(carrier: Carrier, peer: String, since: EpochMilliseconds, payloadLimit: Int, features: [String]) {
        self.carrier = carrier
        self.peer = peer
        self.since = since
        self.payloadLimit = payloadLimit
        self.features = features
    }
}

/// The device's last `runtime.status` (link specification §6.2). The device
/// writes it, so every member is optional: a device that leaves one out must
/// not make its whole Device unreadable.
public struct RuntimeStatus: Codable, Sendable, Hashable {
    public var phase: String?
    public var target: String?
    public var hostAbi: Int?
    public var slot: String?
    public var address: String?
    public var generation: Int?
    public var active: String?
    public var lastGood: String?
    public var running: String?
    public var frame: Int?
    public var app: RunningApp?

    public init(phase: String? = nil, target: String? = nil, hostAbi: Int? = nil, slot: String? = nil,
                address: String? = nil, generation: Int? = nil, active: String? = nil, lastGood: String? = nil,
                running: String? = nil, frame: Int? = nil, app: RunningApp? = nil)
    {
        self.phase = phase
        self.target = target
        self.hostAbi = hostAbi
        self.slot = slot
        self.address = address
        self.generation = generation
        self.active = active
        self.lastGood = lastGood
        self.running = running
        self.frame = frame
        self.app = app
    }
}

/// `{id, output, title}`: the running applet in `runtime.status`, and the
/// pushed app in a `devices.push` result.
public struct RunningApp: Codable, Sendable, Hashable {
    public var id: String
    public var output: String
    public var title: String

    public init(id: String, output: String, title: String) {
        self.id = id
        self.output = output
        self.title = title
    }
}

/// An installed applet (`runtime.apps` row).
public struct InstalledApp: Codable, Sendable, Hashable, Identifiable {
    public var id: String
    public var output: String
    public var version: String
    public var hash: String
    /// True for the launcher (the embedded guest).
    public var home: Bool?

    public init(id: String, output: String, version: String, hash: String, home: Bool? = nil) {
        self.id = id
        self.output = output
        self.version = version
        self.hash = hash
        self.home = home
    }

    public var isHome: Bool { home ?? false }
}

/// Who holds the device mutex: `{kind, name, method}`, the kind and name the
/// holding client gave in its hello and the method that holds the device.
/// The same object in a Device's `busy` and in the error `busy` (§1).
public struct MutexHolder: Codable, Sendable, Hashable {
    public var kind: String
    public var name: String
    public var method: String

    public init(kind: String, name: String, method: String) {
        self.kind = kind
        self.name = name
        self.method = method
    }
}

/// `{holder, since, launching}` while a state-changing operation holds the device.
public struct DeviceBusy: Codable, Sendable, Hashable {
    public var holder: MutexHolder
    public var since: EpochMilliseconds
    /// The device is restarting into a program it was told to start (up to 60 s).
    public var launching: Bool

    public init(holder: MutexHolder, since: EpochMilliseconds, launching: Bool = false) {
        self.holder = holder
        self.since = since
        self.launching = launching
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        holder = try container.decode(MutexHolder.self, forKey: .holder)
        since = try container.decode(EpochMilliseconds.self, forKey: .since)
        // The contract always sends `launching`; a core that leaves it out
        // means "not launching" rather than an unreadable Device.
        launching = try container.decodeIfPresent(Bool.self, forKey: .launching) ?? false
    }

    private enum CodingKeys: String, CodingKey { case holder, since, launching }
}

public struct AppletChannel: Codable, Sendable, Hashable {
    public var app: String
    public var generation: Int
    public var since: EpochMilliseconds

    public init(app: String, generation: Int, since: EpochMilliseconds) {
        self.app = app
        self.generation = generation
        self.since = since
    }
}

// MARK: - Daemon

public struct DaemonState: OpenStringEnum {
    public var rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public static let stopped = DaemonState(rawValue: "stopped")
    public static let starting = DaemonState(rawValue: "starting")
    public static let running = DaemonState(rawValue: "running")
    public static let stopping = DaemonState(rawValue: "stopping")
    public static let failed = DaemonState(rawValue: "failed")
}

/// One installed daemon.
public struct Daemon: Codable, Sendable, Hashable, Identifiable {
    public var id: String
    public var version: String
    public var state: DaemonState
    public var pid: Int?
    public var since: EpochMilliseconds?
    public var channels: Int
    public var lastExit: DaemonExit?
    public var error: String?

    public init(id: String, version: String, state: DaemonState, pid: Int? = nil, since: EpochMilliseconds? = nil,
                channels: Int = 0, lastExit: DaemonExit? = nil, error: String? = nil)
    {
        self.id = id
        self.version = version
        self.state = state
        self.pid = pid
        self.since = since
        self.channels = channels
        self.lastExit = lastExit
        self.error = error
    }
}

/// `{code, signal, at}`: `code` an integer or null, `signal` a name such as `SIGTERM` or null.
public struct DaemonExit: Codable, Sendable, Hashable {
    public var code: Int?
    public var signal: String?
    public var at: EpochMilliseconds

    public init(code: Int?, signal: String?, at: EpochMilliseconds) {
        self.code = code
        self.signal = signal
        self.at = at
    }
}

// MARK: - Hub

public enum ClientKind: String, Codable, Sendable {
    case gui, cli, remote
}

/// `client` in `hub.hello`.
public struct ClientIdentity: Codable, Sendable, Hashable {
    public var kind: ClientKind
    public var name: String
    public var version: String

    public init(kind: ClientKind, name: String, version: String) {
        self.kind = kind
        self.name = name
        self.version = version
    }
}

/// `{id, name, version}`: Hub's identity.
public struct HubIdentity: Codable, Sendable, Hashable {
    public var id: String
    public var name: String
    public var version: String

    public init(id: String, name: String, version: String) {
        self.id = id
        self.name = name
        self.version = version
    }
}

public struct HelloResult: Codable, Sendable, Hashable {
    public var `protocol`: Int
    public var hub: HubIdentity
}

public struct HubStatus: Codable, Sendable, Hashable {
    public struct CarrierInfo: Codable, Sendable, Hashable {
        public var kind: Carrier
        public var port: Int?
    }

    /// The Bun runtime the daemons run on. `version` is null while it is not installed.
    public struct Bun: Codable, Sendable, Hashable {
        public var version: String?
        public var installed: Bool
    }

    public var id: String
    public var name: String
    public var version: String
    /// Where Hub keeps its files.
    public var home: String
    public var startedAt: EpochMilliseconds
    public var carriers: [CarrierInfo]
    public var bun: Bun
}

// MARK: - Device results

public struct PushResult: Codable, Sendable, Hashable {
    public var app: RunningApp
    public var hash: String
    public var generation: Int
}

public struct LaunchResult: Codable, Sendable, Hashable {
    public var output: String
}

public struct ScreenshotResult: Codable, Sendable, Hashable {
    public var path: String
    public var width: Int
    public var height: Int
    public var frame: Int

    public init(path: String, width: Int, height: Int, frame: Int) {
        self.path = path
        self.width = width
        self.height = height
        self.frame = frame
    }
}

/// Progress of `devices.push` and `devices.installProgram`: `{phase, done, total}`.
/// `phase` is `sending` until the device's first event, then the device's own
/// (`receiving`, `received`, `staged`); `done`/`total` are bytes Hub wrote.
public struct TransferProgress: Codable, Sendable, Hashable {
    public var phase: String?
    public var done: Int64?
    public var total: Int64?

    public var fraction: Double? {
        guard let done, let total, total > 0 else { return nil }
        return min(1, max(0, Double(done) / Double(total)))
    }
}

/// The result of `devices.installProgram`: `{phase, name, version}`.
public struct InstallProgramResult: Codable, Sendable, Hashable {
    /// The device's reply: `launching` when it restarts into the new program
    /// (and links again by itself), `installed` when it was told not to.
    public var phase: String
    /// The name the program has on the device.
    public var name: String
    /// The release version installed; nil for a local file without one.
    public var version: String?

    public init(phase: String, name: String, version: String?) {
        self.phase = phase
        self.name = name
        self.version = version
    }
}

// MARK: - Store

/// One applet the store offers, at its newest listed version (§2 StoreApp).
public struct StoreApp: Codable, Sendable, Hashable, Identifiable {
    /// `{version, hash}` of the copy a device holds.
    public struct InstalledCopy: Codable, Sendable, Hashable {
        public var version: String
        public var hash: String

        public init(version: String, hash: String) {
            self.version = version
            self.hash = hash
        }
    }

    /// The applet's manifest id.
    public var id: String
    public var title: String
    public var summary: String
    public var author: String
    public var version: String
    /// Published with the store's official key.
    public var official: Bool
    /// The device targets the package has a variant for.
    public var targets: [String]
    /// The package's size in bytes.
    public var bytes: Int64
    /// Absolute path of the icon (a PNG) in Hub's cache; nil until Hub has fetched it.
    public var icon: String?
    /// Hub holds the verified package. A device's own store page lists the
    /// applet only once this is true; the computer can install it either way.
    public var cached: Bool
    /// Companion services the applet's worker reaches through Hub.
    public var services: [String]
    /// Host names outside the companion the applet's worker fetches.
    public var hosts: [String]
    /// Only in an answer for a device: absent (`.notAsked`) without one,
    /// `.none` when the device holds no copy.
    public var installed: DeviceCopy
    /// Only in an answer for a device: the store's version orders above the device's.
    public var update: Bool?

    /// `installed` keeps "absent" apart from "null".
    public enum DeviceCopy: Sendable, Hashable {
        /// The answer was not for a device.
        case notAsked
        /// The device holds no copy.
        case none
        case copy(InstalledCopy)

        public var copy: InstalledCopy? {
            if case .copy(let copy) = self { return copy }
            return nil
        }
    }

    public init(id: String, title: String, summary: String, author: String, version: String, official: Bool,
                targets: [String], bytes: Int64, icon: String?, cached: Bool, services: [String], hosts: [String],
                installed: DeviceCopy = .notAsked, update: Bool? = nil)
    {
        self.id = id
        self.title = title
        self.summary = summary
        self.author = author
        self.version = version
        self.official = official
        self.targets = targets
        self.bytes = bytes
        self.icon = icon
        self.cached = cached
        self.services = services
        self.hosts = hosts
        self.installed = installed
        self.update = update
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = try c.decode(String.self, forKey: .title)
        summary = try c.decode(String.self, forKey: .summary)
        author = try c.decode(String.self, forKey: .author)
        version = try c.decode(String.self, forKey: .version)
        official = try c.decode(Bool.self, forKey: .official)
        targets = try c.decode([String].self, forKey: .targets)
        bytes = try c.decode(Int64.self, forKey: .bytes)
        icon = try c.decodeIfPresent(String.self, forKey: .icon)
        cached = try c.decode(Bool.self, forKey: .cached)
        services = try c.decode([String].self, forKey: .services)
        hosts = try c.decode([String].self, forKey: .hosts)
        if !c.contains(.installed) {
            installed = .notAsked
        } else if try c.decodeNil(forKey: .installed) {
            installed = .none
        } else {
            installed = .copy(try c.decode(InstalledCopy.self, forKey: .installed))
        }
        update = try c.decodeIfPresent(Bool.self, forKey: .update)
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(title, forKey: .title)
        try c.encode(summary, forKey: .summary)
        try c.encode(author, forKey: .author)
        try c.encode(version, forKey: .version)
        try c.encode(official, forKey: .official)
        try c.encode(targets, forKey: .targets)
        try c.encode(bytes, forKey: .bytes)
        try c.encode(icon, forKey: .icon)
        try c.encode(cached, forKey: .cached)
        try c.encode(services, forKey: .services)
        try c.encode(hosts, forKey: .hosts)
        switch installed {
        case .notAsked: break
        case .none: try c.encodeNil(forKey: .installed)
        case .copy(let copy): try c.encode(copy, forKey: .installed)
        }
        try c.encodeIfPresent(update, forKey: .update)
    }

    private enum CodingKeys: String, CodingKey {
        case id, title, summary, author, version, official, targets, bytes, icon, cached, services, hosts, installed, update
    }
}

extension StoreApp {
    /// The one action of a row for a device.
    public enum Action: Equatable, Sendable {
        case install
        case update
        case installed
    }

    /// nil in an answer that was not for a device.
    public var action: Action? {
        switch installed {
        case .notAsked: nil
        case .none: .install
        case .copy: update == true ? .update : .installed
        }
    }
}

/// The answer of `store.apps`.
public struct StoreCatalog: Codable, Sendable, Hashable {
    public var apps: [StoreApp]
    /// When Hub fetched the catalog (epoch milliseconds).
    public var fetchedAt: EpochMilliseconds

    public init(apps: [StoreApp], fetchedAt: EpochMilliseconds) {
        self.apps = apps
        self.fetchedAt = fetchedAt
    }
}

/// What an applet's package says about it, as the store reads it out of the
/// package (§2 StoreDetails, the answer of `store.details`): of the newest
/// listed version. No client reads a manifest of its own.
public struct StoreDetails: Codable, Sendable, Hashable {
    /// A device the package was built for.
    public struct BuiltFor: Codable, Sendable, Hashable {
        /// The target its build names: `3ds-dev`, `rg-ds-plus`.
        public var target: String
        /// The store's name for the device, the same in every language; the
        /// target itself for a device the store has no name for.
        public var name: String
        /// nil when the store names no build.
        public var hostAbi: Int?

        public init(target: String, name: String, hostAbi: Int? = nil) {
            self.target = target
            self.name = name
            self.hostAbi = hostAbi
        }
    }

    /// The surface the applet lays itself out for.
    public struct Surface: Codable, Sendable, Hashable {
        /// `[width, height]` in points, the least and the most it is laid out for.
        public struct Size: Codable, Sendable, Hashable {
            public var min: [Int]
            public var max: [Int]

            public init(min: [Int], max: [Int]) {
                self.min = min
                self.max = max
            }
        }

        /// A touch surface of its own, not the device's screen.
        public var touch: Bool
        public var size: Size?

        public init(touch: Bool, size: Size? = nil) {
            self.touch = touch
            self.size = size
        }
    }

    public var id: String
    public var version: String
    /// In the store's order.
    public var devices: [BuiltFor]
    /// The capabilities it needs of a device, by id (`input.buttons`, `net.http`).
    public var needs: [String]
    /// The ones it uses where a device has them.
    public var uses: [String]
    /// nil when the package names no surface.
    public var surface: Surface?
    /// What its interface is written with (`solid`).
    public var framework: String?
    /// The version of the applet API it is written for.
    public var api: Int?

    public init(id: String, version: String, devices: [BuiltFor], needs: [String] = [], uses: [String] = [],
                surface: Surface? = nil, framework: String? = nil, api: Int? = nil)
    {
        self.id = id
        self.version = version
        self.devices = devices
        self.needs = needs
        self.uses = uses
        self.surface = surface
        self.framework = framework
        self.api = api
    }
}

// MARK: - Setup

/// A host that answers like an FTP server on ftpd's port.
public struct SetupCandidate: Codable, Sendable, Hashable, Identifiable {
    public var address: String
    public var port: Int
    public var banner: String?
    /// The banner is ftpd's own: almost certainly a 3DS. Offered first.
    public var ftpd: Bool

    public init(address: String, port: Int, banner: String? = nil, ftpd: Bool = true) {
        self.address = address
        self.port = port
        self.banner = banner
        self.ftpd = ftpd
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        address = try container.decode(String.self, forKey: .address)
        port = try container.decode(Int.self, forKey: .port)
        banner = try container.decodeIfPresent(String.self, forKey: .banner)
        // Absent only from a core older than the contract: not known to be ftpd.
        ftpd = try container.decodeIfPresent(Bool.self, forKey: .ftpd) ?? false
    }

    private enum CodingKeys: String, CodingKey { case address, port, banner, ftpd }

    public var id: String { "\(address):\(port)" }
}

public struct DiscoverResult: Codable, Sendable, Hashable {
    public var candidates: [SetupCandidate]

    /// ftpd's own consoles first (§3 Setup), then the other FTP servers, each by address.
    public var ordered: [SetupCandidate] {
        candidates.sorted { lhs, rhs in
            if lhs.ftpd != rhs.ftpd { return lhs.ftpd }
            return lhs.address.localizedStandardCompare(rhs.address) == .orderedAscending
        }
    }
}

/// `program` in `setup.install` and `setup.card`.
public enum ProgramSource: Sendable, Hashable, Encodable {
    case latestRelease
    case release(tag: String)
    case file(path: String)

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .latestRelease: try container.encode("latest", forKey: .release)
        case .release(let tag): try container.encode(tag, forKey: .release)
        case .file(let path): try container.encode(path, forKey: .path)
        }
    }

    private enum CodingKeys: String, CodingKey { case release, path }
}

public struct SetupStage: OpenStringEnum {
    public var rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public static let connecting = SetupStage(rawValue: "connecting")
    public static let reading = SetupStage(rawValue: "reading")
    public static let downloading = SetupStage(rawValue: "downloading")
    public static let uploading = SetupStage(rawValue: "uploading")
    public static let pairing = SetupStage(rawValue: "pairing")
    public static let done = SetupStage(rawValue: "done")
}

/// Progress of `setup.install` and `setup.card`: `{stage, done?, total?, message}`.
public struct SetupProgress: Codable, Sendable, Hashable {
    public var stage: SetupStage
    public var done: Int64?
    public var total: Int64?
    public var message: String?

    public init(stage: SetupStage, done: Int64? = nil, total: Int64? = nil, message: String? = nil) {
        self.stage = stage
        self.done = done
        self.total = total
        self.message = message
    }

    public var fraction: Double? {
        guard let done, let total, total > 0 else { return nil }
        return min(1, max(0, Double(done) / Double(total)))
    }
}

/// The install report of `setup.install` and `setup.card`. A report always
/// means success: a failed setup is the error `setup` (§3 Setup).
public struct InstallReport: Codable, Sendable, Hashable {
    public struct Program: Codable, Sendable, Hashable {
        /// `installed`, or `kept` when the device already held it.
        public var action: String
        /// The program's release version.
        public var version: String?
        /// The PocketJS commit the program was built from.
        public var runtime: String?

        public init(action: String, version: String?, runtime: String?) {
            self.action = action
            self.version = version
            self.runtime = runtime
        }
    }

    /// The device id.
    public var device: String
    /// `key` (a new key was written), `kept` or `address`.
    public var pairing: String
    public var program: Program
    /// Always true in a report.
    public var confirmed: Bool
    /// One sentence telling the owner what to do on the device (Hub's English).
    public var next: String

    public init(device: String, pairing: String, program: Program, confirmed: Bool, next: String) {
        self.device = device
        self.pairing = pairing
        self.program = program
        self.confirmed = confirmed
        self.next = next
    }
}

// MARK: - Daemons results

/// Progress of `daemons.install`: `{stage, message}`; `stage` is `runtime`
/// while Hub downloads Bun for it, then `installing`.
public struct DaemonInstallProgress: Codable, Sendable, Hashable {
    public var stage: String
    public var message: String?
}

public struct DaemonLogLine: Codable, Sendable, Hashable {
    public var at: EpochMilliseconds
    /// `stdout`, `stderr`, or `hub` for Hub's own lines about the daemon.
    public var stream: String
    public var text: String

    public init(at: EpochMilliseconds, stream: String, text: String) {
        self.at = at
        self.stream = stream
        self.text = text
    }
}

public struct DaemonLogsResult: Codable, Sendable, Hashable {
    public var lines: [DaemonLogLine]
}

/// `daemons.runtime`: the Bun runtime the daemons run on.
public struct BunRuntime: Codable, Sendable, Hashable {
    public var version: String?
    public var installed: Bool
    public var path: String?

    public init(version: String?, installed: Bool, path: String?) {
        self.version = version
        self.installed = installed
        self.path = path
    }
}

/// Progress of `daemons.runtime` with `install`: `{done, total}`.
public struct ByteProgress: Codable, Sendable, Hashable {
    public var done: Int64?
    public var total: Int64?

    public var fraction: Double? {
        guard let done, let total, total > 0 else { return nil }
        return min(1, max(0, Double(done) / Double(total)))
    }
}

/// `{}`, the result of methods that return nothing.
public struct EmptyResult: Codable, Sendable, Hashable {
    public init() {}
}
