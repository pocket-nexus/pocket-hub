import Foundation
import HubClient
import Observation

/// Adding a device: the guided first install of a 3DS (CONTROL-API.md §3
/// "Setup"), or the pairing of a device that shows a code, an M5Stack Tab5
/// (§3 "Pairing").
///
///     chooseDevice ─▶ instructions ─▶ searching ─▶ found ─▶ installing ─▶ waitingForDevice ─▶ finished
///          │                              │                    ▲  │
///          │                              └─▶ manualAddress ───┘  └─▶ failed ─▶ (retry)
///          │
///          └─▶ pairWaiting ─▶ pairCode ─▶ paired
///                                ▲ │
///                                └─┘ a wrong code; the device asks again
///
/// **A 3DS**: the owner opens ftpd on the console, Hub finds it
/// (`setup.discover`, or an address typed by hand), one button runs
/// `setup.install` with its progress, and the flow then waits for the console
/// to link: it finishes by itself on the `device.changed` that carries a
/// `link` for the installed device.
///
/// **A device that shows a code**: its owner opens a pairing on it and
/// chooses this Mac there; the request reaches Hub (`pairing.changed`), the
/// owner types the 6 digits the device shows, and `pairing.answer` pairs it.
/// A request that arrives while no flow is open starts one at `pairCode`
/// (`HubModel.makeSetupFlow(answering:)`).
@MainActor
@Observable
public final class SetupFlowModel {
    public enum Step: Equatable, Sendable {
        /// Which kind of device to add.
        case chooseDevice
        /// "Open ftpd on your 3DS."
        case instructions
        /// Looking for consoles running ftpd; repeats until one answers.
        case searching
        /// Consoles found; one button sets the chosen one up.
        case found([SetupCandidate])
        /// The owner types the console's address.
        case manualAddress
        case installing(InstallProgress)
        /// Installed; the owner exits ftpd and opens Pocket Shell, and the console links.
        case waitingForDevice(WaitingInfo)
        /// The console linked.
        case finished(deviceId: String)
        /// What to do on a device that shows a code; the flow waits for its request.
        case pairWaiting
        /// A device asks to be paired: its code is typed here.
        case pairCode(PairEntry)
        /// The device is paired; it links by itself.
        case paired(Device)
        case failed(SetupFailure)
    }

    /// The page on which a device's code is typed.
    public struct PairEntry: Equatable, Sendable {
        /// The request being answered: the latest of that device id.
        public var request: PairRequest
        /// The request waits for a code now. False while a code is on its
        /// way, and after a code that did not pair until the device asks again.
        public var isAsking: Bool
        /// `pairing.answer` is out.
        public var isAnswering = false
        /// Why the last code did not pair the device.
        public var problem: PairProblem?
    }

    public enum PairProblem: Equatable, Sendable {
        /// The code is not the one the device shows.
        case wrongCode
        /// The device left, or its request ended, before the pairing finished.
        case interrupted
    }

    public struct InstallProgress: Equatable, Sendable {
        public var stage: SetupStage
        /// Overall progress across the stages, 0...1.
        public var fraction: Double
        /// Hub's own words for the step (not localized), for a detail line.
        public var hubMessage: String?
    }

    public struct WaitingInfo: Equatable, Sendable {
        public var report: InstallReport
        public var method: Method
    }

    public enum Method: Equatable, Sendable {
        case network(host: String, port: Int?)
        case card(path: String)
    }

    public private(set) var step: Step = .chooseDevice
    /// The text field of `manualAddress`: `192.168.1.57` or `192.168.1.57:5000`.
    public var manualAddress = ""
    /// The text field of `pairCode`: the digits the device shows.
    public var pairCode = ""
    /// The method of the last install, kept so "Try Again" repeats it.
    public private(set) var lastMethod: Method?

    private let client: any HubCalling
    private weak var model: HubModel?
    private var work: Task<Void, Never>?
    /// How long discovery waits between empty answers.
    public var discoveryInterval: Duration = .seconds(2)

    init(client: any HubCalling, model: HubModel?) {
        self.client = client
        self.model = model
    }

    // MARK: - Owner actions

