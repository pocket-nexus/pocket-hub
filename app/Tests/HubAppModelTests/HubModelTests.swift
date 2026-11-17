import Foundation
import HubClient
import MockHubCore
import Testing
@testable import HubAppModel

@Suite("Model: devices, services, outages")
@MainActor
struct HubModelTests {
    @Test func fillsFromTheSnapshot() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        #expect(h.model.hub?.name == "Studio Mac")
        #expect(h.model.devices.map(\.id) == ["0123456789abcdef", "89abcdef01234567"], "linked first")
        #expect(h.model.devices.first?.link?.carrier == .tcp)
        #expect(h.model.services?.map(\.id) == ["nexus.pocket.notes", "nexus.pocket.weather"])
        #expect(h.model.status?.home == h.home.path)
        #expect(h.model.bun?.installed == true)
    }

    /// A service an earlier Pocket Hub installed (CONTROL-API.md §2 `Service`,
    /// `reason: "earlier"`): listed without a release, not started, said in
    /// words the owner can act on, and gone when the owner removes it.
    @Test func aServiceAnEarlierHubInstalledIsListedAndSaid() async throws {
        var scenario = MockScenario.demo
        scenario.services.append(.earlier(id: "nexus.pocket.podcasts"))
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let earlier = try #require(h.model.services?.first { $0.id == "nexus.pocket.podcasts" })
        #expect(earlier.isEarlier && earlier.release == nil && earlier.state == .failed)
        #expect(h.model.services?.first { $0.id == "nexus.pocket.weather" }?.release == "dev")

        await #expect(throws: HubError.self) { try await h.model.perform(.start, service: "nexus.pocket.podcasts") }
        do {
            try await h.model.perform(.start, service: "nexus.pocket.podcasts")
        } catch {
            let words = try #require(UserFacingError(error))
            #expect(words.title == String(localized: "Installed by an earlier Pocket Hub. Install it again."))
            #expect(words.advice == String(localized: "Install the service again from its folder, or remove it."))
        }
        try await h.model.removeService("nexus.pocket.podcasts")
        await eventually("removed") { h.model.services?.contains { $0.id == "nexus.pocket.podcasts" } == false }
        #expect(h.model.services?.count == 2)
    }

    @Test func reactsToDeviceChangedAndUnlinked() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        let living = "0123456789abcdef"

        await h.core.unlink(living, code: "going_away", reason: "sleep")
        await eventually("unlinked") { h.model.device(living)?.isLinked == false }
        #expect(h.model.lastUnlink[living]?.code == "going_away")
        #expect(h.model.lastUnlink[living]?.explanation == String(localized: "The device was turned off or went to sleep."))
        #expect(h.model.devices.allSatisfy { !$0.isLinked })
        #expect(h.model.device(living)?.channels.isEmpty == true)

        await h.core.link(living, carrier: "usb")
        await h.core.link("89abcdef01234567")
        await eventually("travel linked") { h.model.device("89abcdef01234567")?.isLinked == true }
        await h.core.unlink("89abcdef01234567")
        await h.core.link(living, carrier: "usb")
        await eventually("linked again") { h.model.device(living)?.link?.carrier == .usb }
        await eventually("linked sorts first") { h.model.devices.first?.id == living && h.model.devices.last?.isLinked == false }
        #expect(h.model.lastUnlink[living] == nil)

        await h.core.modify(living) { device in
            device.busy = ["holder": ["kind": "cli", "name": "pocket-hub", "method": "devices.push"], "since": 1, "launching": false]
        }
        await eventually("busy") { h.model.device(living)?.busy != nil }
        #expect(h.model.device(living)?.busy?.holder.displayName == String(localized: "The command line"))
        #expect(h.model.device(living)?.busy?.displayText
            == String(localized: "\(String(localized: "The command line")) is \(String(localized: "installing an applet"))"))

        await h.core.unlink(living, code: "lost", reason: "the connection ended without a CLOSE")
        await eventually("lost") { h.model.lastUnlink[living]?.code == "lost" }
        #expect(h.model.lastUnlink[living]?.explanation == String(localized: "The connection was lost."))
        await h.core.link(living)

        await h.core.remove("89abcdef01234567")
        await eventually("removed") { h.model.device("89abcdef01234567") == nil }

        await h.core.emit("device.screenshot", ["id": .string(living), "path": "/tmp/x.png", "width": 400, "height": 480, "frame": 5])
        await eventually("unasked screenshot") { h.model.screenshots[living]?.frame == 5 }
    }

    @Test func anOutageNeverShowsStaleStateAndResyncs() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        await h.core.stopListening()
        await h.core.dropConnections()
        await eventually("not live") { !h.model.isLive }
        #expect(h.model.devices.isEmpty, "a device list shown while Hub is unreachable would be stale")
        #expect(h.model.services == nil)
        #expect(h.model.hub != nil, "Hub's identity is kept for the window title")
        if case .unreachable = h.model.connection {} else if h.model.connection != .connecting {
            Issue.record("unexpected state \(h.model.connection)")
        }
        try await h.core.start()
        await eventually("live again") { h.model.isLive && h.model.devices.count == 2 }
    }

    @Test func servicesFollowEvents() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        try await h.model.perform(.restart, service: "nexus.pocket.notes")
        #expect(h.model.services?.first { $0.id == "nexus.pocket.notes" }?.state == .running)
        await eventually("service log line") {
            h.model.serviceLogs["nexus.pocket.notes"]?.entries.contains { $0.line.text.contains("started") } == true
        }
        try await h.model.loadServiceLog("nexus.pocket.weather")
        #expect(h.model.serviceLogs["nexus.pocket.weather"]?.entries.count == 2)

        let folder = h.home.appending(path: "my-service", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try await h.model.installService(from: folder)
        #expect(h.model.services?.contains { $0.id == "my-service" } == true)
        #expect(h.model.serviceInstall == nil)
        try await h.model.removeService("my-service", purge: true)
        await eventually("removed") { h.model.services?.contains { $0.id == "my-service" } == false }
        #expect(await h.core.purged.contains("my-service"))
    }

    @Test func bunInstallReportsProgress() async throws {
        var scenario = MockScenario.demo
        scenario.bunInstalled = false
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        #expect(h.model.bun?.installed == false)
        try await h.model.installBun()
        #expect(h.model.bun?.installed == true && h.model.bun?.version == "1.3.2")
        #expect(h.model.bunInstallProgress == nil)
    }

    @Test func pushTracksProgressAndUpdatesApplets() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        let living = "0123456789abcdef"
        let file = h.home.appending(path: "snake.pocket")
        try Data("pocket".utf8).write(to: file)
        let push = Task { try await h.model.push(file, to: living) }
        await eventually("push activity") { h.model.pushes[living] != nil }
        await eventually("the sending phase") { h.model.pushes[living]?.phase != nil }
        await eventually("busy while pushing") { h.model.device(living)?.busy != nil }
        let result = try await push.value
        #expect(result.app.output == "snake")
        #expect(h.model.pushes[living] == nil)
        await eventually("new applet listed") { h.model.device(living)?.apps?.contains { $0.output == "snake" } == true }
        await eventually("released") { h.model.device(living)?.busy == nil }
    }

    @Test func busyAndNotLinkedErrorsAreSaidPlainly() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        let living = "0123456789abcdef"
        await h.core.failNext("devices.launch", with: .init(kind: "busy", message: "Busy.",
                                                            data: ["holder": ["kind": "cli", "name": "pocket-hub", "method": "devices.push"]]))
        do {
            try await h.model.launch(InstalledApp(id: "nexus.pocket.weather", output: "weather", version: "1", hash: "h"), on: living)
            Issue.record("expected busy")
        } catch {
            let message = try #require(UserFacingError(error, deviceName: "Living Room 3DS"))
            #expect(message.title == String(localized: "Living Room 3DS is busy."))
            #expect(message.advice.contains(String(localized: "The command line")))
        }
        do {
            _ = try await h.model.takeScreenshot(of: "89abcdef01234567")
            Issue.record("expected not_linked")
        } catch {
            let message = try #require(UserFacingError(error, deviceName: "Travel 2DS"))
            #expect(message.title == String(localized: "Travel 2DS isn't connected right now."))
        }
        let cancelled = HubError.operation(.init(kind: .cancelled, message: ""))
        #expect(UserFacingError(cancelled) == nil, "a cancellation needs no message")
    }

    @Test func screenshotAndLogs() async throws {
        var scenario = MockScenario.demo
        scenario.logInterval = .milliseconds(50)
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let living = "0123456789abcdef"
        // Every screenshot reaches the app, asked for or sent unasked: the
        // app keeps each where the owner keeps screenshots.
        var arrived: [(String, ScreenshotResult)] = []
        h.model.onScreenshot = { arrived.append(($0, $1)) }
        let shot = try await h.model.takeScreenshot(of: living)
        #expect(h.model.screenshots[living] == shot)
        #expect(FileManager.default.fileExists(atPath: shot.path))
        #expect(arrived.count == 1 && arrived.first?.0 == living && arrived.first?.1 == shot)
        await h.core.emit("device.screenshot", ["id": .string(living), "path": "/tmp/x.png", "width": 400, "height": 480, "frame": 9])
        await eventually("unasked screenshot") { arrived.count == 2 }
        #expect(arrived.last?.1.frame == 9)

        try await h.model.setLogs(true, for: living)
        #expect(h.model.logsOn.contains(living))
        await eventually("log lines") { (h.model.deviceLogs[living]?.entries.count ?? 0) >= 2 }

        // Hub re-enables a device's logs after a relink by itself: the model
        // does not ask again for a relink...
        let before = await h.core.received.filter { $0.method == "devices.logs" }.count
        await h.core.unlink(living)
        await h.core.link(living)
        let count = h.model.deviceLogs[living]?.entries.count ?? 0
        await eventually("lines again after the relink") { (h.model.deviceLogs[living]?.entries.count ?? 0) > count }
        #expect(await h.core.received.filter { $0.method == "devices.logs" }.count == before)
        // ...but does after its own reconnection, which makes it a new client.
        await h.core.dropConnections()
        await eventually("asked again after reconnecting") {
            await h.core.received.filter { $0.method == "devices.logs" }.count == before + 1
        }
        try await h.model.setLogs(false, for: living)
        #expect(!h.model.logsOn.contains(living))

        // Asking while the device is away is accepted.
        try await h.model.setLogs(true, for: "89abcdef01234567")
        #expect(h.model.logsOn.contains("89abcdef01234567"))
    }

    @Test func renameAndForget() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        try await h.model.setHubName("  Kitchen Mac ")
        #expect(h.model.hub?.name == "Kitchen Mac")
        try await h.model.forget("89abcdef01234567")
        await eventually("forgotten") { h.model.device("89abcdef01234567") == nil }
    }
}
