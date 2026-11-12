import Foundation
import HubClient
import os

/// One update of a device's own software (`devices.installProgram` with the
/// latest release), from the owner's click to the device linked again on the
/// new version.
///
/// Hub answers the call when the device has taken the program and is about
/// to restart (`phase: launching`); the restart itself is seen only in
/// events: `device.unlinked` with `restart`, the device held with
/// `busy.launching` while it is away, then a `device.changed` linked and no
/// longer held, then (with its apps report) `program.version`. The update is
/// finished only when that version is the one installed.
public struct ProgramUpdate: Equatable, Sendable {
    public enum Stage: Equatable, Sendable {
        /// The call is sent; no progress yet.
        case preparing
        /// Hub downloads the release.
        case downloading(fraction: Double?)
        /// Hub sends it to the device (`sending`, then the device's own phases).
        case sending(fraction: Double?)
        /// The device took it and restarts; it links again by itself.
        case restarting
        /// Linked again; waiting for it to report its version.
        case reconnected
        /// Linked again on the new version (nil when the device reports none).
        case finished(version: String?)
        case failed(UserFacingError)
    }

    public var stage: Stage
    /// The version being installed: the one Hub offered, then the one it answered.
    public var version: String?
    /// The version the device had when the update began.
    public var previous: String?

    /// Still running: no other update may start, and the page shows progress.
    public var isActive: Bool {
        switch stage {
        case .finished, .failed: false
        default: true
        }
    }

    /// The progress bar's value: nil while indeterminate.
    public var fraction: Double? {
        switch stage {
        case .downloading(let fraction), .sending(let fraction): fraction
        default: nil
        }
    }
}

/// How long the app waits for a restarting device. Hub holds a device that
/// restarts into a program for up to 60 s (`busy.launching`); the app gives
/// it a little longer before it says the device has not come back.
public struct ProgramUpdateTimeouts: Sendable {
    /// From Hub's answer to the device linked again.
    public var reconnect: Duration
    /// From the device linked again to its report of the new version.
    public var versionReport: Duration
    /// After Hub stops holding a device that is not linked: the time its link
    /// event may still take to arrive.
    public var graceAfterHold: Duration

    public init(reconnect: Duration = .seconds(75), versionReport: Duration = .seconds(20),
                graceAfterHold: Duration = .seconds(5))
    {
        self.reconnect = reconnect
        self.versionReport = versionReport
        self.graceAfterHold = graceAfterHold
    }
}

/// What the model tracks beside the visible state, per device.
struct ProgramUpdateTracking {
    var call: Task<Void, Never>?
    var timer: Task<Void, Never>?
    /// The link went away after the update began (the restart, or a drop).
    var sawUnlink = false
    /// Hub held the device while it restarted.
    var sawLaunchHold = false
    /// Hub released the device while it was away; the grace timer runs.
    var inGrace = false
}

extension HubModel {
    static let updateLog = Logger(subsystem: "nexus.pocket.hub", category: "program-update")

    /// Updates the device's software to the latest release Hub offers. The
    /// state lands in `programUpdates[deviceId]`; a failure there is in plain words.
    public func updateProgram(on deviceId: String) {
        guard let device = device(deviceId), programUpdates[deviceId]?.isActive != true else { return }
        programUpdates[deviceId] = ProgramUpdate(stage: .preparing, version: device.program.latest,
                                                 previous: device.program.version)
        programTracking[deviceId]?.timer?.cancel()
        programTracking[deviceId] = ProgramUpdateTracking()
        let name = device.displayName
        Self.updateLog.notice("update \(deviceId, privacy: .public): from \(device.program.version ?? "-", privacy: .public) to \(device.program.latest ?? "-", privacy: .public)")
        let progress: @Sendable (TransferProgress) -> Void = { [weak self] update in
            Task { @MainActor in self?.programProgress(deviceId, update) }
        }
        programTracking[deviceId]?.call = Task { [weak self, client] in
            do {
                // The latest release, launched: Hub's defaults.
                let result = try await client.installProgram(device: deviceId, progress: progress)
                self?.programAnswered(deviceId, result)
            } catch {
                self?.programCallFailed(deviceId, error, deviceName: name)
            }
        }
    }

    /// Clears a finished or failed update from the page.
    public func dismissProgramUpdate(of deviceId: String) {
        guard programUpdates[deviceId]?.isActive == false else { return }
        programUpdates[deviceId] = nil
        programTracking[deviceId] = nil
    }

    // MARK: - The call

    private func programProgress(_ id: String, _ progress: TransferProgress) {
        guard let update = programUpdates[id] else { return }
        // A notification that arrives after the answer must not move the stage back.
        switch update.stage {
        case .preparing, .downloading, .sending: break
        default: return
        }
        programUpdates[id]?.stage = progress.phase == "downloading"
            ? .downloading(fraction: progress.fraction)
            : .sending(fraction: progress.fraction)
    }

