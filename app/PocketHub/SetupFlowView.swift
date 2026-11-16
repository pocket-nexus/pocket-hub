import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// The guided "Add a Device" sheet: a 3DS set up over its storage, or a
/// device paired by the code it shows. Every page has one obvious button.
struct SetupFlowView: View {
    @Bindable var flow: SetupFlowModel
    /// This Mac's name as devices list it, for "choose … on the device".
    let macName: String
    let done: (String?) -> Void
    @State private var choosingCard = false
    @FocusState private var codeFocused: Bool

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
        case .chooseDevice:
            Page(symbol: "plus.circle", title: Text("Add a Device")) {
                Text("Choose the kind of device to add.")
                    .foregroundStyle(.secondary)
                VStack(spacing: 8) {
                    KindButton(kind: .nintendo3ds, detail: Text("Set up over Wi-Fi, or with its SD card")) { flow.choose(.nintendo3ds) }
                    KindButton(kind: .tab5, detail: Text("Pair with a code it shows")) { flow.choose(.tab5) }
                }
            }
        case .pairWaiting:
            Page(symbol: DeviceKind.tab5.symbolName(filled: true), title: Text("Add an M5Stack Tab5")) {
                VStack(alignment: .leading, spacing: 10) {
                    Step(number: 1, text: Text("Turn on your Tab5 and make sure it is on the same Wi-Fi network as this Mac."))
                    Step(number: 2, text: Text("On the Tab5, open **Settings**, then **Mac**, and choose **Pair a Computer…**"))
                    if macName.isEmpty {
                        Step(number: 3, text: Text("Choose this Mac in the list. The Tab5 then shows a code."))
                    } else {
                        Step(number: 3, text: Text("Choose **\(macName)** in the list. The Tab5 then shows a code."))
                    }
                }
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Waiting for your Tab5…").foregroundStyle(.secondary)
                }
            }
        case .pairCode(let entry):
            Page(symbol: "123.rectangle", title: Text("Enter the Code on \(entry.request.displayName)")) {
                VStack(spacing: 4) {
                    Text("It asks to connect to this Mac. Type the 6-digit code it shows on its screen.")
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.secondary)
                    // What the request says of itself, to tell two devices apart.
                    Text(verbatim: [entry.request.model, entry.request.address].filter { !$0.isEmpty }.joined(separator: " · "))
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
                TextField("Code", text: $flow.pairCode, prompt: Text(verbatim: "000000"))
                    .textFieldStyle(.roundedBorder)
                    .font(.title2.monospacedDigit())
                    .multilineTextAlignment(.center)
                    .frame(width: 150)
                    .focused($codeFocused)
                    .disabled(entry.isAnswering)
                    .onSubmit { flow.submitPairCode() }
                    .onChange(of: flow.pairCode) { _, typed in
                        // Digits alone, six of them: what a device shows.
                        let digits = String(typed.filter { $0.isASCII && $0.isNumber }.prefix(6))
                        if digits != typed { flow.pairCode = digits }
                    }
                    .accessibilityLabel(Text("The code on the device"))
                    .onAppear { codeFocused = true }
                    .onChange(of: entry.isAnswering) { _, answering in
                        if !answering { codeFocused = true }
                    }
                PairStatus(entry: entry)
            }
        case .paired(let device):
            Page(symbol: "checkmark.seal.fill", title: Text("\(device.displayName) Is Paired")) {
                Text("Whenever it is on and this Mac is on, it connects by itself.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
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
            case .chooseDevice:
                Spacer()
                Button("Cancel", role: .cancel) { close() }.keyboardShortcut(.cancelAction)
            case .pairWaiting:
                Button("Back") { flow.chooseAgain() }
                Spacer()
                Button("Cancel", role: .cancel) { close() }.keyboardShortcut(.cancelAction)
            case .pairCode:
                Spacer()
                // Cancel tells the device, which then stops showing its code.
                Button("Cancel", role: .cancel) { close() }.keyboardShortcut(.cancelAction)
                Button("Pair") { flow.submitPairCode() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!flow.canSubmitPairCode)
            case .paired(let device):
                Spacer()
                Button("Done") { done(device.id) }.keyboardShortcut(.defaultAction)
            case .instructions:
                Button("Back") { flow.chooseAgain() }
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
        flow.declinePairRequest()
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

/// One kind of device on the first page: its symbol, its name and how it is
/// added, as one button.
private struct KindButton: View {
    let kind: DeviceKind
    let detail: Text
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                Image(systemName: kind.symbolName(filled: true))
                    .font(.title2)
                    .frame(width: 34)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 1) {
                    Text(verbatim: kind.displayName)
                    detail.font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Image(systemName: "chevron.right")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .accessibilityHidden(true)
            }
            .frame(width: 300)
            .padding(.vertical, 4)
        }
        .controlSize(.large)
    }
}

/// Under the code's field: that the code is on its way, or why the last one
/// did not pair the device and whether it can be typed again.
private struct PairStatus: View {
    let entry: SetupFlowModel.PairEntry

    var body: some View {
        VStack(spacing: 6) {
            if entry.isAnswering {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Pairing…").foregroundStyle(.secondary)
                }
            } else {
                switch entry.problem {
                case .wrongCode?:
                    Label("That isn't the code on the device. Check it and type it again.", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                case .interrupted?:
                    Label("The device stopped before the pairing finished.", systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                case nil:
                    EmptyView()
                }
                if !entry.isAsking {
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text("Waiting for the device to ask again… If it no longer shows a code, choose this Mac on it again.")
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
        .font(.callout)
        .multilineTextAlignment(.center)
        // Room for both lines at once, so the page does not move when they come and go.
        .frame(height: 76, alignment: .top)
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
