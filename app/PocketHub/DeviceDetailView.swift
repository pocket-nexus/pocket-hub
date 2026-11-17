import AppKit
import HubAppModel
import HubClient
import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    /// A Pocket applet package. Declared by extension only: Hub decides what a valid package is.
    static let pocketApplet = UTType(filenameExtension: "pocket", conformingTo: .data) ?? .data
}

/// One device: its state, its applets, its live log, its details. The store
/// is a page of its own (StorePage); a screenshot is saved, not shown
/// (AppState.keep).
struct DeviceDetailView: View {
    let device: Device
    @Environment(AppState.self) private var state
    @State private var dropTargeted = false

    private var model: HubModel { state.model }

    /// The section shown: the one chosen, unless this device has no such
    /// section (`supports`), which shows its applets. The choice is the
    /// window's and outlives a change of device.
    private var tab: DeviceTab {
        DeviceTab.offered(by: device).contains(state.deviceTab) ? state.deviceTab : .applets
    }

    /// How the last link ended, for the header. "The device is restarting"
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
        // Each section is a page of its own under the toolbar. The device's
        // state (its card, its software update, its applets) is the Applets
        // page, laid out as System Settings lays out a pane: grouped cards
        // that scroll beneath the toolbar.
        Group {
            switch tab {
            case .applets:
                DeviceOverview(device: device, unlink: unlinkNote, report: report)
            case .log:
                DeviceLogView(device: device)
            case .details:
                DeviceDetailsForm(device: device)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // The window keeps the device's name (Window menu, Mission Control);
        // the toolbar leaves it out, as the sidebar's selection and the
        // device's card already say it (HIG, Toolbars › Titles: a redundant
        // title can be left out). The room goes to the sections and actions,
        // which otherwise fold into the overflow menu at narrow widths.
        .navigationTitle(device.displayName)
        .toolbar(removing: .title)
        .toolbar { toolbar }
        .fileImporter(isPresented: $state.importingApplet, allowedContentTypes: [.pocketApplet]) { result in
            if case .success(let url) = result { push(url) }
        }
        .dropDestination(for: URL.self) { urls, _ in
            // A device that takes no applet from the computer takes no drop.
            guard device.supports(.push), device.isLinked,
                  let url = urls.first(where: { $0.pathExtension.lowercased() == "pocket" })
            else { return false }
            push(url)
            return true
        } isTargeted: { dropTargeted = $0 }
        .overlay {
            if dropTargeted && device.supports(.push) {
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
        .alert(item: Binding(get: { model.store.installFailure }, set: { model.store.installFailure = $0 })) { failure in
            Alert(title: Text(verbatim: failure.title),
                  message: Text(verbatim: [failure.advice, failure.detail].compactMap { $0 }.joined(separator: "\n\n")))
        }
        .confirmationDialog("Remove \(device.displayName) from Pocket Hub?", isPresented: $state.confirmingRemoveDevice) {
            Button("Remove", role: .destructive) { forget() }
        } message: {
            Text("The device disconnects. To use it with this Mac again, you will need to add it again.")
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        // On a Mac a segmented control switches views from the toolbar, not
        // from the window's content (HIG, Segmented controls › macOS).
        ToolbarItem(placement: .principal) {
            DeviceTabPicker(device: device, selection: tab)
        }
        // The toolbar holds what this device can do (`supports`): a device
        // that takes no screenshot or no applet from the computer has no
        // button for it.
        ToolbarItemGroup(placement: .primaryAction) {
            if device.supports(.screenshot) {
                // Saved where this Mac saves its own screenshots, and copied.
                Button("Take Screenshot", systemImage: "camera") { state.takeScreenshot(of: device.id) }
                    .disabled(!device.isLinked || model.takingScreenshot.contains(device.id))
                    .help(String(localized: "Save a screenshot of both screens to \(FileManager.default.displayName(atPath: ScreenshotDestination.folder().path))"))
            }
            if device.supports(.push) {
                Button("Install Applet…", systemImage: "square.and.arrow.down") { state.importingApplet = true }
                    .disabled(!device.isLinked || model.pushes[device.id] != nil)
                    .help("Install a .pocket applet on this device")
            }
            // Toolbar symbols carry no border of their own (HIG, Toolbars).
            Menu("More", systemImage: "ellipsis") {
                Button("Remove from Pocket Hub…", role: .destructive) { state.confirmingRemoveDevice = true }
            }
            .help("More actions")
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
        state.presentedError = UserFacingError(error, deviceName: device.displayName, kind: device.kind)
    }
}

extension DeviceTab {
    /// The sections a device's page has: Log for a device that sends one
    /// (`supports`), Applets and Details for every device.
    static func offered(by device: Device) -> [DeviceTab] {
        var tabs: [DeviceTab] = [.applets]
        if device.supports(.logs) { tabs.append(.log) }
        return tabs + [.details]
    }
}

/// The sections of a device's page, in the toolbar.
struct DeviceTabPicker: View {
    let device: Device
    /// The section shown (DeviceDetailView.tab).
    let selection: DeviceTab
    @Environment(AppState.self) private var state

    var body: some View {
        Picker("Section", selection: Binding(get: { selection }, set: { state.deviceTab = $0 })) {
            ForEach(DeviceTab.offered(by: device), id: \.self) { tab in
                switch tab {
                case .applets: Text("Applets").tag(tab)
                case .log: Text("Log").tag(tab)
                case .details: Text("Details").tag(tab)
                }
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .fixedSize()
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
        HStack(alignment: .top, spacing: 14) {
            // A pane's icon as System Settings draws one: a white glyph on a
            // filled rounded square, in the accent colour while connected.
            Image(systemName: device.kind.symbolName(filled: true))
                .font(.system(size: 26, weight: .medium))
                .foregroundStyle(.white)
                .frame(width: 52, height: 52)
                .background((device.isLinked ? Color.accentColor : Color.gray).gradient,
                            in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                Text(device.displayName).font(.title3.weight(.semibold))
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
                    Label("The last setup may not have finished. If the device doesn't connect, set it up again.",
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
                    Text(verbatim: device.kind.howToConnect).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// The Applets page: the device's card, its software update when there is
/// one, and the applets it holds (Open, and Update when the store has a
/// newer version), as grouped cards. What the store offers is the Store
/// page (StorePage).
struct DeviceOverview: View {
    let device: Device
    let unlink: DeviceUnlinked?
    let report: (any Error) -> Void
    @Environment(AppState.self) private var state

    var body: some View {
        let model = state.model
        Form {
            Section {
                DeviceHeader(device: device, unlink: unlink, push: model.pushes[device.id],
                             updating: model.programUpdates[device.id]?.isActive == true)
            }
            SoftwareUpdateSection(device: device)
            // A group of rows, as System Settings lists items with an action
            // each. Alternating row colours are for multicolumn tables (HIG,
            // Lists and tables), which this is not.
            Section("Installed") {
                let apps = device.sortedApps
                if apps.isEmpty {
                    EmptySectionNote(
                        title: device.isLinked ? Text("No Applets") : Text("Applets Unknown"),
                        detail: !device.isLinked ? Text("The list appears when the device connects.")
                            : device.supports(.push) ? Text("Drop a .pocket file here to install one.")
                            : Text("Install one from the store on the device itself."))
                } else {
                    ForEach(apps) { app in
                        AppletRow(app: app, device: device,
                                  isRunning: device.runningApp?.id == app.id,
                                  isLaunching: model.launching[device.id] == app.id) {
                            launch(app)
                        }
                    }
                }
            }
        }
        .formStyle(.grouped)
    }

    private func launch(_ app: InstalledApp) {
        Task {
            do { try await state.model.launch(app, on: device.id) } catch { report(error) }
        }
    }
}

/// What an empty section of a grouped page says in place of its rows.
struct EmptySectionNote: View {
    let title: Text
    let detail: Text

    var body: some View {
        VStack(spacing: 4) {
            title.font(.headline)
            detail.font(.callout).foregroundStyle(.secondary)
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
    }
}

struct AppletRow: View {
    let app: InstalledApp
    let device: Device
    let isRunning: Bool
    let isLaunching: Bool
    let open: () -> Void
    @Environment(AppState.self) private var state
    @State private var confirmingDelete = false

    /// The device's own rows carry no title; the store's entry for the same
    /// id gives one, and an icon. Applets the store does not know keep `output`.
    private var listed: StoreApp? { app.isHome ? nil : state.model.store.known[app.id] }
    private var title: String { listed?.title ?? app.displayName }
    private var isDeleting: Bool { state.model.removing[device.id] == app.id }

    var body: some View {
        let listed = self.listed
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
            let store = state.model.store
            if isDeleting {
                ProgressView().controlSize(.small)
            } else if let update, let install = store.install(for: update, on: device.id) {
                InstallProgressView(install: install) { store.cancelInstall(of: update, on: device.id) }
            } else {
                // A device that takes no applet from the computer updates
                // from its own store page; the row says so in place of a button.
                if update != nil, !device.supports(.push) {
                    Text("Update on the device").font(.caption).foregroundStyle(.secondary)
                } else if let update {
                    Button("Update") { store.install(update, on: device.id) }
                        .buttonStyle(.borderedProminent)
                        .disabled(!device.isLinked || device.busy != nil)
                        .help(String(localized: "Install version \(update.version)"))
                }
                if isLaunching {
                    ProgressView().controlSize(.small)
                } else {
                    Button("Open", action: open)
                        .disabled(!device.isLinked || device.busy != nil || isRunning)
                        .accessibilityLabel(Text("Open \(title)"))
                }
                if !app.isHome {
                    // The row's other actions, in the toolbar's own symbol;
                    // the same items are the row's context menu.
                    Menu { actions } label: { Image(systemName: "ellipsis") }
                        .menuStyle(.borderlessButton)
                        .menuIndicator(.hidden)
                        .fixedSize()
                        .help("More actions")
                }
            }
        }
        .padding(.vertical, 2)
        .contextMenu { if !app.isHome { actions } }
        .confirmationDialog(Text("Delete \(title) from \(device.displayName)?"), isPresented: $confirmingDelete) {
            Button("Delete", role: .destructive, action: delete)
        } message: {
            Text("Its data on the device is deleted with it.")
        }
    }

    /// The device refuses to delete the applet it runs, and the home screen
    /// has no row actions (link specification §6.2 `runtime.remove`). An
    /// applet the store lists has its sheet, as the store's rows do.
    @ViewBuilder
    private var actions: some View {
        Button("Open", action: open)
            .disabled(!device.isLinked || device.busy != nil || isRunning)
        if let listed {
            Button("Details") { state.show(storeApp: listed) }
        }
        Divider()
        Button("Delete…", role: .destructive) {
            if device.deletesApplets {
                confirmingDelete = true
            } else {
                // Software that predates deleting from Hub: say the device's way.
                state.presentedError = .deletingNeedsNewerSoftware(deviceName: device.displayName)
            }
        }
        .disabled(!device.isLinked || device.busy != nil || isRunning || isDeleting)
    }

    private func delete() {
        Task {
            do { try await state.model.remove(app, from: device.id) } catch {
                if let failure = UserFacingError(deleteError: error, appTitle: title, deviceName: device.displayName, kind: device.kind) {
                    state.presentedError = failure
                }
            }
        }
    }

    /// The store's entry for this applet when it offers this device a newer
    /// version (`update` in the device's list, CONTROL-API.md §3 "Store").
    private var update: StoreApp? {
        guard !app.isHome, let offer = state.model.store.offer(of: app.id, for: device.id), offer.action == .update else { return nil }
        return offer
    }
}
