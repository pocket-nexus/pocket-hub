import Foundation
import HubClient

/// An error said in plain words, with one obvious next step.
///
/// The title and advice are written by this app and localized; Hub's own
/// sentence (`error.message`) is kept as `detail`, because Hub writes it in
/// one language and it is sometimes the only specific information there is.
public struct UserFacingError: Error, Identifiable, Sendable, Equatable {
    public private(set) var id = UUID()
    public var title: String
    public var advice: String
    public var detail: String?

    public init(title: String, advice: String, detail: String? = nil) {
        self.title = title
        self.advice = advice
        self.detail = detail
    }

    public static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.title == rhs.title && lhs.advice == rhs.advice && lhs.detail == rhs.detail
    }

    /// nil for a cancellation, which the owner asked for and needs no message.
    public init?(_ error: any Error, deviceName: String? = nil) {
        let name = deviceName ?? String(localized: "The device")
        guard let hubError = error as? HubError else {
            self.init(title: String(localized: "Something went wrong."),
                      advice: String(localized: "Try again. If it keeps happening, restart Pocket Hub."),
                      detail: error.localizedDescription)
            return
        }
        switch hubError {
        case .operation(let operation):
            let detail = operation.message.isEmpty ? nil : operation.message
            switch operation.kind {
            case .cancelled:
                return nil
            case .notFound:
                self.init(title: String(localized: "That item is no longer there."),
                          advice: String(localized: "It may have been removed. The list is now up to date."),
                          detail: detail)
            case .notLinked:
                self.init(title: String(localized: "\(name) isn't connected right now."),
                          advice: String(localized: "Turn it on and open Pocket Shell, then try again."),
                          detail: nil)
            case .busy:
                let holder = operation.holder.map(\.displayName) ?? String(localized: "another app")
                let activity = operation.holder.map(\.activity) ?? String(localized: "using it")
                self.init(title: String(localized: "\(name) is busy."),
                          advice: String(localized: "\(holder) is \(activity) right now. Try again when it has finished."),
                          detail: nil)
            case .link:
                self.init(title: String(localized: "\(name) didn't respond."),
                          advice: String(localized: "Make sure it is on and nearby, then try again."),
                          detail: detail)
            case .invalid:
                self.init(title: String(localized: "This can't be used."),
                          advice: String(localized: "Check the file or the value you chose, then try again."),
                          detail: detail)
            case .release:
                self = .releaseFailure(reason: operation.reason, detail: detail)
            case .store:
                if operation.reason == "rejected" {
                    self.init(title: String(localized: "This applet didn't pass Pocket Hub's safety check."),
                              advice: String(localized: "What was downloaded doesn't match what its publisher signed, so nothing was installed. Try again later."),
                              detail: detail)
                } else {
                    self.init(title: String(localized: "Pocket Hub can't reach the applet store."),
                              advice: String(localized: "Check this Mac's internet connection, then try again."),
                              detail: detail)
                }
            case .daemon:
                self.init(title: String(localized: "The daemon couldn't be installed, started or stopped."),
                          advice: String(localized: "Check its output in Advanced › Daemons, then try again."),
                          detail: detail)
            case .setup:
                self.init(title: String(localized: "The setup didn't finish."),
                          advice: String(localized: "Try the setup again."),
                          detail: detail)
            case .internal, .other:
                self.init(title: String(localized: "Something went wrong in Pocket Hub."),
                          advice: String(localized: "Try again. If it keeps happening, restart Pocket Hub."),
                          detail: detail)
            }
        case .notConnected, .disconnected:
            self.init(title: String(localized: "Pocket Hub isn't responding."),
                      advice: String(localized: "It is restarting. Try again in a moment."),
                      detail: nil)
        case .rpc(_, let message), .invalidResponse(let message):
            self.init(title: String(localized: "This version of Pocket Hub doesn't match its background service."),
                      advice: String(localized: "Reinstall Pocket Hub to bring both up to date."),
                      detail: message)
        case .incompatibleProtocol:
            self.init(title: String(localized: "This version of Pocket Hub doesn't match its background service."),
                      advice: String(localized: "Reinstall Pocket Hub to bring both up to date."),
                      detail: nil)
        }
    }
}

extension UserFacingError {
    /// A failure of `store.apps`.
    public init(storeError error: any Error) {
        self = UserFacingError(error) ?? UserFacingError(title: String(localized: "Something went wrong."),
                                                          advice: String(localized: "Try again."))
    }

    /// A failure of `devices.remove`, worded for the applet and the device.
    /// nil for a cancellation.
    public init?(deleteError error: any Error, appTitle: String, deviceName: String?) {
        let name = deviceName ?? String(localized: "This device")
        if let hubError = error as? HubError, case .operation(let operation) = hubError, operation.kind == .link {
            let detail = operation.message.isEmpty ? nil : operation.message
            // The device's refusals (link specification §6.2 `runtime.remove`).
            switch (operation.linkCode, operation.reason) {
            case ("rejected", "running"):
                self.init(title: String(localized: "\(appTitle) is open on \(name)."),
                          advice: String(localized: "Go back to the home screen on the console, then delete it again."),
                          detail: detail)
                return
            case ("rejected", "home"):
                self.init(title: String(localized: "The home screen can't be deleted."),
                          advice: String(localized: "The console opens its applets from it."),
                          detail: detail)
                return
            case ("unsupported", _):
                self = .deletingNeedsNewerSoftware(deviceName: name)
                return
            default:
                break
            }
        }
        self.init(error, deviceName: deviceName)
    }

