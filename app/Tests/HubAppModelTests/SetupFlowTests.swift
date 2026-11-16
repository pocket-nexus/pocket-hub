import Foundation
import HubClient
import MockHubCore
import Testing
@testable import HubAppModel

@Suite("Setup flow")
@MainActor
struct SetupFlowTests {
    static func scenario(failAt: SetupStage? = nil, linkAfter: Duration? = .milliseconds(300)) -> MockScenario {
        var scenario = MockScenario.firstRun
        scenario.setup.stepDelay = .milliseconds(10)
        scenario.setup.chunks = 3
        scenario.setup.failAt = failAt
        scenario.setup.linkAfter = linkAfter
        return scenario
    }

    @Test func fullSuccessFinishesWhenTheDeviceLinks() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        #expect(h.model.devices.isEmpty)
        let flow = h.model.makeSetupFlow()
        flow.discoveryInterval = .milliseconds(50)
        // The first page asks which kind of device; a 3DS is set up over its storage.
        #expect(flow.step == .chooseDevice)
        flow.choose(.nintendo3ds)
        #expect(flow.step == .instructions)
        flow.chooseAgain()
        #expect(flow.step == .chooseDevice)
        flow.choose(.nintendo3ds)

        flow.startSearching()
        #expect(flow.step == .searching)
        // The first discovery answers nothing; the flow keeps looking.
        await eventually("found") { if case .found = flow.step { true } else { false } }
        guard case .found(let candidates) = flow.step else { return }
        #expect(candidates.map(\.address) == ["192.168.1.57", "192.168.1.20"], "ftpd's own console first")
        #expect(candidates.first?.ftpd == true)
        #expect(await h.core.received.filter { $0.method == "setup.discover" }.count >= 2)

        var stages: [SetupStage] = []
        var fractions: [Double] = []
        flow.install(candidates[0])
        let deadline = ContinuousClock.now + .seconds(5)
        while case .installing(let progress) = flow.step, ContinuousClock.now < deadline {
            if stages.last != progress.stage { stages.append(progress.stage) }
            fractions.append(progress.fraction)
            try await Task.sleep(for: .milliseconds(2))
        }
        #expect(stages.first == .connecting)
        #expect(stages.contains(.downloading) && stages.contains(.uploading))
        #expect(fractions == fractions.sorted(), "the bar never goes back")
        let install = try #require(await h.core.received.first { $0.method == "setup.install" })
        #expect(install.params == ["kind": "3ds", "host": "192.168.1.57", "port": 5000, "program": ["release": "latest"]])

        guard case .waitingForDevice(let info) = flow.step else {
            Issue.record("expected waitingForDevice, got \(flow.step)"); return
        }
        #expect(info.report.device == "5e7a11d0c0ffee01")
        #expect(info.method == .network(host: "192.168.1.57", port: 5000))
        // The record exists, not linked yet.
        await eventually("record") { h.model.device("5e7a11d0c0ffee01") != nil }
        #expect(h.model.device("5e7a11d0c0ffee01")?.isLinked == false)

