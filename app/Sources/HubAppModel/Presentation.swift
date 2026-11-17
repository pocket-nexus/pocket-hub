import Foundation
import HubClient

// Localized words for the values of the contract. Kept beside the model,
// not in views, so every surface built on this layer says the same thing.

extension DeviceKind {
    /// The family's name, for a device that has not named itself.
    public var displayName: String {
        switch self {
        case .nintendo3ds: String(localized: "Nintendo 3DS")
        case .tab5: String(localized: "M5Stack Tab5")
        default: String(localized: "New device")
        }
    }

    /// The family's symbol: a game console, a screen in its bezel. `filled`
    /// is the variant a connected device and a pane's icon take.
    public func symbolName(filled: Bool) -> String {
        switch self {
        case .tab5: filled ? "rectangle.inset.filled" : "rectangle"
        default: filled ? "gamecontroller.fill" : "gamecontroller"
        }
    }

    /// What brings a device of this family onto the link, for one that has
    /// never been seen. A 3DS runs its software from the Homebrew Launcher,
    /// so "Pocket Shell" is the thing to open on it; a Tab5 starts into it.
    public var howToConnect: String {
        switch self {
        case .nintendo3ds: String(localized: "Open Pocket Shell on the device to connect it.")
        default: String(localized: "Turn it on to connect it.")
        }
    }

    /// The same, as the advice of a call that found the device away.
    public var tryAgainWhenConnected: String {
        switch self {
        case .nintendo3ds: String(localized: "Turn it on and open Pocket Shell, then try again.")
        default: String(localized: "Turn it on and make sure it is on the same Wi-Fi network as this Mac, then try again.")
        }
    }
}

extension PairRequest {
    /// The name the device gave in its request, or its family's.
    public var displayName: String { name.isEmpty ? kind.displayName : name }
}

extension Device {
    /// The name the device gave at its last handshake, or a family name before its first.
    public var displayName: String { name.isEmpty ? kind.displayName : name }

    /// The applet running now, from the last `runtime.status`.
    public var runningApp: RunningApp? { link == nil ? nil : status?.app }

    /// The device's own home screen is what runs now: the running applet is
    /// the `home` row of its apps. A UI says "Home screen" rather than the
    /// program's own title.
    public var isOnHomeScreen: Bool {
        guard let running = runningApp else { return false }
        return apps?.contains { $0.isHome && $0.id == running.id } == true
    }

    /// The running applet in the owner's words: "Home screen", or its title.
    public var runningAppTitle: String? {
        guard let running = runningApp else { return nil }
        return isOnHomeScreen ? String(localized: "Home screen") : running.title
    }

    /// Hub knows a newer release of the device's software than it runs.
    public var offersSoftwareUpdate: Bool { program.update && program.latest != nil }

    /// The device deletes an applet at Hub's request: its link uses
    /// `runtime.remove` (link specification §6.1), which software older than
    /// that request does not list.
    public var deletesApplets: Bool { link?.features.contains("runtime.remove") == true }

    /// Applets sorted for display: the launcher first, then by id.
    public var sortedApps: [InstalledApp] {
        (apps ?? []).sorted { lhs, rhs in
            if lhs.isHome != rhs.isHome { return lhs.isHome }
            return lhs.id.localizedStandardCompare(rhs.id) == .orderedAscending
        }
    }
}

extension InstalledApp {
    /// The name a row shows when the store gives none: "Home screen" for the
    /// device's own, else the device's `output` exactly as it reports it. The
    /// contract carries no title for installed applets, and a title made up
    /// from the output name would pass for one.
    public var displayName: String {
        if isHome { return String(localized: "Home screen") }
        return output.isEmpty ? id : output
    }
}

extension Carrier {
    public var displayName: String {
        switch self {
        case .tcp: String(localized: "Wi-Fi")
        case .usb: String(localized: "USB")
        case .uart: String(localized: "Cable")
        case .ble: String(localized: "Bluetooth")
        default: rawValue
        }
    }

