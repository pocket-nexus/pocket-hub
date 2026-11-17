import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The applet store: the whole catalog (StoreModel.all), whatever devices
/// are paired or connected, and an applet's page over it (StoreAppPage).
/// The toolbar narrows the list to one device, by the store's names for the
/// devices the packages were built for. An applet's Install goes onto a
/// device the owner chooses then (StoreInstallButton).
struct StorePage: View {
    @Environment(AppState.self) private var state

    private var store: StoreModel { state.model.store }

    var body: some View {
        @Bindable var state = state
        NavigationStack(path: $state.storePath) {
            Form {
                Section {
                    content
                } footer: {
                    if state.model.devices.isEmpty {
                        Text("Add a device to Pocket Hub to install applets on it.")
                    }
                }
            }
            .formStyle(.grouped)
            .navigationTitle("Store")
            .toolbar { toolbar }
            .navigationDestination(for: String.self) { id in
                StoreAppPage(appId: id)
            }
        }
        .task {
            if store.page == .idle { store.reload() }
        }
        .alert(item: Binding(get: { store.installFailure }, set: { store.installFailure = $0 })) { failure in
            Alert(title: Text(verbatim: failure.title),
                  message: Text(verbatim: [failure.advice, failure.detail].compactMap { $0 }.joined(separator: "\n\n")))
        }
    }

    /// The applets the filter keeps: those built for the device it names.
    /// One whose devices Hub has not read yet (`devices` nil) waits for them.
    private var shown: [StoreApp] {
        guard let filter = state.storeFilter else { return store.all }
        return store.all.filter { $0.devices?.contains { $0.name == filter } == true }
    }

    /// Every device the catalog's packages were built for, by the store's
    /// name, in the store's order.
    private var deviceNames: [String] {
        store.all.flatMap { $0.devices ?? [] }.map(\.name).reduce(into: []) { names, name in
            if !names.contains(name) { names.append(name) }
        }
    }

