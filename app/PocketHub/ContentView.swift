import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The main window: devices and Hub's pages in the sidebar, the selection on the right.
struct ContentView: View {
    @Environment(AppState.self) private var state
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        @Bindable var state = state
        // The sidebar is the window's only navigation and holds a handful of
        // rows, so it stays shown: no toggle in the toolbar, no collapsing.
        // (HIG, Sidebars: letting people hide one is a "consider", for
        // windows whose content wants the room.)
        NavigationSplitView(columnVisibility: .constant(.all)) {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 230, max: 280)
                .toolbar(removing: .sidebarToggle)
        } detail: {
            Detail()
        }
        .sheet(isPresented: $state.showingSetup) {
            SetupSheet()
        }
        .alert(item: $state.presentedError) { error in
            Alert(title: Text(verbatim: error.title),
                  message: Text(verbatim: [error.advice, error.detail].compactMap { $0 }.joined(separator: "\n\n")))
        }
        .onAppear {
            NSApp.setActivationPolicy(.regular)
        }
        .onDisappear {
            // With the window closed Hub lives in the menu bar only, out of the Dock and the app switcher.
            NSApp.setActivationPolicy(.accessory)
        }
        // The window opens on the store once the core answers, and goes back
        // to it when the device it shows is removed. Both are watched: with
        // no device paired the list of devices never changes.
        .onChange(of: state.model.isLive, initial: true) { placeSelection() }
        .onChange(of: state.model.devices.map(\.id)) { placeSelection() }
    }
}

extension ContentView {
    private func placeSelection() {
        guard state.model.isLive else { return }
        if case .device(let id) = state.selection, state.model.device(id) == nil { state.selection = .store }
        if state.selection == nil { state.selection = .store }
    }
}

private struct SetupSheet: View {
    @Environment(AppState.self) private var state
    @State private var flow: SetupFlowModel?

    var body: some View {
        Group {
            if let flow {
                SetupFlowView(flow: flow, macName: state.model.hub?.name ?? "") { deviceId in
                    state.showingSetup = false
                    if let deviceId { state.selection = .device(deviceId) }
                }
            } else {
                ProgressView().frame(width: 520, height: 440)
            }
        }
        .onAppear {
            guard flow == nil else { return }
            // A device that asked opens the sheet on its code (AppState.startPairing).
            let made = state.model.makeSetupFlow(answering: state.setupRequest)
            state.setupRequest = nil
            state.setupFlow = made
            flow = made
        }
    }
}

struct Sidebar: View {
    @Environment(AppState.self) private var state

    var body: some View {
        @Bindable var state = state
        let model = state.model
        List(selection: $state.selection) {
            // The store comes first and is there whatever the devices are:
            // its catalog is the whole store's, and only its buttons need a
            // device (StorePage).
            Label("Store", systemImage: "bag")
                .tag(SidebarItem.store)
            Section("Devices") {
                if model.isLive {
                    ForEach(model.devices) { device in
                        DeviceRow(device: device)
                            .tag(SidebarItem.device(device.id))
                    }
                    Button {
                        state.startSetup()
                    } label: {
                        Label("Add a Device…", systemImage: "plus.circle")
                    }
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
                } else {
                    Label("Not connected to Pocket Hub", systemImage: "bolt.horizontal.circle")
                        .foregroundStyle(.secondary)
                        .selectionDisabled()
                }
            }
            // Below the devices is only what runs on this Mac, a developer
            // surface kept out of the owner's way.
            Section("Advanced") {
                Label("Daemons", systemImage: "gearshape.2")
                    .tag(SidebarItem.daemons)
            }
        }
        .listStyle(.sidebar)
        // Nothing sits at the sidebar's foot (HIG, Sidebars: the bottom edge
        // is often off screen). Whether the core answers is told where it
        // matters: the Devices section and the page on the right.
    }
}

