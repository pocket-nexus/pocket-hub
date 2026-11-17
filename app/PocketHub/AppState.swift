import AppKit
import HubAppModel
import HubClient
import Observation
import SwiftUI

/// Where the window is: the sidebar selection and the sheets.
enum SidebarItem: Hashable {
    /// The applet store: the whole catalog, whatever devices are paired.
    case store
    case device(String)
    case daemons
}

/// The sections of a device's page. A screenshot has no section: it is
/// kept where the owner keeps screenshots (AppState.keep).
enum DeviceTab: Hashable {
    case applets, log, details
}

/// State shared by the window, the menu bar item and the app delegate.
@MainActor
@Observable
final class AppState {
    static let shared = AppState()

    let model: HubModel
    let launchAtLogin = LaunchAtLogin()
    let language = AppLanguage()
    var selection: SidebarItem?
    /// The section shown on a device's page; kept here so the menu bar and the
    /// keyboard commands can choose it, whichever page is open.
    var deviceTab: DeviceTab = .applets
    var showingSetup = false
    /// The request the Add a Device sheet opens on, when a device that asks
    /// to be paired opened it; nil when the owner did.
    var setupRequest: PairRequest?
    /// The flow of the sheet that is open, for the debug commands.
    weak var setupFlow: SetupFlowModel?
    /// The requests a sheet was already opened for (device id and time), so
    /// one the owner closed the sheet on does not open it again.
    private var presentedRequests: Set<String> = []
    /// The device page's file picker for a .pocket applet, and its question
    /// before removing the device: here so the Device menu can open them as
    /// the toolbar does (HIG, Toolbars › macOS: every toolbar item is also a
    /// menu command).
    var importingApplet = false
    var confirmingRemoveDevice = false
    /// The Store page's navigation: the ids of the applets whose pages are
    /// open over its list, the last one shown. A row opens one, so do an
    /// installed applet's Details and a link (docs/DEEP-LINKS.md §2).
    var storePath: [String] = []
    /// The device the Store page lists applets for, by the store's name for
    /// it; nil lists every applet.
    var storeFilter: String?
    #if DEBUG
    /// Counts the debug command `store try`: an applet page's Try It.
    var debugTries = 0
    #endif
    /// Counts the links followed, so one still waiting for the core gives way
    /// to a newer one.
    private var linksFollowed = 0
    /// An error to show in the window, from an action started anywhere.
    var presentedError: UserFacingError?
    /// Set by the first SwiftUI view that has `openWindow`; used by the delegate
    /// and the menu bar to bring the window back.
    var openMainWindow: (() -> Void)?
    private var openedAtLaunch = false
    /// When the owner chose "Quit and Stop Pocket Hub": the core shuts down
    /// too. A plain Quit leaves it running and devices connected.
    var stopHubOnQuit = false

    private init() {
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0.1.0"
        // POCKET_HUB_NO_LAUNCH=1 keeps the app from starting a core (useful with a mock started by hand).
        let launch = ProcessInfo.processInfo.environment["POCKET_HUB_NO_LAUNCH"] != "1"
        model = HubModel(clientName: "Pocket Hub", version: version, launchCore: launch)
        model.onScreenshot = { [weak self] deviceId, shot in self?.keep(shot, of: deviceId) }
        model.start()
        Task { await self.watchPairRequests() }
        #if DEBUG
        DebugSnapshots.startIfRequested()
        DebugCommands.startIfRequested(self)
        #endif
    }

    var selectedDeviceID: String? {
        if case .device(let id) = selection { return id }
        return nil
    }

    /// Opens the window once at launch, unless the app was started as a login item.
    func openWindowAtLaunchIfNeeded() {
        guard !openedAtLaunch else { return }
        openedAtLaunch = true
        if !AppDelegate.launchedAsLoginItem { showWindow() }
    }

    func showWindow() {
        NSApp.setActivationPolicy(.regular)
        openMainWindow?()
        NSApp.activate()
    }

