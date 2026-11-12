import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The applet store: what the store offers for the chosen device, with one
/// action per applet (Install, Update, or Installed with Open).
struct StoreView: View {
    @Environment(AppState.self) private var state

    private var store: StoreModel { state.model.store }

    var body: some View {
        let model = state.model
        @Bindable var state = state
        VStack(spacing: 0) {
            if case .device(let id, _) = store.context, let device = model.device(id) {
                DeviceBanner(device: device, interrupted: store.interrupted?.deviceId == id ? store.interrupted : nil)
            } else {
                NoDeviceBanner()
            }
            Divider()
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle("Applet Store")
        .toolbar { toolbar }
        .sheet(item: $state.storeDetails) { app in
            StoreAppDetails(app: app) { state.storeDetails = nil }
        }
        .alert(item: Binding(get: { store.installFailure }, set: { store.installFailure = $0 })) { failure in
            Alert(title: Text(verbatim: failure.title),
                  message: Text(verbatim: [failure.advice, failure.detail].compactMap { $0 }.joined(separator: "\n\n")))
        }
        .task { if store.page == .idle { store.reload() } }
    }

    @ViewBuilder
    private var content: some View {
        switch (store.context, store.page) {
        case (.device(let id, false), _):
            let name = state.model.device(id)?.displayName ?? String(localized: "This device")
            ContentUnavailableView {
                Label { Text("\(name) hasn't connected yet") } icon: { Image(systemName: "questionmark.app.dashed") }
            } description: {
                Text("The store shows the applets a console can run once it has connected. Open Pocket Shell on the 3DS, then come back here.")
            }
        case (_, .idle), (_, .loading):
            ProgressView("Loading the store…")
        case (_, .failed(let failure)):
            ContentUnavailableView {
                Label { Text(verbatim: failure.title) } icon: { Image(systemName: "wifi.exclamationmark") }
            } description: {
                VStack(spacing: 6) {
                    Text(verbatim: failure.advice)
                    if let detail = failure.detail {
                        Text(verbatim: detail).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                }
            } actions: {
                Button("Try Again") { store.reload(refresh: true, clearing: true) }
                    .keyboardShortcut(.defaultAction)
            }
        case (let context, .loaded(let catalog)):
            if catalog.apps.isEmpty {
                ContentUnavailableView("No Applets", systemImage: "bag",
                                       description: Text("The store offers nothing for this device yet."))
            } else {
                List(catalog.apps) { app in
                    StoreRow(app: app, context: context) { state.storeDetails = app }
                }
                .listStyle(.inset(alternatesRowBackgrounds: true))
            }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup {
            let devices = state.model.devices
            if devices.count > 1 {
                Picker("Device", selection: Binding(get: { store.deviceId ?? "" }, set: { store.choose(device: $0) })) {
                    ForEach(devices) { device in
                        Text(verbatim: device.displayName).tag(device.id)
                    }
                }
                .help("The device the store shows applets for")
            }
            if store.isRefreshing {
                ProgressView().controlSize(.small)
            }
            Button("Refresh", systemImage: "arrow.clockwise") { store.reload(refresh: true) }
                .help("Fetch the store's list again")
                .disabled(!state.model.isLive)
        }
    }
}

/// The page's device, and why its actions are off when they are.
private struct DeviceBanner: View {
    let device: Device
    /// An install the link ended under, while the device is away.
    let interrupted: StoreModel.InterruptedInstall?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: device.isLinked ? "gamecontroller.fill" : "gamecontroller")
                .foregroundStyle(device.isLinked ? Color.green : Color.secondary)
                .accessibilityHidden(true)
            if let interrupted, !device.isLinked {
                // The one message for it: no alert, and not "turn it on".
                Text("The connection to \(device.displayName) ended while installing \(interrupted.appTitle). When it reconnects, this list shows whether \(interrupted.appTitle) was installed.")
                    .foregroundStyle(.orange)
            } else if device.isLinked {
                Text("Applets for \(device.displayName)")
            } else if device.hasKnownTarget {
                Text("\(device.displayName) isn't connected. Turn it on and open Pocket Shell to install applets.")
                    .foregroundStyle(.secondary)
            } else {
                Text("Applets for \(device.displayName)")
            }
            Spacer()
            if let busy = device.busy {
                Label { Text(verbatim: busy.displayText) } icon: { ProgressView().controlSize(.mini) }
                    .foregroundStyle(.orange)
            }
        }
        .font(.callout)
        .padding(.horizontal)
        .padding(.vertical, 10)
    }
}

