import Foundation

/// Splits a byte stream into the lines of the control API: one JSON object per
/// line, each ended by `\n` (CONTROL-API.md §1).
///
/// Reads arrive in arbitrary chunks: one object may span several reads, one
/// read may hold several objects, and a multi-byte UTF-8 character may be cut
/// between two reads. The framer therefore works on bytes and only hands out
/// complete lines; text decoding happens on a whole line, never on a chunk.
public struct LineFramer: Sendable {
    public enum FramingError: Error, Equatable {
        /// A line grew past `maximumLineLength` without a newline. The stream
        /// cannot be resynchronised safely, so the connection must be dropped.
        case lineTooLong(Int)
    }

    /// The largest line accepted. A Device with 64 applets is a few tens of
    /// kilobytes; 8 MiB leaves room for any list the contract can produce.
    public let maximumLineLength: Int
    private var buffer = Data()

    public init(maximumLineLength: Int = 8 << 20) {
        self.maximumLineLength = maximumLineLength
    }

    /// Appends a chunk and returns every line it completed, without their
    /// terminators. A `\r` before the `\n` is dropped too; empty lines are skipped.
    public mutating func append(_ chunk: Data) throws -> [Data] {
        buffer.append(chunk)
        var lines: [Data] = []
        var start = buffer.startIndex
        while let newline = buffer[start...].firstIndex(of: 0x0A) {
            var end = newline
            if end > start, buffer[end - 1] == 0x0D { end -= 1 }
            if end > start { lines.append(Data(buffer[start..<end])) }
            start = newline + 1
        }
        buffer.removeSubrange(buffer.startIndex..<start)
        if buffer.count > maximumLineLength {
            let length = buffer.count
            buffer.removeAll()
            throw FramingError.lineTooLong(length)
        }
        return lines
    }

    /// Bytes received after the last complete line.
    public var pendingByteCount: Int { buffer.count }
}
