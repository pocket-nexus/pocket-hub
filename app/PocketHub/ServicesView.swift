import AppKit
import HubAppModel
import HubClient
import SwiftUI
import UniformTypeIdentifiers

/// The installed services (each applet's computer side), their state and
/// output, installing one, and the Bun runtime they run on.
struct ServicesView: View {
    @Environment(AppState.self) private var state
    @State private var selection: String?
    @State private var showingLog = false
    @State private var importing = false
    @State private var confirmingRemove: Service?

    private var model: HubModel { state.model }

    var body: some View {
        // The runtime, the install under way and the services are cards of one
        // grouped page. A row's output opens in the inspector, from the row's
        // own button or its context menu.
        Form {
            Section {
                BunRuntimeBar()
            }
            if let install = model.serviceInstall {
                Section {
                    HStack {
                        ProgressView().controlSize(.small)
                        Text("Installing \(install.source)…")
                        Text(verbatim: serviceInstallStageText(install.stage)).foregroundStyle(.secondary)
                        Spacer()
                    }
                }
            }
            Section("Services") {
                if let services = model.services {
                    if services.isEmpty {
                        VStack(spacing: 8) {
                            EmptySectionNote(
                                title: Text("No Services"),
                                detail: Text("Some applets come with a part that runs on this Mac. Install one from a folder or a .tar.gz file."))
                            Button("Install Service…") { importing = true }
                        }
                        .padding(.bottom, 8)
                    } else {
                        ForEach(services) { service in
                            ServiceRow(service: service, perform: perform, installAgain: { importing = true }) { showOutput(of: service.id) }
                                .contextMenu {
                                    Button("Start") { perform(.start, service.id) }.disabled(service.state == .running || service.isEarlier)
                                    Button("Stop") { perform(.stop, service.id) }.disabled(service.state == .stopped)
                                    Button("Restart") { perform(.restart, service.id) }.disabled(service.isEarlier)
                                    Divider()
                                    Button("Show Output") { showOutput(of: service.id) }
                                    Divider()
                                    Button("Remove…", role: .destructive) { confirmingRemove = service }
                                }
                        }
                    }
                } else {
                    EmptySectionNote(title: Text("Services Unavailable"),
                                     detail: Text("Pocket Hub couldn't list its services."))
                }
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Services")
        .toolbar {
            ToolbarItemGroup {
                Button("Install Service…", systemImage: "plus") { importing = true }
                    .disabled(model.serviceInstall != nil)
                    .help("Install a service from a folder or a .tar.gz file")
                Button("Show Output", systemImage: "sidebar.right") { showingLog.toggle() }
                    .disabled(selection == nil)
                    .help("Show the selected service's output")
            }
        }
        .inspector(isPresented: $showingLog) {
            if let id = selection, let service = model.services?.first(where: { $0.id == id }) {
                ServiceLogView(service: service)
                    .inspectorColumnWidth(min: 280, ideal: 380, max: 700)
                    .navigationTitle(service.id)
            } else {
                ContentUnavailableView("No Service Selected", systemImage: "gearshape.2")
            }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.folder, .gzip]) { result in
            if case .success(let url) = result { install(url) }
        }
        .confirmationDialog(Text("Remove \(confirmingRemove?.id ?? "")?"),
                            isPresented: Binding(get: { confirmingRemove != nil }, set: { if !$0 { confirmingRemove = nil } })) {
            Button("Remove", role: .destructive) {
                if let id = confirmingRemove?.id { remove(id, purge: false) }
            }
            Button("Remove and Delete Its Data", role: .destructive) {
                if let id = confirmingRemove?.id { remove(id, purge: true) }
            }
        } message: {
            Text("Its applet keeps working on the device but can no longer reach this Mac. Remove keeps the service's own data for a later install.")
        }
    }

    private func showOutput(of id: String) {
        selection = id
        showingLog = true
    }

    private func perform(_ action: ServiceAction, _ id: String) {
        Task {
            do { try await model.perform(action, service: id) } catch { state.presentedError = UserFacingError(error) }
        }
    }

    private func install(_ url: URL) {
        Task {
            let access = url.startAccessingSecurityScopedResource()
            defer { if access { url.stopAccessingSecurityScopedResource() } }
            do { try await model.installService(from: url) } catch { state.presentedError = UserFacingError(error) }
        }
    }

    private func remove(_ id: String, purge: Bool) {
        Task {
            do { try await model.removeService(id, purge: purge) } catch { state.presentedError = UserFacingError(error) }
        }
    }
}

struct ServiceRow: View {
    let service: Service
    let perform: (ServiceAction, String) -> Void
    /// Opens the install panel: what an earlier Pocket Hub installed is
    /// installed again from its folder.
    let installAgain: () -> Void
    let showOutput: () -> Void

