import Foundation
import Testing
@testable import HubClient

/// Every object of CONTROL-API.md decoded from JSON written as the contract
/// shows it: nulls where a member is "X or null", absent optionals.
@Suite("Decoding the contract's objects")
struct DecodingTests {
    func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try JSONDecoder.hub.decode(T.self, from: Data(json.utf8))
    }

    @Test func linkedDevice() throws {
        let device = try decode(Device.self, """
        {"id":"0123456789abcdef","kind":"3ds","name":"Living Room","model":"New Nintendo 3DS XL","target":"3ds",
         "hostAbi":2,"lastAddress":"192.168.1.31","lastSeen":1762876800000,"setupUnconfirmed":false,"duplicateId":false,
         "link":{"carrier":"tcp","peer":"192.168.1.31:50211","since":1762876800000,"payloadLimit":16384,
                 "features":["runtime","apps","channel"]},
         "status":{"phase":"accepted","target":"3ds","hostAbi":2,"slot":"00000000000000a1","address":"192.168.1.31",
                   "generation":3,"active":"1f2e3d4c5b6a7980","lastGood":"1f2e3d4c5b6a7980","running":"0000000000000000",
                   "frame":1024,"app":{"id":"nexus.pocket.weather","output":"weather","title":"Weather"}},
         "apps":[{"id":"nexus.pocket.launcher","output":"launcher","version":"1.0.0","hash":"0000000000000000","home":true},
                 {"id":"nexus.pocket.weather","output":"weather","version":"0.3.1","hash":"9a8b7c6d5e4f3021"}],
         "busy":{"holder":{"kind":"cli","name":"pocket-hub","method":"devices.push"},"since":1762876801000,"launching":false},
         "program":{"version":"2026.11.2","latest":"2026.11.11","update":true},
         "channels":[{"app":"nexus.pocket.weather","generation":1,"since":1762876802000}]}
        """)
        #expect(device.id == "0123456789abcdef")
        #expect(device.isLinked)
        #expect(device.link?.carrier == .tcp)
        #expect(device.link?.payloadLimit == 16384)
        #expect(device.link?.features == ["runtime", "apps", "channel"])
        #expect(device.status?.app?.title == "Weather")
        #expect(device.status?.frame == 1024)
        #expect(device.apps?.count == 2)
        #expect(device.apps?[0].isHome == true)
        #expect(device.apps?[1].home == nil)
        #expect(device.busy?.holder == MutexHolder(kind: "cli", name: "pocket-hub", method: "devices.push"))
        #expect(device.busy?.launching == false)
        #expect(device.channels.first?.generation == 1)
        #expect(device.lastSeen?.date == Date(timeIntervalSince1970: 1_762_876_800))
        #expect(device.program == DeviceProgram(version: "2026.11.2", latest: "2026.11.11", update: true))
        // A core from before `supports` existed knew the 3DS alone, which takes every operation.
        #expect(device.kind == .nintendo3ds && device.supports == DeviceOperation.all)
    }

    @Test func aTab5AndADeviceOfAnUnknownFamily() throws {
        let tab5 = try decode(Device.self, """
        {"id":"7ab5000000000001","kind":"tab5","supports":[],"name":"Desk Tab5","model":"M5Stack Tab5","target":"tab5-home",
         "hostAbi":3,"lastAddress":"192.168.1.63","lastSeen":null,"setupUnconfirmed":false,"duplicateId":false,
         "link":null,"status":null,"apps":null,"busy":null,"program":{"version":null,"latest":null,"update":false},"channels":[]}
        """)
        #expect(tab5.kind == .tab5 && tab5.supports.isEmpty)
        #expect(!tab5.supports(.push) && !tab5.supports(.screenshot) && !tab5.supports(.logs) && !tab5.supports(.program))
        #expect(try JSONDecoder.hub.decode(Device.self, from: JSONEncoder().encode(tab5)) == tab5)
        // A family and an operation this client does not know are kept as they came.
        let other = try decode(Device.self, """
        {"id":"a","kind":"","supports":["push","screenshot","logs","teleport"],"name":"","model":"","target":"psp","hostAbi":0,
         "lastAddress":null,"lastSeen":null,"setupUnconfirmed":false,"duplicateId":false,"link":null,"status":null,
         "apps":null,"busy":null,"program":{"version":null,"latest":null,"update":false},"channels":[]}
        """)
        #expect(other.kind.rawValue.isEmpty && other.supports(.push) && !other.supports(.program))
        #expect(other.supports.contains(DeviceOperation(rawValue: "teleport")))
    }

    @Test func aPairRequestAndItsEvent() throws {
        let request = try decode(PairRequest.self, """
        {"device":"7ab5000000000002","kind":"tab5","name":"Kitchen Tab5","model":"M5Stack Tab5","target":"tab5-home",
         "address":"192.168.1.63","since":1762876800000,"expires":1762877100000}
        """)
        #expect(request.id == "7ab5000000000002" && request.kind == .tab5 && request.address == "192.168.1.63")
        #expect(request.expires - request.since == 300_000)
        let event = try HubEvent(params: ["topic": "pairing.changed", "requests": [try JSONValue(encoding: request)]])
        #expect(event == .pairingChanged([request]))
        #expect(try HubEvent(params: ["topic": "pairing.changed", "requests": []]) == .pairingChanged([]))
    }

    @Test func programBeforeAnyReportOrLook() throws {
        let device = try decode(Device.self, """
        {"id":"a","kind":"3ds","name":"","model":"","target":"","hostAbi":0,"lastAddress":null,"lastSeen":null,
         "setupUnconfirmed":false,"duplicateId":false,"link":null,"status":null,"apps":null,"busy":null,
         "program":{"version":null,"latest":null,"update":false},"channels":[]}
        """)
        #expect(device.program == DeviceProgram())
        // A core from before `program` existed: nothing known, no update offered.
        let older = try decode(Device.self, """
        {"id":"a","kind":"3ds","name":"","model":"","target":"","hostAbi":0,"lastAddress":null,"lastSeen":null,
         "setupUnconfirmed":false,"duplicateId":false,"link":null,"status":null,"apps":null,"busy":null,"channels":[]}
        """)
        #expect(older.program == DeviceProgram(version: nil, latest: nil, update: false))
        // Round trip through the client's own encoding.
        let encoded = try JSONEncoder().encode(device)
        #expect(try JSONDecoder.hub.decode(Device.self, from: encoded) == device)
    }

    @Test func installProgramResult() throws {
        let launched = try decode(InstallProgramResult.self, #"{"phase":"launching","name":"pocket-shell.3dsx","version":"2026.11.11"}"#)
        #expect(launched == InstallProgramResult(phase: "launching", name: "pocket-shell.3dsx", version: "2026.11.11"))
        let local = try decode(InstallProgramResult.self, #"{"phase":"installed","name":"pocket-shell.3dsx","version":null}"#)
        #expect(local.version == nil)
    }

    @Test func pairedDeviceNeverLinked() throws {
        let device = try decode(Device.self, """
        {"id":"89abcdef01234567","kind":"3ds","name":"","model":"","target":"","hostAbi":0,"lastAddress":null,
         "lastSeen":null,"setupUnconfirmed":true,"duplicateId":false,"link":null,"status":null,"apps":null,
         "busy":null,"channels":[]}
        """)
        #expect(!device.isLinked)
        #expect(device.name.isEmpty)
        #expect(device.lastAddress == nil && device.lastSeen == nil)
        #expect(device.setupUnconfirmed)
        #expect(device.status == nil && device.apps == nil && device.busy == nil)
    }

    @Test func unknownCarrierAndPartialStatusStillDecode() throws {
        let device = try decode(Device.self, """
        {"id":"a","kind":"psp","name":"P","model":"","target":"","hostAbi":1,"lastAddress":null,"lastSeen":null,
         "setupUnconfirmed":false,"duplicateId":true,
         "link":{"carrier":"lora","peer":"x","since":1,"payloadLimit":1024,"features":[]},
         "status":{"phase":"booted","app":null},"apps":[],
         "busy":{"holder":{"kind":"gui","name":"Pocket Hub","method":"devices.installProgram"},"since":5,"launching":true},"channels":[]}
        """)
        #expect(device.link?.carrier.rawValue == "lora")
        #expect(device.status?.phase == "booted")
        #expect(device.status?.app == nil)
        #expect(device.busy?.launching == true)
        #expect(device.busy?.holder.method == "devices.installProgram")
        #expect(device.duplicateId)
    }

    @Test func daemons() throws {
        let running = try decode(Daemon.self, """
        {"id":"nexus.pocket.weather","version":"0.3.1","state":"running","pid":48211,"since":1762876800000,
         "channels":1,"lastExit":null,"error":null}
        """)
        #expect(running.state == .running && running.pid == 48211 && running.lastExit == nil)
        let failed = try decode(Daemon.self, """
        {"id":"nexus.pocket.notes","version":"1.2.0","state":"failed","pid":null,"since":null,"channels":0,
         "lastExit":{"code":1,"signal":null,"at":1762876800000},"error":"exited with code 1"}
        """)
        #expect(failed.state == .failed && failed.pid == nil && failed.since == nil)
        #expect(failed.lastExit == DaemonExit(code: 1, signal: nil, at: 1_762_876_800_000))
        #expect(failed.error == "exited with code 1")
        let named = try decode(DaemonExit.self, #"{"code":null,"signal":"SIGKILL","at":1}"#)
        #expect(named.signal == "SIGKILL" && named.code == nil)
    }

    @Test func hubObjects() throws {
        let hello = try decode(HelloResult.self, #"{"protocol":1,"hub":{"id":"fedcba9876543210","name":"Studio Mac","version":"0.1.0"}}"#)
        #expect(hello.protocol == 1 && hello.hub.name == "Studio Mac")
        let status = try decode(HubStatus.self, """
        {"id":"fedcba9876543210","name":"Studio Mac","version":"0.1.0","home":"/Users/x/Library/Application Support/Pocket Hub",
         "startedAt":1762876800000,"carriers":[{"kind":"tcp","port":8628},{"kind":"usb"}],"bun":{"version":null,"installed":false}}
        """)
        #expect(status.carriers.count == 2 && status.carriers[1].port == nil)
        #expect(status.bun.installed == false && status.bun.version == nil)
    }

    @Test func deviceResults() throws {
        let push = try decode(PushResult.self, #"{"app":{"id":"nexus.pocket.notes","output":"notes","title":"Notes"},"hash":"abcdef0123456789","generation":4}"#)
        #expect(push.app.output == "notes" && push.generation == 4)
        let shot = try decode(ScreenshotResult.self, #"{"path":"/tmp/s.png","width":400,"height":480,"frame":77}"#)
        #expect(shot == ScreenshotResult(path: "/tmp/s.png", width: 400, height: 480, frame: 77))
        let launch = try decode(LaunchResult.self, #"{"output":"weather"}"#)
        #expect(launch.output == "weather")
        let progress = try decode(TransferProgress.self, #"{"phase":"upload","done":50,"total":200}"#)
        #expect(progress.fraction == 0.25)
    }

    @Test func setupObjects() throws {
        let discover = try decode(DiscoverResult.self, """
        {"candidates":[{"address":"192.168.1.9","port":5000,"banner":"220 FileZilla","ftpd":false},
                       {"address":"192.168.1.57","port":5000,"banner":"220 Hello!","ftpd":true}]}
        """)
        #expect(discover.ordered.map(\.address) == ["192.168.1.57", "192.168.1.9"], "ftpd's own first")
        #expect(discover.ordered.first?.ftpd == true)
        let progress = try decode(SetupProgress.self, #"{"stage":"uploading","done":10,"total":40,"message":"Copying"}"#)
        #expect(progress.stage == .uploading && progress.fraction == 0.25)
        let bare = try decode(SetupProgress.self, #"{"stage":"pairing","message":"Writing"}"#)
        #expect(bare.done == nil && bare.fraction == nil)
        let report = try decode(InstallReport.self, """
        {"device":"5e7a11d0c0ffee01","pairing":"key","program":{"action":"installed","version":"0.4.0","runtime":"0.4.0"},
         "confirmed":true,"next":"Exit ftpd, then open Pocket Shell."}
        """)
        #expect(report.device == "5e7a11d0c0ffee01" && report.pairing == "key" && report.confirmed)
        #expect(report.program.action == "installed" && report.program.runtime == "0.4.0")
        let kept = try decode(InstallReport.self, """
        {"device":"5e7a11d0c0ffee01","pairing":"kept","program":{"action":"kept","version":null,"runtime":null},"confirmed":true,"next":"."}
        """)
        #expect(kept.program.version == nil && kept.program.runtime == nil)
    }

    @Test func daemonResults() throws {
        let logs = try decode(DaemonLogsResult.self, #"{"lines":[{"at":1,"stream":"stderr","text":"boom"}]}"#)
        #expect(logs.lines == [DaemonLogLine(at: 1, stream: "stderr", text: "boom")])
        let bun = try decode(BunRuntime.self, #"{"version":null,"installed":false,"path":null}"#)
        #expect(bun == BunRuntime(version: nil, installed: false, path: nil))
        let install = try decode(DaemonInstallProgress.self, #"{"stage":"unpacking","message":"Unpacking"}"#)
        #expect(install.stage == "unpacking")
        let bytes = try decode(ByteProgress.self, #"{"done":5,"total":10}"#)
        #expect(bytes.fraction == 0.5)
    }

    @Test func storeApps() throws {
        let catalog = try decode(StoreCatalog.self, """
        {"apps":[
          {"id":"nexus.pocket.hackernews","title":"Hacker News","summary":"Top stories.","author":"pocket-nexus","version":"1.1.0",
           "official":true,"targets":["3ds","3ds-dev"],"devices":[{"target":"3ds-dev","name":"Nintendo 3DS"}],"sample":true,
           "bytes":96512,"icon":"/tmp/i.png","cached":true,
           "services":[],"hosts":["hacker-news.firebaseio.com"],"installed":{"version":"1.0.0","hash":"0011223344556677"},"update":true},
          {"id":"io.example.pixels","title":"Pixels","summary":"Draw.","author":"someone","version":"0.2.0","official":false,
           "targets":["3ds"],"devices":null,"sample":false,"bytes":48000,"icon":null,"cached":false,"services":["x"],"hosts":[],"installed":null,"update":false},
          {"id":"io.example.all","title":"All","summary":"No device asked.","author":"someone","version":"1.0.0","official":false,
           "targets":["tab5-home"],"devices":null,"sample":false,"bytes":1,"icon":null,"cached":true,"services":[],"hosts":[]}],
         "fetchedAt":1794398383966}
        """)
        #expect(catalog.fetchedAt == 1_794_398_383_966)
        let (hn, pixels, all) = (catalog.apps[0], catalog.apps[1], catalog.apps[2])
        #expect(hn.installed == .copy(.init(version: "1.0.0", hash: "0011223344556677")) && hn.update == true)
        #expect(hn.action == .update && hn.icon == "/tmp/i.png" && hn.bytes == 96512)
        #expect(hn.devices == [.init(target: "3ds-dev", name: "Nintendo 3DS")] && hn.sample && pixels.devices == nil && !pixels.sample)
        #expect(pixels.installed == StoreApp.DeviceCopy.none && pixels.action == .install && pixels.icon == nil && !pixels.cached)
        #expect(all.installed == .notAsked && all.update == nil && all.action == nil, "absent without a device")
        let installedSame = StoreApp(id: "a", title: "A", summary: "", author: "", version: "1.0.0", official: true, targets: [],
                                     bytes: 0, icon: nil, cached: true, services: [], hosts: [],
                                     installed: .copy(.init(version: "1.0.0", hash: "x")), update: false)
        #expect(installedSame.action == .installed)
        // Encoding keeps absent apart from null.
        #expect(try JSONValue(encoding: all)["installed"] == nil)
        #expect(try JSONValue(encoding: pixels)["installed"] == .null)
        #expect(try event(#"{"topic":"store.changed","fetchedAt":5}"#) == .storeChanged(fetchedAt: 5))
        #expect(try error(#"{"code":1,"message":"No answer.","data":{"kind":"store","reason":"unreachable"}}"#).kind == .store)
    }

    @Test func storeDetails() throws {
        let details = try decode(StoreDetails.self, """
        {"id":"dev.pocket-nexus.hn","version":"0.3.0",
         "devices":[{"target":"rg-ds-plus","name":"Omarchy Pocket","hostAbi":12},{"target":"x-new","name":"x-new","hostAbi":null}],
         "needs":["input.buttons","net.http"],"uses":["system.agent"],
         "surface":{"touch":true,"size":{"min":[320,240],"max":[640,720]}},"framework":"solid","api":1}
        """)
        #expect(details.devices == [.init(target: "rg-ds-plus", name: "Omarchy Pocket", hostAbi: 12), .init(target: "x-new", name: "x-new")])
        #expect(details.needs == ["input.buttons", "net.http"] && details.uses == ["system.agent"])
        #expect(details.surface == .init(touch: true, size: .init(min: [320, 240], max: [640, 720])))
        #expect(details.framework == "solid" && details.api == 1)
        // A package that names no surface, framework or API: null members.
        let plain = try decode(StoreDetails.self, """
        {"id":"a","version":"1.0.0","devices":[],"needs":[],"uses":[],"surface":null,"framework":null,"api":null}
        """)
        #expect(plain.surface == nil && plain.framework == nil && plain.api == nil)
    }

    @Test func programSourceEncoding() throws {
        #expect(try JSONValue(encoding: ProgramSource.latestRelease) == ["release": "latest"])
        #expect(try JSONValue(encoding: ProgramSource.file(path: "/tmp/p.3dsx")) == ["path": "/tmp/p.3dsx"])
        #expect(try JSONValue(encoding: ProgramSource.release(tag: "v2026.11.11.3")) == ["release": "v2026.11.11.3"])
    }

    // MARK: Events

    func event(_ json: String) throws -> HubEvent {
        try HubEvent(params: JSONDecoder.hub.decode(JSONValue.self, from: Data(json.utf8)))
    }

    @Test func everyEventTopic() throws {
        let deviceJSON = """
        {"id":"a","kind":"3ds","name":"A","model":"","target":"","hostAbi":0,"lastAddress":null,"lastSeen":null,
         "setupUnconfirmed":false,"duplicateId":false,"link":null,"status":null,"apps":null,"busy":null,"channels":[]}
        """
        guard case .deviceChanged(let device) = try event(#"{"topic":"device.changed","device":\#(deviceJSON)}"#) else {
            Issue.record("device.changed"); return
        }
        #expect(device.name == "A")
        #expect(try event(#"{"topic":"device.removed","id":"a"}"#) == .deviceRemoved(id: "a"))
        #expect(try event(#"{"topic":"device.unlinked","id":"a","code":"silence","reason":"10 s without a frame"}"#)
            == .deviceUnlinked(DeviceUnlinked(id: "a", code: "silence", reason: "10 s without a frame")))
        #expect(try event(#"{"topic":"device.unlinked","id":"a","code":"lost","reason":"the connection ended"}"#)
            == .deviceUnlinked(DeviceUnlinked(id: "a", code: "lost", reason: "the connection ended")))
        #expect(try event(#"{"topic":"device.log","id":"a","level":"warn","source":"guest","text":"hi","frame":12}"#)
            == .deviceLog(DeviceLogLine(id: "a", level: "warn", source: "guest", text: "hi", frame: 12)))
        #expect(try event(#"{"topic":"device.log","id":"a","level":"info","source":"host","text":"x","frame":13,"dropped":4}"#)
            == .deviceLog(DeviceLogLine(id: "a", level: "info", source: "host", text: "x", frame: 13, dropped: 4)))
        guard case .deviceScreenshot(let shot) = try event(#"{"topic":"device.screenshot","id":"a","path":"/p.png","width":400,"height":480,"frame":9}"#) else {
            Issue.record("device.screenshot"); return
        }
        #expect(shot.result == ScreenshotResult(path: "/p.png", width: 400, height: 480, frame: 9))
        guard case .daemonChanged(let daemon) = try event(#"{"topic":"daemon.changed","daemon":{"id":"d","version":"1","state":"starting","pid":null,"since":5,"channels":0,"lastExit":null,"error":null}}"#) else {
            Issue.record("daemon.changed"); return
        }
        #expect(daemon.state == .starting)
        #expect(try event(#"{"topic":"daemon.removed","id":"d"}"#) == .daemonRemoved(id: "d"))
        #expect(try event(#"{"topic":"daemon.log","id":"d","at":7,"stream":"stdout","text":"ok"}"#)
            == .daemonLog(id: "d", line: DaemonLogLine(at: 7, stream: "stdout", text: "ok")))
        #expect(try event(#"{"topic":"hub.changed","hub":{"id":"h","name":"New","version":"0.1.0"}}"#)
            == .hubChanged(HubIdentity(id: "h", name: "New", version: "0.1.0")))
        guard case .unknown(let topic, _) = try event(#"{"topic":"agent.something","x":1}"#) else {
            Issue.record("unknown"); return
        }
        #expect(topic == "agent.something")
    }

    // MARK: Errors

    func error(_ json: String) throws -> HubError {
        HubError(rpcError: try JSONDecoder.hub.decode(JSONValue.self, from: Data(json.utf8)))
    }

    @Test func errorsMapToKinds() throws {
        let kinds: [(String, HubOperationError.Kind)] = [
            ("not_found", .notFound), ("not_linked", .notLinked), ("busy", .busy), ("link", .link),
            ("cancelled", .cancelled), ("setup", .setup), ("invalid", .invalid), ("daemon", .daemon), ("release", .release), ("internal", .internal),
            ("brand_new", .other("brand_new")),
        ]
        for (raw, kind) in kinds {
            let mapped = try error(#"{"code":1,"message":"Sentence.","data":{"kind":"\#(raw)"}}"#)
            #expect(mapped.kind == kind)
            #expect(mapped.kind?.rawValue == raw)
        }
        let busy = try error(#"{"code":1,"message":"Busy.","data":{"kind":"busy","holder":{"kind":"cli","name":"pocket-hub","method":"devices.push"}}}"#)
        guard case .operation(let busyError) = busy else { Issue.record("busy"); return }
        #expect(busyError.holder == MutexHolder(kind: "cli", name: "pocket-hub", method: "devices.push"))
        #expect(busyError.message == "Busy.")
        let link = try error(#"{"code":1,"message":"No answer.","data":{"kind":"link","code":"timeout","phase":"launching"}}"#)
        if case .operation(let e) = link { #expect(e.linkCode == "timeout" && e.phase == "launching") }
        let setup = try error(#"{"code":1,"message":"Readback failed.","data":{"kind":"setup","step":"pairing","reason":"storage","detail":"RETR timed out","device":"5e7a11d0c0ffee01","confirmed":false}}"#)
        if case .operation(let e) = setup {
            #expect(e.step == "pairing" && e.reason == "storage" && e.detail == "RETR timed out")
            #expect(e.device == "5e7a11d0c0ffee01" && e.confirmed == false)
        }
        for reason in ["unreachable", "not_found", "rejected"] {
            let release = try error(#"{"code":1,"message":"No release.","data":{"kind":"release","reason":"\#(reason)"}}"#)
            if case .operation(let e) = release { #expect(e.kind == .release && e.reason == reason) } else { Issue.record("release") }
        }
        let daemon = try error(#"{"code":1,"message":"Bun is missing.","data":{"kind":"daemon","id":"dev.x"}}"#)
        if case .operation(let e) = daemon { #expect(e.kind == .daemon && e.daemonId == "dev.x") }
        #expect(try error(#"{"code":-32601,"message":"No method."}"#) == .rpc(code: -32601, message: "No method."))
        #expect(try error(#"{"code":1,"message":"No data."}"#).kind == .internal)
        #expect(try error(#"{"code":1,"message":"c","data":{"kind":"cancelled"}}"#).isCancelled)
    }
}
