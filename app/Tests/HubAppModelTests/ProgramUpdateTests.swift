import Foundation
import HubClient
import MockHubCore
import Testing
@testable import HubAppModel

/// Updating a device's software from the app: offered when Hub says so,
/// progress, the restart as an expected step, done when the device is back on
/// the new version, and every failure in plain words.
@Suite("Software update")
@MainActor
struct ProgramUpdateTests {
    static let living = "0123456789abcdef"
    static let travel = "89abcdef01234567"

    static func scenario(_ change: (inout MockProgramBehavior) -> Void = { _ in }) -> MockScenario {
        var scenario = MockScenario.demo
        scenario.mutexHold = .milliseconds(20)
        scenario.program.stepDelay = .milliseconds(30)
        scenario.program.relinkAfter = .milliseconds(300)
        scenario.program.reportDelay = .milliseconds(150)
        change(&scenario.program)
        return scenario
    }

    static func harness(_ scenario: MockScenario) async throws -> Harness {
        let h = try await Harness(scenario)
        h.model.programUpdateTimeouts = ProgramUpdateTimeouts(reconnect: .seconds(3), versionReport: .seconds(1),
                                                              graceAfterHold: .milliseconds(300))
        return h
    }

    func stage(_ h: Harness, _ id: String = living) -> ProgramUpdate.Stage? {
        h.model.programUpdates[id]?.stage
    }

    func failure(_ h: Harness, _ id: String = living) -> UserFacingError? {
        if case .failed(let failure)? = stage(h, id) { return failure }
        return nil
    }