private struct NoDeviceBanner: View {
    @Environment(AppState.self) private var state

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "info.circle").foregroundStyle(.secondary).accessibilityHidden(true)
            Text("Everything the store offers. Add a 3DS to install applets on it.")
                .foregroundStyle(.secondary)
            Spacer()
            Button("Add a 3DS…") { state.showingSetup = true }
        }
        .font(.callout)
        .padding(.horizontal)
        .padding(.vertical, 10)
    }
}

/// One applet: icon, title, author, summary, version and size, and its action.
private struct StoreRow: View {
    let app: StoreApp
    let context: StoreModel.Context
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
        switch context {
        case .noDevice, .device(_, false):
            EmptyView()
        case .device(let id, true):
            let device = state.model.device(id)
            let store = state.model.store
            if let install = store.install(for: app, on: id) {
                InstallProgressView(install: install) { store.cancelInstall(of: app, on: id) }
            } else {
                let unavailable = unavailableReason(device)
                switch app.action {
                case .install?:
                    Button("Install") { store.install(app, on: id) }
                        .disabled(unavailable != nil)
                        .help(unavailable ?? String(localized: "Install \(app.title) on \(device?.displayName ?? "")"))
                case .update?:
                    Button("Update") { store.install(app, on: id) }
                        .buttonStyle(.borderedProminent)
                        .disabled(unavailable != nil)
                        .help(unavailable ?? String(localized: "Install version \(app.version)"))
                case .installed?:
                    HStack(spacing: 8) {
                        Text("Installed").font(.caption).foregroundStyle(.secondary)
                        Button("Open") { open(id) }
                            .disabled(unavailable != nil)
                            .help(unavailable ?? String(localized: "Open \(app.title) on the console"))
                    }
                case nil:
                    EmptyView()
                }
            }
        }
    }

    /// Why the action cannot run now, in the owner's words; nil when it can.
    private func unavailableReason(_ device: Device?) -> String? {
        guard let device else { return String(localized: "The device is no longer paired.") }
        if !device.isLinked { return String(localized: "\(device.displayName) isn't connected right now.") }
        if let busy = device.busy { return busy.displayText }
        return nil
    }

    private func open(_ deviceId: String) {
        Task {
            do { try await state.model.store.open(app, on: deviceId) } catch {
                state.presentedError = UserFacingError(error, deviceName: state.model.device(deviceId)?.displayName)
            }
        }
    }
}

private struct InstallProgressView: View {
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

/// What the store declares about an applet. Nothing here is invented: every
/// line is a field of the store's entry.
struct StoreAppDetails: View {
    let app: StoreApp
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 14) {
                StoreIcon(path: app.icon, size: 64)
                VStack(alignment: .leading, spacing: 4) {
                    Text(verbatim: app.title).font(.title2.bold())
                    AuthorLabel(app: app)
                }
            }
            Text(verbatim: app.summary)
            Form {
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
                    Text("Not on this Mac yet. Installing from here downloads it first; until then the console's own store doesn't list it.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                LabeledContent("Identifier") { Text(verbatim: app.id).textSelection(.enabled) }
            }
            .formStyle(.grouped)
            HStack {
                Spacer()
                Button("Done", action: close).keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 480)
    }

    /// The 3DS's targets get the console's name; others keep the target's own.
    private func targetName(_ target: String) -> String {
        target == "3ds" || target.hasPrefix("3ds-") ? String(localized: "Nintendo 3DS") : target
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
