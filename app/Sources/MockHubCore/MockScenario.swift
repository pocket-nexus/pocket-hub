import Foundation
import HubClient

/// A paired device in the mock. Rendered to the contract's JSON by hand
/// (`json`), not through HubClient's Codable models, so a decoding mistake in
/// the client is not mirrored by the mock.
public struct MockDevice: Sendable {
    public var id: String
    public var kind = "3ds"
    public var name = ""
    public var model = ""
    public var target = ""
    public var hostAbi = 0
    public var lastAddress: String?
    public var lastSeen: Int64?
    public var setupUnconfirmed = false
    public var duplicateId = false
    public var link: JSONValue = .null
    public var status: JSONValue = .null
    public var apps: JSONValue = .null
    public var busy: JSONValue = .null
    public var channels: [JSONValue] = []
    /// The newest release of the device's program Hub knows (`program.latest`).
    public var latestProgram: String?

    public init(id: String) { self.id = id }

    /// The version the device reports for its own program: its `home` row's.
    public var programVersion: String? {
        installedApps.first { $0["home"]?.boolValue == true }?["version"]?.stringValue
    }

    /// `{version, latest, update}`, with the contract's ordering: every dated
    /// version (three numbers, the first a year) below every SemVer one, dates
    /// part by part, SemVer `major.minor.patch` by number. The mock leaves
    /// pre-releases and any other form unordered: never newer.
    public var program: JSONValue {
        let version = programVersion
        var update = false
        if let version, let latestProgram, let a = Self.ordered(latestProgram), let b = Self.ordered(version) {
            update = a.lexicographicallyPrecedes(b) == false && a != b
        }
        return ["version": version.map(JSONValue.string) ?? .null,
                "latest": latestProgram.map(JSONValue.string) ?? .null, "update": .bool(update)]
    }

    /// A program version as a key that sorts as the contract orders: a leading
    /// 0 for a date, 1 for SemVer; nil for a form the mock does not order.
    static func ordered(_ version: String) -> [Int]? {
        let parts = version.split(separator: ".", omittingEmptySubsequences: false).map { Int($0) }
        guard parts.count == 3, parts.allSatisfy({ $0 != nil }) else { return nil }
        let numbers = parts.compactMap { $0 }
        return [numbers[0] >= 2000 ? 0 : 1] + numbers
    }

    /// Sets the version of the `home` row, as a device that restarted into a new program reports it.
    public mutating func setProgramVersion(_ version: String) {
        apps = .array(installedApps.map { row in
            guard row["home"]?.boolValue == true, case .object(var members) = row else { return row }
            members["version"] = .string(version)
            return .object(members)
        })
    }

    public var json: JSONValue {
        [
            "id": .string(id), "kind": .string(kind), "name": .string(name), "model": .string(model),
            "target": .string(target), "hostAbi": .number(Double(hostAbi)),
            "lastAddress": lastAddress.map(JSONValue.string) ?? .null,
            "lastSeen": lastSeen.map { .number(Double($0)) } ?? .null,
            "setupUnconfirmed": .bool(setupUnconfirmed), "duplicateId": .bool(duplicateId),
            "link": link, "status": status, "apps": apps, "busy": busy, "program": program,
            "channels": .array(channels),
        ]
    }

    public var isLinked: Bool { !link.isNull }

