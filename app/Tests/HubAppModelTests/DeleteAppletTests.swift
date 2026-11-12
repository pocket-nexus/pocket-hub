import Foundation
import HubClient
import MockHubCore
import Testing
@testable import HubAppModel

/// Deleting an installed applet (CONTROL-API.md `devices.remove`, link
/// specification §6.2 `runtime.remove`).
@Suite("Deleting an applet")
@MainActor
struct DeleteAppletTests {
    let living = "0123456789abcdef"

    @Test func deletingDropsTheRowAndReleasesTheDevice() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        let device = try #require(h.model.device(living))
        #expect(device.deletesApplets, "the device's link uses runtime.remove")
        let notes = try #require(device.apps?.first { $0.id == "nexus.pocket.notes" })
        let deleting = Task { try await h.model.remove(notes, from: living) }
        await eventually("deleting") { h.model.removing[living] == notes.id }
        await eventually("held while deleting") { h.model.device(living)?.busy?.holder.method == "devices.remove" }
        #expect(h.model.device(living)?.busy?.holder.activity == String(localized: "deleting an applet"))
        try await deleting.value
        #expect(h.model.removing[living] == nil)
        await eventually("its row is gone") { h.model.device(living)?.apps?.contains { $0.id == notes.id } == false }
        await eventually("released") { h.model.device(living)?.busy == nil }
    }

    @Test func theDevicesRefusalsAreSaidPlainly() async throws {
        let h = try await Harness(.demo)
        defer { Task { await h.tearDown() } }
        let apps = try #require(h.model.device(living)?.apps)
        let home = try #require(apps.first { $0.isHome })
        let notes = try #require(apps.first { $0.id == "nexus.pocket.notes" })

        do {
            try await h.model.remove(home, from: living)
            Issue.record("expected the home screen to be refused")
        } catch {
            let message = try #require(UserFacingError(deleteError: error, appTitle: "Home screen", deviceName: "Living Room 3DS"))
            #expect(message.title == String(localized: "The home screen can't be deleted."))
        }

        try await h.model.launch(notes, on: living)
        await eventually("Notes runs") { h.model.device(living)?.runningApp?.id == notes.id }
        do {
            try await h.model.remove(notes, from: living)
            Issue.record("expected the running applet to be refused")
        } catch {
            let message = try #require(UserFacingError(deleteError: error, appTitle: "Notes", deviceName: "Living Room 3DS"))
            #expect(message.title == String(localized: "Notes is open on Living Room 3DS."))
        }
        #expect(h.model.device(living)?.apps?.contains { $0.id == notes.id } == true, "nothing was deleted")

        // Software that predates deleting from Hub answers `unsupported`.
        await h.core.failNext("devices.remove", with: .init(kind: "link", message: "The device does not support runtime.remove.",
                                                            data: ["code": "unsupported"]))
        do {
            try await h.model.remove(notes, from: living)
            Issue.record("expected unsupported")
        } catch {
            let message = try #require(UserFacingError(deleteError: error, appTitle: "Notes", deviceName: "Living Room 3DS"))
            #expect(message == .deletingNeedsNewerSoftware(deviceName: "Living Room 3DS"))
        }
    }
}
