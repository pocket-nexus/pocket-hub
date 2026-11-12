import Darwin
import Foundation
import Testing
@testable import HubClient

/// Where the app finds the core it starts: one place each, nothing searched.
@Suite("Core launcher")
struct CoreLauncherTests {
    /// A folder named like an app, with an executable at Contents/Helpers/pocket-hubd.
    private func appBundle(in folder: URL, withHelper: Bool = true) throws -> Bundle {
        let app = folder.appending(path: "Pocket Hub.app")
        let helpers = app.appending(path: "Contents/Helpers")
        try FileManager.default.createDirectory(at: helpers, withIntermediateDirectories: true)
        if withHelper {
            let helper = helpers.appending(path: "pocket-hubd")
            try "#!/bin/sh\n".write(to: helper, atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: helper.path)
        }
        return try #require(Bundle(url: app))
    }

    @Test func theAppBundlesHelper() throws {
        let folder = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: folder) }
        let bundle = try appBundle(in: folder)
        let launcher = CoreLauncher.standard(bundle: bundle, environment: [:])
        #expect(launcher.locate()?.path == bundle.bundleURL.appending(path: "Contents/Helpers/pocket-hubd").path)
    }

    @Test func pocketHubdWhenItIsSet() throws {
        let folder = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: folder) }
        let bundle = try appBundle(in: folder)
        let explicit = folder.appending(path: "my-hubd")
        try "#!/bin/sh\n".write(to: explicit, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: explicit.path)
        let launcher = CoreLauncher.standard(bundle: bundle, environment: ["POCKET_HUBD": explicit.path])
        #expect(launcher.locate()?.path == explicit.path)
    }

    /// A wrong $POCKET_HUBD is a missing core, not a reason to start the bundle's.
    @Test func aPocketHubdThatNamesNothingIsNotReplaced() throws {
        let folder = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: folder) }
        let bundle = try appBundle(in: folder)
        let launcher = CoreLauncher.standard(bundle: bundle, environment: ["POCKET_HUBD": folder.appending(path: "nowhere").path])
        #expect(launcher.locate() == nil)
        if case .notFound = launcher.launch() {} else { Issue.record("expected notFound") }
    }

    /// Outside an app bundle (a test runner, a bare executable) there is no core
    /// to start, even inside the repository that builds one.
    @Test func noCoreOutsideAnAppBundle() throws {
        let folder = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: folder) }
        let plain = try #require(Bundle(url: folder))
        #expect(CoreLauncher.standard(bundle: plain, environment: [:]).locate() == nil)
        let empty = try appBundle(in: folder, withHelper: false)
        #expect(CoreLauncher.standard(bundle: empty, environment: [:]).locate() == nil)
    }

    /// The client starts the core from an actor, on one of the threads Swift's
    /// concurrency runs on, which block SIGTERM. What it starts must not
    /// inherit that: the core ends at SIGTERM (CONTROL-API.md §1).
    @Test func whatItStartsEndsAtSIGTERM() async throws {
        let folder = try temporaryHome()
        defer { try? FileManager.default.removeItem(at: folder) }
        // Stands for the core: a process that does nothing about signals.
        let script = folder.appending(path: "pocket-hubd")
        try "#!/bin/sh\nexec /bin/sleep 30\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        actor Starter {
            func start(_ launcher: CoreLauncher) -> CoreLauncher.Outcome { launcher.launch() }
        }
        guard case .started(let core) = await Starter().start(CoreLauncher(executable: { script }, environment: [:])) else {
            Issue.record("expected the stand-in to start")
            return
        }
        defer { kill(core.pid, SIGKILL) }
        core.discardOutput()
        try await Task.sleep(for: .milliseconds(200))
        #expect(core.exitStatus == nil, "it runs until it is told to end")
        kill(core.pid, SIGTERM)
        let deadline = ContinuousClock.now + .seconds(5)
        while core.exitStatus == nil, ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(20)) }
        #expect(core.exitStatus == 128 + SIGTERM, "ended by the signal, not still running with it blocked")
    }
}
