import Foundation

/// A version as SemVer 2.0.0 writes it, ordered by its precedence (§11):
/// major, minor and patch numerically; a version with a pre-release orders
/// below the same version without one, and pre-release identifiers compare
/// one by one, numeric ones numerically and below alphanumeric ones; build
/// metadata is ignored. The app compares the core it finds with the core it
/// carries by this order (CONTROL-API.md §1, "Starting and stopping the core").
public struct SemanticVersion: Comparable, Sendable, CustomStringConvertible {
    public let major: Int
    public let minor: Int
    public let patch: Int
    public let preRelease: [String]

    /// nil for text that is not a SemVer version: `MAJOR.MINOR.PATCH`, numbers
    /// without leading zeros, then an optional `-pre.release` and `+build`.
    public init?(_ text: String) {
        var rest = Substring(text)
        if let plus = rest.firstIndex(of: "+") { rest = rest[..<plus] }
        var preRelease: [String] = []
        if let dash = rest.firstIndex(of: "-") {
            preRelease = rest[rest.index(after: dash)...].split(separator: ".", omittingEmptySubsequences: false).map(String.init)
            guard preRelease.allSatisfy({ identifier in
                !identifier.isEmpty && identifier.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-") }
                    && !(Self.isNumeric(identifier) && identifier.count > 1 && identifier.hasPrefix("0"))
            }) else { return nil }
            rest = rest[..<dash]
        }
        let parts = rest.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 3, parts.allSatisfy({ Self.isNumeric(String($0)) && !($0.count > 1 && $0.hasPrefix("0")) }),
              let major = Int(parts[0]), let minor = Int(parts[1]), let patch = Int(parts[2])
        else { return nil }
        (self.major, self.minor, self.patch, self.preRelease) = (major, minor, patch, preRelease)
    }

    public var description: String {
        "\(major).\(minor).\(patch)" + (preRelease.isEmpty ? "" : "-" + preRelease.joined(separator: "."))
    }

    public static func < (a: SemanticVersion, b: SemanticVersion) -> Bool {
        if (a.major, a.minor, a.patch) != (b.major, b.minor, b.patch) {
            return (a.major, a.minor, a.patch) < (b.major, b.minor, b.patch)
        }
        switch (a.preRelease.isEmpty, b.preRelease.isEmpty) {
        case (true, true), (true, false): return false
        case (false, true): return true
        case (false, false): break
        }
        for (x, y) in zip(a.preRelease, b.preRelease) where x != y {
            switch (Int(x).flatMap { isNumeric(x) ? $0 : nil }, Int(y).flatMap { isNumeric(y) ? $0 : nil }) {
            case let (x?, y?): return x < y
            case (.some, nil): return true
            case (nil, .some): return false
            case (nil, nil): return x < y
            }
        }
        return a.preRelease.count < b.preRelease.count
    }

    public static func == (a: SemanticVersion, b: SemanticVersion) -> Bool {
        (a.major, a.minor, a.patch) == (b.major, b.minor, b.patch) && a.preRelease == b.preRelease
    }

    private static func isNumeric(_ text: String) -> Bool {
        !text.isEmpty && text.allSatisfy { $0.isASCII && $0.isNumber }
    }
}
