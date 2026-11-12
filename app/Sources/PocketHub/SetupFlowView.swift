import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The guided "Add a 3DS" sheet. Every page has one obvious button.
struct SetupFlowView: View {
    @Bindable var flow: SetupFlowModel
    let done: (String?) -> Void
    @State private var choosingCard = false

    var body: some View {
        VStack(spacing: 0) {
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(28)
            Divider()
            footer
                .padding(16)
        }
        .frame(width: 520, height: 440)
        .fileImporter(isPresented: $choosingCard, allowedContentTypes: [.folder]) { result in
            if case .success(let url) = result { flow.installFromCard(at: url) }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch flow.step {
        case .instructions:
            Page(symbol: "gamecontroller", title: Text("Add a Nintendo 3DS")) {
                VStack(alignment: .leading, spacing: 10) {
                    Step(number: 1, text: Text("Turn on your 3DS and make sure it is on the same Wi-Fi network as this Mac."))
                    Step(number: 2, text: Text("Open the Homebrew Launcher and start **ftpd**."))
                    Step(number: 3, text: Text("Leave ftpd open, then click Continue."))
                }
            }
        case .searching:
            Page(symbol: "dot.radiowaves.left.and.right", title: Text("Looking for your 3DS…")) {
                ProgressView().controlSize(.large)
                Text("Keep ftpd open on the 3DS. This usually takes a few seconds.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
        case .found(let candidates):
            Page(symbol: "checkmark.circle", title: Self.single(candidates) != nil ? Text("Found your 3DS") : Text("Found \(candidates.count) consoles")) {
                if let only = Self.single(candidates) {
                    Text("Pocket Hub will copy what your 3DS needs and introduce it to this Mac. Keep ftpd open until it finishes.")
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.secondary)
                    Text(verbatim: only.address).font(.title3.monospaced())
                } else {
                    Text("Choose the one to set up. The address is shown at the top of ftpd on each console.")
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.secondary)
                    VStack(spacing: 8) {
                        ForEach(candidates) { candidate in
                            Button {
                                flow.install(candidate)
                            } label: {
                                HStack {
                                    VStack(alignment: .leading, spacing: 1) {
                                        Text(verbatim: candidate.address).monospaced()
                                        if candidate.ftpd {
                                            Text("A 3DS running ftpd").font(.caption).foregroundStyle(.secondary)
                                        } else {
                                            Text("Another device that shares files").font(.caption).foregroundStyle(.secondary)
                                        }
                                    }
                                    Spacer()
                                    Text("Set Up")
                                }
                                .frame(maxWidth: 280)
                            }
                            .controlSize(.large)
                        }
                    }
                }
            }
        case .manualAddress:
            Page(symbol: "keyboard", title: Text("Type the 3DS's Address")) {
                Text("ftpd shows it at the top of its screen, for example 192.168.1.57:5000.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                TextField("Address", text: $flow.manualAddress, prompt: Text(verbatim: "192.168.1.57:5000"))
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 240)
                    .onSubmit { if flow.manualAddressIsValid { flow.installFromManualAddress() } }
                    .accessibilityLabel(Text("The 3DS's address"))
            }
        case .installing(let progress):
            Page(symbol: "arrow.down.circle", title: Text("Setting Up Your 3DS")) {
                VStack(alignment: .leading, spacing: 6) {
                    ProgressView(value: progress.fraction)
                        .accessibilityLabel(Text("Setup progress"))
                    Text(progress.stage.displayName).foregroundStyle(.secondary)
                }
                .frame(width: 360)
                Text("Keep ftpd open and the 3DS nearby.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        case .waitingForDevice(let info):
            Page(symbol: "hand.point.up.left", title: Text("Almost Done")) {
                VStack(alignment: .leading, spacing: 10) {
                    switch info.method {
                    case .network:
                        Step(number: 1, text: Text("On your 3DS, exit ftpd."))
                        Step(number: 2, text: Text("Open **Pocket Shell** from the Homebrew Launcher."))
                    case .card:
                        Step(number: 1, text: Text("Eject the SD card and put it back in your 3DS."))
                        Step(number: 2, text: Text("Turn on the 3DS and open **Pocket Shell** from the Homebrew Launcher."))
                    }
                }
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Waiting for your 3DS to connect…").foregroundStyle(.secondary)
                }
                Text(verbatim: info.report.next)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
            }
        case .finished:
            Page(symbol: "checkmark.seal.fill", title: Text("Your 3DS Is Connected")) {
                Text("Whenever you open Pocket Shell on it and this Mac is on, it connects by itself.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
        case .failed(let failure):
            Page(symbol: "exclamationmark.triangle", title: Text(verbatim: failure.title), tint: .orange) {
                Text(verbatim: failure.advice)
                    .multilineTextAlignment(.center)
                if let detail = failure.hubMessage {
                    Text(verbatim: detail)
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .multilineTextAlignment(.center)
                        .textSelection(.enabled)
                }
            }
        }
    }

    @ViewBuilder
    private var footer: some View {
        HStack {
            switch flow.step {
            case .instructions:
                Button("Use the SD Card Instead…") { choosingCard = true }
                    .help("Set up with the 3DS's SD card in this Mac's card reader")
                Spacer()
                Button("Cancel", role: .cancel) { close() }.keyboardShortcut(.cancelAction)
                Button("Continue") { flow.startSearching() }.keyboardShortcut(.defaultAction)
            case .searching:
                Button("Type the Address Instead") { flow.enterAddressManually() }
                Spacer()
                Button("Cancel", role: .cancel) { close() }.keyboardShortcut(.cancelAction)
            case .found(let candidates):
                Button("Type the Address Instead") { flow.enterAddressManually() }
                Spacer()
                Button("Cancel", role: .cancel) { close() }.keyboardShortcut(.cancelAction)
                if let only = Self.single(candidates) {
                    Button("Set Up This 3DS") { flow.install(only) }.keyboardShortcut(.defaultAction)
                }
            case .manualAddress:
                Button("Back") { flow.startOver() }
                Spacer()
                Button("Cancel", role: .cancel) { close() }.keyboardShortcut(.cancelAction)
                Button("Set Up") { flow.installFromManualAddress() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!flow.manualAddressIsValid)
            case .installing:
                Spacer()
                Button("Stop", role: .cancel) { flow.cancel() }.keyboardShortcut(.cancelAction)
            case .waitingForDevice:
                Spacer()
                Button("Close") { close() }
                    .help("The 3DS will still appear in Pocket Hub when it connects")
            case .finished(let id):
                Spacer()
                Button("Done") { done(id) }.keyboardShortcut(.defaultAction)
            case .failed(let failure):
                Button("Start Over") { flow.startOver() }
                Spacer()
                Button("Cancel", role: .cancel) { close() }.keyboardShortcut(.cancelAction)
                Button(failure.recoveryTitle) { flow.retry() }.keyboardShortcut(.defaultAction)
            }
        }
    }

    private func close() {
        flow.cancel()
        done(nil)
    }

    /// The one console to offer with a single button: the only ftpd console
    /// found, or the only host found at all. Several hosts get a list, ftpd's first.
    static func single(_ candidates: [SetupCandidate]) -> SetupCandidate? {
        let ftpd = candidates.filter(\.ftpd)
        if ftpd.count == 1 { return ftpd[0] }
        return candidates.count == 1 ? candidates[0] : nil
    }
}

private struct Page<Content: View>: View {
    let symbol: String
    let title: Text
    var tint: Color = .accentColor
    @ViewBuilder let content: Content

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: symbol)
                .font(.system(size: 44))
                .foregroundStyle(tint)
                .accessibilityHidden(true)
            title.font(.title2.bold()).multilineTextAlignment(.center)
            content
        }
    }
}

private struct Step: View {
    let number: Int
    let text: Text

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(verbatim: "\(number)")
                .font(.callout.bold())
                .frame(width: 22, height: 22)
                .background(Circle().fill(.quaternary))
            text
        }
    }
}
