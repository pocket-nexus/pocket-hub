import AppKit
import HubAppModel

/// File operations on a screenshot Hub wrote. Reads and copies happen off
/// the main actor.
enum ScreenshotFile {
    static func loadImage(path: String) async -> NSImage? {
        let data = await Task.detached(priority: .userInitiated) { try? Data(contentsOf: URL(fileURLWithPath: path)) }.value
        return data.flatMap(NSImage.init(data:))
    }

    @MainActor
    static func copyToPasteboard(path: String) async {
        guard let image = await loadImage(path: path) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.writeObjects([image])
    }

    /// Copies Hub's PNG into `folder` under a name of the device's and the
    /// time's (ScreenshotDestination.file), never over a file already there.
    /// Returns where it is.
    @discardableResult
    static func copy(path: String, into folder: URL, device: String, date: Date = Date()) async throws -> URL {
        try await Task.detached(priority: .userInitiated) {
            let files = FileManager.default
            let destination = ScreenshotDestination.file(in: folder, device: device, date: date) { files.fileExists(atPath: $0.path) }
            try files.copyItem(at: URL(fileURLWithPath: path), to: destination)
            return destination
        }.value
    }
}
