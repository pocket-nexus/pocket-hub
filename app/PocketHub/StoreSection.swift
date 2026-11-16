import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The store's applets a device does not hold yet, a group of the device's
/// Applets page, each with Install. What the device holds is the group above
/// it, with Update when the store has a newer version (AppletRow).
///
/// The store is part of each device rather than a page of its own: Hub
/// answers its list for one device (CONTROL-API.md §3 "Store", by the
/// device's target), and every action on it goes onto that device.
/// DeviceOverview makes the store's device the one shown.
struct AvailableAppletsSection: View {
    let device: Device
    @Environment(AppState.self) private var state

    private var store: StoreModel { state.model.store }

    var body: some View {
        Section {
            content
        } header: {
            HStack {
                Text("Available in the Store")
                Spacer()
                if store.isRefreshing {
                    ProgressView().controlSize(.small)
                }
                Button("Refresh", systemImage: "arrow.clockwise") { store.reload(refresh: true) }
                    .labelStyle(.iconOnly)
                    .buttonStyle(.borderless)
                    .help("Fetch the store's list again")
                    .disabled(!device.hasKnownTarget)
            }
        } footer: {
            footer
        }
    }

    @ViewBuilder
    private var content: some View {
        if !device.hasKnownTarget {
            // Never linked: its target, which decides what the store offers it, is unknown.
            EmptySectionNote(title: Text("\(device.displayName) hasn't connected yet"),
                             detail: Text("Once it connects, the store shows the applets it can run."))
        } else if store.deviceId != device.id {
            loading
        } else {
            switch store.page {
            case .idle, .loading:
                loading
            case .failed(let failure):
                VStack(spacing: 8) {
                    EmptySectionNote(title: Text(verbatim: failure.title), detail: Text(verbatim: failure.advice))
                    if let detail = failure.detail {
                        Text(verbatim: detail).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    Button("Try Again") { store.reload(refresh: true, clearing: true) }
                }
                .frame(maxWidth: .infinity)
                .padding(.bottom, 8)
            case .loaded(let catalog):
                // An applet the device holds is a row of the group above.
                let available = catalog.apps.filter { $0.action == .install }
                if catalog.apps.isEmpty {
                    EmptySectionNote(title: Text("No Applets"),
                                     detail: Text("The store offers nothing for this device yet."))
                } else if available.isEmpty {
                    EmptySectionNote(title: Text("All Installed"),
                                     detail: Text("Everything the store offers for this device is on it."))
                } else {
                    ForEach(available) { app in
                        StoreRow(app: app, device: device) { state.storeDetails = app }
                    }
                }
            }
        }
    }

    private var loading: some View {
        ProgressView("Loading the store…")
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
    }

    /// Why Install is off, when it is: the one message for an install the
    /// link ended under (no alert, and not "turn it on"), else the device
    /// being away.
    @ViewBuilder
    private var footer: some View {
        if let interrupted = store.interrupted, interrupted.deviceId == device.id, !device.isLinked {
            Text("The connection to \(device.displayName) ended while installing \(interrupted.appTitle). When it reconnects, this list shows whether \(interrupted.appTitle) was installed.")
                .foregroundStyle(.orange)
        } else if !device.supports(.push) {
            // Its applets come from its own store page, which asks this Mac
            // for them (CONTROL-API.md §2 `supports`): the list shows what
            // that page offers, and has no Install.
            Text(installsItself(device))
        } else if device.hasKnownTarget && !device.isLinked {
            Text(verbatim: notConnectedForInstall(device))
        }
    }
}

/// Where a device that takes no applet from the computer installs one: on a
/// Tab5 the store is the Mac icon of its home screen.
@MainActor
func installsItself(_ device: Device) -> LocalizedStringKey {
    device.kind == .tab5
        ? "\(device.displayName) installs applets itself: open **Mac** on its home screen. This Mac sends them while Pocket Hub is running."
        : "\(device.displayName) installs applets itself, from the store on the device. This Mac sends them while Pocket Hub is running."
}

/// Why nothing can be installed now, and what brings the device back.
@MainActor
func notConnectedForInstall(_ device: Device) -> String {
    device.kind == .nintendo3ds
        ? String(localized: "\(device.displayName) isn't connected. Turn it on and open Pocket Shell to install applets.")
        : String(localized: "\(device.displayName) isn't connected. Turn it on to install applets.")
}

/// One applet: icon, title, author, summary, version and size, and its action.
struct StoreRow: View {
    let app: StoreApp
    let device: Device
    let showDetails: () -> Void
    @Environment(AppState.self) private var state

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            StoreIcon(path: app.icon, size: 44)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(verbatim: app.title).font(.headline)
                    AuthorLabel(app: app)
                }
                Text(verbatim: app.summary).lineLimit(2).foregroundStyle(.secondary)
                Text("Version \(app.version) · \(app.sizeText)").font(.caption).foregroundStyle(.tertiary)
            }
            Spacer(minLength: 12)
            Button("Details", systemImage: "info.circle", action: showDetails)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("What this applet reaches, and where it runs")
            action
                .frame(minWidth: 110, alignment: .trailing)
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .onTapGesture(count: 2, perform: showDetails)
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder
    private var action: some View {
        let store = state.model.store
        if !device.supports(.push) {
            // No Install: the section's footer says where this device installs.
            EmptyView()
        } else if let install = store.install(for: app, on: device.id) {
            InstallProgressView(install: install) { store.cancelInstall(of: app, on: device.id) }
        } else {
            let unavailable = installUnavailableReason(state.model.device(device.id))
            Button("Install") { store.install(app, on: device.id) }
                .disabled(unavailable != nil)
                .help(unavailable ?? String(localized: "Install \(app.title) on \(device.displayName)"))
        }
    }
}