    /// The owner chose the kind of device to add: a 3DS is set up over its
    /// storage, every other kind pairs by the code it shows. A device that
    /// already asks is answered at once.
    public func choose(_ kind: DeviceKind) {
        work?.cancel()
        if kind == .nintendo3ds {
            step = .instructions
        } else if let request = model?.pairRequests.first {
            answer(request)
        } else {
            step = .pairWaiting
        }
    }

    /// Back to the choice of the kind of device.
    public func chooseAgain() {
        cancel()
        step = .chooseDevice
    }

    /// "My 3DS is ready": starts looking for it.
    public func startSearching() {
        work?.cancel()
        step = .searching
        work = Task { [weak self] in await self?.discover() }
    }

    public func enterAddressManually() {
        work?.cancel()
        step = .manualAddress
    }

    /// Whether `manualAddress` holds something `setup.install` can use.
    public var manualAddressIsValid: Bool { Self.parse(manualAddress) != nil }

    public func installFromManualAddress() {
        guard let (host, port) = Self.parse(manualAddress) else { return }
        install(.network(host: host, port: port))
    }

    public func install(_ candidate: SetupCandidate) {
        install(.network(host: candidate.address, port: candidate.port))
    }

    /// The alternative to ftpd: the console's SD card mounted on this Mac.
    public func installFromCard(at url: URL) {
        install(.card(path: url.path(percentEncoded: false)))
    }

    /// The one next action of a failure.
    public func retry() {
        guard case .failed(let failure) = step else { return }
        switch failure.recovery {
        case .searchAgain: startSearching()
        case .installAgain:
            if let lastMethod { install(lastMethod) } else { startSearching() }
        case .startOver: step = .instructions
        case .pairAgain: choose(.tab5)
        }
    }

    // MARK: - Pairing by a code

    /// Starts at the code of `request`: a device asked.
    func answer(_ request: PairRequest) {
        work?.cancel()
        pairCode = ""
        step = .pairCode(PairEntry(request: request, isAsking: true))
    }

    /// The 6 digits of `pairCode`, when it holds exactly that: spaces the
    /// owner typed between them are dropped, and digits of another script
    /// are not a code (the device keys its proof with ASCII digits).
    public var pairCodeDigits: String? {
        let digits = pairCode.filter { !$0.isWhitespace }
        guard digits.utf8.count == 6, digits.utf8.allSatisfy({ (48...57).contains($0) }) else { return nil }
        return digits
    }

    /// Whether the Pair button can send the code now.
    public var canSubmitPairCode: Bool {
        guard case .pairCode(let entry) = step else { return false }
        return entry.isAsking && !entry.isAnswering && pairCodeDigits != nil
    }

    /// Sends the code to the device (`pairing.answer`).
    public func submitPairCode() {
        guard canSubmitPairCode, case .pairCode(var entry) = step, let code = pairCodeDigits else { return }
        entry.isAnswering = true
        entry.problem = nil
        step = .pairCode(entry)
        let device = entry.request.device
        work?.cancel()
        work = Task { [weak self, client] in
            do {
                let paired = try await client.answerPairing(device: device, code: code)
                self?.didPair(paired)
            } catch {
                self?.pairingFailed(error, device: device)
            }
        }
    }

    /// The owner closes the sheet on a request that still waits: the device
    /// is told (`pairing.decline`), so it does not go on showing a code that
    /// nobody will enter.
    public func declinePairRequest() {
        guard case .pairCode(let entry) = step, entry.isAsking, !entry.isAnswering else { return }
        let device = entry.request.device
        Task { [client] in try? await client.declinePairing(device: device) }
    }

    private func didPair(_ device: Device) {
        pairCode = ""
        step = .paired(device)
    }

    /// A code that did not pair keeps the page: the device asks again while
    /// it has tries left, and `pairRequestsDidChange` then lets the owner
    /// type another. Anything else is a failure of its own.
    private func pairingFailed(_ error: any Error, device: String) {
        guard case .pairCode(var entry) = step, entry.request.device == device else { return }
        entry.isAnswering = false
        pairCode = ""
        guard let hubError = error as? HubError, case .operation(let operation) = hubError else {
            step = .failed(SetupFailure(pairingError: error))
            return
        }
        switch operation.kind {
        case .cancelled:
            break
        case .pairing where operation.reason == "wrong_code", .invalid:
            entry.problem = .wrongCode
        case .pairing, .notFound:
            // `gone`, `bad_proof`, or a request that ended before the code reached it.
            entry.problem = .interrupted
        default:
            step = .failed(SetupFailure(pairingError: error))
            return
        }
        step = .pairCode(entry)
    }

