#if DEBUG
import AppKit

/// Development aid, compiled into debug builds only: with
/// `POCKET_HUB_SNAPSHOT=<folder>` the app renders each of its own visible
/// windows into `<folder>/window-<n>.png` every second
/// (`POCKET_HUB_SNAPSHOT_INTERVAL_MS` changes the period).
///
/// It exists so the UI can be checked on a machine where the terminal has no
/// Screen Recording permission (`screencapture` then fails): a process may
/// always draw its own views into a bitmap. Menus (the menu bar extra's menu
/// included) are drawn by the system and are not captured.
@MainActor
enum DebugSnapshots {
    static func startIfRequested() {
        guard let folder = ProcessInfo.processInfo.environment["POCKET_HUB_SNAPSHOT"], !folder.isEmpty else { return }
        let directory = URL(fileURLWithPath: folder, isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let interval = ProcessInfo.processInfo.environment["POCKET_HUB_SNAPSHOT_INTERVAL_MS"].flatMap(Int.init) ?? 1000
        Task { @MainActor in
            while true {
                try? await Task.sleep(for: .milliseconds(interval))
                capture(into: directory)
            }
        }
    }

    private static func capture(into directory: URL) {
        let windows = NSApp.windows.filter { $0.isVisible && $0.frame.width > 200 && $0.frame.height > 150 }
        for (index, window) in windows.enumerated() {
            // The theme frame holds the title bar and the toolbar as well as the content.
            guard let view = window.contentView?.superview ?? window.contentView,
                  let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)
            else { continue }
            view.cacheDisplay(in: view.bounds, to: rep)
            guard let png = rep.representation(using: .png, properties: [:]) else { continue }
            try? png.write(to: directory.appending(path: "window-\(index).png"), options: .atomic)
        }
    }
}
#endif
