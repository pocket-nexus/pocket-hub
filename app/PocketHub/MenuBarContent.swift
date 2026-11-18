import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The menu bar icon: the state at a glance.
struct MenuBarLabel: View {
    @Environment(AppState.self) private var state
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        let model = state.model
        icon(model)
            .accessibilityLabel(Text("Pocket Hub"))
            .accessibilityValue(value(model))
            .onAppear {
                // The label exists from launch on, so it is where the app learns
                // how to open its window and opens it once.
                state.openMainWindow = { openWindow(id: "main") }
                state.openWindowAtLaunchIfNeeded()
            }
    }

    /// The app icon's pocket, outlined with no device linked and filled with
    /// one; a warning sign when the core does not answer. The two pockets are
    /// template images (Assets.xcassets), so the menu bar colours them as it
    /// does a system symbol.
    private func icon(_ model: HubModel) -> Image {
        guard model.isLive else { return Image(systemName: "exclamationmark.triangle") }
        return Image(model.linkedDevices.isEmpty ? .menuBarIcon : .menuBarIconFill)
    }

    private func value(_ model: HubModel) -> Text {
        guard model.isLive else { return Text("Not running") }
        return Text("\(model.linkedDevices.count) devices connected")
    }
}

/// The menu bar menu: status, a few quick actions per device, open, quit.
struct MenuBarContent: View {
    @Environment(AppState.self) private var state

    var body: some View {
        let model = state.model
        // An update Sparkle found by itself: with no window open, its alert
        // is behind other apps' windows, and this brings it to the front.
        if let version = state.updater.waitingUpdate {
            Button("Update Pocket Hub to Version \(version)…") { state.updater.checkForUpdates() }
            Divider()
        }
        if model.isLive {
            let linked = model.linkedDevices.count
            if linked == 0 {
                Text("No devices connected")
            } else {
                Text("\(linked) devices connected")
            }
        } else if case .startingCore = model.connection {
            Text("Starting Pocket Hub…")
        } else {
            Text("Pocket Hub isn't responding")
        }
        Divider()
        if model.isLive {
            ForEach(model.devices) { device in
                Menu {
                    DeviceMenuItems(device: device)
                } label: {
                    DeviceMenuLabel(device: device)
                }
            }
            // A device that asks to be paired by its code: the sheet opens
            // by itself (AppState), and from here when the owner closed it.
            ForEach(model.pairRequests) { request in
                Button("Pair \(request.displayName)…") { state.startPairing(request) }
            }
            Button("Add a Device…") { state.startSetup() }
            Divider()
        }
        Button("Open Pocket Hub") { state.showWindow() }
            .keyboardShortcut("o")
        SettingsLink { Text("Settings…") }
            .keyboardShortcut(",")
        // The app menu's item, here too: with its window closed, Hub is out
        // of the Dock and this is the only menu it has.
        Button("Check for Updates…") { state.updater.checkForUpdates() }
            .disabled(!state.updater.canCheckForUpdates)
        Divider()
        Button("Quit Pocket Hub") { state.quit(stoppingHub: false) }
            .keyboardShortcut("q")
        Button("Quit and Stop Pocket Hub") { state.quit(stoppingHub: true) }
            .keyboardShortcut("q", modifiers: [.command, .option])
    }
}

/// A device's line in the menu: its name and state, and a discreet note of
/// a software update.
struct DeviceMenuLabel: View {
    let device: Device
    @Environment(AppState.self) private var state

    var body: some View {
        if state.model.programUpdates[device.id]?.isActive == true {
            Text("\(device.displayName) — Updating…")
        } else if device.isLinked && device.offersSoftwareUpdate {
            Text("\(device.displayName) — Connected · update available")
        } else if device.isLinked {
            Text("\(device.displayName) — Connected")
        } else {
            Text("\(device.displayName) — Not connected")
        }
    }
}

/// A device's submenu: what runs, the update when there is one, quick actions.
struct DeviceMenuItems: View {
    let device: Device
    @Environment(AppState.self) private var state

    var body: some View {
        if device.isOnHomeScreen {
            Text("On the home screen")
            Divider()
        } else if let title = device.runningAppTitle {
            Text("Running \(title)")
            Divider()
        }
        if let update = state.model.programUpdates[device.id], update.isActive {
            Text(verbatim: update.stage.displayText)
            Divider()
        } else if device.offersSoftwareUpdate, let latest = device.program.latest {
            Button("Update to Version \(latest)…") { state.updateSoftware(of: device.id) }
                .disabled(!device.isLinked || device.busy != nil)
            Divider()
        }
        if device.supports(.screenshot) {
            Button("Take Screenshot") { state.takeScreenshot(of: device.id) }
                .disabled(!device.isLinked || device.busy != nil)
        }
        Button("Show in Pocket Hub") { state.show(device: device.id) }
    }
}
