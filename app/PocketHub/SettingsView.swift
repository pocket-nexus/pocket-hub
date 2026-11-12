import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// Hub's name, the app's language, opening at login, and where Hub keeps its files.
struct SettingsView: View {
    @Environment(AppState.self) private var state
    @State private var name = ""
    @State private var saving = false

    var body: some View {
        let model = state.model
        let login = state.launchAtLogin
        let language = state.language
        Form {
            Section {
                LabeledContent("Name") {
                    HStack {
                        TextField("Name", text: $name, prompt: Text("This Mac's name for your devices"))
                            .labelsHidden()
                            .onSubmit(saveName)
                            .disabled(!model.isLive)
                        Button("Rename", action: saveName)
                            .disabled(!model.isLive || saving || name.trimmingCharacters(in: .whitespaces).isEmpty
                                      || name == model.hub?.name)
                    }
                }
                Text("Your devices show this name. They see a new name the next time they connect.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section {
                // Each language is named in itself, as macOS lists them, so
                // it can be found whatever language the app is in now.
                Picker("Language", selection: Binding(get: { language.choice }, set: { language.choose($0) })) {
                    Text("System Language").tag(AppLanguage.Choice.system)
                    Text(verbatim: "English").tag(AppLanguage.Choice.english)
                    Text(verbatim: "简体中文").tag(AppLanguage.Choice.simplifiedChinese)
                }
                if language.needsRestart {
                    HStack {
                        Text("Pocket Hub shows the new language after it reopens.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button("Reopen Now") { state.relaunch() }
                    }
                }
            }
            Section {
                Toggle("Open Pocket Hub at login", isOn: Binding(get: { login.isEnabled }, set: { login.setEnabled($0) }))
                    .disabled(login.state == .unavailable)
                switch login.state {
                case .requiresApproval:
                    HStack {
                        Text("Allow Pocket Hub in System Settings to finish.").font(.caption).foregroundStyle(.secondary)
                        Button("Open Login Items") { login.openLoginItemsSettings() }
                    }
                case .unavailable:
                    Text("Available when Pocket Hub runs from its app bundle.").font(.caption).foregroundStyle(.secondary)
                default:
                    EmptyView()
                }
                if let error = login.lastError {
                    Text(verbatim: error).font(.caption).foregroundStyle(.red)
                }
            }
            Section {
                LabeledContent("Files") {
                    if let home = model.status?.home {
                        HStack {
                            Text(verbatim: home)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .textSelection(.enabled)
                            Button("Show in Finder") {
                                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: home, isDirectory: true)])
                            }
                        }
                    } else {
                        Text("Unknown while Pocket Hub isn't running").foregroundStyle(.secondary)
                    }
                }
                Text("Pocket Hub keeps its paired devices, daemons and screenshots here. Set POCKET_HUB_HOME to use another folder.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if let status = model.status {
                    LabeledContent("Version") { Text(verbatim: status.version) }
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 500)
        .fixedSize(horizontal: false, vertical: true)
        .onAppear {
            name = model.hub?.name ?? ""
            login.refresh()
        }
        .onChange(of: model.hub?.name) { _, newName in
            if !saving, let newName { name = newName }
        }
    }

    private func saveName() {
        saving = true
        Task {
            defer { saving = false }
            do { try await state.model.setHubName(name) } catch { state.presentedError = UserFacingError(error) }
        }
    }
}