    /// Marks the device linked over `carrier`, with a status and applets as a
    /// fresh 3DS running Pocket Shell would report them.
    public mutating func linkUp(carrier: String = "tcp", address: String = "192.168.1.42", now: Int64 = MockClock.now()) {
        if name.isEmpty { name = "New 3DS" }
        if model.isEmpty { model = "New Nintendo 3DS XL" }
        if target.isEmpty { target = "3ds" }
        if hostAbi == 0 { hostAbi = 2 }
        lastAddress = address
        lastSeen = now
        setupUnconfirmed = false
        link = ["carrier": .string(carrier), "peer": .string("\(address):51234"), "since": .number(Double(now)),
                "payloadLimit": 16384, "features": ["runtime", "runtime.native", "runtime.remove", "apps", "channel", "devtools"]]
        status = [
            "phase": "accepted", "target": "3ds", "hostAbi": .number(Double(hostAbi)), "slot": "00000000000000a1",
            "address": .string(address), "generation": 3, "active": "1f2e3d4c5b6a7980", "lastGood": "1f2e3d4c5b6a7980",
            "running": "0000000000000000", "frame": 1024,
            "app": ["id": "nexus.pocket.home", "output": "home", "title": "Pocket"],
        ]
        if apps.isNull {
            apps = [
                ["id": "nexus.pocket.home", "output": "home", "version": "2026.11.2", "hash": "0000000000000000", "home": true],
                ["id": "nexus.pocket.weather", "output": "weather", "version": "0.3.1", "hash": "9a8b7c6d5e4f3021"],
                ["id": "nexus.pocket.notes", "output": "notes", "version": "1.2.0", "hash": "1122334455667788"],
                // Pushed by hand, unknown to the store.
                ["id": "dev.example.sketch", "output": "sketch-main", "version": "0.1.0", "hash": "a1b2c3d4e5f60718"],
            ]
        }
    }

    public mutating func linkDown(now: Int64 = MockClock.now()) {
        link = .null
        busy = .null
        channels = []
        lastSeen = now
    }

    var installedApps: [JSONValue] {
        if case .array(let rows) = apps { return rows }
        return []
    }
}

/// An installed daemon in the mock, rendered by hand like `MockDevice`.
public struct MockDaemon: Sendable {
    public var id: String
    public var version: String
    public var state = "stopped"
    public var pid: Int?
    public var since: Int64?
    public var channels = 0
    public var lastExit: JSONValue = .null
    public var error: String?
    public var log: [JSONValue] = []

    public init(id: String, version: String) {
        self.id = id
        self.version = version
    }

    public var json: JSONValue {
        [
            "id": .string(id), "version": .string(version), "state": .string(state),
            "pid": pid.map { .number(Double($0)) } ?? .null, "since": since.map { .number(Double($0)) } ?? .null,
            "channels": .number(Double(channels)), "lastExit": lastExit, "error": error.map(JSONValue.string) ?? .null,
        ]
    }
}

/// One applet of the mock store, rendered to the contract's StoreApp by hand.
public struct MockStoreApp: Sendable {
    public var id: String
    public var title: String
    public var summary: String
    public var author: String
    public var version: String
    public var official: Bool
    public var targets: [String]
    public var bytes: Int
    public var cached: Bool
    public var services: [String]
    public var hosts: [String]
    /// The output name a device's apps report gives it once installed.
    public var output: String

    public init(id: String, title: String, summary: String, author: String = "pocket-nexus", version: String = "1.0.0",
                official: Bool = true, targets: [String] = ["3ds", "3ds-dev"], bytes: Int = 120_000, cached: Bool = true,
                services: [String] = [], hosts: [String] = [], output: String? = nil)
    {
        self.id = id
        self.title = title
        self.summary = summary
        self.author = author
        self.version = version
        self.official = official
        self.targets = targets
        self.bytes = bytes
        self.cached = cached
        self.services = services
        self.hosts = hosts
        self.output = output ?? String(id.split(separator: ".").last ?? Substring(id))
    }

    /// The StoreApp object; `device` adds `installed` and `update` for that device.
    func json(icon: String?, device: MockDevice?) -> JSONValue {
        var object: [String: JSONValue] = [
            "id": .string(id), "title": .string(title), "summary": .string(summary), "author": .string(author),
            "version": .string(version), "official": .bool(official), "targets": .array(targets.map(JSONValue.string)),
            "bytes": .number(Double(bytes)), "icon": icon.map(JSONValue.string) ?? .null, "cached": .bool(cached),
            "services": .array(services.map(JSONValue.string)), "hosts": .array(hosts.map(JSONValue.string)),
        ]
        if let device {
            let row = device.installedApps.first { $0["id"]?.stringValue == id }
            if let row {
                let held = row["version"]?.stringValue ?? "0.0.0"
                object["installed"] = ["version": .string(held), "hash": row["hash"] ?? "0000000000000000"]
                object["update"] = .bool(MockStoreApp.orders(version, above: held))
            } else {
                object["installed"] = .null
                object["update"] = false
            }
        }
        return .object(object)
    }

