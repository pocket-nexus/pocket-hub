import Foundation
import HubClient
import MockHubCore
import Testing
@testable import HubAppModel

/// Links that open the app (docs/DEEP-LINKS.md): the grammar of §1, and the
/// catalog lookup and the choice of a device of §2.
@Suite("Deep links")
@MainActor
struct DeepLinkTests {
    let living = "0123456789abcdef"
    let travel = "89abcdef01234567"

    func link(_ text: String) -> DeepLink? {
        URL(string: text).flatMap(DeepLink.init)
    }

    // MARK: - §1 The link

    @Test func theStoresLinkNamesAnApplet() {
        #expect(link("pocket-hub://store/app/dev.pocket-nexus.hn") == .storeApp(id: "dev.pocket-nexus.hn"))
        #expect(link("pocket-hub://store/app/io.example.pixels") == .storeApp(id: "io.example.pixels"))
        #expect(link("pocket-hub://store/app/a1.b2.c3.d4.e5.f6") == .storeApp(id: "a1.b2.c3.d4.e5.f6"), "up to six labels")
    }

    @Test func theSchemeAndTheHostCompareWithoutCase() {
        #expect(link("Pocket-Hub://STORE/app/dev.pocket-nexus.hn") == .storeApp(id: "dev.pocket-nexus.hn"))
        #expect(link("pocket-hub://store/App/dev.pocket-nexus.hn") == nil, "the path does not")
        #expect(link("pocket-hub://store/app/Dev.Pocket-Nexus.HN") == nil, "nor does an id")
    }

    @Test func aQueryAndAFragmentAreIgnored() {
        #expect(link("pocket-hub://store/app/dev.pocket-nexus.hn?version=1.2.0&from=web#top") == .storeApp(id: "dev.pocket-nexus.hn"))
    }

    @Test(arguments: [
        "https://applets.pocket.nexus/app/dev.pocket-nexus.hn",   // another scheme
        "pockethub://store/app/dev.pocket-nexus.hn",
        "pocket-hub://devices/app/dev.pocket-nexus.hn",           // another host
        "pocket-hub:store/app/dev.pocket-nexus.hn",               // no host
        "pocket-hub:///app/dev.pocket-nexus.hn",
        "pocket-hub://store",                                      // another path
        "pocket-hub://store/",
        "pocket-hub://store/app",
        "pocket-hub://store/app/",
        "pocket-hub://store/dev.pocket-nexus.hn",
        "pocket-hub://store/app/dev.pocket-nexus.hn/",
        "pocket-hub://store/app/dev.pocket-nexus.hn/install",
        "pocket-hub://store/apps/dev.pocket-nexus.hn",
        "pocket-hub://store//app/dev.pocket-nexus.hn",
        "pocket-hub://user@store/app/dev.pocket-nexus.hn",        // a user, a port
        "pocket-hub://store:8628/app/dev.pocket-nexus.hn",
        "pocket-hub://store/app/hn",                              // an id outside the grammar
        "pocket-hub://store/app/dev.hn",
        "pocket-hub://store/app/dev..hn",
        "pocket-hub://store/app/dev.pocket-nexus.-hn",
        "pocket-hub://store/app/dev.pocket_nexus.hn",
        "pocket-hub://store/app/a.b.c.d.e.f.g",
        "pocket-hub://store/app/dev.pocket-nexus.hn%20",
        "pocket-hub://store/app/dev%2Epocket-nexus%2Ehn",         // an id is written as it is
        "pocket-hub://store/app%2Fdev.pocket-nexus.hn",
        "pocket-hub://store/app/..%2F..%2Fetc.passwd.x",
    ])
    func anythingElseIsNotALink(_ text: String) {
        #expect(link(text) == nil)
    }

    // MARK: - §2 What Hub does with it

    @Test func anAppletTheCatalogHoldsIsFoundWithoutAFetch() async throws {
        let h = try await Harness(StoreModelTests.scenario(.firstRun))
        defer { Task { await h.tearDown() } }
        let app = try #require(try await h.model.store.lookUp(appId: "nexus.pocket.hackernews"))
        #expect(app.title == "Hacker News")
        #expect(app.action == nil, "out of the whole catalog, for no device")
        let asked = await h.core.received.filter { $0.method == "store.apps" }
        #expect(!asked.contains { $0.params["refresh"] != nil }, "the catalog Hub holds answered")
    }

