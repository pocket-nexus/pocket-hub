import Foundation
import HubClient

/// A link that opens the app on one thing (docs/DEEP-LINKS.md §1).
///
/// Parsing is all this type does. A link comes from outside Hub (a web page),
/// so what it may name is exactly the specification's grammar and nothing is
/// taken from it but an id to compare with Hub's own catalog.
public enum DeepLink: Equatable, Sendable {
    /// `pocket-hub://store/app/<id>`: one applet of the store.
    case storeApp(id: String)

    /// The scheme the app is registered for (`CFBundleURLTypes`).
    public static let scheme = "pocket-hub"

    /// nil for anything that is not a link of §1.
    public init?(_ url: URL) {
        // The scheme and the host compare without case; the path does not.
        guard let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
              parts.scheme?.lowercased() == Self.scheme,
              parts.host?.lowercased() == "store",
              parts.user == nil, parts.password == nil, parts.port == nil
        else { return nil }
        // The path as it was written: an id needs no percent-encoding, so an
        // encoded "/" or "." is not another way to spell one.
        let segments = parts.percentEncodedPath.split(separator: "/", omittingEmptySubsequences: false)
        guard segments.count == 3, segments[0].isEmpty, segments[1] == "app" else { return nil }
        // The store's rule for a published id (§1).
        guard segments[2].wholeMatch(of: #/[a-z0-9]+(\.[a-z0-9][a-z0-9-]*){2,5}/#) != nil else { return nil }
        // A query and a fragment are reserved and ignored.
        self = .storeApp(id: String(segments[2]))
    }
}

extension StoreApp {
    /// The applet has a variant for the device's target, as Hub offers a
    /// device its list (CONTROL-API.md §3 "Store"). A device that has never
    /// linked has no known target and runs nothing yet.
    public func runs(on device: Device) -> Bool {
        device.hasKnownTarget && targets.contains(device.target)
    }
}
