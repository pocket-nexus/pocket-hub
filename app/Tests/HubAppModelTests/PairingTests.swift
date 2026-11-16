import Foundation
import HubClient
import MockHubCore
import Testing
@testable import HubAppModel

/// Adding a device that shows a code (CONTROL-API.md §3 Pairing): the flow
/// from the request to the paired device, and what a Tab5's page offers.
@Suite("Pairing by a code, and a Tab5")
@MainActor
struct PairingTests {
    static let tab5 = "7ab5000000000002"

    static func scenario() -> MockScenario {
        var scenario = MockScenario.firstRun
        scenario.pairing.answerDelay = .milliseconds(20)
        scenario.pairing.askAgainAfter = .milliseconds(80)
        scenario.pairing.linkAfter = .milliseconds(150)
        return scenario
    }

    static func request(code: String = "314159") -> MockPairRequest {
        var request = MockPairRequest(device: tab5)
        request.name = "Kitchen Tab5"
        request.code = code
        return request
    }

    func entry(_ flow: SetupFlowModel) -> SetupFlowModel.PairEntry? {
        if case .pairCode(let entry) = flow.step { entry } else { nil }
    }

    @Test func theOwnerChoosesATab5AndTypesItsCode() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        #expect(flow.step == .chooseDevice)
        flow.choose(.tab5)
        #expect(flow.step == .pairWaiting, "no device asks yet")
        #expect(!flow.canSubmitPairCode)

        // Its owner chooses this Mac on the device: the request reaches the page.
        await h.core.askToPair(Self.request())
        await eventually("the request") { entry(flow)?.isAsking == true }
        #expect(h.model.pairRequests.map(\.device) == [Self.tab5])
        let asked = try #require(entry(flow))
        #expect(asked.request.displayName == "Kitchen Tab5" && asked.request.kind == .tab5 && asked.problem == nil)

        // Six ASCII digits are a code; spaces between them are dropped.
        for (typed, digits) in [("", nil), ("31415", nil), ("3141592", nil), ("31415a", nil), ("٣١٤١٥٩", nil),
                                ("314159", "314159"), ("314 159", "314159"), (" 314159 ", "314159")] as [(String, String?)] {
            flow.pairCode = typed
            #expect(flow.pairCodeDigits == digits, "\(typed)")
            #expect(flow.canSubmitPairCode == (digits != nil), "\(typed)")
        }

        // Another code than the device's: nothing is paired, the page says so,
        // and the code can be typed again once the device asks again.
        flow.pairCode = "271 828"
        flow.submitPairCode()
        #expect(entry(flow)?.isAnswering == true && !flow.canSubmitPairCode)
        await eventually("the wrong code is said") { entry(flow)?.problem == .wrongCode }
        #expect(flow.pairCode.isEmpty && entry(flow)?.isAnswering == false)
        #expect(await h.core.received.last { $0.method == "pairing.answer" }?.params == ["device": .string(Self.tab5), "code": "271828"])
        #expect(h.model.devices.isEmpty)
        await eventually("the device asks again") { entry(flow)?.isAsking == true }
        #expect(entry(flow)?.problem == .wrongCode, "the reason stays until the next code")