/// Why an install cannot run on the device now, in the owner's words; nil
/// when it can.
@MainActor
private func installUnavailableReason(_ device: Device?) -> String? {
    guard let device else { return String(localized: "The device is no longer paired.") }
    if !device.isLinked { return String(localized: "\(device.displayName) isn't connected right now.") }
    if let busy = device.busy { return busy.displayText }
    return nil
}

struct InstallProgressView: View {
    let install: StoreModel.Install
    let cancel: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            // Wide enough for the longest phase in either language; a phase
            // that is still longer wraps rather than being cut off.
            VStack(alignment: .trailing, spacing: 2) {
                if let fraction = install.fraction {
                    ProgressView(value: fraction)
                } else {
                    ProgressView().progressViewStyle(.linear)
                }
                Text(verbatim: transferPhaseText(install.phase))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.trailing)
                    .lineLimit(2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(width: 140, alignment: .trailing)
            Button("Stop", systemImage: "xmark.circle.fill", action: cancel)
                .labelStyle(.iconOnly)
                .buttonStyle(.borderless)
                .help("Stop installing")
        }
    }
}

struct AuthorLabel: View {
    let app: StoreApp

    var body: some View {
        HStack(spacing: 3) {
            Text(verbatim: app.author).font(.caption).foregroundStyle(.secondary)
            if app.official {
                Image(systemName: "checkmark.seal.fill")
                    .font(.caption)
                    .foregroundStyle(Color.accentColor)
                    .help("An official applet, signed with the store's own key")
                    .accessibilityLabel(Text("Official"))
            }
        }
    }
}

/// One applet of the store: what the store declares about it, and the one
/// action that fits the device. A row's Details opens it, and so does a link
/// from the store's website (docs/DEEP-LINKS.md §2), which is why the action
/// is here: the link shows this sheet and the owner installs from it.
///
/// The sheet acts on the device whose page the window shows, and the store's
/// list for that device says what it holds (`installed`, `update`). Nothing
/// here is invented: every line is a field of the store's entry.
struct StoreAppDetails: View {
    let app: StoreApp
    let close: () -> Void
    @Environment(AppState.self) private var state
    /// A failure of Open, shown over this sheet like one of an install.
    @State private var failure: UserFacingError?

