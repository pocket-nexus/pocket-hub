import Foundation

/// Where Hub lives on this computer (CONTROL-API.md §1, "Transport").
///
/// The folder is this Hub's alone. `~/Library/Application Support/Pocket Hub`
/// and `~/Library/Logs/Pocket Hub` belong to an earlier, unrelated Pocket Hub
/// and nothing here reads or writes them.
public struct HubPaths: Sendable, Equatable {
    /// `<home>`: `$POCKET_HUB_HOME`, else `~/Library/Application Support/nexus.pocket.hub`.
    public var home: URL

    /// The longest `hubd.sock` path the core accepts; a longer home is refused
    /// by the core, and there is no second location to look in.
    public static let maximumSocketPathLength = 100

    public init(home: URL) {
        self.home = home
    }

    /// The paths for this process's environment.
    public static func current(environment: [String: String] = ProcessInfo.processInfo.environment) -> HubPaths {
        if let custom = environment["POCKET_HUB_HOME"], !custom.isEmpty {
            return HubPaths(home: URL(fileURLWithPath: (custom as NSString).expandingTildeInPath, isDirectory: true))
        }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appending(path: "Library/Application Support")
        return HubPaths(home: support.appending(path: "nexus.pocket.hub", directoryHint: .isDirectory))
    }

    /// `<home>/hubd.sock`, the only place the core listens.
    public func socketPath() -> String { home.appending(path: "hubd.sock").path(percentEncoded: false) }

    /// Whether the socket path fits; when it does not the core refuses the home.
    public var socketPathIsUsable: Bool { socketPath().utf8.count <= Self.maximumSocketPathLength }

    /// `<home>/logs/hubd.log`, which the core writes itself.
    public var coreLog: URL { home.appending(path: "logs/hubd.log") }
}
