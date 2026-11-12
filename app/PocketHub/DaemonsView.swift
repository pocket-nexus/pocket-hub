import AppKit
import HubAppModel
import HubClient
import SwiftUI
import UniformTypeIdentifiers

/// The installed daemons (each applet's computer side), their state and
/// output, installing one, and the Bun runtime they run on.
struct DaemonsView: View {
    @Environment(AppState.self) private var state
    @State private var selection: String?
    @State private var showingLog = false
    @State private var importing = false
    @State private var confirmingRemove: Daemon?

    private var model: HubModel { state.model }

    var body: some View {
        // The runtime, the install under way and the daemons are cards of one
        // grouped page. A row's output opens in the inspector, from the row's
        // own button or its context menu.
        Form {
            Section {
                BunRuntimeBar()
            }
            if let install = model.daemonInstall {
                Section {
                    HStack {
                        ProgressView().controlSize(.small)
                        Text("Installing \(install.source)…")
                        Text(verbatim: daemonInstallStageText(install.stage)).foregroundStyle(.secondary)
                        Spacer()
                    }
                }
            }
            Section("Daemons") {
                if let daemons = model.daemons {
                    if daemons.isEmpty {
                        VStack(spacing: 8) {
                            EmptySectionNote(
                                title: Text("No Daemons"),
                                detail: Text("Some applets come with a part that runs on this Mac. Install one from a folder or a .tar.gz file."))
                            Button("Install Daemon…") { importing = true }
                        }
                        .padding(.bottom, 8)
                    } else {
                        ForEach(daemons) { daemon in
                            DaemonRow(daemon: daemon, perform: perform) { showOutput(of: daemon.id) }
                                .contextMenu {
                                    Button("Start") { perform(.start, daemon.id) }.disabled(daemon.state == .running)
                                    Button("Stop") { perform(.stop, daemon.id) }.disabled(daemon.state == .stopped)
                                    Button("Restart") { perform(.restart, daemon.id) }
                                    Divider()
                                    Button("Show Output") { showOutput(of: daemon.id) }
                                    Divider()
                                    Button("Remove…", role: .destructive) { confirmingRemove = daemon }
                                }
                        }
                    }
                } else {
                    EmptySectionNote(title: Text("Daemons Unavailable"),
                                     detail: Text("Pocket Hub couldn't list its daemons."))
                }
            }
        }
        .formStyle(.grouped)
        .navigationTitle("Daemons")
        .toolbar {
            ToolbarItemGroup {
                Button("Install Daemon…", systemImage: "plus") { importing = true }
                    .disabled(model.daemonInstall != nil)
                    .help("Install a daemon from a folder or a .tar.gz file")
                Button("Show Output", systemImage: "sidebar.right") { showingLog.toggle() }
                    .disabled(selection == nil)
                    .help("Show the selected daemon's output")
            }
        }
        .inspector(isPresented: $showingLog) {
            if let id = selection, let daemon = model.daemons?.first(where: { $0.id == id }) {
                DaemonLogView(daemon: daemon)
                    .inspectorColumnWidth(min: 280, ideal: 380, max: 700)
                    .navigationTitle(daemon.id)
            } else {
                ContentUnavailableView("No Daemon Selected", systemImage: "gearshape.2")
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
            Text("Its applet keeps working on the console but can no longer reach this Mac. Remove keeps the daemon's own data for a later install.")
        }
    }

    private func showOutput(of id: String) {
        selection = id
        showingLog = true
    }

    private func perform(_ action: DaemonAction, _ id: String) {
        Task {
            do { try await model.perform(action, daemon: id) } catch { state.presentedError = UserFacingError(error) }
        }
    }

    private func install(_ url: URL) {
        Task {
            let access = url.startAccessingSecurityScopedResource()
            defer { if access { url.stopAccessingSecurityScopedResource() } }
            do { try await model.installDaemon(from: url) } catch { state.presentedError = UserFacingError(error) }
        }
    }

    private func remove(_ id: String, purge: Bool) {
        Task {
            do { try await model.removeDaemon(id, purge: purge) } catch { state.presentedError = UserFacingError(error) }
        }
    }
}

struct DaemonRow: View {
    let daemon: Daemon
    let perform: (DaemonAction, String) -> Void
    let showOutput: () -> Void

    var body: some View {
        HStack {
            Circle()
                .fill(color)
                .frame(width: 9, height: 9)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: daemon.id)
                HStack(spacing: 6) {
                    Text(daemon.state.displayName)
                    Text(verbatim: daemon.version)
                    if let pid = daemon.pid { Text("PID \(String(pid))") }
                    if daemon.channels > 0 { Text("\(daemon.channels) channels open") }
                    if let since = daemon.since { Text(since.date.formatted(.relative(presentation: .named))) }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if let error = daemon.error {
                    Text(verbatim: error).font(.caption).foregroundStyle(.red)
                }
            }
            Spacer()
            switch daemon.state {
            case .running:
                Button("Restart", systemImage: "arrow.clockwise") { perform(.restart, daemon.id) }
                    .labelStyle(.iconOnly).help("Restart")
                Button("Stop", systemImage: "stop.fill") { perform(.stop, daemon.id) }
                    .labelStyle(.iconOnly).help("Stop")
            case .starting, .stopping:
                ProgressView().controlSize(.small)
            default:
                Button("Start", systemImage: "play.fill") { perform(.start, daemon.id) }
                    .labelStyle(.iconOnly).help("Start")
            }
            Button("Show Output", systemImage: "text.alignleft", action: showOutput)
                .labelStyle(.iconOnly).help("Show Output")
        }
        .buttonStyle(.borderless)
        .padding(.vertical, 2)
        .accessibilityElement(children: .contain)
    }

    private var color: Color {
        switch daemon.state {
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
                        Text("Not installed. Daemons need it to run.").foregroundStyle(.secondary)
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