    private func programAnswered(_ id: String, _ result: InstallProgramResult) {
        guard programUpdates[id]?.isActive == true else { return }
        Self.updateLog.notice("update \(id, privacy: .public): Hub answered \(result.phase, privacy: .public) \(result.version ?? "-", privacy: .public)")
        programTracking[id]?.call = nil
        if let version = result.version { programUpdates[id]?.version = version }
        guard result.phase == "launching" else {
            // Not restarted (Hub was told not to launch it): the program is on
            // the device and runs from its next start.
            finishProgramUpdate(id, .finished(version: programUpdates[id]?.version))
            return
        }
        programUpdates[id]?.stage = .restarting
        startProgramTimer(id, after: programUpdateTimeouts.reconnect)
        // The restart may already have been seen: the answer and the events
        // reach the model on different paths.
        if let device = device(id) { programDeviceDidChange(device, fromHub: false) }
    }

    private func programCallFailed(_ id: String, _ error: any Error, deviceName: String) {
        guard programUpdates[id] != nil else { return }
        Self.updateLog.notice("update \(id, privacy: .public) failed: \(String(describing: error), privacy: .public)")
        programTracking[id]?.call = nil
        if let failure = UserFacingError(programUpdateError: error, deviceName: deviceName) {
            finishProgramUpdate(id, .failed(failure))
        } else {
            // Cancelled: nothing to say.
            programUpdates[id] = nil
            programTracking[id] = nil
        }
    }

    // MARK: - The restart, from events

    /// Called with every new state of a device. `fromHub` is false for the
    /// model's own patch after `device.unlinked`, which clears `busy` without
    /// Hub having said so.
    func programDeviceDidChange(_ device: Device, fromHub: Bool = true) {
        guard let update = programUpdates[device.id], update.isActive else { return }
        let id = device.id
        if device.busy?.launching == true { programTracking[id]?.sawLaunchHold = true }
        if !device.isLinked { programTracking[id]?.sawUnlink = true }
        let tracking = programTracking[id] ?? ProgramUpdateTracking()
        switch update.stage {
        case .restarting:
            if device.isLinked, device.busy == nil, tracking.sawUnlink {
                programTracking[id]?.inGrace = false
                programUpdates[id]?.stage = .reconnected
                startProgramTimer(id, after: programUpdateTimeouts.versionReport)
                checkReportedVersion(device)
            } else if fromHub, !device.isLinked, device.busy == nil, tracking.sawLaunchHold {
                // Hub let go of the device: its launch hold ended without a
                // link, or the link is about to be reported. A short grace
                // tells the two apart without depending on event order.
                if !tracking.inGrace {
                    programTracking[id]?.inGrace = true
                    startProgramTimer(id, after: programUpdateTimeouts.graceAfterHold)
                }
            } else if fromHub, device.busy != nil, tracking.inGrace {
                // Held again: still restarting.
                programTracking[id]?.inGrace = false
                startProgramTimer(id, after: programUpdateTimeouts.reconnect)
            }
        case .reconnected:
            if device.isLinked {
                checkReportedVersion(device)
            } else {
                // Gone again before reporting: wait for it as for the restart.
                programUpdates[id]?.stage = .restarting
                programTracking[id]?.sawLaunchHold = false
                startProgramTimer(id, after: programUpdateTimeouts.reconnect)
            }
        default:
            break
        }
    }

    /// Finishes when the device reports the version installed. A device that
    /// still reports the older one may not have sent its new report yet; the
    /// version-report timer decides.
    private func checkReportedVersion(_ device: Device) {
        guard let update = programUpdates[device.id], update.stage == .reconnected,
              let reported = device.program.version else { return }
        if let expected = update.version {
            if reported == expected { finishProgramUpdate(device.id, .finished(version: reported)) }
        } else if reported != update.previous {
            finishProgramUpdate(device.id, .finished(version: reported))
        }
    }

    private func startProgramTimer(_ id: String, after delay: Duration) {
        programTracking[id]?.timer?.cancel()
        programTracking[id]?.timer = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled else { return }
            self?.programTimerFired(id)
        }
    }

    private func programTimerFired(_ id: String) {
        guard let update = programUpdates[id] else { return }
        let name = device(id)?.displayName
        switch update.stage {
        case .restarting:
            finishProgramUpdate(id, .failed(.updateNotReconnected(deviceName: name)))
        case .reconnected:
            let reported = device(id)?.program.version
            if let reported, let expected = update.version, reported != expected {
                // It reports a version, and not the one installed.
                finishProgramUpdate(id, .failed(.updateNotApplied(version: reported)))
            } else {
                // Linked again, and no report says otherwise (it reports
                // none, or Hub named no version for a local file).
                finishProgramUpdate(id, .finished(version: reported ?? update.version))
            }
        default:
            break
        }
    }

    private func finishProgramUpdate(_ id: String, _ stage: ProgramUpdate.Stage) {
        Self.updateLog.notice("update \(id, privacy: .public) ended: \(String(describing: stage), privacy: .public)")
        programUpdates[id]?.stage = stage
        programTracking[id]?.timer?.cancel()
        programTracking[id]?.timer = nil
    }

    /// The connection to Hub ended: an update's call ends with it, and what
    /// the device does next cannot be followed until the next sync.
    func dropProgramUpdates() {
        for tracking in programTracking.values {
            tracking.call?.cancel()
            tracking.timer?.cancel()
        }
        programTracking = [:]
        programUpdates = [:]
    }

    func forgetProgramUpdate(of id: String) {
        programTracking[id]?.call?.cancel()
        programTracking[id]?.timer?.cancel()
        programTracking[id] = nil
        programUpdates[id] = nil
    }
}