    public var symbolName: String {
        switch self {
        case .tcp: "wifi"
        case .usb: "cable.connector"
        case .uart: "cable.connector.horizontal"
        case .ble: "dot.radiowaves.left.and.right"
        default: "link"
        }
    }
}

extension DeviceUnlinked {
    /// Why the link ended, in the owner's words (link specification §4.8).
    public var explanation: String {
        switch code {
        case "going_away": String(localized: "The device was turned off or went to sleep.")
        case "lost": String(localized: "The connection was lost.")
        case "silence", "handshake_timeout": String(localized: "The device stopped responding.")
        case "replaced": String(localized: "The device connected again.")
        case "forgotten": String(localized: "The device was removed from Pocket Hub.")
        case "restart": String(localized: "The device is restarting.")
        case "auth": String(localized: "The device doesn't recognize this Mac anymore. Add it again.")
        default: String(localized: "The connection was lost.")
        }
    }
}

extension DaemonState {
    public var displayName: String {
        switch self {
        case .stopped: String(localized: "Stopped")
        case .starting: String(localized: "Starting")
        case .running: String(localized: "Running")
        case .stopping: String(localized: "Stopping")
        case .failed: String(localized: "Failed")
        default: rawValue
        }
    }
}

extension SetupStage {
    /// What Hub is doing, without the words "runtime", "shell" or "pairing".
    public var displayName: String {
        switch self {
        case .connecting: String(localized: "Connecting to your 3DS…")
        case .reading: String(localized: "Checking your 3DS…")
        case .downloading: String(localized: "Downloading…")
        case .uploading: String(localized: "Copying to your 3DS…")
        case .pairing: String(localized: "Finishing up…")
        case .done: String(localized: "Done")
        default: String(localized: "Working…")
        }
    }
}

extension MutexHolder {
    /// Who holds the device: "Pocket Hub", "The command line", or the client's name.
    public var displayName: String {
        switch kind {
        case "cli": String(localized: "The command line")
        case "remote": String(localized: "Pocket Studio (\(name))")
        default: name.isEmpty ? String(localized: "Another app") : name
        }
    }

    /// What the holder is doing, from the method that holds the device.
    public var activity: String {
        switch method {
        case "devices.push", "store.install": String(localized: "installing an applet")
        case "devices.launch": String(localized: "opening an applet")
        case "devices.remove": String(localized: "deleting an applet")
        case "devices.installProgram": String(localized: "updating this 3DS's software")
        default: String(localized: "using it")
        }
    }
}

extension DeviceBusy {
    /// "Pocket Hub is installing an applet", or that the device is restarting.
    public var displayText: String {
        if launching { return String(localized: "Restarting with updated software…") }
        return String(localized: "\(holder.displayName) is \(holder.activity)")
    }
}

/// The phase of a push or a program install, in the owner's words.
public func transferPhaseText(_ phase: String?) -> String {
    switch phase {
    case "downloading": String(localized: "Downloading…")
    case "sending": String(localized: "Sending…")
    case "receiving", "received": String(localized: "The device is receiving it…")
    case "staged": String(localized: "Installing on the device…")
    default: String(localized: "Preparing…")
    }
}

extension ProgramUpdate.Stage {
    /// What the update is doing, in the owner's words.
    public var displayText: String {
        switch self {
        case .preparing: String(localized: "Getting the update ready…")
        case .downloading: String(localized: "Downloading the update…")
        case .sending: String(localized: "Sending the update to your 3DS…")
        case .restarting: String(localized: "Your 3DS restarts and reconnects by itself…")
        case .reconnected: String(localized: "Your 3DS is back. Checking its software version…")
        case .finished: String(localized: "Your 3DS's software is up to date.")
        case .failed(let failure): failure.title
        }
    }
}

/// The stage of a daemon install, in the owner's words.
public func daemonInstallStageText(_ stage: String) -> String {
    switch stage {
    case "runtime": String(localized: "Downloading Bun…")
    case "installing": String(localized: "Installing…")
    default: String(localized: "Preparing…")
    }
}

