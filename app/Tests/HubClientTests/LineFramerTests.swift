import Foundation
import Testing
@testable import HubClient

@Suite("Line framing")
struct LineFramerTests {
    @Test func oneObjectSplitAcrossReads() throws {
        var framer = LineFramer()
        let line = Data(#"{"jsonrpc":"2.0","id":1,"result":{}}"#.utf8) + Data([0x0A])
        var out: [Data] = []
        for byte in line { out += try framer.append(Data([byte])) }
        #expect(out == [line.dropLast()])
        #expect(framer.pendingByteCount == 0)
    }

    @Test func severalObjectsInOneRead() throws {
        var framer = LineFramer()
        let chunk = Data("{\"a\":1}\n{\"b\":2}\n{\"c\":".utf8)
        let lines = try framer.append(chunk)
        #expect(lines.map { String(decoding: $0, as: UTF8.self) } == [#"{"a":1}"#, #"{"b":2}"#])
        #expect(framer.pendingByteCount == 5)
        let rest = try framer.append(Data("3}\n".utf8))
        #expect(rest.map { String(decoding: $0, as: UTF8.self) } == [#"{"c":3}"#])
    }

    @Test func utf8CharacterCutBetweenReads() throws {
        // "掌上设备" is 12 bytes; cut every character in the middle.
        let text = #"{"name":"掌上设备 🎮"}"#
        let bytes = Data(text.utf8) + Data([0x0A])
        for cut in 1..<bytes.count {
            var framer = LineFramer()
            var lines = try framer.append(bytes.prefix(cut))
            lines += try framer.append(bytes.suffix(from: cut))
            #expect(lines.count == 1)
            #expect(String(data: lines[0], encoding: .utf8) == text)
        }
    }

    @Test func crlfAndEmptyLines() throws {
        var framer = LineFramer()
        let lines = try framer.append(Data("\n{\"a\":1}\r\n\n".utf8))
        #expect(lines.map { String(decoding: $0, as: UTF8.self) } == [#"{"a":1}"#])
    }

    @Test func lineTooLongIsAnError() {
        var framer = LineFramer(maximumLineLength: 16)
        #expect(throws: LineFramer.FramingError.self) {
            _ = try framer.append(Data(repeating: 0x41, count: 17))
        }
    }

    @Test func encodedLinesHaveNoLineBreak() throws {
        let value: JSONValue = ["text": "two\nlines\r\u{2028}", "n": 1_700_000_000_123]
        let line = try value.encodedLine()
        #expect(!line.contains(0x0A))
        #expect(!line.contains(0x0D))
        #expect(String(decoding: line, as: UTF8.self).contains("1700000000123"))
        let back = try JSONDecoder.hub.decode(JSONValue.self, from: line)
        #expect(back == value)
    }
}
