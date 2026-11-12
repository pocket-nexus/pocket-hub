import AppKit
import HubAppModel
import HubClient
import Observation
import SwiftUI

/// Where the window is: the sidebar selection and the sheets.
enum SidebarItem: Hashable {
    case device(String)
    case daemons
    case store
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
    /// The store applet whose details sheet is open.
    var storeDetails: StoreApp?
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
