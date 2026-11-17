import AppKit
import HubAppModel
import HubClient
import SwiftUI

@main
struct PocketHubApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var state = AppState.shared

    var body: some Scene {
        Window("Pocket Hub", id: "main") {
            ContentView()
                .environment(state)
                .frame(minWidth: 800, minHeight: 480)
        }
        // The window is opened by the menu bar label at launch, so a launch at
        // login can start in the menu bar only.
        .defaultLaunchBehavior(.suppressed)
        // No external event opens this scene: a pocket-hub: link is followed
        // by the app delegate, which shows the window itself (AppState.open,
        // docs/DEEP-LINKS.md §2). Left to SwiftUI, a link is also an external
        // event that presents this scene again. With the window already open
        // that rebuilt its content and played its opening animation, which
        // looked like the window closing and opening (measured: the title
        // fell back to the scene's and the frame animated before the sheet
        // came; with this line neither happens, and the delegate still gets
        // the link with the window open, with it closed and at launch).
        .handlesExternalEvents(matching: [])
        .defaultSize(width: 940, height: 620)
        .commands { HubCommands(state: state) }

        Settings {
            SettingsView()
                .environment(state)
        }

        MenuBarExtra {
            MenuBarContent()
                .environment(state)
        } label: {
            MenuBarLabel()
                .environment(state)
        }
        .menuBarExtraStyle(.menu)
    }
}

/// Menu commands with keyboard shortcuts.
struct HubCommands: Commands {
    let state: AppState

    var body: some Commands {
        // Where a Mac app has it: in the app menu, after About.
        CommandGroup(after: .appInfo) {
            Button("Check for Updates…") { state.updater.checkForUpdates() }
                .disabled(!state.updater.canCheckForUpdates)
        }
        CommandGroup(replacing: .newItem) {
            Button("Add a Device…") { state.startSetup() }
                .keyboardShortcut("n")
                .disabled(!state.model.isLive)
        }
        CommandGroup(after: .toolbar) {
            // The Store page and a device's applets (their updates) both
            // come from the catalog this fetches again.
            Button("Refresh Store") { state.model.store.reload(refresh: true) }
                .keyboardShortcut("r")
                .disabled(!state.model.isLive)
        }
        CommandMenu("Device") {
            Button("Take Screenshot") {
                if let id = state.selectedDeviceID { state.takeScreenshot(of: id) }
            }
            .keyboardShortcut("s", modifiers: [.command, .shift])
            .disabled(!selectedDeviceIsLinked || !selectedDeviceSupports(.screenshot))
            Button("Install Applet…") { state.importingApplet = true }
            .keyboardShortcut("i", modifiers: [.command, .shift])
            .disabled(!selectedDeviceIsLinked || !selectedDeviceSupports(.push) || state.selectedDeviceID.flatMap { state.model.pushes[$0] } != nil)
            Button("Show Log") { state.deviceTab = .log }
            .keyboardShortcut("l")
            .disabled(!selectedDeviceSupports(.logs))
            Divider()
            Button("Remove from Pocket Hub…") { state.confirmingRemoveDevice = true }
            .disabled(state.selectedDeviceID == nil)
        }
        CommandGroup(replacing: .appTermination) {
            // ⌘Q closes the app only: Hub keeps running in the background and
            // devices stay connected. Stopping Hub is the explicit second item.
            Button("Quit Pocket Hub") { state.quit(stoppingHub: false) }
                .keyboardShortcut("q")
            Button("Quit and Stop Pocket Hub") { state.quit(stoppingHub: true) }
                .keyboardShortcut("q", modifiers: [.command, .option])
        }
    }

    private var selectedDeviceIsLinked: Bool {
        guard let id = state.selectedDeviceID else { return false }
        return state.model.device(id)?.isLinked == true
    }

    /// A menu command is off for a device that does not take it (`supports`),
    /// as the toolbar has no button for it.
    private func selectedDeviceSupports(_ operation: DeviceOperation) -> Bool {
        guard let id = state.selectedDeviceID else { return false }
        return state.model.device(id)?.supports(operation) == true
    }
}
