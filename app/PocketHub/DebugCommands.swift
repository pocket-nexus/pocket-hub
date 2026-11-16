#if DEBUG
import AppKit
import HubAppModel
import HubClient
import SwiftUI

/// Development aid, compiled into debug builds only: with
/// `POCKET_HUB_DEBUG_COMMANDS=<file>` (a FIFO) the app reads one command per
/// line and performs what the matching control does, so a verification run
/// can drive the window when Accessibility scripting is unavailable (another
/// Space, another app in front). Each command calls the same model method as
/// the button it stands for; results show in the window and in the snapshots.
///
///   select daemons | device <id>             tab applets | screenshot | log | details
///   store refresh | install <app> | cancel <app> | details <app> | close-details
///                                            (the store of the selected device's Applets page)
///   screenshot | logs on | logs off | open <app id> | delete <app id> | home | update | update-dismiss
///                                            (on the selected device)
///   menu-preview | menu-preview-close        (the menu bar menu's views in a window)
///   window | window-size <width> <height>
///   settings | language system | en | zh-Hans | reopen   (the Settings window and its language)
///   link <url>                               (what the system does with a pocket-hub: link, docs/DEEP-LINKS.md)
///   store pick <device id> | store add-device            (the open applet sheet's device menu and its Add a Device…)
///   add | add choose 3ds|tab5 | add back | add code <digits> | add pair | add cancel
///                                            (the Add a Device sheet: its first page, the code's field and Pair)
@MainActor
enum DebugCommands {
    static func startIfRequested(_ state: AppState) {
        guard let path = ProcessInfo.processInfo.environment["POCKET_HUB_DEBUG_COMMANDS"], !path.isEmpty else { return }
        let lines = AsyncStream<String> { continuation in
            Thread.detachNewThread {
                // Reopen after each writer closes, as a FIFO reader must.
                while let handle = FileHandle(forReadingAtPath: path) {
                    let data = handle.readDataToEndOfFile()
                    for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
                        continuation.yield(String(line))
                    }
                }
                continuation.finish()
            }
        }
        Task { @MainActor in
            for await line in lines { perform(line, state) }
        }
    }

    private static func perform(_ line: String, _ state: AppState) {
        let words = line.split(separator: " ").map(String.init)
        let store = state.model.store
        func storeApp(_ id: String) -> StoreApp? { store.apps.first { $0.id == id } }
        let device = state.selectedDeviceID
        func onDevice(_ action: @escaping @MainActor (String) async throws -> Void) {
            guard let device else { print("debug command needs a selected device: \(line)"); return }
            Task { @MainActor in
                do { try await action(device) } catch {
                    let named = state.model.device(device)
                    state.presentedError = UserFacingError(error, deviceName: named?.displayName, kind: named?.kind ?? DeviceKind(rawValue: ""))
                }
            }
        }
        switch words {
        case ["window"]: state.showWindow()
        case let w where w.count == 3 && w[0] == "window-size":
            // The window clamps the size to its minimum (800 × 480).
            if let width = Double(w[1]), let height = Double(w[2]),
               let window = NSApp.windows.first(where: { $0.identifier?.rawValue.hasPrefix("main") == true })
                   ?? NSApp.windows.first(where: { $0.isVisible && $0.frame.width > 400 })
            {
                window.setContentSize(NSSize(width: width, height: height))
            }
        case ["select", "daemons"]: state.selection = .daemons
        case let w where w.count == 3 && w[0] == "select" && w[1] == "device": state.selection = .device(w[2])
        case ["tab", "applets"]: state.deviceTab = .applets
        case ["tab", "screenshot"]: state.deviceTab = .screenshot
        case ["tab", "log"]: state.deviceTab = .log
        case ["tab", "details"]: state.deviceTab = .details
        // Each of these is what the matching control does.
        case ["screenshot"]: if let device { state.deviceTab = .screenshot; state.takeScreenshot(of: device, reveal: false) }
        case ["logs", "on"]: onDevice { try await state.model.setLogs(true, for: $0) }
        case ["logs", "off"]: onDevice { try await state.model.setLogs(false, for: $0) }
        case let w where w.count == 2 && w[0] == "open":
            let appId = w[1]
            onDevice { id in
                guard let app = state.model.device(id)?.apps?.first(where: { $0.id == appId }) else { return }
                try await state.model.launch(app, on: id)
            }
        case let w where w.count == 2 && w[0] == "delete":
            let appId = w[1]
            onDevice { id in
                guard let app = state.model.device(id)?.apps?.first(where: { $0.id == appId }) else { return }
                try await state.model.remove(app, from: id)
            }
        case ["home"]:
            onDevice { id in
                guard let app = state.model.device(id)?.apps?.first(where: \.isHome) else { return }
                try await state.model.launch(app, on: id)
            }
        case ["update"]: if let device { state.model.updateProgram(on: device) }
        case ["update-dismiss"]: if let device { state.model.dismissProgramUpdate(of: device) }
        case ["menu-preview"]: MenuPreview.show(state)
        case ["menu-preview-close"]: MenuPreview.close()
        case ["settings"]: NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
        case let w where w.count == 2 && w[0] == "language":
            if let choice = AppLanguage.Choice(rawValue: w[1]) { state.language.choose(choice) }
        case ["reopen"]: state.relaunch()
        case let w where w.count == 2 && w[0] == "link":
            if let url = URL(string: w[1]) { state.open(url) } else { print("debug command: not a URL: \(w[1])") }
        case ["add"]: state.startSetup()
        case ["add", "choose", "3ds"]: state.setupFlow?.choose(.nintendo3ds)
        case ["add", "choose", "tab5"]: state.setupFlow?.choose(.tab5)
        case ["add", "back"]: state.setupFlow?.chooseAgain()
        case let w where w.count == 3 && w[0] == "add" && w[1] == "code": state.setupFlow?.pairCode = w[2]
        case ["add", "pair"]: state.setupFlow?.submitPairCode()
        case ["add", "cancel"]:
            state.setupFlow?.declinePairRequest()
            state.setupFlow?.cancel()
            state.showingSetup = false
        case ["store", "add-device"]:
            state.storeDetails = nil
            state.startSetup()
        case ["store", "refresh"]: store.reload(refresh: true)
        case ["store", "close-details"]: state.storeDetails = nil
        case let w where w.count == 3 && w[0] == "store":
            switch w[1] {
            case "install": if let app = storeApp(w[2]), let device = store.deviceId { store.install(app, on: device) }
            case "cancel": if let app = storeApp(w[2]), let device = store.deviceId { store.cancelInstall(of: app, on: device) }
            case "details": state.storeDetails = storeApp(w[2])
            case "pick": if let app = state.storeDetails { state.show(app, on: w[2]) }
            default: print("debug command not understood: \(line)")
            }
        default: print("debug command not understood: \(line)")
        }
    }
}