    private var store: StoreModel { state.model.store }
    /// The paired devices that can run the applet.
    private var able: [Device] { store.devices(running: app) }
    /// The device the sheet acts on: the one shown, when it can run the applet.
    private var device: Device? { able.first { $0.id == state.selectedDeviceID } }
    /// The applet as the store offers it to that device now; nil until its
    /// list is here.
    private var offer: StoreApp? {
        guard let device, store.deviceId == device.id else { return nil }
        return store.apps.first { $0.id == app.id }
    }
    /// Install or Update is on offer: then it is the sheet's one prominent button.
    private var offersInstall: Bool { offer?.action == .install || offer?.action == .update }
    /// The store's entry as it is now. The sheet stays open across an install,
    /// which changes what the entry says (`cached`), and across a refresh.
    private var entry: StoreApp { offer ?? store.known[app.id] ?? app }

    var body: some View {
        let app = entry
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 14) {
                StoreIcon(path: app.icon, size: 64)
                VStack(alignment: .leading, spacing: 4) {
                    Text(verbatim: app.title).font(.title2.bold())
                    AuthorLabel(app: app)
                }
                Spacer(minLength: 12)
                if let device { action(on: device) }
            }
            Text(verbatim: app.summary)
            note
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Form {
                if able.count > 1 {
                    // Several devices can run it: the owner says which one.
                    Picker("Device", selection: Binding(get: { device?.id ?? "" },
                                                        set: { state.show(app, on: $0) })) {
                        ForEach(able) { Text($0.displayName).tag($0.id) }
                    }
                }
                LabeledContent("Version") { Text(verbatim: app.version) }
                LabeledContent("Size") { Text(verbatim: app.sizeText) }
                LabeledContent("Runs on") {
                    Text(verbatim: Array(NSOrderedSet(array: app.targets.map(targetName))).compactMap { $0 as? String }.joined(separator: ", "))
                }
                Section("What it connects to") {
                    if app.services.isEmpty && app.hosts.isEmpty {
                        Text("Nothing: it works without the network.")
                    }
                    if !app.services.isEmpty {
                        LabeledContent("Through this Mac") { Text(verbatim: app.services.joined(separator: ", ")) }
                    }
                    if !app.hosts.isEmpty {
                        LabeledContent("Websites") { Text(verbatim: app.hosts.joined(separator: ", ")) }
                    }
                }
                if !app.cached {
                    Text("Not on this Mac yet. Installing from here downloads it first; until then the device's own store doesn't list it.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                LabeledContent("Identifier") { Text(verbatim: app.id).textSelection(.enabled) }
            }
            .formStyle(.grouped)
            HStack {
                if state.model.devices.isEmpty {
                    Button("Add a Device…") {
                        close()
                        state.startSetup()
                    }
                }
                Spacer()
                // Return closes the sheet and never installs: a link can bring
                // this sheet up while the owner is typing somewhere else
                // (docs/DEEP-LINKS.md §2, "a link shows; it never installs").
                // With Install on offer Done steps back to Esc, so the sheet
                // keeps one prominent button.
                Button("Done", action: close)
                    .keyboardShortcut(offersInstall ? .cancelAction : .defaultAction)
            }
        }
        .padding(20)
        .frame(width: 480)
        // The window may show another page, or a device that cannot run the
        // applet (it was removed, or one that can has just linked for the
        // first time): the sheet moves it to one that can.
        .onChange(of: able.map(\.id), initial: true) {
            if device == nil { state.showDevice(for: app) }
        }
        .alert(item: Binding(get: { failure ?? store.installFailure },
                             set: { _ in failure = nil; store.installFailure = nil })) { failure in
            Alert(title: Text(verbatim: failure.title),
                  message: Text(verbatim: [failure.advice, failure.detail].compactMap { $0 }.joined(separator: "\n\n")))
        }
    }