    /// `major.minor.patch` ordering; an equal version is no update.
    static func orders(_ lhs: String, above rhs: String) -> Bool {
        let a = lhs.split(separator: ".").map { Int($0) ?? 0 }
        let b = rhs.split(separator: ".").map { Int($0) ?? 0 }
        for i in 0..<3 {
            let x = i < a.count ? a[i] : 0, y = i < b.count ? b[i] : 0
            if x != y { return x > y }
        }
        return false
    }
}

/// What the mock's `devices.installProgram` does.
public struct MockProgramBehavior: Sendable {
    /// The `release` reason every update fails with (`unreachable`, `not_found`, `rejected`); nil: the release is fine.
    public var releaseFailure: String?
    /// The link drops while the program is being sent: `device.unlinked` `lost`, the call fails with `link` `closed`.
    public var dropLinkWhileSending = false
    /// The device writes the program and cannot start it: the error `link` `launch` (installed).
    public var launchFails = false
    /// After the restart, the device links again after this delay; nil: it never does
    /// (Hub then releases its hold after `launchHoldLimit`).
    public var relinkAfter: Duration? = .seconds(2)
    /// The version the device reports after it links again; nil: the release's.
    public var reportedAfterRestart: String?
    /// How long after linking again the device reports its apps (the new version).
    public var reportDelay: Duration = .milliseconds(400)
    /// Hub's launch hold: how long it holds a restarting device that does not link again.
    public var launchHoldLimit: Duration = .seconds(60)
    /// Pause between progress notifications.
    public var stepDelay: Duration = .milliseconds(150)

    public init() {}
}

/// What the mock's `setup.install` and `setup.card` do.
public struct MockSetupBehavior: Sendable {
    /// The stage at which the install fails with kind `setup`, if any.
    public var failAt: SetupStage?
    /// When set, the install fails with this kind instead of `setup` (e.g. `internal`).
    public var failureKind: String?
    /// Pause between progress notifications.
    public var stepDelay: Duration = .milliseconds(120)
    /// Progress steps within `downloading` and `uploading`.
    public var chunks = 5
    /// The `reason` of that failure (`unreachable`, `companions_full`, …), if any.
    public var failureReason: String?
    /// After a successful install, the new device links after this delay; nil: it never links.
    public var linkAfter: Duration? = .seconds(3)
    /// The id the installed device gets. A setup of a known console keeps its id.
    public var deviceId = "5e7a11d0c0ffee01"
    /// When false, the install stores a key and then fails with `confirmed: false`
    /// and the device (the readback did not return); the device may still link.
    public var confirmed = true
    /// Discovery returns no candidate for this many calls first.
    public var discoverEmptyCalls = 0

    public init() {}
}

/// The whole configuration of a mock core.
public struct MockScenario: Sendable {
    public var hubId = "fedcba9876543210"
    public var hubName = "Studio Mac"
    public var hubVersion = "0.1.0-mock"
    public var devices: [MockDevice] = []
    public var daemons: [MockDaemon] = []
    public var candidates: [SetupCandidate] = []
    public var setup = MockSetupBehavior()
    public var bunInstalled = true
    /// Delay added before answering a method, by method name.
    public var responseDelays: [String: Duration] = [:]
    /// How long a state-changing call takes while holding the device mutex.
    public var mutexHold: Duration = .milliseconds(150)
    /// How often device logs produce a line while on.
    public var logInterval: Duration = .milliseconds(700)
    /// The store's catalog.
    public var store: [MockStoreApp] = MockScenario.storeCatalog
    /// false: `store.apps` and `store.install` fail with `store` / `unreachable`.
    public var storeReachable = true
    /// Applets whose package fails Hub's check: `store` / `rejected`.
    public var storeRejected: Set<String> = []
    /// Applets whose install the link ends under once the device has them
    /// (a 3DS busy longer than Hub's silence limit): `device.unlinked`
    /// `silence`, the error `link` `closed`, then the device links again
    /// holding the applet after `storeRelinkAfter`.
    public var storeLinkLoss: Set<String> = []
    public var storeRelinkAfter: Duration = .seconds(2)
    /// Pause between store install progress notifications.
    public var storeStepDelay: Duration = .milliseconds(150)
    /// The newest release of the devices' program: its version and size. nil: the release source does not answer.
    public var programRelease: (version: String, bytes: Int)? = ("2026.11.11", 310_000)
    public var program = MockProgramBehavior()