    /// The whole catalog. A failed fetch is said in its place, never an
    /// older list (CONTROL-API.md §3 `store.apps`).
    @ViewBuilder
    private var content: some View {
        if case .failed(let failure) = store.page {
            VStack(spacing: 8) {
                EmptySectionNote(title: Text(verbatim: failure.title), detail: Text(verbatim: failure.advice))
                if let detail = failure.detail {
                    Text(verbatim: detail).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                }
                Button("Try Again") { store.reload(refresh: true, clearing: true) }
            }
            .frame(maxWidth: .infinity)
            .padding(.bottom, 8)
        } else if store.all.isEmpty {
            if store.page == .loaded {
                EmptySectionNote(title: Text("No Applets"), detail: Text("The store offers nothing yet."))
            } else {
                ProgressView("Loading the store…")
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
            }
        } else if shown.isEmpty, let filter = state.storeFilter {
            EmptySectionNote(title: Text("No Applets"), detail: Text("The store offers nothing for \(filter) yet."))
        } else {
            ForEach(shown) { app in
                StoreRow(app: app) { state.storePath.append(app.id) }
            }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        // A segmented control narrows what the page shows (HIG, Segmented
        // controls): every applet, or those built for one device.
        if deviceNames.count > 1 {
            ToolbarItem(placement: .principal) {
                Picker("Device", selection: Binding(get: { state.storeFilter }, set: { state.storeFilter = $0 })) {
                    Text("All").tag(String?.none)
                    ForEach(deviceNames, id: \.self) { name in
                        Text(verbatim: name).tag(Optional(name))
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                .help("Show the applets for one device")
            }
        }
        ToolbarItem(placement: .primaryAction) {
            Button("Refresh", systemImage: "arrow.clockwise") { store.reload(refresh: true) }
                .disabled(store.isRefreshing)
                .help("Fetch the store's list again")
        }
    }
}

/// One applet of the list: icon, title, author, summary, version and size,
/// which open its page, and its Install.
struct StoreRow: View {
    let app: StoreApp
    let open: () -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Button(action: open) {
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
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(String(localized: "Show \(app.title)"))
            StoreInstallButton(app: app)
                .frame(minWidth: 110, alignment: .trailing)
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
    }
}

/// An applet's Install, with the device chosen as it is pressed: of the
/// paired devices that can run the applet (StoreModel.devices(running:)),
/// the one there is, or a menu of them, each said with what the store's list
/// for it offers (`installed`, `update`). The progress of an install while
/// one runs. Nothing with no device that can run it: the page says why.
struct StoreInstallButton: View {
    let app: StoreApp
    /// An applet page's one prominent button; a row's is plain.
    var prominent = false
    @Environment(AppState.self) private var state

    private var store: StoreModel { state.model.store }

    var body: some View {
        let able = store.devices(running: app)
        if let busy = able.first(where: { store.install(for: app, on: $0.id) != nil }),
           let install = store.install(for: app, on: busy.id)
        {
            InstallProgressView(install: install) { store.cancelInstall(of: app, on: busy.id) }
        } else if able.count == 1, let device = able.first {
            StoreAppAction(app: app, device: device, prominent: prominent) { state.presentedError = $0 }
        } else if able.count > 1 {
            Menu {
                ForEach(able) { device in item(on: device) }
            } label: {
                Text(menuTitle(able))
            }
            .prominent(prominent)
            .fixedSize()
            .help("Choose the device")
        }
    }

    /// Update when a device holds an older version, else Install when one
    /// can take it, else Open.
    private func menuTitle(_ able: [Device]) -> LocalizedStringKey {
        let actions = able.compactMap { device in device.supports(.push) ? store.offer(of: app.id, for: device.id)?.action : nil }
        if actions.contains(.update) { return "Update" }
        if actions.contains(.install) { return "Install" }
        return "Open"
    }

    /// What the applet can do on one device, as a menu item; one that cannot
    /// take it now says why, off.
    @ViewBuilder
    private func item(on device: Device) -> some View {
        let name = device.displayName
        if !device.isLinked {
            Button("\(name) — Not Connected") {}.disabled(true)
        } else if device.busy != nil {
            Button("\(name) — Busy") {}.disabled(true)
        } else if let offer = store.offer(of: app.id, for: device.id) {
            switch offer.action {
            case .install where device.supports(.push):
                Button("Install on \(name)") { store.install(offer, on: device.id) }
            case .update where device.supports(.push):
                Button("Update on \(name)") { store.install(offer, on: device.id) }
            case .installed:
                Button("Open on \(name)") { open(on: device) }
                    .disabled(device.runningApp?.id == app.id)
            case .install, .update:
                // Its applets come from its own store page (CONTROL-API.md §2 `supports`).
                Button("\(name) — Install It on the Device") {}.disabled(true)
            case nil:
                EmptyView()
            }
        } else {
            Button("\(name) — Loading…") {}.disabled(true)
        }
    }

    private func open(on device: Device) {
        Task {
            do { try await store.open(app, on: device.id) } catch {
                if let failure = UserFacingError(error, deviceName: device.displayName, kind: device.kind) {
                    state.presentedError = failure
                }
            }
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

/// What an applet can do on one device, as the store's list for that device
/// says: Install, Update when it holds an older version, Open when it holds
/// this one. A device that takes no applet from the computer has no Install
/// and no Update (the applet's page says where it installs).
struct StoreAppAction: View {
    let app: StoreApp
    let device: Device
    var prominent = false
    /// A failure of Open.
    let report: (UserFacingError) -> Void
    @Environment(AppState.self) private var state

    private var store: StoreModel { state.model.store }

    var body: some View {
        if let offer = store.offer(of: app.id, for: device.id) {
            action(for: offer)
        } else if store.list(for: device.id) == nil, store.page != .loaded {
            ProgressView().controlSize(.small)
        }
    }

    @ViewBuilder
    private func action(for offer: StoreApp) -> some View {
        let unavailable = installUnavailableReason(device)
        switch offer.action {
        case .install where !device.supports(.push), .update where !device.supports(.push):
            EmptyView()
        case .install:
            Button("Install") { store.install(offer, on: device.id) }
                .prominent(prominent)
                .disabled(unavailable != nil)
                .help(unavailable ?? String(localized: "Install \(app.title) on \(device.displayName)"))
        case .update:
            Button("Update") { store.install(offer, on: device.id) }
                .prominent(prominent)
                .disabled(unavailable != nil)
                .help(unavailable ?? String(localized: "Install version \(offer.version) on \(device.displayName)"))
        case .installed:
            if state.model.launching[device.id] == app.id {
                ProgressView().controlSize(.small)
            } else {
                Button("Open", action: open)
                    .controlSize(prominent ? .large : .regular)
                    .disabled(unavailable != nil || device.runningApp?.id == app.id)
                    .help(unavailable ?? String(localized: "Open \(app.title) on \(device.displayName)"))
            }
        case nil:
            EmptyView()
        }
    }

    private func open() {
        Task {
            do { try await store.open(app, on: device.id) } catch {
                if let failure = UserFacingError(error, deviceName: device.displayName, kind: device.kind) { report(failure) }
            }
        }
    }
}

extension View {
    /// An applet page's one prominent button; a row's plain one.
    @ViewBuilder
    func prominent(_ on: Bool) -> some View {
        if on {
            buttonStyle(.borderedProminent).controlSize(.large)
        } else {
            self
        }
    }
}

/// Why an install cannot run on the device now, in the owner's words; nil
/// when it can.
@MainActor
func installUnavailableReason(_ device: Device?) -> String? {
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