    func show(device id: String) {
        selection = .device(id)
        showWindow()
    }

    func startSetup() {
        showingSetup = true
        showWindow()
    }

    // MARK: - A device asks to be paired (CONTROL-API.md §3 Pairing)

    /// Opens the Add a Device sheet on a device's request as soon as one
    /// waits and no sheet is in the way: its owner is at the device, which
    /// shows a code for 5 minutes, and has just chosen this Mac there. The
    /// window comes to the front for it, as the system's own pairing requests
    /// do. Runs for the app's life; each pass waits for the next change of
    /// what it read.
    private func watchPairRequests() async {
        while true {
            presentPairRequestIfNeeded()
            await withCheckedContinuation { continuation in
                withObservationTracking {
                    _ = model.pairRequests
                    _ = showingSetup
                } onChange: { continuation.resume() }
            }
        }
    }

    private func presentPairRequestIfNeeded() {
        func key(_ request: PairRequest) -> String { "\(request.device) \(request.since)" }
        let waiting = model.pairRequests
        presentedRequests.formIntersection(waiting.map(key))
        // A sheet that is open handles requests itself (SetupFlowModel).
        guard !showingSetup, let request = waiting.first(where: { !presentedRequests.contains(key($0)) }) else { return }
        presentedRequests.insert(key(request))
        startPairing(request)
    }

    /// Opens the sheet on `request`'s code: from the watch above, or from the
    /// menu bar's item for a request whose sheet the owner closed.
    func startPairing(_ request: PairRequest) {
        guard !showingSetup else { return }
        setupRequest = request
        startSetup()
    }

    // MARK: - Links (docs/DEEP-LINKS.md §2)

    /// Follows a link the app was opened with. The window comes to the front
    /// first: whatever the link turns out to be, the owner asked for Hub.
    /// Nothing here changes a device; the applet's sheet holds the action.
    func open(_ url: URL) {
        showWindow()
        linksFollowed += 1
        let mine = linksFollowed
        guard let link = DeepLink(url) else {
            reportLink(.linkNotUnderstood)
            return
        }
        Task {
            // A link can be what starts the app, and so the core.
            await model.whenLive()
            guard mine == linksFollowed else { return }
            switch link {
            case .storeApp(let id):
                do {
                    let app = try await model.store.lookUp(appId: id)
                    guard mine == linksFollowed else { return }
                    if let app { show(storeApp: app) } else { reportLink(.appletNotInStore) }
                } catch {
                    guard mine == linksFollowed else { return }
                    reportLink(UserFacingError(storeError: error))
                }
            }
        }
    }

    /// Says what a link could not show.
    private func reportLink(_ failure: UserFacingError) {
        presentedError = failure
    }

    /// Opens an applet's page in the Store, over its list: from a link, or
    /// from an installed applet's Details. The device is chosen when
    /// installing, on that page.
    func show(storeApp app: StoreApp) {
        selection = .store
        storePath = [app.id]
        showWindow()
    }

    /// Takes a screenshot from anywhere (toolbar, menu bar, keyboard). It is
    /// kept as every screenshot is (`keep`).
    func takeScreenshot(of deviceId: String) {
        Task {
            do {
                try await model.takeScreenshot(of: deviceId)
            } catch {
                let device = model.device(deviceId)
                presentedError = UserFacingError(error, deviceName: device?.displayName, kind: device?.kind ?? DeviceKind(rawValue: ""))
                if presentedError != nil { showWindow() }
            }
        }
    }

