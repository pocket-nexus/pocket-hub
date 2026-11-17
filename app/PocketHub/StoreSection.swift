import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The applet store: the whole catalog (StoreModel.all), whatever devices
/// are paired or connected. What an applet's button does goes onto the
/// device chosen in the toolbar's Install On menu, as the store's list for
/// that device says (`installed`, `update`, CONTROL-API.md §3 "Store"): an
/// applet it cannot run has no button. With no device paired the page lists
/// and shows details, and installs nothing.
struct StorePage: View {
    @Environment(AppState.self) private var state

    private var store: StoreModel { state.model.store }
    /// The device the page installs on.
    private var device: Device? { store.deviceId.flatMap(state.model.device) }

    var body: some View {
        Form {
            Section {
                content
            } footer: {
                footer
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Store")
        .toolbar { toolbar }
        .task {
            if store.page == .idle { store.reload() }
        }
        // While an applet's sheet is open it shows this failure itself: an
        // alert of this page would wait behind the sheet.
        .alert(item: Binding(get: { state.storeDetails == nil ? store.installFailure : nil },
                             set: { store.installFailure = $0 })) { failure in
            Alert(title: Text(verbatim: failure.title),
                  message: Text(verbatim: [failure.advice, failure.detail].compactMap { $0 }.joined(separator: "\n\n")))
        }
    }

    /// The whole catalog. A failed fetch is said in its place, never an
    /// older list (CONTROL-API.md §3 `store.apps`); a fetch for another
    /// device keeps the list on screen while its buttons wait.
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
            if case .loaded = store.page {
                EmptySectionNote(title: Text("No Applets"), detail: Text("The store offers nothing yet."))
            } else {
                ProgressView("Loading the store…")
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
            }
        } else {
            ForEach(store.all) { app in
                StoreRow(app: app, device: device) { state.storeDetails = app }
            }
        }
    }

    /// Why the buttons are off or absent, when they are: no device, one
    /// that never connected (its target, which says what it can run, is
    /// unknown), one that installs by itself, an install the link ended
    /// under (no alert, and not "turn it on"), one that is away.
    @ViewBuilder
    private var footer: some View {
        if state.model.devices.isEmpty {
            Text("Add a device to Pocket Hub to install applets on it.")
        } else if let device {
            if let interrupted = store.interrupted, interrupted.deviceId == device.id, !device.isLinked {
                Text("The connection to \(device.displayName) ended while installing \(interrupted.appTitle). When it reconnects, this list shows whether \(interrupted.appTitle) was installed.")
                    .foregroundStyle(.orange)
            } else if !device.hasKnownTarget {
                Text("\(device.displayName) hasn't connected yet. Once it does, the store shows what it can install.")
            } else if !device.supports(.push) {
                // Its applets come from its own store page, which asks this
                // Mac for them (CONTROL-API.md §2 `supports`): the list shows
                // what that page offers, and has no Install.
                Text(installsItself(device))
            } else if !device.isLinked {
                Text(verbatim: notConnectedForInstall(device))
            }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            // The device the buttons install on: one menu for the page, so
            // a row says what that device holds of each applet.
            if let device {
                Menu {
                    Picker("Install On", selection: Binding(get: { device.id }, set: { store.choose(device: $0) })) {
                        ForEach(state.model.devices) { device in
                            Label(device.displayName, systemImage: device.kind.symbolName(filled: device.isLinked))
                                .tag(device.id)
                        }
                    }
                    .pickerStyle(.inline)
                } label: {
                    Label("Install on \(device.displayName)", systemImage: device.kind.symbolName(filled: device.isLinked))
                        .labelStyle(.titleAndIcon)
                }
                .help("Choose the device to install applets on")
            }
            Button("Refresh", systemImage: "arrow.clockwise") { store.reload(refresh: true) }
                .disabled(store.isRefreshing)
                .help("Fetch the store's list again")
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

/// One applet: icon, title, author, summary, version and size, and what it
/// can do on the device the store installs on (StoreAppAction).
struct StoreRow: View {
    let app: StoreApp
    /// The device the Store page installs on; nil with none paired.
    let device: Device?
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
            if let device {
                StoreAppAction(app: app, device: device) { state.presentedError = $0 }
                    .frame(minWidth: 110, alignment: .trailing)
            }
        }
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .onTapGesture(count: 2, perform: showDetails)
        .accessibilityElement(children: .contain)
    }
}

/// What an applet can do on a device, as the store's list for that device
/// says: Install, Update when it holds an older version, Open when it holds
/// this one, the progress of an install while one runs. A device that takes
/// no applet from the computer has no Install and no Update (its page's
/// footer says where it installs); an applet the device cannot run says so.
/// A row's button is plain; the sheet's is its one prominent button.
struct StoreAppAction: View {
    let app: StoreApp
    let device: Device
    var prominent = false
    /// A failure of Open: a row shows it in the window, the sheet over itself.
    let report: (UserFacingError) -> Void
    @Environment(AppState.self) private var state

    private var store: StoreModel { state.model.store }

