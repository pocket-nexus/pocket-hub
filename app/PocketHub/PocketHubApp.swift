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
                .frame(minWidth: 720, minHeight: 480)
        }
        // The window is opened by the menu bar label at launch, so a launch at
        // login can start in the menu bar only.
        .defaultLaunchBehavior(.suppressed)
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
        CommandGroup(replacing: .newItem) {
            Button("Add a 3DS…") { state.startSetup() }
                .keyboardShortcut("n")
                .disabled(!state.model.isLive)
        }
        CommandGroup(after: .toolbar) {
            Button("Refresh Store") { state.model.store.reload(refresh: true) }
                .keyboardShortcut("r")
                .disabled(state.selection != .store || !state.model.isLive)
        }
        CommandMenu("Device") {
            Button("Take Screenshot") {
                if let id = state.selectedDeviceID {
                    state.deviceTab = .screenshot
                    state.takeScreenshot(of: id, reveal: false)
                }
            }
            .keyboardShortcut("s", modifiers: [.command, .shift])
            .disabled(!selectedDeviceIsLinked)
            Button("Show Log") { state.deviceTab = .log }
            .keyboardShortcut("l")
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
}
