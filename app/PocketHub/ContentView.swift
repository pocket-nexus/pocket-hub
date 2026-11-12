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
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 230, max: 320)
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
        .onChange(of: state.model.devices.map(\.id), initial: true) { _, ids in
            guard state.model.isLive else { return }
            if case .device(let id) = state.selection, !ids.contains(id) { state.selection = nil }
            if state.selection == nil, let first = ids.first { state.selection = .device(first) }
        }
    }
}

private struct SetupSheet: View {
    @Environment(AppState.self) private var state
    @State private var flow: SetupFlowModel?

    var body: some View {
        Group {
            if let flow {
                SetupFlowView(flow: flow) { deviceId in
                    state.showingSetup = false
                    if let deviceId { state.selection = .device(deviceId) }
                }
            } else {
                ProgressView().frame(width: 520, height: 440)
            }
        }
        .onAppear { if flow == nil { flow = state.model.makeSetupFlow() } }
    }
}

struct Sidebar: View {
    @Environment(AppState.self) private var state

    var body: some View {
        @Bindable var state = state
        let model = state.model
        List(selection: $state.selection) {
            Section("Devices") {
                if model.isLive {
                    ForEach(model.devices) { device in
                        DeviceRow(device: device)
                            .tag(SidebarItem.device(device.id))
                    }
                    Button {
                        state.showingSetup = true
                    } label: {
                        Label("Add a 3DS…", systemImage: "plus.circle")
                    }
                    .buttonStyle(.borderless)
                    .foregroundStyle(.secondary)
                } else {
                    Label("Not connected to Pocket Hub", systemImage: "bolt.horizontal.circle")
                        .foregroundStyle(.secondary)
                        .selectionDisabled()
                }
            }
            Section("Pocket Hub") {
                Label("Applet Store", systemImage: "bag")
                    .tag(SidebarItem.store)
            }
            // Developer surfaces, kept out of the owner's way but fully working.
            Section("Advanced") {
                Label("Daemons", systemImage: "gearshape.2")
                    .tag(SidebarItem.daemons)
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom) {
            ConnectionFooter()
        }
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
            // The controller glyph is wider than the sidebar's icon column;
            // at full scale it runs into the name.
            Image(systemName: device.isLinked ? "gamecontroller.fill" : "gamecontroller")
                .imageScale(.small)
                .foregroundStyle(device.isLinked ? Color.green : Color.secondary)
        }
        .accessibilityElement(children: .combine)
        .accessibilityValue(device.isLinked ? Text("Connected") : Text("Not connected"))
    }
}

/// The core's reachability, at the bottom of the sidebar.
struct ConnectionFooter: View {
    @Environment(AppState.self) private var state

    var body: some View {
        let model = state.model
        HStack(spacing: 6) {
            Circle()
                .fill(model.isLive ? Color.green : Color.orange)
                .frame(width: 7, height: 7)
                .accessibilityHidden(true)
            switch model.connection {
            case .connected:
                Text(verbatim: model.hub?.name ?? "")
            case .connecting, .startingCore:
                Text("Connecting…")
            case .unreachable, .stopped:
                Text("Not running")
            }
            Spacer()
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
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
            case .store:
                StoreView()
            case nil:
                if model.devices.isEmpty {
                    ContentUnavailableView {
                        Label("No Devices Yet", systemImage: "gamecontroller")
                    } description: {
                        Text("Add your Nintendo 3DS to install applets, take screenshots and more. It takes a couple of minutes.")
                    } actions: {
                        Button("Add a 3DS…") { state.showingSetup = true }
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