    /// Stops what the flow is doing. An install in progress is cancelled on Hub.
    public func cancel() {
        work?.cancel()
        work = nil
    }

    /// Back to the first page (from `found`, `manualAddress` or a failure).
    public func startOver() {
        cancel()
        step = .instructions
    }

    // MARK: - From the model

    /// Called by `HubModel` with the requests that wait for a code, each time
    /// they change.
    func pairRequestsDidChange(_ requests: [PairRequest]) {
        switch step {
        case .pairWaiting:
            if let first = requests.first { answer(first) }
        case .pairCode(var entry):
            if let again = requests.first(where: { $0.device == entry.request.device }) {
                // The device asks (again, after a code that did not pair).
                entry.request = again
                entry.isAsking = true
                step = .pairCode(entry)
            } else if entry.isAnswering {
                // The code is on its way: Hub lists no request meanwhile.
                entry.isAsking = false
                step = .pairCode(entry)
            } else if entry.isAsking {
                // The device left, or its request ran out: nothing to answer.
                pairCode = ""
                step = .pairWaiting
            }
        default:
            break
        }
    }

    /// Called by `HubModel` for every Device it learns about.
    func deviceDidChange(_ device: Device) {
        guard device.isLinked else { return }
        switch step {
        case .waitingForDevice(let info) where info.report.device == device.id:
            step = .finished(deviceId: device.id)
        case .failed(let failure) where failure.unconfirmedDevice == device.id:
            // The key whose readback did not return was in place after all:
            // the console linked with it, which settles the setup.
            step = .finished(deviceId: device.id)
        default:
            break
        }
    }

    // MARK: - Work

    private func discover() async {
        while !Task.isCancelled {
            do {
                let candidates = try await client.discover(kind: "3ds")
                if Task.isCancelled { return }
                if !candidates.isEmpty {
                    step = .found(candidates)
                    return
                }
            } catch {
                if Task.isCancelled { return }
                step = .failed(SetupFailure(discoveryError: error))
                return
            }
            try? await Task.sleep(for: discoveryInterval)
        }
    }

    private func install(_ method: Method) {
        work?.cancel()
        lastMethod = method
        step = .installing(InstallProgress(stage: .connecting, fraction: 0, hubMessage: nil))
        let progress: @Sendable (SetupProgress) -> Void = { [weak self] update in
            Task { @MainActor in self?.progressed(update) }
        }
        work = Task { [weak self, client] in
            do {
                let report: InstallReport = switch method {
                case .network(let host, let port):
                    try await client.install(kind: "3ds", host: host, port: port, progress: progress)
                case .card(let path):
                    try await client.installFromCard(kind: "3ds", path: path, progress: progress)
                }
                self?.installed(report, method: method)
            } catch {
                self?.installFailed(error)
            }
        }
    }

    private func progressed(_ update: SetupProgress) {
        guard case .installing = step else { return }
        step = .installing(InstallProgress(stage: update.stage, fraction: Self.overall(update), hubMessage: update.message))
    }

    private func installed(_ report: InstallReport, method: Method) {
        if let device = model?.device(report.device), device.isLinked {
            // Already linked (a console that was set up before and is running Pocket Shell).
            step = .finished(deviceId: device.id)
        } else {
            step = .waitingForDevice(WaitingInfo(report: report, method: method))
        }
    }

    private func installFailed(_ error: any Error) {
        if let hubError = error as? HubError, hubError.isCancelled {
            // The owner cancelled: back to where the console was chosen.
            step = .instructions
            return
        }
        step = .failed(SetupFailure(installError: error))
    }

    /// Weights of the stages in the overall bar; the transfers dominate.
    static let stageWeights: [(SetupStage, Double)] = [
        (.connecting, 0.05), (.reading, 0.05), (.downloading, 0.4), (.uploading, 0.4), (.pairing, 0.1),
    ]

    static func overall(_ update: SetupProgress) -> Double {
        if update.stage == .done { return 1 }
        var base = 0.0
        for (stage, weight) in stageWeights {
            if stage == update.stage { return base + weight * (update.fraction ?? 0) }
            base += weight
        }
        return base
    }