struct DeviceRow: View {
    let device: Device

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text(device.displayName)
                Group {
                    if let busy = device.busy {
                        Text(verbatim: busy.displayText)
                    } else if let title = device.runningAppTitle {
                        Text(verbatim: title)
                    } else if let link = device.link {
                        Text(link.carrier.displayName)
                    } else {
                        Text("Not connected")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        } icon: {
            // The family's glyph (a controller, a touch screen) is wider than
            // the sidebar's icon column; at full scale it runs into the name.
            // A connected device keeps the sidebar's tint (the owner's accent
            // colour, as every sidebar icon does; HIG, Sidebars); one that is
            // away is dimmed.
            Image(systemName: device.kind.symbolName(filled: device.isLinked))
                .imageScale(.small)
                .foregroundStyle(device.isLinked ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
        }
        .accessibilityElement(children: .combine)
        .accessibilityValue(device.isLinked ? Text("Connected") : Text("Not connected"))
    }
}

/// The right side: the selected page, or why there is none.
struct Detail: View {
    @Environment(AppState.self) private var state

    var body: some View {
        let model = state.model
        if !model.isLive {
            ConnectionStatusView()
        } else {
            switch state.selection {
            case .store:
                StorePage()
            case .device(let id):
                if let device = model.device(id) {
                    DeviceDetailView(device: device)
                        .id(id)
                } else {
                    ContentUnavailableView("Device Not Found", systemImage: "questionmark.circle",
                                           description: Text("It may have been removed."))
                }
            case .daemons:
                DaemonsView()
            case nil:
                if model.devices.isEmpty {
                    ContentUnavailableView {
                        Label("No Devices Yet", systemImage: "gamecontroller")
                    } description: {
                        Text("Add a device to install applets on it, take screenshots and more.")
                    } actions: {
                        Button("Add a Device…") { state.startSetup() }
                            .controlSize(.large)
                            .buttonStyle(.borderedProminent)
                    }
                } else {
                    ContentUnavailableView("Select a Device", systemImage: "sidebar.left",
                                           description: Text("Choose a device in the sidebar."))
                }
            }
        }
    }
}

/// What the window shows while the core cannot be reached. It says so plainly
/// and keeps retrying; nothing stale is shown.
struct ConnectionStatusView: View {
    @Environment(AppState.self) private var state

    var body: some View {
        let model = state.model
        Group {
            switch model.connection {
            case .connecting, .connected:
                ContentUnavailableView {
                    Label { Text("Connecting to Pocket Hub…") } icon: { ProgressView().controlSize(.large) }
                }
            case .startingCore:
                ContentUnavailableView {
                    Label { Text("Starting Pocket Hub…") } icon: { ProgressView().controlSize(.large) }
                }
            case .stopped:
                ContentUnavailableView("Pocket Hub Is Stopped", systemImage: "stop.circle")
            case .unreachable(.coreFailedToStart(let line), _):
                // The core said why it cannot start; it is not started again
                // until the owner asks, so there is no countdown here.
                let failure = CoreStartFailure(line: line)
                ContentUnavailableView {
                    Label { Text(verbatim: failure.title) } icon: { Image(systemName: "exclamationmark.triangle") }
                } description: {
                    VStack(spacing: 8) {
                        Text(verbatim: failure.advice)
                        Text(verbatim: failure.detail)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                } actions: {
                    Button("Try Again") { model.retryNow() }
                        .keyboardShortcut(.defaultAction)
                }
            case .unreachable(let reason, let retryAt):
                ContentUnavailableView {
                    Label("Pocket Hub Isn't Responding", systemImage: "bolt.horizontal.circle")
                } description: {
                    VStack(spacing: 6) {
                        switch reason {
                        case .coreNotFound:
                            Text("Its background service is missing from this copy of the app. Reinstall Pocket Hub.")
                        case .coreFailedToStart:
                            EmptyView()
                        case .incompatible:
                            Text("Its background service is a different version. Reinstall Pocket Hub.")
                        case .notRunning, .failed:
                            Text("Your devices will reappear as soon as it is back.")
                        }
                        TimelineView(.periodic(from: .now, by: 1)) { context in
                            let seconds = Int(retryAt.timeIntervalSince(context.date).rounded(.up))
                            if seconds > 0 {
                                Text("Trying again in \(seconds) s")
                            } else {
                                Text("Trying again…")
                            }
                        }
                        .font(.caption)
                    }
                } actions: {
                    Button("Try Now") { model.retryNow() }
                }
            }
        }
    }
}