    /// Install, Update or Open, as the store's list for the device says; the
    /// progress of an install while one runs.
    @ViewBuilder
    private func action(on device: Device) -> some View {
        if let install = store.install(for: app, on: device.id) {
            InstallProgressView(install: install) { store.cancelInstall(of: app, on: device.id) }
        } else if let offer {
            let unavailable = installUnavailableReason(device)
            switch offer.action {
            case .install where !device.supports(.push), .update where !device.supports(.push):
                // The note below says where this device installs it.
                EmptyView()
            case .install:
                Button("Install") { store.install(offer, on: device.id) }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .disabled(unavailable != nil)
                    .help(unavailable ?? String(localized: "Install \(app.title) on \(device.displayName)"))
            case .update:
                Button("Update") { store.install(offer, on: device.id) }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.large)
                    .disabled(unavailable != nil)
                    .help(unavailable ?? String(localized: "Install version \(offer.version)"))
            case .installed:
                if state.model.launching[device.id] == app.id {
                    ProgressView().controlSize(.small)
                } else {
                    Button("Open") { open(on: device) }
                        .controlSize(.large)
                        .disabled(unavailable != nil || device.runningApp?.id == app.id)
                        .help(unavailable ?? String(localized: "Open \(app.title) on \(device.displayName)"))
                }
            case nil:
                EmptyView()
            }
        } else if case .failed = store.page {
            // The device's page, under this sheet, says why its list is not here.
            EmptyView()
        } else {
            ProgressView().controlSize(.small)
        }
    }

    /// Why the sheet has no action, or why its action cannot run now.
    @ViewBuilder
    private var note: some View {
        let devices = state.model.devices
        if devices.isEmpty {
            Text("Add a device to Pocket Hub to install this applet on it.")
        } else if able.isEmpty {
            // A device that has never linked has no known target yet (StoreApp.runs(on:)).
            if let unknown = devices.first(where: { !$0.hasKnownTarget }) {
                Text("\(unknown.displayName) hasn't connected yet, so Pocket Hub can't tell whether it runs this applet.")
            } else if devices.count == 1, let only = devices.first {
                Text("\(app.title) isn't available for \(only.displayName).")
            } else {
                Text("\(app.title) isn't available for any of your devices.")
            }
        } else if let device, !device.supports(.push), offer?.action != .installed {
            Text(installsItself(device))
        } else if let device, !device.isLinked {
            Text(verbatim: notConnectedForInstall(device))
        }
    }

    private func open(on device: Device) {
        Task {
            do { try await store.open(app, on: device.id) } catch {
                failure = UserFacingError(error, deviceName: device.displayName, kind: device.kind)
            }
        }
    }

    /// A target of a family Hub knows gets the family's name (`3ds-dev`:
    /// Nintendo 3DS, `tab5-home`: M5Stack Tab5); others keep the target's own.
    private func targetName(_ target: String) -> String {
        for kind in [DeviceKind.nintendo3ds, .tab5] where target == kind.rawValue || target.hasPrefix(kind.rawValue + "-") {
            return kind.displayName
        }
        return target
    }
}

/// An icon from Hub's cache, read off the main actor; a placeholder until then.
struct StoreIcon: View {
    let path: String?
    let size: CGFloat
    @State private var image: NSImage?

    var body: some View {
        Group {
            if let image {
                Image(nsImage: image).resizable().interpolation(.high)
            } else {
                RoundedRectangle(cornerRadius: size * 0.22)
                    .fill(.quaternary)
                    .overlay { Image(systemName: "app").foregroundStyle(.secondary) }
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: size * 0.22))
        .accessibilityHidden(true)
        .task(id: path) {
            guard let path else { image = nil; return }
            image = await StoreIconCache.shared.image(path: path)
        }
    }
}

/// Icons are cached by path: Hub names each icon file by its content's hash.
@MainActor
final class StoreIconCache {
    static let shared = StoreIconCache()
    private let cache = NSCache<NSString, NSImage>()

    func image(path: String) async -> NSImage? {
        if let hit = cache.object(forKey: path as NSString) { return hit }
        guard let image = await ScreenshotFile.loadImage(path: path) else { return nil }
        cache.setObject(image, forKey: path as NSString)
        return image
    }
}