    /// Keeps a screenshot that arrived, asked for or sent by the device: a
    /// copy in the folder where this Mac keeps its own screenshots
    /// (ScreenshotDestination), as Simulator keeps a simulated device's, and
    /// the image on the clipboard. No page shows it.
    private func keep(_ shot: ScreenshotResult, of deviceId: String) {
        let name = model.device(deviceId)?.displayName ?? deviceId
        let folder = ScreenshotDestination.folder()
        Task {
            await ScreenshotFile.copyToPasteboard(path: shot.path)
            do {
                try await ScreenshotFile.copy(path: shot.path, into: folder, device: name)
            } catch {
                presentedError = .screenshotNotSaved(folder: FileManager.default.displayName(atPath: folder.path),
                                                     detail: error.localizedDescription)
                showWindow()
            }
        }
    }

    /// Starts updating a device's software from anywhere (the menu bar) and
    /// shows its Applets page, where the progress and the outcome are.
    func updateSoftware(of deviceId: String) {
        model.updateProgram(on: deviceId)
        deviceTab = .applets
        show(device: deviceId)
    }

    /// Quits the app; with `stoppingHub` the core is shut down as well.
    func quit(stoppingHub: Bool) {
        stopHubOnQuit = stoppingHub
        NSApp.terminate(nil)
    }

    /// Opens a new instance of the app and quits this one, the core left
    /// running: how a language choice takes effect (AppLanguage). The new
    /// instance is started by Launch Services, so it shares no descriptor with
    /// this process; if it cannot be opened, this one stays and says why. It
    /// gets this one's environment (`POCKET_HUB_HOME` and the like) but not its
    /// arguments: an `-AppleLanguages` there would override the choice.
    func relaunch() {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.createsNewApplicationInstance = true
        configuration.environment = ProcessInfo.processInfo.environment
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: configuration) { _, error in
            Task { @MainActor in
                if let error {
                    self.presentedError = UserFacingError(error)
                    self.showWindow()
                    return
                }
                // terminate(_:) waits in a nested event loop for the
                // delegate's reply, which a main-actor task sends
                // (AppDelegate.applicationShouldTerminate). Called from this
                // task, it would hold the main queue that reply needs and
                // never return, so it runs on the next turn of the run loop.
                self.stopHubOnQuit = false
                NSApp.perform(#selector(NSApplication.terminate(_:)), with: nil, afterDelay: 0)
            }
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    /// Whether macOS opened the app as a login item: then it starts in the menu
    /// bar only. Read from the launch Apple event (legacy login items set
    /// `keyAELaunchedAsLogInItem`; whether `SMAppService.mainApp` does is unverified).
    nonisolated(unsafe) static var launchedAsLoginItem = false

    func applicationWillFinishLaunching(_ notification: Notification) {
        if let event = NSAppleEventManager.shared().currentAppleEvent,
           event.eventID == kAEOpenApplication,
           event.paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem
        {
            Self.launchedAsLoginItem = true
        }
    }

    /// A pocket-hub: link (docs/DEEP-LINKS.md §1): the system hands the app
    /// every URL of the scheme it is registered for, starting it when it was
    /// not running. It arrives here with the window open and with Hub in the
    /// menu bar only (measured on both, and on a launch by a link). Each
    /// link names one thing to show, so of several the last is the one followed.
    /// This is the only thing that acts on a link: the window's scene takes
    /// no external event (PocketHubApp).
    func application(_ application: NSApplication, open urls: [URL]) {
        guard let url = urls.last else { return }
        MainActor.assumeIsolated { AppState.shared.open(url) }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false // Hub lives in the menu bar when its window is closed.
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows {
            MainActor.assumeIsolated { AppState.shared.showWindow() }
        }
        return true
    }

    /// Quitting leaves Hub running and devices connected, unless the owner
    /// chose "Quit and Stop Pocket Hub": then the core is asked to shut down
    /// (links close, daemons stop) and the app waits until it is gone, bounded
    /// by the client's own timeout so a hung core cannot keep it alive.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        MainActor.assumeIsolated {
            let state = AppState.shared
            Task { @MainActor in
                await state.model.quit(shutdownCore: state.stopHubOnQuit)
                NSApp.reply(toApplicationShouldTerminate: true)
            }
        }
        return .terminateLater
    }
}