/// Why the core could not start, in plain words, from its one English line.
///
/// The core has no reason codes for a failed start (CONTROL-API.md §1), so the
/// cases are told apart by the line itself; anything unrecognised keeps a
/// generic title and shows the line as detail.
public struct CoreStartFailure: Equatable, Sendable {
    public var title: String
    public var advice: String
    /// The core's line, shown under the advice.
    public var detail: String

    public init(line: String) {
        detail = line
        let lowered = line.lowercased()
        if lowered.contains("link port"), lowered.contains("in use") {
            let port = line.firstMatch(of: /link port (\d+)/).map { String($0.output.1) } ?? "8628"
            title = String(localized: "Another program is using the network port Pocket Hub needs.")
            advice = String(localized: "Pocket Hub's devices connect on port \(port). Quit the program that uses it (it may be another copy of Pocket Hub), then click Try Again.")
        } else if lowered.contains("home") || lowered.contains("socket") || lowered.contains("folder") {
            title = String(localized: "Pocket Hub can't use its folder.")
            advice = String(localized: "Check that the folder below exists and that you can write to it, then click Try Again.")
        } else {
            title = String(localized: "Pocket Hub's background service couldn't start.")
            advice = String(localized: "Click Try Again. If it keeps happening, restart this Mac.")
        }
    }
}

extension StoreApp {
    /// "1.2 MB".
    public var sizeText: String {
        ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
    }
}

/// What a capability an applet declares lets it do (PocketJS
/// `contracts/spec/applet.ts`), for the ids of `StoreDetails.needs` and
/// `uses`. nil for an id this app has no words for: a UI shows the id
/// itself. The ids come out of a publisher's package, so the list is the
/// app's, not the package's.
///
/// An applet's compatibility (these and `screenText`) is said in English in
/// every language of the app, as the owner chose: these are the applet
/// platform's terms, and the store's website says them in English too. So
/// they are plain strings, not keys of the String Catalog.
public func capabilityText(_ id: String) -> String? {
    switch id {
    case "text.glyphs.baked": "Text drawn with its own built-in characters"
    case "text.glyphs.runtime": "Characters it doesn't include, from the device's fonts"
    case "input.buttons": "The device's buttons"
    case "input.analog.left": "The left stick"
    case "input.text": "Typing on a keyboard"
    case "display.auxiliary": "A touch screen of its own"
    case "input.touch.auxiliary": "Touch on that screen"
    case "net.http": "Internet requests, made by the device for it"
    case "ui.physics": "2D physics"
    case "audio.pcm": "Sound it makes itself"
    case "media.playback": "Audio and video streams"
    case "data.fs": "Files of its own"
    case "system.agent": "The device's coding agent can operate it"
    default: id.hasPrefix("camera.") ? "The camera" : nil
    }
}

extension StoreDetails {
    /// The names of the devices it was built for, in the store's order, each
    /// once: "Omarchy Pocket, M5Stack Tab5, Nintendo 3DS". A device's name is
    /// the store's and the same in every language.
    public var deviceNames: [String] {
        devices.map(\.name).reduce(into: []) { names, name in
            if !names.contains(name) { names.append(name) }
        }
    }

    /// The surface it lays itself out for, in a line: "Touch, 320 × 240 to
    /// 640 × 720", in English in every language as `capabilityText` is. nil
    /// when the package names none, or names the device's own screen with
    /// no sizes, which says nothing.
    public var screenText: String? {
        guard let surface else { return nil }
        let size = surface.size.flatMap { size -> String? in
            guard size.min.count == 2, size.max.count == 2 else { return nil }
            return "\(size.min[0]) × \(size.min[1]) to \(size.max[0]) × \(size.max[1])"
        }
        switch (surface.touch, size) {
        case (true, let size?): return "Touch, \(size)"
        case (true, nil): return "Touch"
        case (false, let size?): return size
        case (false, nil): return nil
        }
    }
}
