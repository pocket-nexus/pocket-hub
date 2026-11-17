import HubAppModel
import HubClient
import SwiftUI

/// One applet's page in the Store, over its list: what the store declares
/// about it, Try It on its sample data (AppletPlayerSection), what its
/// package says about it, and its Install, the device chosen as it is
/// pressed (StoreInstallButton). A row opens it, so do an installed
/// applet's Details and a link from the store's website (docs/DEEP-LINKS.md
/// §2): a link shows this page, and the owner installs from it.
///
/// Nothing here is invented: every line is a field of the store's entry, or
/// of the details the store reads out of the package (`store.details`),
/// asked for when the page opens. Until they are here, the devices it runs
/// on are the entry's targets.
struct StoreAppPage: View {
    let appId: String
    @Environment(AppState.self) private var state
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

    var body: some View {
        if let app = store.known[appId] {
            page(app)
        } else {
            ContentUnavailableView("This applet isn't in the store.", systemImage: "bag",
                                   description: Text("It may have been removed since the link was made."))
        }
    }

    private func page(_ app: StoreApp) -> some View {
        Form {
            Section {
                HStack(alignment: .top, spacing: 16) {
                    StoreIcon(path: app.icon, size: 88)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(verbatim: app.title).font(.title.bold())
                        AuthorLabel(app: app)
                        Text(verbatim: app.summary)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .padding(.top, 4)
                    }
                    Spacer(minLength: 12)
                    // A link can bring this page up while the owner is typing
                    // somewhere else (docs/DEEP-LINKS.md §2, "a link shows; it
                    // never installs"): Install is never the default button.
                    StoreInstallButton(app: app, prominent: true)
                }
                .padding(.vertical, 6)
                note(app)
            }
            if AppletPlayer.plays(app) {
                Section("Try It") {
                    AppletPlayerSection(app: app)
                }
            }
            Section {
                LabeledContent("Version") { Text(verbatim: app.version) }
                LabeledContent("Size") { Text(verbatim: app.sizeText) }
                LabeledContent("Runs on") { Text(verbatim: runsOn(app)) }
                if let screen = loaded?.screenText {
                    LabeledContent("Screen") { Text(verbatim: screen) }
                }
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
            Section {
                LabeledContent("Identifier") { Text(verbatim: app.id).textSelection(.enabled) }
                if let framework = loaded?.framework {
                    LabeledContent("Framework") { Text(verbatim: framework) }
                }
                if let api = loaded?.api {
                    LabeledContent("Applet API") { Text(verbatim: "\(api)") }
                }
            } footer: {
                if !app.cached {
                    Text("Not on this Mac yet. Installing from here downloads it first; until then the device's own store doesn't list it.")
                }
            }
        }
        .formStyle(.grouped)
        .navigationTitle(app.title)
        // The details of the version shown: again when the store lists a
        // newer one while the page is open, and on Try Again.
        .task(id: "\(app.id) \(app.version) \(attempt)") {
            details = .loading
            switch await store.details(of: app) {
            case .success(let answer): details = .loaded(answer)
            case .failure(let failure): details = .failed(failure)
            }
        }
    }

    /// Why Install is absent or cannot run now: no device, none that can
    /// run the applet, the one that can installs by itself or is away, an
    /// install of it the link ended under.
    @ViewBuilder
    private func note(_ app: StoreApp) -> some View {
        let devices = state.model.devices
        let able = store.devices(running: app)
        Group {
            if devices.isEmpty {
                HStack {
                    Text("Add a device to Pocket Hub to install this applet on it.")
                    Spacer()
                    Button("Add a Device…") { state.startSetup() }
                }
            } else if let interrupted = store.interrupted, interrupted.appId == app.id,
                      let device = state.model.device(interrupted.deviceId), !device.isLinked
            {
                Text("The connection to \(device.displayName) ended while installing \(interrupted.appTitle). When it reconnects, this page shows whether \(interrupted.appTitle) was installed.")
                    .foregroundStyle(.orange)
            } else if able.isEmpty {
                // A device that has never linked has no known target yet (StoreApp.runs(on:)).
                if let unknown = devices.first(where: { !$0.hasKnownTarget }) {
                    Text("\(unknown.displayName) hasn't connected yet, so Pocket Hub can't tell whether it runs this applet.")
                } else if devices.count == 1, let only = devices.first {
                    Text("\(app.title) isn't available for \(only.displayName).")
                } else {
                    Text("\(app.title) isn't available for any of your devices.")
                }
            } else if able.count == 1, let device = able.first {
                if !device.supports(.push), store.offer(of: app.id, for: device.id)?.action != .installed {
                    Text(installsItself(device))
                } else if !device.isLinked {
                    Text(verbatim: notConnectedForInstall(device))
                }
            }
        }
        .font(.callout)
        .foregroundStyle(.secondary)
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
    /// package was built for (the details, else the entry's `devices`), and
    /// until Hub has read them, the entry's targets.
    private func runsOn(_ app: StoreApp) -> String {
        if let names = loaded?.deviceNames { return names.joined(separator: ", ") }
        let named = app.devices?.map(\.name) ?? app.targets.map(targetName)
        return named.reduce(into: [String]()) { names, name in
            if !names.contains(name) { names.append(name) }
        }.joined(separator: ", ")
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
