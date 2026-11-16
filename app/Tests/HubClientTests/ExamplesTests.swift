import Foundation
import Testing
@testable import HubClient

/// docs/control-api-examples.json: lines the real `pocket-hubd` wrote and read
/// in its end-to-end test, one per method, notification, event topic and
/// error kind. Every one is decoded here with the client's own types, and the
/// client's typed methods are checked to send the requests' params.
@Suite("The server's examples")
struct ExamplesTests {
    static let url = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appending(path: "docs/control-api-examples.json")

    static func examples() throws -> JSONValue {
        try JSONDecoder.hub.decode(JSONValue.self, from: Data(contentsOf: url))
    }

    static func section(_ name: String) throws -> [String: JSONValue] {
        guard case .object(let members)? = try examples()[name] else { throw HubError.invalidResponse(name) }
        return members
    }

    /// Answers one method with the example's result and records the params it was sent.
    final class Replay: HubCalling, @unchecked Sendable {
        let example: JSONValue
        private let lock = NSLock()
        private var sent: JSONValue?
        private var progressed = 0

        init(_ example: JSONValue) { self.example = example }

        var params: JSONValue? { lock.withLock { sent } }
        var progressCount: Int { lock.withLock { progressed } }

        func call(_ method: String, params: JSONValue?, progress: (@Sendable (JSONValue) -> Void)?) async throws -> JSONValue {
            #expect(method == example["request"]?["method"]?.stringValue)
            lock.withLock { sent = params ?? [:] }
            if case .array(let notes)? = example["progress"] {
                for note in notes {
                    progress?(note["params"] ?? .null)
                    lock.withLock { progressed += 1 }
                }
            }
            if let error = example["response"]?["error"] { throw HubError(rpcError: error) }
            return example["response"]?["result"] ?? .null
        }
    }

    /// The example's params without members that are null (the client leaves an
    /// absent optional out instead of sending null; the server reads both alike).
    static func withoutNulls(_ value: JSONValue?) -> JSONValue {
        guard case .object(let members)? = value else { return value ?? [:] }
        return .object(members.filter { !$0.value.isNull }.mapValues { withoutNulls($0) })
    }