        flow.pairCode = "314159"
        flow.submitPairCode()
        await eventually("paired") { if case .paired = flow.step { true } else { false } }
        guard case .paired(let device) = flow.step else { return }
        #expect(device.id == Self.tab5 && device.kind == .tab5 && device.displayName == "Kitchen Tab5" && !device.isLinked)
        #expect(h.model.pairRequests.isEmpty)
        // It links by itself, and is a device like any other from then on.
        await eventually("linked") { h.model.device(Self.tab5)?.isLinked == true }
        #expect(h.model.device(Self.tab5)?.target == "tab5-home")
    }

    @Test func aDeviceThatAsksOpensTheFlowOnItsCodeAndCancelTellsIt() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        await h.core.askToPair(Self.request())
        await eventually("the request") { h.model.pairRequests.count == 1 }
        // The request is there when the model syncs, too.
        let request = try #require(h.model.pairRequests.first)
        #expect(request.expires - request.since == 300_000 && request.address == "192.168.1.63")

        let flow = h.model.makeSetupFlow(answering: request)
        #expect(entry(flow)?.request == request && entry(flow)?.isAsking == true)
        // Closing the sheet dismisses the request, so the device stops showing its code.
        flow.declinePairRequest()
        await eventually("declined") { h.model.pairRequests.isEmpty }
        #expect(await h.core.received.contains { $0.method == "pairing.decline" && $0.params == ["device": .string(Self.tab5)] })
        // With nothing to answer, the page goes back to what to do on the device.
        await eventually("waiting again") { flow.step == .pairWaiting }

        // Choosing a Tab5 while one asks goes to its code at once.
        await h.core.askToPair(Self.request())
        await eventually("asks again") { entry(flow) != nil }
        let other = h.model.makeSetupFlow()
        await eventually("still listed") { !h.model.pairRequests.isEmpty }
        other.choose(.tab5)
        #expect(entry(other)?.request.device == Self.tab5)
    }

    @Test func aDeviceThatLeavesOrRunsOutOfTries() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        flow.choose(.tab5)
        await h.core.askToPair(Self.request())
        await eventually("the request") { entry(flow)?.isAsking == true }
        // The device leaves before a code is typed.
        flow.pairCode = "31"
        await h.core.withdrawPairRequest(Self.tab5)
        await eventually("back to waiting") { flow.step == .pairWaiting }
        #expect(flow.pairCode.isEmpty)

        // Three wrong codes: the device drops its code and does not ask again.
        var last = Self.request()
        last.tries = 1
        await h.core.askToPair(last)
        await eventually("the request") { entry(flow)?.isAsking == true }
        flow.pairCode = "000000"
        flow.submitPairCode()
        await eventually("the wrong code is said") { entry(flow)?.problem == .wrongCode }
        try await Task.sleep(for: .milliseconds(300))
        #expect(entry(flow)?.isAsking == false && !flow.canSubmitPairCode, "the page waits; nothing can be sent")
        flow.pairCode = "314159"
        flow.submitPairCode()
        #expect(entry(flow)?.isAnswering == false)
        #expect(h.model.devices.isEmpty)
    }

    @Test func aFailureThatIsNotTheCodes() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let flow = h.model.makeSetupFlow()
        flow.choose(.tab5)
        await h.core.askToPair(Self.request())
        await eventually("the request") { entry(flow)?.isAsking == true }

        // The device left while the code was on its way.
        await h.core.failNext("pairing.answer", with: .init(kind: "pairing", message: "The device left before the pairing finished. Nothing was stored.", data: ["reason": "gone"]))
        flow.pairCode = "314159"
        flow.submitPairCode()
        await eventually("interrupted") { entry(flow)?.problem == .interrupted }

        // Hub could not store the device: a failure of its own, with one next step.
        await h.core.failNext("pairing.answer", with: .init(kind: "internal", message: "The device took the pairing and Hub could not store its record: disk full."))
        flow.pairCode = "314159"
        flow.submitPairCode()
        await eventually("failed") { if case .failed = flow.step { true } else { false } }
        guard case .failed(let failure) = flow.step else { return }
        #expect(failure.recovery == .pairAgain && failure.hubMessage?.contains("disk full") == true)
        #expect(!failure.title.isEmpty && !failure.advice.isEmpty)
        // The request still waits, so trying again is its code's page.
        flow.retry()
        #expect(entry(flow)?.request.device == Self.tab5)
    }

    @Test func aTab5OffersWhatItsFamilyTakes() async throws {
        let h = try await Harness(.withTab5)
        defer { Task { await h.tearDown() } }
        let tab5 = try #require(h.model.device(MockScenario.tab5Id))
        let console = try #require(h.model.device("0123456789abcdef"))
        #expect(tab5.kind == .tab5 && tab5.displayName == "Desk Tab5" && tab5.isLinked)
        #expect(tab5.supports.isEmpty && console.supports == DeviceOperation.all)
        // Its own software is the launcher's row; Hub knows no release for it.
        #expect(tab5.program == DeviceProgram(version: "0.3.0", latest: nil, update: false))
        #expect(!tab5.offersSoftwareUpdate && console.offersSoftwareUpdate)
        // It deletes and opens applets as a 3DS does.
        #expect(tab5.deletesApplets && tab5.isOnHomeScreen)

        // The store lists what its own store page offers: the applets of its target.
        h.model.store.choose(device: tab5.id)
        h.model.store.reload()
        await eventually("the store's list") { if case .loaded = h.model.store.page { true } else { false } }
        #expect(Set(h.model.store.apps.map(\.id)) == ["nexus.pocket.hackernews", "nexus.pocket.notes", "io.example.tabonly"])

        // What Hub refuses for its family is said as that, not as a lost link.
        do {
            _ = try await h.model.takeScreenshot(of: tab5.id)
            Issue.record("a Tab5 takes no screenshot")
        } catch {
            let hubError = try #require(error as? HubError)
            #expect(hubError.kind == .link)
            let said = try #require(UserFacingError(error, deviceName: tab5.displayName, kind: tab5.kind))
            #expect(said.title == String(localized: "\(tab5.displayName) can't do this."))
            #expect(said.detail == "This device takes no screenshot.")
        }
        // A device that is away is asked back in its family's words.
        await h.core.unlink(tab5.id)
        await eventually("unlinked") { h.model.device(tab5.id)?.isLinked == false }
        #expect(h.model.lastUnlink[tab5.id]?.explanation == String(localized: "The device was turned off or went to sleep."))
        do {
            _ = try await h.model.client.launch(device: tab5.id, app: "notes")
            Issue.record("the device is away")
        } catch {
            let away = try #require(UserFacingError(error, deviceName: tab5.displayName, kind: tab5.kind))
            #expect(away.advice == String(localized: "Turn it on and make sure it is on the same Wi-Fi network as this Mac, then try again."))
            let console3ds = try #require(UserFacingError(error, deviceName: "3DS", kind: .nintendo3ds))
            #expect(console3ds.advice == String(localized: "Turn it on and open Pocket Shell, then try again."))
        }
    }

    @Test func familiesHaveNamesAndSymbols() {
        #expect(DeviceKind.nintendo3ds.displayName == String(localized: "Nintendo 3DS"))
        #expect(DeviceKind.tab5.displayName == String(localized: "M5Stack Tab5"))
        #expect(DeviceKind(rawValue: "").displayName == String(localized: "New device"))
        #expect(Device(id: "a", kind: .tab5).displayName == String(localized: "M5Stack Tab5"))
        #expect(PairRequest(device: "a", kind: .tab5, name: "").displayName == String(localized: "M5Stack Tab5"))
        #expect(DeviceKind.nintendo3ds.symbolName(filled: true) == "gamecontroller.fill")
        #expect(DeviceKind.tab5.symbolName(filled: true) == "rectangle.inset.filled")
        #expect(DeviceKind.tab5.howToConnect != DeviceKind.nintendo3ds.howToConnect)
    }
}