    /// What deleting says for a device whose software predates deleting from
    /// Hub (its link does not use `runtime.remove`): the console's own way.
    public static func deletingNeedsNewerSoftware(deviceName: String) -> UserFacingError {
        UserFacingError(title: String(localized: "\(deviceName) can't delete applets from this Mac."),
                        advice: String(localized: "Delete it on the console instead: hold its icon on the home screen. A newer version of its software deletes applets from here."))
    }

    /// A failure of `store.install`, worded for the applet and the device.
    /// nil for a cancellation the owner asked for.
    public init?(storeInstallError error: any Error, appTitle: String, deviceName: String?) {
        if let hubError = error as? HubError, case .operation(let operation) = hubError, operation.kind == .notFound {
            let name = deviceName ?? String(localized: "This device")
            self.init(title: String(localized: "\(appTitle) isn't available for \(name)."),
                      advice: String(localized: "The store no longer offers it for this kind of device. The list is now up to date."),
                      detail: operation.message.isEmpty ? nil : operation.message)
            return
        }
        self.init(error, deviceName: deviceName)
    }
}

// MARK: - Links (docs/DEEP-LINKS.md §2)

extension UserFacingError {
    /// A link that is not one of the specification's: another host, a path of
    /// another shape, an id outside its grammar. Nothing of the link is shown:
    /// it is text from outside Hub.
    public static var linkNotUnderstood: UserFacingError {
        UserFacingError(title: String(localized: "Pocket Hub can't open this link."),
                        advice: String(localized: "It may be incomplete, or made for a newer version of Pocket Hub."))
    }

    /// A link to an applet the store does not list, asked with a fresh catalog.
    public static var appletNotInStore: UserFacingError {
        UserFacingError(title: String(localized: "This applet isn't in the store."),
                        advice: String(localized: "It may have been removed since the link was made."))
    }
}

// MARK: - Updating a device's software

extension UserFacingError {
    /// A failure of `devices.installProgram` started by the update button,
    /// worded for the update. nil for a cancellation.
    public init?(programUpdateError error: any Error, deviceName: String?) {
        let name = deviceName ?? String(localized: "Your 3DS")
        guard let hubError = error as? HubError, case .operation(let operation) = hubError else {
            self.init(error, deviceName: deviceName)
            return
        }
        let detail = operation.message.isEmpty ? nil : operation.message
        switch operation.kind {
        case .release:
            self = .releaseFailure(reason: operation.reason, detail: detail)
        case .notLinked:
            self.init(title: String(localized: "\(name) isn't connected right now."),
                      advice: String(localized: "Turn it on and wait until it shows as connected, then try the update again."),
                      detail: nil)
        case .link:
            switch operation.linkCode {
            case "launch":
                // The device wrote the file and could not start it (§6.2 `launch`).
                self.init(title: String(localized: "Your 3DS received the update but couldn't restart with it."),
                          advice: String(localized: "The new version starts the next time you open Pocket Shell from the Homebrew Launcher."),
                          detail: detail)
            case "unsupported":
                self.init(title: String(localized: "This 3DS can't be updated from this Mac."),
                          advice: String(localized: "Set it up again with Add a Device… to bring its software up to date."),
                          detail: detail)
            default:
                self.init(title: String(localized: "The connection to your 3DS was lost during the update."),
                          advice: String(localized: "Make sure your 3DS is on and nearby, then try the update again."),
                          detail: detail)
            }
        default:
            self.init(error, deviceName: deviceName)
        }
    }

    /// The error `release`, by its `reason`. Hub fails before sending anything
    /// to the device when the release cannot be used.
    static func releaseFailure(reason: String?, detail: String?) -> UserFacingError {
        switch reason {
        case "not_found":
            UserFacingError(title: String(localized: "This update is no longer available."),
                            advice: String(localized: "Nothing was sent to your 3DS. Pocket Hub looks for the newest update each time your 3DS connects."),
                            detail: detail)
        case "rejected":
            UserFacingError(title: String(localized: "The update didn't pass Pocket Hub's safety check."),
                            advice: String(localized: "What was downloaded doesn't match what was published, so nothing was sent to your 3DS. Try again later."),
                            detail: detail)
        default:
            UserFacingError(title: String(localized: "Pocket Hub can't download the update right now."),
                            advice: String(localized: "Nothing was sent to your 3DS. Check this Mac's internet connection, then try again."),
                            detail: detail)
        }
    }

    /// The device took the update and restarted, and did not link again in time.
    public static func updateNotReconnected(deviceName: String?) -> UserFacingError {
        let name = deviceName ?? String(localized: "Your 3DS")
        return UserFacingError(title: String(localized: "\(name) hasn't reconnected after the update."),
                               advice: String(localized: "Make sure it is on and nearby. If it shows the Homebrew Launcher, open Pocket Shell from there; it reconnects by itself."))
    }

    /// The device linked again and still reports the version it had.
    public static func updateNotApplied(version: String) -> UserFacingError {
        UserFacingError(title: String(localized: "Your 3DS reconnected, but its software is still version \(version)."),
                        advice: String(localized: "Try the update again."))
    }
}
