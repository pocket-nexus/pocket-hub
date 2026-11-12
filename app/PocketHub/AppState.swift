import AppKit
import HubAppModel
import HubClient
import Observation
import SwiftUI

/// Where the window is: the sidebar selection and the sheets.
enum SidebarItem: Hashable {
    case device(String)
    case daemons
}

/// The sections of a device's page.
enum DeviceTab: Hashable {
    case applets, screenshot, log, details
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
    /// The device page's file picker for a .pocket applet, and its question
    /// before removing the device: here so the Device menu can open them as
    /// the toolbar does (HIG, Toolbars › macOS: every toolbar item is also a
    /// menu command).
    var importingApplet = false
    var confirmingRemoveDevice = false
    /// The store applet whose sheet is open: from a row's Details, or from a
    /// link (docs/DEEP-LINKS.md §2).
    var storeDetails: StoreApp?
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
        model.start()
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

    // MARK: - Links (docs/DEEP-LINKS.md §2)

    /// Follows a link the app was opened with. The window comes to the front
    /// first: whatever the link turns out to be, the owner asked for Hub.
    /// Nothing here changes a device; the applet's sheet holds the action.
    func open(_ url: URL) {
        showWindow()
        linksFollowed += 1
        let mine = linksFollowed
        guard let link = DeepLink(url) else {
            presentedError = .linkNotUnderstood
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
                    if let app { show(storeApp: app) } else { presentedError = .appletNotInStore }
                } catch {
                    guard mine == linksFollowed else { return }
                    presentedError = UserFacingError(storeError: error)
                }
            }
        }
    }

    /// Opens the store's sheet of an applet, on a device that can run it when
    /// there is one.
    func show(storeApp app: StoreApp) {
        showDevice(for: app)
        storeDetails = app
        showWindow()
    }

    /// Puts the window on the Applets page of the device that fits `app` best
    /// (StoreModel.device(for:preferring:)), where the store's list says what
    /// that device holds of it. With no device that can run it, the window
    /// stays where it is.
    func showDevice(for app: StoreApp) {
        guard let device = model.store.device(for: app, preferring: selectedDeviceID) else { return }
        show(app, on: device.id)
    }

    /// The same, for a device the owner chose in the sheet.
    func show(_ app: StoreApp, on deviceId: String) {
        selection = .device(deviceId)
        deviceTab = .applets
        model.store.choose(device: deviceId)
    }

    /// Takes a screenshot from anywhere (menu bar, keyboard), copies it and shows it.
    func takeScreenshot(of deviceId: String, reveal: Bool) {
        Task {
            do {
                let shot = try await model.takeScreenshot(of: deviceId)
                await ScreenshotFile.copyToPasteboard(path: shot.path)
                if reveal {
                    deviceTab = .screenshot
                    show(device: deviceId)
                }
            } catch {
                presentedError = UserFacingError(error, deviceName: model.device(deviceId)?.displayName)
                if presentedError != nil { showWindow() }
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