    @Test func anIdTheCatalogLacksIsAskedForOnceMoreFresh() async throws {
        let h = try await Harness(StoreModelTests.scenario(.firstRun))
        defer { Task { await h.tearDown() } }
        let app = try await h.model.store.lookUp(appId: "dev.pocket-nexus.unlisted")
        #expect(app == nil)
        let asked = await h.core.received.filter { $0.method == "store.apps" }
        #expect(asked.filter { $0.params["refresh"] == true }.count == 1, "published since the last fetch, perhaps")
        #expect(asked.last?.params == ["refresh": true], "and it is the last thing asked")
    }

    @Test func aStoreThatDoesNotAnswerIsAFailureNotAMiss() async throws {
        var scenario = StoreModelTests.scenario(.firstRun)
        scenario.storeReachable = false
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        do {
            _ = try await h.model.store.lookUp(appId: "nexus.pocket.hackernews")
            Issue.record("expected the store's failure")
        } catch {
            #expect(UserFacingError(storeError: error).title == String(localized: "Pocket Hub can't reach the applet store."))
        }
    }

    @Test func aLinkedDeviceIsChosenBeforeOneThatIsAway() async throws {
        let h = try await Harness(StoreModelTests.scenario())
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        let hn = try #require(try await store.lookUp(appId: "nexus.pocket.hackernews"))
        #expect(store.devices(running: hn).map(\.id) == [living, travel])
        #expect(store.device(for: hn, preferring: nil)?.id == living)
        #expect(store.device(for: hn, preferring: travel)?.id == living, "the page open is of a device that cannot take it now")
        #expect(store.device(for: hn, preferring: "no such device")?.id == living)
    }

    @Test func amongLinkedDevicesTheOneShownIsChosen() async throws {
        var scenario = StoreModelTests.scenario()
        scenario.devices[1].linkUp(address: "192.168.1.77")
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        let hn = try #require(try await store.lookUp(appId: "nexus.pocket.hackernews"))
        #expect(store.device(for: hn, preferring: travel)?.id == travel)
        #expect(store.device(for: hn, preferring: living)?.id == living)
        #expect(store.device(for: hn, preferring: nil)?.id == h.model.devices.first?.id, "else the first, as the sidebar lists them")
    }

    @Test func aDeviceThatIsAwayIsChosenWhenNoneIsLinked() async throws {
        var scenario = StoreModelTests.scenario()
        scenario.devices.removeFirst()
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        let hn = try #require(try await store.lookUp(appId: "nexus.pocket.hackernews"))
        #expect(store.device(for: hn, preferring: nil)?.id == travel)
    }

    @Test func noDeviceIsChosenForAnAppletNoneCanRun() async throws {
        var scenario = StoreModelTests.scenario()
        // A device that has never linked: no known target.
        scenario.devices.append(MockDevice(id: "fedcba9876543210"))
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        let tab = try #require(try await store.lookUp(appId: "io.example.tabonly"))
        #expect(store.devices(running: tab).isEmpty, "a variant for the Tab5 alone")
        #expect(store.device(for: tab, preferring: living) == nil)
        let hn = try #require(try await store.lookUp(appId: "nexus.pocket.hackernews"))
        #expect(!store.devices(running: hn).contains { $0.id == "fedcba9876543210" }, "its target is not known yet")

        let none = try await Harness(StoreModelTests.scenario(.firstRun))
        defer { Task { await none.tearDown() } }
        #expect(none.model.store.device(for: hn, preferring: nil) == nil, "no device is paired")
    }

    @Test func aLinkWaitsForTheCore() async throws {
        let home = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let client = HubClient(configuration: .init(
            identity: ClientIdentity(kind: .gui, name: "Pocket Hub", version: "0.0.1"),
            socketPath: { HubPaths(home: home).socketPath() },
            retryDelays: [.milliseconds(50), .milliseconds(100)]))
        let model = HubModel(client: client)
        model.start()
        // What following a link does first (AppState.open): no core answers yet.
        let found = Task { @MainActor () -> [String] in
            await model.whenLive()
            return model.devices.map(\.id)
        }
        try await Task.sleep(for: .milliseconds(200))
        #expect(!model.isLive)
        let core = MockHubCore(home: home, scenario: StoreModelTests.scenario())
        try await core.start()
        #expect(await found.value.count == 2, "live together with its lists")
        await model.quit(shutdownCore: false)
        await core.stop()
    }
}