    /// `host` or `host:port`; the host is an IPv4 address or a name.
    static func parse(_ text: String) -> (host: String, port: Int?)? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !trimmed.contains(" ") else { return nil }
        let parts = trimmed.split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count <= 2, let host = parts.first, !host.isEmpty else { return nil }
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: ".-"))
        guard host.unicodeScalars.allSatisfy(allowed.contains) else { return nil }
        if parts.count == 2 {
            guard let port = Int(parts[1]), (1...65535).contains(port) else { return nil }
            return (String(host), port)
        }
        return (String(host), nil)
    }
}

/// A setup failure in plain words with its one next action.
public struct SetupFailure: Equatable, Sendable {
    public enum Recovery: Equatable, Sendable {
        /// Look for the console again (it was not reachable).
        case searchAgain
        /// Run the same install again.
        case installAgain
        /// Go back to the first page.
        case startOver
        /// Wait for the device to ask again (a pairing by a code).
        case pairAgain
    }

    /// The stage that failed, when Hub said.
    public var stage: SetupStage?
    public var title: String
    public var advice: String
    public var recovery: Recovery
    /// Hub's sentence, for a detail line.
    public var hubMessage: String?
    /// A key was stored for this device and its final readback did not return:
    /// the device is listed with `setupUnconfirmed`, and the flow still
    /// finishes if it links.
    public var unconfirmedDevice: String?

    public var recoveryTitle: String {
        switch recovery {
        case .searchAgain, .installAgain, .pairAgain: String(localized: "Try Again")
        case .startOver: String(localized: "Start Over")
        }
    }

    init(stage: SetupStage?, title: String, advice: String, recovery: Recovery, hubMessage: String?,
         unconfirmedDevice: String? = nil)
    {
        self.stage = stage
        self.title = title
        self.advice = advice
        self.recovery = recovery
        self.hubMessage = hubMessage
        self.unconfirmedDevice = unconfirmedDevice
    }

    init(discoveryError error: any Error) {
        let hubMessage = (error as? HubError).flatMap { if case .operation(let e) = $0 { e.message } else { nil } }
        self.init(stage: nil,
                  title: String(localized: "Pocket Hub couldn't look for your 3DS."),
                  advice: String(localized: "Check that this Mac is connected to Wi-Fi, then try again. You can also type the 3DS's address."),
                  recovery: .searchAgain, hubMessage: hubMessage)
    }

    /// A failure of `pairing.answer` that is not the code's: Hub went away,
    /// or could not store the device.
    init(pairingError error: any Error) {
        let hubError = error as? HubError
        let message: String? = if case .operation(let operation)? = hubError { operation.message } else { nil }
        switch hubError {
        case .notConnected?, .disconnected?:
            self.init(stage: nil, title: String(localized: "Pocket Hub stopped responding during the pairing."),
                      advice: String(localized: "Wait a moment for it to restart, then choose this Mac on the device again."),
                      recovery: .pairAgain, hubMessage: nil)
        default:
            self.init(stage: nil, title: String(localized: "The pairing didn't finish."),
                      advice: String(localized: "Choose this Mac on the device again, then enter the code it shows."),
                      recovery: .pairAgain, hubMessage: message ?? (hubError == nil ? error.localizedDescription : nil))
        }
    }

    init(installError error: any Error) {
        guard let hubError = error as? HubError else {
            self.init(stage: nil, title: String(localized: "The setup didn't finish."),
                      advice: String(localized: "Try again."), recovery: .installAgain, hubMessage: error.localizedDescription)
            return
        }
        switch hubError {
        case .operation(let operation) where operation.kind == .setup:
            self.init(setup: operation)
        case .operation(let operation) where operation.kind == .invalid:
            self.init(stage: nil, title: String(localized: "That address doesn't look right."),
                      advice: String(localized: "Check the address shown in ftpd on the 3DS and type it again."),
                      recovery: .startOver, hubMessage: operation.message)
        case .operation(let operation) where operation.kind == .notFound:
            self.init(stage: nil, title: String(localized: "Pocket Hub can't find the SD card."),
                      advice: String(localized: "Insert the 3DS's SD card in this Mac and choose it again."),
                      recovery: .startOver, hubMessage: operation.message)
        case .notConnected, .disconnected:
            self.init(stage: nil, title: String(localized: "Pocket Hub stopped responding during the setup."),
                      advice: String(localized: "Wait a moment for it to restart, then try again."),
                      recovery: .installAgain, hubMessage: nil)
        default:
            let message: String? = if case .operation(let e) = hubError { e.message } else { nil }
            self.init(stage: nil, title: String(localized: "The setup didn't finish."),
                      advice: String(localized: "Try again. If it keeps happening, restart Pocket Hub."),
                      recovery: .installAgain, hubMessage: message)
        }
    }

