import AppKit
import HubAppModel
import HubClient
import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    /// A Pocket applet package. Declared by extension only: Hub decides what a valid package is.
    static let pocketApplet = UTType(filenameExtension: "pocket", conformingTo: .data) ?? .data
}

/// One device: its state, its applets, a screenshot, its live log.
struct DeviceDetailView: View {
    let device: Device
    @Environment(AppState.self) private var state
    @State private var importing = false
    @State private var confirmingForget = false
    @State private var dropTargeted = false

    private var model: HubModel { state.model }

    /// How the last link ended, for the header. "The console is restarting"
    /// holds only while Hub holds the device for that restart; once the hold
    /// is over it is no longer true, and while this app updates the device
    /// the update's own banner tells the restart.
    private var unlinkNote: DeviceUnlinked? {
        guard let unlink = model.lastUnlink[device.id] else { return nil }
        if unlink.code == "restart", device.busy?.launching != true || model.programUpdates[device.id] != nil {
            return nil
        }
        return unlink
    }

    var body: some View {
        @Bindable var state = state
        VStack(spacing: 0) {
            DeviceHeader(device: device, unlink: unlinkNote, push: model.pushes[device.id],
                         updating: model.programUpdates[device.id]?.isActive == true)
                .padding()
            SoftwareUpdateBanner(device: device)
            Divider()
            Picker("Section", selection: $state.deviceTab) {
                Text("Applets").tag(DeviceTab.applets)
                Text("Screenshot").tag(DeviceTab.screenshot)
                Text("Log").tag(DeviceTab.log)
                Text("Details").tag(DeviceTab.details)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .frame(maxWidth: 440)
            .padding(.vertical, 10)
            Group {
                switch state.deviceTab {
                case .applets:
                    AppletList(device: device)
                case .screenshot:
                    ScreenshotView(device: device, shot: model.screenshots[device.id],
                                   isTaking: model.takingScreenshot.contains(device.id), take: takeScreenshot)
                        .padding()
                case .log:
                    DeviceLogView(device: device)
                case .details:
                    DeviceDetailsForm(device: device)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle(device.displayName)
        .navigationSubtitle(device.isLinked ? Text("Connected") : Text("Not connected"))
        .toolbar { toolbar }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.pocketApplet]) { result in
            if case .success(let url) = result { push(url) }
        }
        .dropDestination(for: URL.self) { urls, _ in
            guard let url = urls.first(where: { $0.pathExtension.lowercased() == "pocket" }), device.isLinked else { return false }
            push(url)
            return true
        } isTargeted: { dropTargeted = $0 }
        .overlay {
            if dropTargeted {
                RoundedRectangle(cornerRadius: 10)
                    .strokeBorder(Color.accentColor, lineWidth: 3)
                    .background(Color.accentColor.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
                    .overlay {
                        Label {
                            if device.isLinked {
                                Text("Drop to install on \(device.displayName)")
                            } else {
                                Text("\(device.displayName) isn't connected")
                            }
                        } icon: {
                            Image(systemName: "square.and.arrow.down.on.square")
                        }
                            .font(.title3)
                            .padding()
                            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
                    }
                    .padding(6)
                    .allowsHitTesting(false)
            }
        }
        .confirmationDialog("Remove \(device.displayName) from Pocket Hub?", isPresented: $confirmingForget) {
            Button("Remove", role: .destructive) { forget() }
        } message: {
            Text("The console disconnects. To use it with this Mac again, you will need to set it up again.")
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup {
            Button("Take Screenshot", systemImage: "camera", action: takeScreenshot)
                .disabled(!device.isLinked || model.takingScreenshot.contains(device.id))
                .help("Take a screenshot of both screens")
            Button("Install Applet…", systemImage: "square.and.arrow.down") { importing = true }
                .disabled(!device.isLinked || model.pushes[device.id] != nil)
                .help("Install a .pocket applet on this console")
            Menu("More", systemImage: "ellipsis.circle") {
                Button("Remove from Pocket Hub…", role: .destructive) { confirmingForget = true }
            }
            .help("More actions")
        }
    }

    private func takeScreenshot() {
        state.deviceTab = .screenshot
        Task {
            do { try await model.takeScreenshot(of: device.id) } catch { report(error) }
        }
    }

    private func push(_ url: URL) {
        state.deviceTab = .applets
        Task {
            let access = url.startAccessingSecurityScopedResource()
            defer { if access { url.stopAccessingSecurityScopedResource() } }
            do { try await model.push(url, to: device.id) } catch { report(error) }
        }
    }

    private func forget() {
        let id = device.id
        Task {
            do {
                try await model.forget(id)
                if state.selection == .device(id) { state.selection = nil }
            } catch { report(error) }
        }
    }

    private func report(_ error: any Error) {
        state.presentedError = UserFacingError(error, deviceName: device.displayName)
    }
}

/// The device's name, link and activity at a glance.
struct DeviceHeader: View {
    let device: Device
    let unlink: DeviceUnlinked?
    let push: HubModel.PushActivity?
    /// This app is updating the device's software: the banner below shows it,
    /// so the header does not repeat Hub's hold.
    var updating = false

    var body: some View {
        HStack(alignment: .top, spacing: 16) {
            Image(systemName: "gamecontroller.fill")
                .font(.system(size: 34))
                .foregroundStyle(device.isLinked ? Color.accentColor : .secondary)
                .frame(width: 56, height: 56)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 12))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text(device.displayName).font(.title2.bold())
                if !device.model.isEmpty {
                    Text(device.model).foregroundStyle(.secondary)
                }
                ConnectionLine(device: device, unlink: unlink)
                if let title = device.runningAppTitle {
                    Label {
                        if device.isOnHomeScreen { Text("On the home screen") } else { Text("Running \(title)") }
                    } icon: {
                        Image(systemName: device.isOnHomeScreen ? "house" : "play.circle")
                    }
                    .foregroundStyle(.secondary)
                }
                if let busy = device.busy, !updating {
                    Label {
                        Text(verbatim: busy.displayText)
                    } icon: {
                        ProgressView().controlSize(.mini)
                    }
                    .foregroundStyle(.orange)
                }
                if device.setupUnconfirmed {
                    Label("The last setup may not have finished. If the console doesn't connect, set it up again.",
                          systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
                if device.duplicateId {
                    Label("Two consoles are using copies of the same SD card. Only one can connect at a time.",
                          systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                }
                if let push {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Installing \(push.fileName)…").font(.callout)
                        Text(verbatim: transferPhaseText(push.phase)).font(.caption).foregroundStyle(.secondary)
                        if let fraction = push.fraction {
                            ProgressView(value: fraction)
                        } else {
                            ProgressView().progressViewStyle(.linear)
                        }
                    }
                    .frame(maxWidth: 320)
                }
            }
            Spacer()
        }
    }
}

struct ConnectionLine: View {
    let device: Device
    let unlink: DeviceUnlinked?

    var body: some View {
        if let link = device.link {
            Label {
                Text("Connected over \(link.carrier.displayName) since \(link.since.date.formatted(date: .omitted, time: .shortened))")
            } icon: {
                Image(systemName: link.carrier.symbolName)
            }
            .foregroundStyle(.green)
        } else {
            VStack(alignment: .leading, spacing: 2) {
                Label {
                    if let seen = device.lastSeen {
                        Text("Not connected · last seen \(seen.date.formatted(.relative(presentation: .named)))")
                    } else {
                        Text("Not connected yet")
                    }
                } icon: {
                    Image(systemName: "bolt.horizontal.circle")
                }
                .foregroundStyle(.secondary)
                if let unlink {
                    Text(unlink.explanation).font(.caption).foregroundStyle(.secondary)
                } else if device.lastSeen == nil {
                    Text("Open Pocket Shell on the console to connect it.").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// The installed applets, with Open.
struct AppletList: View {
    let device: Device
    @Environment(AppState.self) private var state

    var body: some View {
        let apps = device.sortedApps
        if apps.isEmpty {
            if device.isLinked {
                ContentUnavailableView("No Applets", systemImage: "square.grid.2x2",
                                       description: Text("Drop a .pocket file here to install one."))
            } else {
                ContentUnavailableView("Applets Unknown", systemImage: "square.grid.2x2",
                                       description: Text("The list appears when the console connects."))
            }
        } else {
            List(apps) { app in
                AppletRow(app: app, device: device,
                          isRunning: device.runningApp?.id == app.id,
                          isLaunching: state.model.launching[device.id] == app.id) {
                    launch(app)
                }
            }
            .listStyle(.inset(alternatesRowBackgrounds: true))
        }
    }

    private func launch(_ app: InstalledApp) {
        Task {
            do { try await state.model.launch(app, on: device.id) } catch {
                state.presentedError = UserFacingError(error, deviceName: device.displayName)
            }
        }
    }
}

struct AppletRow: View {
    let app: InstalledApp
    let device: Device
    let isRunning: Bool
    let isLaunching: Bool
    let open: () -> Void
    @Environment(AppState.self) private var state

    var body: some View {
        // The device's own rows carry no title; the store's entry for the same
        // id gives one, and an icon. Applets the store does not know keep `output`.
        let listed = app.isHome ? nil : state.model.store.known[app.id]
        HStack {
            if let listed {
                StoreIcon(path: listed.icon, size: 28)
            } else {
                Image(systemName: app.isHome ? "house" : "app")
                    .foregroundStyle(.secondary)
                    .frame(width: 28)
                    .accessibilityHidden(true)
            }
            VStack(alignment: .leading) {
                if let listed {
                    Text(verbatim: listed.title)
                } else if app.isHome {
                    Text("Home screen")
                } else {
                    Text(verbatim: app.displayName)
                }
                // Ids are developers' words: a row says the version, and
                // whether the store knows the applet. The store's details
                // sheet keeps the id.
                Group {
                    if listed != nil || app.isHome {
                        Text("Version \(app.version)")
                    } else {
                        Text("Version \(app.version) · Not from the store")
                    }
                }
                .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if isRunning {
                Text("Running").font(.caption).foregroundStyle(.green)
            }
            if isLaunching {
                ProgressView().controlSize(.small)
            } else {
                Button("Open", action: open)
                    .disabled(!device.isLinked || device.busy != nil || isRunning)
                    .accessibilityLabel(Text("Open \(listed?.title ?? app.displayName)"))
            }
        }
        .padding(.vertical, 2)
    }
}