/// The menu bar menu's own views (`MenuBarContent`'s items, each device's
/// submenu expanded) in an ordinary window, so the snapshot hook can draw
/// them: the system draws the real menu, which a process cannot capture.
/// The words and the enabled states are the menu's; the look is not.
@MainActor
enum MenuPreview {
    private static var window: NSWindow?

    static func show(_ state: AppState) {
        close()
        let root = MenuPreviewContent()
            .environment(state)
            .padding(16)
            .frame(width: 420, alignment: .leading)
        let window = NSWindow(contentRect: NSRect(x: 80, y: 80, width: 420, height: 420),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.title = "Menu preview (debug)"
        window.contentView = NSHostingView(rootView: root)
        window.setContentSize(window.contentView?.fittingSize ?? NSSize(width: 420, height: 420))
        window.isReleasedWhenClosed = false
        window.orderFront(nil)
        self.window = window
    }

    static func close() {
        window?.close()
        window = nil
    }
}

private struct MenuPreviewContent: View {
    @Environment(AppState.self) private var state

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            MenuBarContent()
            Divider()
            ForEach(state.model.devices) { device in
                VStack(alignment: .leading, spacing: 4) {
                    DeviceMenuLabel(device: device).font(.headline)
                    DeviceMenuItems(device: device).padding(.leading, 16)
                }
            }
        }
        .buttonStyle(.link)
    }
}
#endif
