import Foundation
import HubClient
import MockHubCore
import Testing
@testable import HubAppModel

@Suite("Applet store")
@MainActor
struct StoreModelTests {
    static func scenario(_ base: MockScenario = .demo) -> MockScenario {
        var scenario = base
        scenario.storeStepDelay = .milliseconds(20)
        scenario.mutexHold = .milliseconds(20)
        return scenario
    }

    func loaded(_ store: StoreModel) -> Bool {
        if case .loaded = store.page { true } else { false }
    }

    func app(_ store: StoreModel, _ id: String) -> StoreApp? {
        store.apps.first { $0.id == id }
    }

    @Test func withNoDeviceTheWholeCatalogShowsWithoutActions() async throws {
        let h = try await Harness(Self.scenario(.firstRun))
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        #expect(store.context == .noDevice)
        #expect(store.apps.count == 5, "every applet, whatever its targets")
        #expect(store.apps.allSatisfy { $0.action == nil })
        #expect(store.known["nexus.pocket.hackernews"]?.title == "Hacker News")
        let request = try #require(await h.core.received.last { $0.method == "store.apps" })
        #expect(request.params == [:])
    }

    @Test func aDevicesPageCarriesItsActions() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        #expect(store.context == .device(id: "0123456789abcdef", knownTarget: true), "the linked device by default")
        #expect(app(store, "io.example.tabonly") == nil, "no variant for a 3DS")
        #expect(app(store, "nexus.pocket.hackernews")?.action == .install)
        #expect(app(store, "nexus.pocket.weather")?.action == .update, "0.4.0 above the device's 0.3.1")
        #expect(app(store, "nexus.pocket.notes")?.action == .installed, "an equal version is no update")
        let icon = try #require(app(store, "nexus.pocket.notes")?.icon)
        #expect(FileManager.default.fileExists(atPath: icon))
        #expect(store.known.count == 5, "the catalog by id comes from the whole catalog")
        // The Store page lists the whole catalog, in the store's order,
        // whatever the device; the device's own list says what it can run.
        #expect(store.all.map(\.id) == ["nexus.pocket.hackernews", "nexus.pocket.weather", "nexus.pocket.notes", "io.example.pixels", "io.example.tabonly"])
        #expect(store.list(for: "0123456789abcdef")?.contains { $0.id == "io.example.tabonly" } == false)
        #expect(store.list(for: "89abcdef01234567") == nil, "another device's list is not here")
    }

    @Test func installingShowsProgressAndEndsInstalled() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        let living = "0123456789abcdef"
        let pixels = try #require(app(store, "io.example.pixels"))
        #expect(!pixels.cached)
        var phases: [String] = []
        store.install(pixels, on: living)
        let deadline = ContinuousClock.now + .seconds(5)
        while let install = store.install(for: pixels, on: living), ContinuousClock.now < deadline {
            if let phase = install.phase, phases.last != phase { phases.append(phase) }
            try await Task.sleep(for: .milliseconds(2))
        }
        #expect(phases.first == "downloading")
        #expect(phases.contains("sending"))
        #expect(store.installFailure == nil)
        await eventually("installed") { app(store, "io.example.pixels")?.action == .installed }
        #expect(h.model.device(living)?.apps?.contains { $0.id == "io.example.pixels" } == true)
        let call = try #require(await h.core.received.last { $0.method == "store.install" })
        #expect(call.params == ["device": .string(living), "app": "io.example.pixels"])
    }

    @Test func failuresAreSaidPlainly() async throws {
        var scenario = Self.scenario()
        scenario.storeRejected = ["nexus.pocket.hackernews"]
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        let living = "0123456789abcdef"
        let hn = try #require(app(store, "nexus.pocket.hackernews"))

        store.install(hn, on: living)
        await eventually("rejected") { store.installFailure != nil }
        #expect(store.installFailure?.title == String(localized: "This applet didn't pass Pocket Hub's safety check."))
        store.installFailure = nil

        await h.core.failNext("store.install", with: .init(kind: "busy", message: "Busy.",
            data: ["holder": ["kind": "cli", "name": "pocket-hub", "method": "store.install"]]))
        store.install(hn, on: living)
        await eventually("busy") { store.installFailure != nil }
        #expect(store.installFailure?.advice.contains(String(localized: "The command line")) == true)
        store.installFailure = nil

        await h.core.failNext("store.install", with: .init(kind: "not_found", message: "No variant."))
        store.install(hn, on: living)
        await eventually("not found") { store.installFailure != nil }
        #expect(store.installFailure?.title.contains("Hacker News") == true)
        store.installFailure = nil

        await h.core.failNext("store.install", with: .init(kind: "not_linked", message: "Not linked."))
        store.install(hn, on: living)
        await eventually("not linked") { store.installFailure != nil }
        #expect(store.installFailure?.title == String(localized: "\(h.model.device(living)!.displayName) isn't connected right now."))
    }

    @Test func anInstallTheLinkEndsUnderIsOneMessageUntilTheDeviceIsBack() async throws {
        var scenario = Self.scenario()
        scenario.storeLinkLoss = ["nexus.pocket.hackernews"]
        scenario.storeRelinkAfter = .milliseconds(500)
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        let living = "0123456789abcdef"
        let hn = try #require(app(store, "nexus.pocket.hackernews"))
        store.install(hn, on: living)
        await eventually("interrupted") { store.interrupted != nil }
        #expect(store.interrupted?.deviceId == living && store.interrupted?.appTitle == "Hacker News")
        #expect(store.installFailure == nil, "no alert beside the page's own message")
        #expect(store.install(for: hn, on: living) == nil)
        #expect(h.model.lastUnlink[living]?.code == "silence")
        // Back on a new link with the applet: the list, from its apps, says so.
        await eventually("back") { store.interrupted == nil && h.model.device(living)?.isLinked == true }
        await eventually("installed from the device's report") { app(store, "nexus.pocket.hackernews")?.action == .installed }
        #expect(store.installFailure == nil)
    }

    @Test func aLinkErrorOnALinkThatStaysIsAFailure() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        await h.core.failNext("store.install", with: .init(kind: "link", message: "The device answered timeout.", data: ["code": "timeout"]))
        store.install(try #require(app(store, "nexus.pocket.hackernews")), on: "0123456789abcdef")
        await eventually("failure") { store.installFailure != nil }
        #expect(store.interrupted == nil)
    }

    @Test func anUnreachableStoreShowsNoOlderList() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        await h.core.setStoreReachable(false)
        store.reload(refresh: true)
        await eventually("failed") { if case .failed = store.page { true } else { false } }
        #expect(store.apps.isEmpty, "the list on screen is gone once the store fails")
        if case .failed(let failure) = store.page {
            #expect(failure.title == String(localized: "Pocket Hub can't reach the applet store."))
        }
        await h.core.setStoreReachable(true)
        store.reload(refresh: true, clearing: true)
        await eventually("loaded again") { loaded(store) }
        #expect(await h.core.received.contains { $0.method == "store.apps" && $0.params["refresh"] == true })
    }

    @Test func storeChangedAndDeviceChangesReload() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        var catalog = MockScenario.storeCatalog
        catalog[2].version = "1.3.0" // notes, held at 1.2.0
        await h.core.setStore(catalog)
        await eventually("notes offers an update") { app(store, "nexus.pocket.notes")?.action == .update }
        // The device's apps change (an install from elsewhere): installed follows.
        await h.core.modify("0123456789abcdef") { device in
            device.apps = [["id": "nexus.pocket.hackernews", "output": "hn", "version": "1.1.0", "hash": "aa"]]
        }
        await eventually("hn installed") { app(store, "nexus.pocket.hackernews")?.action == .installed }
        #expect(app(store, "nexus.pocket.notes")?.action == .install)
    }

    @Test func cancellingAnInstall() async throws {
        var scenario = Self.scenario()
        scenario.storeStepDelay = .milliseconds(300)
        let h = try await Harness(scenario)
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        let pixels = try #require(app(store, "io.example.pixels"))
        store.install(pixels, on: "0123456789abcdef")
        await eventually("downloading") { store.install(for: pixels, on: "0123456789abcdef")?.phase == "downloading" }
        store.cancelInstall(of: pixels, on: "0123456789abcdef")
        await eventually("stopped") { store.install(for: pixels, on: "0123456789abcdef") == nil }
        #expect(store.installFailure == nil, "a cancellation needs no message")
        #expect(await h.core.received.contains { $0.method == "cancel" })
    }

    @Test func severalDevicesNotConnectedAndNeverLinked() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        // Paired, not connected: its target's catalog, with its own installed state.
        store.choose(device: "89abcdef01234567")
        await eventually("travel's page") {
            loaded(store) && store.context == .device(id: "89abcdef01234567", knownTarget: true)
        }
        #expect(h.model.device("89abcdef01234567")?.isLinked == false)
        #expect(app(store, "nexus.pocket.notes")?.action == .install)
        #expect(await h.core.received.last { $0.method == "store.apps" && $0.params["device"] != nil }?.params["device"] == "89abcdef01234567")
        // A device that never linked has no known target: nothing is asked for it.
        await h.core.upsert(MockDevice(id: "fffffffffffffff0"))
        await eventually("listed") { h.model.device("fffffffffffffff0") != nil }
        store.choose(device: "fffffffffffffff0")
        await eventually("never linked") { store.context == .device(id: "fffffffffffffff0", knownTarget: false) && loaded(store) }
        #expect(store.apps.isEmpty)
        #expect(await !h.core.received.contains { $0.method == "store.apps" && $0.params["device"] == "fffffffffffffff0" })
    }

    @Test func anAppletsDetailsAreTheStoresReadingOfItsPackage() async throws {
        let h = try await Harness(Self.scenario())
        defer { Task { await h.tearDown() } }
        let store = h.model.store
        await eventually("loaded") { loaded(store) }
        let hn = try #require(app(store, "nexus.pocket.hackernews"))

        let details = try await store.details(of: hn).get()
        let request = try #require(await h.core.received.last { $0.method == "store.details" })
        #expect(request.params == ["app": "nexus.pocket.hackernews"])
        #expect(details.version == hn.version)
        // The store's names, in its order, each device once (the mock's `3ds`
        // and `3ds-dev` are both a 3DS).
        #expect(details.deviceNames == ["Omarchy Pocket", "M5Stack Tab5", "Nintendo 3DS"])
        #expect(details.needs == ["text.glyphs.baked", "input.buttons", "display.auxiliary", "input.touch.auxiliary", "net.http"])
        #expect(details.screenText == "Touch, 320 × 240 to 640 × 720", "English in every language")

        // A store that does not answer: the store's failure, in plain words.
        await h.core.setStoreReachable(false)
        guard case .failure(let failure) = await store.details(of: hn) else {
            Issue.record("details from a store that does not answer")
            return
        }
        #expect(failure.title == String(localized: "Pocket Hub can't reach the applet store."))
    }

    @Test func aSurfaceIsSaidOnlyWhenItSaysSomething() {
        let screen = { (surface: StoreDetails.Surface?) in
            StoreDetails(id: "a", version: "1.0.0", devices: [], surface: surface).screenText
        }
        let sized = StoreDetails.Surface.Size(min: [320, 240], max: [640, 720])
        #expect(screen(nil) == nil)
        #expect(screen(.init(touch: false)) == nil, "the device's own screen, no sizes")
        #expect(screen(.init(touch: true)) == "Touch")
        #expect(screen(.init(touch: false, size: sized)) == "320 × 240 to 640 × 720")
        #expect(screen(.init(touch: true, size: .init(min: [320], max: [640, 720]))) == "Touch", "a size that is not [w, h]")
    }
}
