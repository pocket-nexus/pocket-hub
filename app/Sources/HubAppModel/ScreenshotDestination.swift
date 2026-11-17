import Foundation

/// Where the app keeps a device's screenshot: where macOS keeps the owner's
/// own screenshots, as Simulator keeps a simulated device's. That folder is
/// the `location` of the `com.apple.screencapture` domain (the Screenshot
/// app's Options › Save To), and the Desktop while it is not set, which is
/// macOS's own default. Hub's file stays in its cache (CONTROL-API.md §3
/// `devices.screenshot`); the app keeps a copy here.
public enum ScreenshotDestination {
    /// The domain and key macOS keeps the screenshot folder under.
    public static let defaultsDomain = "com.apple.screencapture"
    public static let locationKey = "location"

    /// The folder: `location` as macOS stores it (a path, `~` allowed), or
    /// the Desktop of `home` when it is not set.
    public static func folder(location: String?, home: URL) -> URL {
        guard let location, !location.isEmpty else { return home.appending(path: "Desktop", directoryHint: .isDirectory) }
        let expanded = location.hasPrefix("~") ? home.path + location.dropFirst() : location
        return URL(fileURLWithPath: expanded, isDirectory: true)
    }

    /// The folder this Mac's owner has chosen.
    public static func folder() -> URL {
        folder(location: UserDefaults(suiteName: defaultsDomain)?.string(forKey: locationKey),
               home: FileManager.default.homeDirectoryForCurrentUser)
    }

    /// The file a screenshot of `device` taken at `date` is kept as: the
    /// device's name and the time, "ewind 2026-11-17 11.02.33.png", with
    /// " 2", " 3"… before the extension when that name is taken. A `/` or
    /// `:` of the name, which a file name cannot hold, becomes `-`.
    public static func file(in folder: URL, device: String, date: Date, timeZone: TimeZone = .current,
                            exists: (URL) -> Bool) -> URL
    {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = timeZone
        formatter.dateFormat = "yyyy-MM-dd HH.mm.ss"
        let name = device.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
        let stem = "\(name) \(formatter.string(from: date))"
        var candidate = folder.appending(path: "\(stem).png")
        var number = 2
        while exists(candidate) {
            candidate = folder.appending(path: "\(stem) \(number).png")
            number += 1
        }
        return candidate
    }
}