    /// Runs the update and records each distinct stage until it ends.
    func run(_ h: Harness, _ id: String = living, timeout: Duration = .seconds(8)) async -> [String] {
        h.model.updateProgram(on: id)
        var seen: [String] = []
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            guard let stage = stage(h, id) else { break }
            let name = switch stage {
            case .preparing: "preparing"
            case .downloading: "downloading"
            case .sending: "sending"
            case .restarting: "restarting"
            case .reconnected: "reconnected"
            case .finished: "finished"
            case .failed: "failed"
            }
            if seen.last != name { seen.append(name) }
            if name == "finished" || name == "failed" { break }
            try? await Task.sleep(for: .milliseconds(2))
        }
        return seen
    }

    @Test func offeredOnlyWhenHubSaysSo() async throws {
        let h = try await Self.harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let living = try #require(h.model.device(Self.living))
        #expect(living.program == DeviceProgram(version: "2026.11.2", latest: "2026.11.11", update: true))
        #expect(living.offersSoftwareUpdate)
        let travel = try #require(h.model.device(Self.travel))
        #expect(travel.program.update == false && !travel.offersSoftwareUpdate, "never linked: nothing to offer")
        #expect(h.model.programUpdates.isEmpty)
        // Hub says it is current: no offer, whatever the strings look like.
        await h.core.modify(Self.living) { $0.latestProgram = "2026.11.2" }
        await eventually("not offered") { h.model.device(Self.living)?.offersSoftwareUpdate == false }
    }

    @Test func updateRestartsAndFinishesOnTheNewVersion() async throws {
        let h = try await Self.harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let stages = await run(h)
        #expect(stages.first == "preparing")
        #expect(stages.contains("downloading"))
        #expect(stages.contains("sending"))
        #expect(stages.contains("restarting"), "the restart is a step of its own")
        #expect(stages.contains("reconnected"), "linked again before the version report")
        #expect(stage(h) == .finished(version: "2026.11.11"))
        #expect(h.model.programUpdates[Self.living]?.previous == "2026.11.2")
        let device = try #require(h.model.device(Self.living))
        #expect(device.isLinked && device.busy == nil)
        #expect(device.program == DeviceProgram(version: "2026.11.11", latest: "2026.11.11", update: false))
        #expect(!device.offersSoftwareUpdate, "nothing is offered once it is current")
        // Hub's defaults: the latest release, launched.
        let call = try #require(await h.core.received.last { $0.method == "devices.installProgram" })
        #expect(call.params == ["device": .string(Self.living)])
        h.model.dismissProgramUpdate(of: Self.living)
        #expect(h.model.programUpdates[Self.living] == nil)
    }

    @Test func aReleaseThatCannotBeUsedSendsNothing() async throws {
        let cases: [(String, String)] = [
            ("unreachable", String(localized: "Pocket Hub can't download the update right now.")),
            ("not_found", String(localized: "This update is no longer available.")),
            ("rejected", String(localized: "The update didn't pass Pocket Hub's safety check.")),
        ]
        for (reason, title) in cases {
            let h = try await Self.harness(Self.scenario { $0.releaseFailure = reason })
            let stages = await run(h)
            #expect(stages.last == "failed", "\(reason)")
            #expect(failure(h)?.title == title, "\(reason)")
            #expect(failure(h)?.detail?.isEmpty == false, "Hub's sentence is kept as detail")
            #expect(!stages.contains("sending") && !stages.contains("restarting"))
            await eventually("released") { h.model.device(Self.living)?.busy == nil }
            #expect(h.model.device(Self.living)?.isLinked == true)
            #expect(h.model.device(Self.living)?.offersSoftwareUpdate == true, "still offered: Try Again")
            await h.tearDown()
        }
    }

    @Test func busyNamesWhoHoldsTheDevice() async throws {
        let h = try await Self.harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        await h.core.failNext("devices.installProgram", with: .init(kind: "busy", message: "The device is busy.",
            data: ["holder": ["kind": "cli", "name": "pocket-hub", "method": "store.install"]]))
        _ = await run(h)
        let failure = try #require(failure(h))
        #expect(failure.title == String(localized: "Living Room 3DS is busy."))
        #expect(failure.advice.contains(String(localized: "The command line")))
        #expect(failure.advice.contains(String(localized: "installing an applet")))
    }

    @Test func notConnected() async throws {
        let h = try await Self.harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        // Offered while away (Hub knows both versions), refused by Hub.
        await h.core.unlink(Self.living)
        await eventually("away") { h.model.device(Self.living)?.isLinked == false }
        #expect(h.model.device(Self.living)?.offersSoftwareUpdate == true)
        _ = await run(h)
        #expect(failure(h)?.title == String(localized: "Living Room 3DS isn't connected right now."))
    }

    @Test func theLinkDroppingMidwayIsSaidPlainly() async throws {
        let h = try await Self.harness(Self.scenario { $0.dropLinkWhileSending = true })
        defer { Task { await h.tearDown() } }
        let stages = await run(h)
        #expect(stages.contains("sending"))
        #expect(failure(h)?.title == String(localized: "The connection to your 3DS was lost during the update."))
        #expect(h.model.lastUnlink[Self.living]?.code == "lost")
    }

    @Test func aDeviceThatCannotStartTheNewVersion() async throws {
        let h = try await Self.harness(Self.scenario { $0.launchFails = true })
        defer { Task { await h.tearDown() } }
        _ = await run(h)
        #expect(failure(h)?.title == String(localized: "Your 3DS received the update but couldn't restart with it."))
    }

    @Test func aDeviceThatDoesNotComeBack() async throws {
        let h = try await Self.harness(Self.scenario {
            $0.relinkAfter = nil
            $0.launchHoldLimit = .milliseconds(400)
        })
        defer { Task { await h.tearDown() } }
        let stages = await run(h)
        #expect(stages.contains("restarting"))
        #expect(failure(h) == .updateNotReconnected(deviceName: "Living Room 3DS"))
        #expect(h.model.device(Self.living)?.busy == nil, "Hub let go of it")
    }

    @Test func aDeviceThatComesBackOnTheOldVersion() async throws {
        let h = try await Self.harness(Self.scenario { $0.reportedAfterRestart = "2026.11.2" })
        defer { Task { await h.tearDown() } }
        let stages = await run(h)
        #expect(stages.contains("reconnected"))
        #expect(failure(h) == .updateNotApplied(version: "2026.11.2"))
    }

    @Test func aDeviceThatComesBackOnAnotherOlderVersion() async throws {
        // Not the version it had, not the one installed: still not updated.
        let h = try await Self.harness(Self.scenario { $0.reportedAfterRestart = "2026.10.30" })
        defer { Task { await h.tearDown() } }
        _ = await run(h)
        #expect(failure(h) == .updateNotApplied(version: "2026.10.30"))
    }

    @Test func aSecondClickWhileRunningDoesNothing() async throws {
        let h = try await Self.harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        h.model.updateProgram(on: Self.living)
        h.model.updateProgram(on: Self.living)
        await eventually("finished", timeout: .seconds(8)) { stage(h) == .finished(version: "2026.11.11") }
        #expect(await h.core.received.filter { $0.method == "devices.installProgram" }.count == 1)
    }

    @Test func homeScreenWording() async throws {
        let h = try await Self.harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let device = try #require(h.model.device(Self.living))
        #expect(device.isOnHomeScreen, "the running applet is the home row")
        #expect(device.runningAppTitle == String(localized: "Home screen"))
        try await h.model.launch(appId: "nexus.pocket.weather", on: Self.living)
        await eventually("weather runs") { h.model.device(Self.living)?.runningApp?.id == "nexus.pocket.weather" }
        #expect(h.model.device(Self.living)?.isOnHomeScreen == false)
        #expect(h.model.device(Self.living)?.runningAppTitle == "Weather")
        // An applet the store does not know keeps its output name exactly.
        #expect(InstalledApp(id: "dev.x.demo", output: "pocket3ds-demo-main", version: "0.1.0", hash: "aa").displayName
            == "pocket3ds-demo-main")
    }
}
