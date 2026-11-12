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
        } else if device.hasKnownTarget && !device.isLinked {
            Text("\(device.displayName) isn't connected. Turn it on and open Pocket Shell to install applets.")
        }
    }
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
        if let install = store.install(for: app, on: device.id) {
            InstallProgressView(install: install) { store.cancelInstall(of: app, on: device.id) }
        } else {
            let unavailable = unavailableReason(state.model.device(device.id))
            Button("Install") { store.install(app, on: device.id) }
                .disabled(unavailable != nil)
                .help(unavailable ?? String(localized: "Install \(app.title) on \(device.displayName)"))
        }
    }

    /// Why the action cannot run now, in the owner's words; nil when it can.
    private func unavailableReason(_ device: Device?) -> String? {
        guard let device else { return String(localized: "The device is no longer paired.") }
        if !device.isLinked { return String(localized: "\(device.displayName) isn't connected right now.") }
        if let busy = device.busy { return busy.displayText }
        return nil
    }
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