    var body: some View {
        if let install = store.install(for: app, on: device.id) {
            InstallProgressView(install: install) { store.cancelInstall(of: app, on: device.id) }
        } else if !device.hasKnownTarget {
            // What it can run is unknown until it connects (the page's footer).
            EmptyView()
        } else if let list = store.list(for: device.id) {
            if let offer = list.first(where: { $0.id == app.id }) {
                action(for: offer)
            } else {
                Text("Not for \(device.displayName)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        } else if case .failed = store.page {
            // The page, or the device's page under the sheet, says why its list is not here.
            EmptyView()
        } else {
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
                .help(unavailable ?? String(localized: "Install version \(offer.version)"))
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

private extension View {
    /// The sheet's one prominent button; a row's plain one.
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

/// One applet of the store: what the store declares about it, what its
/// package says about it, and the one action that fits the device. A row's
/// Details opens it, and so does a link from the store's website
/// (docs/DEEP-LINKS.md §2), which is why the action is here: the link shows
/// this sheet and the owner installs from it.
///
/// The sheet acts on the device the store installs on (the Store page's
/// Install On, or the device whose page opened it), and the store's list for
/// that device says what it holds (`installed`, `update`). Nothing
/// here is invented: every line is a field of the store's entry, or of the
/// details the store reads out of the package (`store.details`), asked for
/// when the sheet opens. Until they are here, the devices it runs on are the
/// entry's targets.
struct StoreAppDetails: View {
    let app: StoreApp
    let close: () -> Void
    @Environment(AppState.self) private var state
    /// A failure of Open, shown over this sheet like one of an install.
    @State private var failure: UserFacingError?
    /// The details of the version shown, once asked for.
    @State private var details: DetailsLoad = .loading
    /// Counts Try Again, so the details are asked for again.
    @State private var attempt = 0

    private enum DetailsLoad: Equatable {
        case loading
        case loaded(StoreDetails)
        case failed(UserFacingError)
    }

    private var loaded: StoreDetails? {
        if case .loaded(let details) = details { return details }
        return nil
    }

    private var store: StoreModel { state.model.store }
    /// The paired devices that can run the applet.
    private var able: [Device] { store.devices(running: app) }
    /// The device the sheet acts on: the one the store installs on, when it
    /// can run the applet.
    private var device: Device? { able.first { $0.id == store.deviceId } }
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
                if let device {
                    StoreAppAction(app: app, device: device, prominent: true) { failure = $0 }
                }
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
                                                        set: { store.choose(device: $0) })) {
                        ForEach(able) { Text($0.displayName).tag($0.id) }
                    }
                }
                LabeledContent("Version") { Text(verbatim: app.version) }
                LabeledContent("Size") { Text(verbatim: app.sizeText) }
                LabeledContent("Runs on") { Text(verbatim: runsOn(app)) }
                if let screen = loaded?.screenText {
                    LabeledContent("Screen") { Text(verbatim: screen) }
                }
                capabilities
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
                Section {
                    LabeledContent("Identifier") { Text(verbatim: app.id).textSelection(.enabled) }
                    if let framework = loaded?.framework {
                        LabeledContent("Framework") { Text(verbatim: framework) }
                    }
                    if let api = loaded?.api {
                        LabeledContent("Applet API") { Text(verbatim: "\(api)") }
                    }
                }
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
        // The details of the version shown: again when the store lists a
        // newer one while the sheet is open, and on Try Again.
        .task(id: "\(app.id) \(app.version) \(attempt)") {
            details = .loading
            switch await store.details(of: app) {
            case .success(let answer): details = .loaded(answer)
            case .failure(let failure): details = .failed(failure)
            }
        }
        // The store may install on a device that cannot run the applet (it
        // was removed, or one that can has just linked for the first time):
        // the sheet chooses one that can.
        .onChange(of: able.map(\.id), initial: true) {
            if device == nil { state.chooseDevice(for: app) }
        }
        .alert(item: Binding(get: { failure ?? store.installFailure },
                             set: { _ in failure = nil; store.installFailure = nil })) { failure in
            Alert(title: Text(verbatim: failure.title),
                  message: Text(verbatim: [failure.advice, failure.detail].compactMap { $0 }.joined(separator: "\n\n")))
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

    /// What the package needs of a device and what it uses where a device
    /// has it, a row per capability by its id (`input.buttons`), as the
    /// package states it: the owner chose ids over words that explain them.
    /// While the details are on their way, or why they are not.
    @ViewBuilder
    private var capabilities: some View {
        switch details {
        case .loading:
            Section {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading details…").foregroundStyle(.secondary)
                }
            }
        case .failed(let failure):
            Section {
                VStack(alignment: .leading, spacing: 4) {
                    Text(verbatim: failure.title)
                    Text(verbatim: failure.advice).font(.callout).foregroundStyle(.secondary)
                }
                Button("Try Again") { attempt += 1 }
            }
        case .loaded(let details):
            if !details.needs.isEmpty {
                Section("What it needs") {
                    ForEach(details.needs, id: \.self, content: capabilityRow)
                }
            }
            if !details.uses.isEmpty {
                Section("What it uses if the device has it") {
                    ForEach(details.uses, id: \.self, content: capabilityRow)
                }
            }
        }
    }

    private func capabilityRow(_ id: String) -> some View {
        Text(verbatim: id).textSelection(.enabled)
    }

    /// The devices it runs on, each once: the store's names for those its
    /// package was built for, and until the details are here, the entry's
    /// targets.
    private func runsOn(_ app: StoreApp) -> String {
        let names = loaded?.deviceNames ?? app.targets.map(targetName).reduce(into: []) { names, name in
            if !names.contains(name) { names.append(name) }
        }
        return names.joined(separator: ", ")
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