    var body: some View {
        HStack {
            Circle()
                .fill(color)
                .frame(width: 9, height: 9)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: service.id)
                HStack(spacing: 6) {
                    Text(service.state.displayName)
                    if let release = service.release {
                        if release == "dev" { Text("Development release") } else { Text("Release \(release)") }
                    }
                    if let pid = service.pid { Text("PID \(String(pid))") }
                    if service.channels > 0 { Text("\(service.channels) channels open") }
                    if let since = service.since { Text(since.date.formatted(.relative(presentation: .named))) }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if let store = service.store {
                    if store.isNewer {
                        Text("Its data was saved by a newer Pocket Hub and is kept as it is. Update Pocket Hub to use it.")
                            .font(.caption).foregroundStyle(.orange)
                    } else {
                        Text("Data kept on this Mac: \(Self.size(store.bytes ?? 0)) of \(Self.size(store.maxBytes))")
                            .font(.caption).foregroundStyle(.secondary)
                            .help(Text("\(Self.size(store.fileBytes)) on disk"))
                    }
                }
                if service.isEarlier {
                    // The core's sentence is English; this one is the app's, in both languages.
                    Text("Installed by an earlier Pocket Hub. Install it again.").font(.caption).foregroundStyle(.red)
                } else if let error = service.error {
                    Text(verbatim: error).font(.caption).foregroundStyle(.red)
                }
            }
            Spacer()
            switch service.state {
            case _ where service.isEarlier:
                Button("Install Again…", action: installAgain)
            case .running:
                Button("Restart", systemImage: "arrow.clockwise") { perform(.restart, service.id) }
                    .labelStyle(.iconOnly).help("Restart")
                Button("Stop", systemImage: "stop.fill") { perform(.stop, service.id) }
                    .labelStyle(.iconOnly).help("Stop")
            case .starting, .stopping:
                ProgressView().controlSize(.small)
            default:
                Button("Start", systemImage: "play.fill") { perform(.start, service.id) }
                    .labelStyle(.iconOnly).help("Start")
            }
            Button("Show Output", systemImage: "text.alignleft", action: showOutput)
                .labelStyle(.iconOnly).help("Show Output")
        }
        .buttonStyle(.borderless)
        .padding(.vertical, 2)
        .accessibilityElement(children: .contain)
    }

    private static func size(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }

    private var color: Color {
        switch service.state {
        case .running: .green
        case .starting, .stopping: .yellow
        case .failed: .red
        default: .secondary
        }
    }
}

/// The Bun runtime: installed or not, with a download button.
struct BunRuntimeBar: View {
    @Environment(AppState.self) private var state

    var body: some View {
        let model = state.model
        HStack(spacing: 12) {
            Image(systemName: "shippingbox")
                .font(.title2)
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text("Bun Runtime").font(.headline)
                if let bun = model.bun {
                    if bun.installed {
                        Text("Version \(bun.version ?? "?") installed").foregroundStyle(.secondary)
                        if let path = bun.path { Text(verbatim: path).font(.caption).foregroundStyle(.tertiary).textSelection(.enabled) }
                    } else {
                        Text("Not installed. Services need it to run.").foregroundStyle(.secondary)
                    }
                } else {
                    Text("Unknown").foregroundStyle(.secondary)
                }
            }
            Spacer()
            if let progress = model.bunInstallProgress {
                if let fraction = progress {
                    ProgressView(value: fraction).frame(width: 140)
                } else {
                    ProgressView().controlSize(.small)
                }
            } else if model.bun?.installed == false {
                Button("Download Bun") {
                    Task {
                        do { try await model.installBun() } catch { state.presentedError = UserFacingError(error) }
                    }
                }
            }
        }
    }
}
