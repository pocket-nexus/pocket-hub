import Foundation
import Observation
import ServiceManagement

/// "Open Pocket Hub at login", through `SMAppService.mainApp`.
///
/// Registration only works from a signed app bundle; from `swift run` the
/// service reports itself unavailable and the toggle says so.
@MainActor
@Observable
public final class LaunchAtLogin {
    public enum State: Equatable, Sendable {
        case enabled
        case disabled
        /// Registered, waiting for the owner's approval in System Settings › Login Items.
        case requiresApproval
        /// Not possible: the app is not running from an app bundle (`swift run`).
        case unavailable
    }

    public private(set) var state: State = .disabled
    public private(set) var lastError: String?

    public init() {
        refresh()
    }

    public func refresh() {
        guard Bundle.main.bundleURL.pathExtension == "app" else {
            state = .unavailable
            return
        }
        state = switch SMAppService.mainApp.status {
        case .enabled: .enabled
        case .requiresApproval: .requiresApproval
        // A copy that was never registered can report `.notFound`; registering
        // it may still succeed, and a failure is shown from `register()`.
        case .notRegistered, .notFound: .disabled
        @unknown default: .disabled
        }
    }

    public var isEnabled: Bool { state == .enabled || state == .requiresApproval }

    public func setEnabled(_ enabled: Bool) {
        lastError = nil
        do {
            if enabled {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
        } catch {
            lastError = error.localizedDescription
        }
        refresh()
    }

    public func openLoginItemsSettings() {
        SMAppService.openSystemSettingsLoginItems()
    }
}
