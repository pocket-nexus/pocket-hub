import Foundation

/// A bounded log: the newest `capacity` entries, each with a stable id for lists.
public struct LogBuffer<Line: Sendable & Hashable>: Sendable {
    public struct Entry: Identifiable, Sendable, Hashable {
        public let id: Int
        public let line: Line
        public let received: Date
    }

    public let capacity: Int
    public private(set) var entries: [Entry] = []
    private var nextId = 0

    public init(capacity: Int = 2000) {
        self.capacity = capacity
    }

    public mutating func append(_ line: Line, received: Date = Date()) {
        entries.append(Entry(id: nextId, line: line, received: received))
        nextId += 1
        if entries.count > capacity { entries.removeFirst(entries.count - capacity) }
    }

    public mutating func replace(with lines: [Line]) {
        entries.removeAll()
        for line in lines { append(line) }
    }

    public mutating func clear() {
        entries.removeAll()
    }

    public var isEmpty: Bool { entries.isEmpty }
}