        await eventually("finished by itself") { flow.step == .finished(deviceId: "5e7a11d0c0ffee01") }
        #expect(h.model.device("5e7a11d0c0ffee01")?.isLinked == true)
    }

    @Test(arguments: [SetupStage.connecting, .reading, .downloading, .uploading, .pairing])
    func eachFailureStageIsSaidPlainly(stage: SetupStage) async throws {
        let h = try await Harness(Self.scenario(failAt: stage))
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        flow.install(SetupCandidate(address: "192.168.1.57", port: 5000))
        await eventually("failed") { if case .failed = flow.step { true } else { false } }
        guard case .failed(let failure) = flow.step else { return }
        #expect(failure.stage == stage)
        #expect(!failure.title.isEmpty && !failure.advice.isEmpty)
        #expect(failure.hubMessage != nil)
        for word in ["runtime", "shell", "pairing"] {
            #expect(!failure.title.lowercased().contains(word) && !failure.advice.lowercased().contains(word))
        }
        #expect(failure.recovery == (stage == .connecting ? .searchAgain : .installAgain))
        #expect(h.model.device("5e7a11d0c0ffee01") == nil, "a failed setup leaves no record in the mock")

        // The one next action works once the cause is gone.
        await h.core.setSetupBehavior({ var b = Self.scenario().setup; b.linkAfter = .milliseconds(100); return b }())
        flow.discoveryInterval = .milliseconds(50)
        flow.retry()
        await eventually("finished after retry", timeout: .seconds(8)) {
            if case .found(let candidates) = flow.step { flow.install(candidates[0]) }
            return flow.step == .finished(deviceId: "5e7a11d0c0ffee01")
        }
    }

    @Test func otherFailures() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()

        await h.core.failNext("setup.install", with: .init(kind: "internal", message: "Disk full."))
        flow.install(SetupCandidate(address: "192.168.1.57", port: 5000))
        await eventually("internal failure") { if case .failed = flow.step { true } else { false } }
        if case .failed(let failure) = flow.step {
            #expect(failure.stage == nil && failure.recovery == .installAgain && failure.hubMessage == "Disk full.")
        }

        await h.core.failNext("setup.discover", with: .init(kind: "internal", message: "No interfaces."))
        flow.startSearching()
        await eventually("discovery failure") { if case .failed = flow.step { true } else { false } }
        if case .failed(let failure) = flow.step { #expect(failure.recovery == .searchAgain) }

        flow.startOver()
        #expect(flow.step == .instructions)
    }

    @Test func cancellingAnInstallCancelsItOnHub() async throws {
        var scenario = Self.scenario()
        scenario.setup.stepDelay = .milliseconds(200)
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        flow.install(SetupCandidate(address: "192.168.1.57", port: 5000))
        await eventually("installing") {
            if case .installing(let p) = flow.step { p.stage != .connecting || p.hubMessage != nil } else { false }
        }
        flow.cancel()
        await eventually("back to the start") { flow.step == .instructions }
        #expect(await h.core.received.contains { $0.method == "cancel" })
    }

    @Test func aConsoleAlreadyLinkedFinishesAtOnce() async throws {
        var scenario = MockScenario.demo
        scenario.setup.stepDelay = .milliseconds(5)
        scenario.setup.deviceId = "0123456789abcdef" // the linked console of the demo
        scenario.setup.linkAfter = nil
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        flow.install(SetupCandidate(address: "192.168.1.31", port: 5000))
        await eventually("finished") { flow.step == .finished(deviceId: "0123456789abcdef") }
    }

    @Test func anUnconfirmedKeyIsAFailureThatStillFinishesIfTheConsoleLinks() async throws {
        var scenario = Self.scenario(linkAfter: nil)
        scenario.setup.confirmed = false
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        flow.install(SetupCandidate(address: "192.168.1.57", port: 5000))
        await eventually("failed") { if case .failed = flow.step { true } else { false } }
        guard case .failed(let failure) = flow.step else { return }
        #expect(failure.unconfirmedDevice == "5e7a11d0c0ffee01")
        #expect(failure.title == String(localized: "Pocket Hub couldn't double-check the last step."))
        await eventually("flagged") { h.model.device("5e7a11d0c0ffee01")?.setupUnconfirmed == true }
        // The key was in place after all: the console links and the flow ends.
        await h.core.link("5e7a11d0c0ffee01")
        await eventually("finished") { flow.step == .finished(deviceId: "5e7a11d0c0ffee01") }
    }

    @Test(arguments: [("unreachable", SetupFailure.Recovery.searchAgain), ("companions_full", .startOver),
                      ("release_unavailable", .installAgain), ("storage", .installAgain), ("card", .startOver)])
    func reasonsDecideTheWords(reason: String, recovery: SetupFailure.Recovery) async throws {
        var scenario = Self.scenario()
        scenario.setup.failAt = .reading
        scenario.setup.failureReason = reason
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        flow.install(SetupCandidate(address: "192.168.1.57", port: 5000))
        await eventually("failed") { if case .failed = flow.step { true } else { false } }
        guard case .failed(let failure) = flow.step else { return }
        #expect(failure.recovery == recovery, "\(reason)")
        #expect(failure.stage == .reading)
        // The reason, not the stage, words it: no two reasons share a title.
        let byStage = SetupFailure(setup: HubOperationError(kind: .setup, message: "", step: "reading"))
        #expect(failure.title != byStage.title || reason == "storage", "\(reason)")
    }

    @Test func manualAddressAndCard() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        flow.enterAddressManually()
        #expect(flow.step == .manualAddress)
        flow.manualAddress = "192.168.1.57:5000"
        #expect(flow.manualAddressIsValid)
        flow.installFromManualAddress()
        await eventually("waiting") { if case .waitingForDevice = flow.step { true } else { false } }
        let install = try #require(await h.core.received.last { $0.method == "setup.install" })
        #expect(install.params["host"] == "192.168.1.57" && install.params["port"] == 5000)

        let card = h.model.makeSetupFlow()
        card.installFromCard(at: h.home)
        await eventually("card waiting") { if case .waitingForDevice = card.step { true } else { false } }
        if case .waitingForDevice(let info) = card.step { #expect(info.method == .card(path: h.home.path(percentEncoded: false))) }
        #expect(await h.core.received.contains { $0.method == "setup.card" && $0.params["path"]?.stringValue == h.home.path(percentEncoded: false) })
    }

    @Test func addressParsing() {
        #expect(SetupFlowModel.parse("192.168.1.57")! == ("192.168.1.57", nil))
        #expect(SetupFlowModel.parse(" 192.168.1.57:5000 ")! == ("192.168.1.57", 5000))
        #expect(SetupFlowModel.parse("my-3ds.local")! == ("my-3ds.local", nil))
        #expect(SetupFlowModel.parse("") == nil)
        #expect(SetupFlowModel.parse("192.168.1.57:") == nil)
        #expect(SetupFlowModel.parse("192.168.1.57:99999") == nil)
        #expect(SetupFlowModel.parse("a b") == nil)
        #expect(SetupFlowModel.parse("ftp://x") == nil)
    }

    @Test func overallProgressIsMonotonic() {
        var last = -1.0
        for stage in [SetupStage.connecting, .reading, .downloading, .uploading, .pairing, .done] {
            for done in [0, 50, 100] {
                let value = SetupFlowModel.overall(SetupProgress(stage: stage, done: Int64(done), total: 100))
                #expect(value >= last)
                last = value
            }
        }
        #expect(last == 1)
    }
}
