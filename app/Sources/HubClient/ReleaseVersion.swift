/// A version of Pocket Hub as its releases are numbered: three numbers
/// (`0.2.1`), the same for the app, the core and the command line. Ordered
/// part by part. Any other form (`0.1.0-mock`, a build of something else) is
/// no release: it parses to nil, so it is never older or newer than one
/// (CONTROL-API.md §1, "Starting and stopping the core").
public struct ReleaseVersion: Comparable, Sendable, CustomStringConvertible {
    public let parts: [Int]

    public init?(_ text: String) {
        let pieces = text.split(separator: ".", omittingEmptySubsequences: false)
        guard pieces.count == 3,
              pieces.allSatisfy({ !$0.isEmpty && $0.allSatisfy { $0.isASCII && $0.isNumber } })
        else { return nil }
        let parts = pieces.compactMap { Int($0) }
        guard parts.count == 3 else { return nil }
        self.parts = parts
    }

    public static func < (lhs: ReleaseVersion, rhs: ReleaseVersion) -> Bool {
        lhs.parts.lexicographicallyPrecedes(rhs.parts)
    }

    public var description: String { parts.map(String.init).joined(separator: ".") }
}