    /// The error `setup`, worded from `reason` first (the cause), then `step`
    /// (where it happened). Hub's English sentence stays as the detail.
    init(setup operation: HubOperationError) {
        let stage = operation.step.map(SetupStage.init(rawValue:))
        let message = operation.message.isEmpty ? nil : operation.message
        if operation.confirmed == false, let device = operation.device {
            self.init(stage: stage, title: String(localized: "Pocket Hub couldn't double-check the last step."),
                      advice: String(localized: "Exit ftpd and open Pocket Shell on the 3DS. If it doesn't connect within a minute, run the setup again."),
                      recovery: .installAgain, hubMessage: message, unconfirmedDevice: device)
            return
        }
        switch operation.reason {
        case "unreachable":
            self.init(stage: stage, title: String(localized: "Pocket Hub couldn't reach your 3DS."),
                      advice: String(localized: "Make sure ftpd is open on the 3DS and that it is on the same Wi-Fi network as this Mac."),
                      recovery: .searchAgain, hubMessage: message)
        case "companions_full":
            self.init(stage: stage, title: String(localized: "Your 3DS already knows 8 computers."),
                      advice: String(localized: "Open Pocket Shell on the 3DS, remove a computer you no longer use, then set this one up again."),
                      recovery: .startOver, hubMessage: message)
        case "release_unavailable":
            self.init(stage: stage, title: String(localized: "Pocket Hub couldn't download what your 3DS needs."),
                      advice: String(localized: "Check this Mac's internet connection, then try again."),
                      recovery: .installAgain, hubMessage: message)
        case "storage":
            self.init(stage: stage, title: String(localized: "Pocket Hub couldn't read or write your 3DS's SD card."),
                      advice: String(localized: "Make sure the SD card is in the 3DS and ftpd is still open, then try again."),
                      recovery: .installAgain, hubMessage: message)
        case "card":
            self.init(stage: stage, title: String(localized: "That folder isn't your 3DS's SD card."),
                      advice: String(localized: "Choose the SD card itself (the one that holds the 3ds folder)."),
                      recovery: .startOver, hubMessage: message)
        default:
            switch stage {
            case .connecting?:
                self.init(stage: stage, title: String(localized: "Pocket Hub couldn't reach your 3DS."),
                          advice: String(localized: "Make sure ftpd is open on the 3DS and that it is on the same Wi-Fi network as this Mac."),
                          recovery: .searchAgain, hubMessage: message)
            case .reading?:
                self.init(stage: stage, title: String(localized: "Pocket Hub couldn't read your 3DS's SD card."),
                          advice: String(localized: "Make sure the SD card is in the 3DS and ftpd is still open, then try again."),
                          recovery: .installAgain, hubMessage: message)
            case .downloading?:
                self.init(stage: stage, title: String(localized: "The download didn't finish."),
                          advice: String(localized: "Check this Mac's internet connection, then try again."),
                          recovery: .installAgain, hubMessage: message)
            case .uploading?:
                self.init(stage: stage, title: String(localized: "Copying to your 3DS stopped."),
                          advice: String(localized: "Keep ftpd open and the 3DS close to your Wi-Fi router, then try again."),
                          recovery: .installAgain, hubMessage: message)
            case .pairing?:
                self.init(stage: stage, title: String(localized: "Pocket Hub couldn't finish setting up your 3DS."),
                          advice: String(localized: "Keep ftpd open on the 3DS and try again. Nothing on the 3DS is lost."),
                          recovery: .installAgain, hubMessage: message)
            default:
                self.init(stage: stage, title: String(localized: "The setup didn't finish."),
                          advice: String(localized: "Keep ftpd open on the 3DS and try again."),
                          recovery: .installAgain, hubMessage: message)
            }
        }
    }
}