    @Test func everyMethodRoundTripsThroughTheTypedClient() async throws {
        let methods = try Self.section("methods")
        var covered: Set<String> = []
        for (name, example) in methods {
            let replay = Replay(example)
            let p = example["request"]?["params"] ?? [:]
            let device = p["device"]?.stringValue ?? ""
            let path = p["path"]?.stringValue ?? ""
            let id = p["id"]?.stringValue ?? ""
            func program() -> ProgramSource {
                if let file = p["program"]?["path"]?.stringValue { return .file(path: file) }
                if let tag = p["program"]?["release"]?.stringValue, tag != "latest" { return .release(tag: tag) }
                return .latestRelease
            }
            switch name {
            case "hub.hello":
                let client = try p["client"].orThrow("client").decoded() as ClientIdentity
                let hello = try await replay.hello(protocol: p["protocol"]?.intValue ?? 1, client: client)
                #expect(hello.protocol == 1 && !hello.hub.id.isEmpty)
            case "hub.status": _ = try await replay.status()
            case "hub.setName": _ = try await replay.setName(p["name"]?.stringValue ?? "")
            case "hub.subscribe": try await replay.subscribe()
            case "hub.shutdown": try await replay.shutdown()
            case "devices.list": #expect(try await !replay.devices().isEmpty)
            case "devices.get": _ = try await replay.device(device)
            case "devices.forget": try await replay.forget(device: device)
            case "devices.status": _ = try await replay.runtimeStatus(device: device)
            case "devices.push":
                let phases = Locked<[String]>([])
                let result = try await replay.push(device: device, path: path) { progress in phases.mutate { $0.append(progress.phase ?? "") } }
                #expect(!result.hash.isEmpty)
                #expect(phases.value.first == "sending", "push progress starts at sending")
            case "devices.launch": _ = try await replay.launch(device: device, app: p["app"]?.stringValue ?? "")
            case "devices.remove": try await replay.remove(device: device, app: p["app"]?.stringValue ?? "")
            case "devices.screenshot":
                let shot = try await replay.screenshot(device: device, path: p["path"]?.stringValue)
                #expect(shot.width > 0 && shot.height > 0)
            case "devices.logs": try await replay.setLogs(device: device, on: p["on"]?.boolValue ?? false)
            case "devices.installProgram":
                let phases = Locked<[String]>([])
                let result = try await replay.installProgram(device: device, program: p["program"].map { _ in program() },
                                                             launch: p["launch"]?.boolValue) { progress in
                    phases.mutate { $0.append(progress.phase ?? "") }
                }
                #expect(!result.name.isEmpty)
                if p["program"] == nil || p["program"]?["release"] != nil {
                    #expect(result.version != nil, "a release names its version")
                    #expect(phases.value.first == "downloading", "a release's progress starts with downloading")
                }
            case "setup.discover": _ = try await replay.discover(kind: p["kind"]?.stringValue ?? "3ds")
            case "setup.install":
                let report = try await replay.install(kind: p["kind"]?.stringValue ?? "3ds", host: p["host"]?.stringValue ?? "",
                                                      port: p["port"]?.intValue, program: program())
                #expect(report.confirmed)
            case "setup.card":
                let report = try await replay.installFromCard(kind: p["kind"]?.stringValue ?? "3ds", path: path, program: program())
                #expect(report.confirmed && report.pairing == "key")
            case "pairing.list":
                let requests = try await replay.pairRequests()
                #expect(requests.first?.kind == .tab5 && requests.first?.expires ?? 0 > requests.first?.since ?? 0)
            case "pairing.answer":
                let paired = try await replay.answerPairing(device: device, code: p["code"]?.stringValue ?? "")
                #expect(paired.kind == .tab5 && paired.supports.isEmpty, "a Tab5 takes none of the four operations")
            case "pairing.decline": try await replay.declinePairing(device: device)
            case "daemons.list": _ = try await replay.daemons()
            case "daemons.install":
                let stages = Locked<[String]>([])
                _ = try await replay.installDaemon(path: path) { progress in stages.mutate { $0.append(progress.stage) } }
                #expect(stages.value.allSatisfy { ["runtime", "installing"].contains($0) })
            case "daemons.remove": try await replay.removeDaemon(id: id, purge: p["purge"]?.boolValue ?? false)
            case "daemons.start": _ = try await replay.daemon(.start, id: id)
            case "daemons.stop": _ = try await replay.daemon(.stop, id: id)
            case "daemons.restart": _ = try await replay.daemon(.restart, id: id)
            case "daemons.logs": #expect(try await !replay.daemonLogs(id: id, tail: p["tail"]?.intValue).isEmpty)
            case "daemons.runtime": _ = try await replay.bunRuntime(install: p["install"]?.boolValue ?? false)
            case "store.apps":
                let catalog = try await replay.storeApps(device: p["device"]?.stringValue, refresh: p["refresh"]?.boolValue ?? false)
                #expect(!catalog.apps.isEmpty && catalog.fetchedAt > 0)
                if p["device"] != nil {
                    #expect(catalog.apps.allSatisfy { $0.installed != .notAsked && $0.update != nil }, "a device's answer carries installed and update")
                }
            case "store.install":
                let phases = Locked<[String]>([])
                let result = try await replay.storeInstall(device: device, app: p["app"]?.stringValue ?? "") { progress in
                    phases.mutate { $0.append(progress.phase ?? "") }
                }
                #expect(!result.app.title.isEmpty)
                #expect(phases.value.first == "downloading", "store.install progress starts with downloading")
            default:
                Issue.record("the examples hold a method this client does not know: \(name)")
                continue
            }
            #expect(replay.params == Self.withoutNulls(p), "params of \(name)")
            if case .array(let notes)? = example["progress"] { #expect(replay.progressCount == notes.count) }
            covered.insert(name)
        }
        let contract: Set<String> = ["hub.hello", "hub.status", "hub.setName", "hub.subscribe", "hub.shutdown",
            "devices.list", "devices.get", "devices.forget", "devices.status", "devices.push", "devices.launch",
            "devices.remove", "devices.screenshot", "devices.logs", "devices.installProgram", "setup.discover", "setup.install",
            "setup.card", "pairing.list", "pairing.answer", "pairing.decline", "daemons.list", "daemons.install", "daemons.remove", "daemons.start", "daemons.stop",
            "daemons.restart", "daemons.logs", "daemons.runtime", "store.apps", "store.install"]
        #expect(covered == contract)
    }

    @Test func everyEventDecodes() throws {
        for (topic, example) in try Self.section("events") {
            #expect(example["method"] == "event")
            let event = try HubEvent(params: example["params"] ?? .null)
            if case .unknown = event { Issue.record("\(topic) decoded as unknown") }
        }
    }

    @Test func everyErrorMapsToItsKind() throws {
        for (name, example) in try Self.section("errors") {
            let error = HubError(rpcError: try #require(example["response"]?["error"]))
            if let code = Int(name) {
                #expect(error == .rpc(code: code, message: example["response"]?["error"]?["message"]?.stringValue ?? ""))
            } else {
                #expect(error.kind?.rawValue == name, "\(name)")
                if case .other = error.kind { Issue.record("unknown kind \(name)") }
                if name == "busy", case .operation(let e) = error {
                    #expect(e.holder != nil && e.holder?.method.isEmpty == false)
                }
                if name == "setup", case .operation(let e) = error {
                    #expect(e.step != nil)
                }
                if name == "store", case .operation(let e) = error {
                    #expect(["unreachable", "rejected"].contains(e.reason ?? ""))
                }
                if name == "release", case .operation(let e) = error {
                    #expect(["unreachable", "not_found", "rejected"].contains(e.reason ?? ""))
                }
                if name == "pairing", case .operation(let e) = error {
                    #expect(["wrong_code", "bad_proof", "gone"].contains(e.reason ?? ""))
                }
                if name == "link", case .operation(let e) = error {
                    // The example is devices.remove refused: the device's code and reason.
                    #expect(e.linkCode == "rejected" && ["running", "home"].contains(e.reason ?? ""))
                }
            }
        }
    }

    /// Every error kind of the contract has an example. `release` is the
    /// newest; its absence is reported here rather than passing silently.
    @Test func everyErrorKindOfTheContractHasAnExample() throws {
        let kinds = Set(try Self.section("errors").keys)
        for kind in ["not_found", "not_linked", "busy", "cancelled", "setup", "invalid", "daemon", "release", "store", "pairing"] {
            #expect(kinds.contains(kind), "the examples hold no error \(kind)")
        }
    }

    @Test func devicesCarryTheirProgram() throws {
        let list = try #require(try Self.section("methods")["devices.list"])
        let devices: [Device] = try #require(list["response"]?["result"]?["devices"]).decode([Device].self)
        #expect(!devices.isEmpty)
        guard case .array(let rows)? = list["response"]?["result"]?["devices"] else { Issue.record("devices"); return }
        for (raw, device) in zip(rows, devices) {
            #expect(raw["program"] != nil, "the server sends program")
            #expect(device.program.version == raw["program"]?["version"]?.stringValue)
            #expect(device.program.latest == raw["program"]?["latest"]?.stringValue)
            #expect(device.program.update == raw["program"]?["update"]?.boolValue)
        }
    }

    /// A Device says its family and what Hub can ask of it (§2 `supports`).
    @Test func devicesSayWhatTheySupport() throws {
        let list = try #require(try Self.section("methods")["devices.list"])
        let devices: [Device] = try #require(list["response"]?["result"]?["devices"]).decode([Device].self)
        let console = try #require(devices.first)
        #expect(console.kind == .nintendo3ds && console.supports == DeviceOperation.all)
        guard case .array(let rows)? = list["response"]?["result"]?["devices"] else { Issue.record("devices"); return }
        #expect(rows.first?["supports"] == ["push", "screenshot", "logs", "program"], "the server sends supports")
        // The pairing by a code's answer is a Tab5, which takes none of them.
        let answer = try #require(try Self.section("methods")["pairing.answer"])
        let tab5: Device = try #require(answer["response"]?["result"]).decode(Device.self)
        #expect(tab5.kind == .tab5 && DeviceOperation.all.allSatisfy { !tab5.supports($0) })
    }

    @Test func theCancelNotificationIsWhatTheClientSends() throws {
        let cancel = try #require(try Self.section("notifications")["cancel"])
        #expect(cancel["method"] == "cancel" && cancel["id"] == nil && cancel["params"]?["id"]?.intValue != nil)
    }
}