    public static var storeCatalog: [MockStoreApp] {
        [
            MockStoreApp(id: "nexus.pocket.hackernews", title: "Hacker News", summary: "Top stories from Hacker News.",
                         version: "1.1.0", bytes: 96_512, services: [], hosts: ["hacker-news.firebaseio.com"], output: "hn"),
            MockStoreApp(id: "nexus.pocket.weather", title: "Weather", summary: "Today's forecast where you are.",
                         version: "0.4.0", bytes: 210_944, services: ["weather"], hosts: ["api.open-meteo.com"]),
            MockStoreApp(id: "nexus.pocket.notes", title: "Notes", summary: "Notes on the device.",
                         version: "1.2.0", bytes: 131_576, services: ["store"]),
            MockStoreApp(id: "io.example.pixels", title: "Pixels", summary: "A small drawing pad.", author: "someone",
                         version: "0.2.0", official: false, bytes: 48_000, cached: false),
            MockStoreApp(id: "io.example.tabonly", title: "Tab Clock", summary: "A clock for the Tab5.", author: "someone",
                         official: false, targets: ["tab5-home"], bytes: 30_000),
        ]
    }

    public init() {}

    /// No device yet: the first-run experience.
    public static var firstRun: MockScenario {
        var scenario = MockScenario()
        scenario.candidates = [
            SetupCandidate(address: "192.168.1.20", port: 5000, banner: "220 FileZilla Server", ftpd: false),
            SetupCandidate(address: "192.168.1.57", port: 5000, banner: "220 Hello!", ftpd: true),
        ]
        scenario.setup.discoverEmptyCalls = 1
        scenario.daemons = []
        return scenario
    }

    /// Two paired consoles, one linked, two daemons, and a console running ftpd.
    public static var demo: MockScenario {
        var scenario = MockScenario.firstRun
        var living = MockDevice(id: "0123456789abcdef")
        living.name = "Living Room 3DS"
        living.model = "New Nintendo 3DS XL"
        living.target = "3ds"
        living.hostAbi = 2
        living.linkUp(address: "192.168.1.31")
        living.channels = [["app": "nexus.pocket.weather", "generation": 1, "since": .number(Double(MockClock.now() - 60_000))]]
        // Runs 2026.11.2; Hub knows 2026.11.11: an update is offered.
        living.latestProgram = scenario.programRelease?.version
        var travel = MockDevice(id: "89abcdef01234567")
        travel.name = "Travel 2DS"
        travel.model = "New Nintendo 2DS XL"
        travel.target = "3ds"
        travel.hostAbi = 2
        travel.lastAddress = "192.168.1.77"
        travel.lastSeen = MockClock.now() - 86_400_000
        scenario.devices = [living, travel]
        var weather = MockDaemon(id: "nexus.pocket.weather", version: "0.3.1")
        weather.state = "running"
        weather.pid = 48211
        weather.since = MockClock.now() - 60_000
        weather.channels = 1
        weather.log = [
            ["at": .number(Double(MockClock.now() - 59_000)), "stream": "stdout", "text": "weather daemon 0.3.1 ready"],
            ["at": .number(Double(MockClock.now() - 30_000)), "stream": "stdout", "text": "forecast refreshed for 1 channel"],
        ]
        var notes = MockDaemon(id: "nexus.pocket.notes", version: "1.2.0")
        notes.state = "failed"
        notes.since = MockClock.now() - 3_600_000
        notes.lastExit = ["code": 1, "signal": .null, "at": .number(Double(MockClock.now() - 3_600_000))]
        notes.error = "The daemon exited with code 1 during start."
        notes.log = [["at": .number(Double(MockClock.now() - 3_600_000)), "stream": "stderr", "text": "error: cannot find module './store'"]]
        scenario.daemons = [weather, notes]
        return scenario
    }
}

public enum MockClock {
    public static func now() -> Int64 { Int64(Date().timeIntervalSince1970 * 1000) }
}
