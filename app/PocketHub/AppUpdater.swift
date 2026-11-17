import AppKit
import Observation
import Sparkle

/// Pocket Hub's updates of itself, by Sparkle. Info.plist says where the
/// releases are listed (the appcast.xml of the latest release on GitHub,
/// written by app/scripts/package.sh), the key they are signed with, and that
/// checking and installing are automatic: Sparkle looks once a day, downloads
/// a new version in the background and installs it when the app quits. An
/// app left running for a week with an update waiting is shown Sparkle's own
/// alert to install it now (SUScheduledImpatientCheckInterval).
///
/// The core outlives the app, so an update reaches it when the updated app
/// connects: a core older than the app is replaced by the app's own
/// (HubClient, CONTROL-API.md §1).
///
/// A Debug build does not start the updater: it is a build of the
/// repository, which a release must not replace. Its menus and settings are
/// the same, with Check for Updates… unavailable.
@MainActor
@Observable
final class AppUpdater: NSObject {
    /// Whether Check for Updates… can run now (not while a check or an
    /// install is under way, nor in a Debug build).
    private(set) var canCheckForUpdates = false
    private(set) var automaticallyChecks = false
    private(set) var automaticallyDownloads = false
    /// The version of an update that Sparkle found by itself and is showing,
    /// until the owner attends to it. The menu bar offers it: Hub mostly
    /// runs with no window, as an app out of the Dock, and Sparkle then shows
    /// its alert behind other apps' windows (Sparkle, "Gentle Update
    /// Reminders").
    private(set) var waitingUpdate: String?

    @ObservationIgnored private var controller: SPUStandardUpdaterController!
    @ObservationIgnored private var observations: [NSKeyValueObservation] = []

    override init() {
        super.init()
        #if DEBUG
        let start = false
        #else
        let start = true
        #endif
        controller = SPUStandardUpdaterController(startingUpdater: start, updaterDelegate: nil, userDriverDelegate: self)
        // The three settings are KVO compliant, and change outside this
        // object too: the update alert has its own "automatically" checkbox.
        let updater = controller.updater
        observations = [
            updater.observe(\.canCheckForUpdates, options: [.initial, .new]) { [weak self] updater, _ in
                MainActor.assumeIsolated { self?.canCheckForUpdates = updater.canCheckForUpdates }
            },
            updater.observe(\.automaticallyChecksForUpdates, options: [.initial, .new]) { [weak self] updater, _ in
                MainActor.assumeIsolated { self?.automaticallyChecks = updater.automaticallyChecksForUpdates }
            },
            updater.observe(\.automaticallyDownloadsUpdates, options: [.initial, .new]) { [weak self] updater, _ in
                MainActor.assumeIsolated { self?.automaticallyDownloads = updater.automaticallyDownloadsUpdates }
            },
        ]
    }

    /// Check for Updates…; with an update already shown, brings it to the front.
    func checkForUpdates() {
        controller.checkForUpdates(nil)
    }

    /// The owner's choices in Settings. Sparkle keeps them in the app's user
    /// defaults; Info.plist holds only their first values, so they are set
    /// here on a change and never at launch.
    func setAutomaticallyChecks(_ on: Bool) {
        controller.updater.automaticallyChecksForUpdates = on
    }

    func setAutomaticallyDownloads(_ on: Bool) {
        controller.updater.automaticallyDownloadsUpdates = on
    }
}

extension AppUpdater: @preconcurrency SPUStandardUserDriverDelegate {
    var supportsGentleScheduledUpdateReminders: Bool { true }

    func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem,
                                                   state: SPUUserUpdateState)
    {
        // One the owner asked for is in front of them already.
        guard !state.userInitiated else { return }
        waitingUpdate = update.displayVersionString
    }

    func standardUserDriverDidReceiveUserAttention(forUpdate update: SUAppcastItem) {
        waitingUpdate = nil
    }

    func standardUserDriverWillFinishUpdateSession() {
        waitingUpdate = nil
    }
}
