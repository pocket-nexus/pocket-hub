import Foundation

/// The events of CONTROL-API.md §4, delivered after `hub.subscribe`.
public enum HubEvent: Sendable, Hashable {
    case deviceChanged(Device)
    case deviceRemoved(id: String)
    case deviceUnlinked(DeviceUnlinked)
    case deviceLog(DeviceLogLine)
    case deviceScreenshot(DeviceScreenshot)
    /// The requests to be paired that wait for a code: the whole list.
    case pairingChanged([PairRequest])
    case serviceChanged(Service)
    case serviceRemoved(id: String, release: String?)
    case serviceLog(id: String, release: String?, line: ServiceLogLine)
    case hubChanged(HubIdentity)
    /// The catalog Hub holds, or what it has cached of it, changed: call `store.apps` again.
    case storeChanged(fetchedAt: EpochMilliseconds)
    /// A topic this client does not know, kept whole.
    case unknown(topic: String, params: JSONValue)

    public enum Topic: String, Sendable, CaseIterable {
        case deviceChanged = "device.changed"
        case deviceRemoved = "device.removed"
        case deviceUnlinked = "device.unlinked"
        case deviceLog = "device.log"
        case deviceScreenshot = "device.screenshot"
        case pairingChanged = "pairing.changed"
        case serviceChanged = "service.changed"
        case serviceRemoved = "service.removed"
        case serviceLog = "service.log"
        case hubChanged = "hub.changed"
        case storeChanged = "store.changed"
    }

    /// Decodes the `params` of an `event` notification: `{topic, …members}`.
    public init(params: JSONValue) throws {
        guard let topic = params["topic"]?.stringValue else {
            throw HubError.invalidResponse("event without a topic")
        }
        func member<T: Decodable>(_ name: String, _ type: T.Type = T.self) throws -> T {
            guard let value = params[name] else {
                throw HubError.invalidResponse("event \(topic) without \(name)")
            }
            return try value.decode(T.self)
        }
        switch Topic(rawValue: topic) {
        case .deviceChanged: self = .deviceChanged(try member("device"))
        case .deviceRemoved: self = .deviceRemoved(id: try member("id"))
        case .deviceUnlinked: self = .deviceUnlinked(try params.decode())
        case .deviceLog: self = .deviceLog(try params.decode())
        case .deviceScreenshot: self = .deviceScreenshot(try params.decode())
        case .pairingChanged: self = .pairingChanged(try member("requests"))
        case .serviceChanged: self = .serviceChanged(try member("service"))
        case .serviceRemoved: self = .serviceRemoved(id: try member("id"), release: params["release"]?.stringValue)
        case .serviceLog: self = .serviceLog(id: try member("id"), release: params["release"]?.stringValue, line: try params.decode())
        case .hubChanged: self = .hubChanged(try member("hub"))
        case .storeChanged: self = .storeChanged(fetchedAt: try member("fetchedAt"))
        case nil: self = .unknown(topic: topic, params: params)
        }
    }

    /// The topic family, used to order events against a resync snapshot.
    var family: Family {
        switch self {
        case .deviceChanged, .deviceRemoved, .deviceUnlinked, .deviceLog, .deviceScreenshot: .devices
        case .pairingChanged: .pairing
        case .serviceChanged, .serviceRemoved, .serviceLog: .services
        case .hubChanged: .hub
        // A hint to call store.apps again; it carries no state to roll back.
        case .storeChanged, .unknown: .other
        }
    }

    enum Family { case devices, pairing, services, hub, other }
}

/// `device.unlinked`: the link ended. `code` is the close code's name (link
/// specification §4.8: `going_away`, `silence`, `forgotten`, …), or `lost`
/// when the connection ended without a CLOSE.
public struct DeviceUnlinked: Codable, Sendable, Hashable {
    public var id: String
    public var code: String
    public var reason: String?

    public init(id: String, code: String, reason: String?) {
        self.id = id
        self.code = code
        self.reason = reason
    }
}

/// `device.log`: one line of the device's log while logs are on.
public struct DeviceLogLine: Codable, Sendable, Hashable {
    public var id: String
    /// `debug`, `info`, `warn`, `error`.
    public var level: String
    /// `host` or `guest`.
    public var source: String
    public var text: String
    public var frame: Int
    /// Lines the device dropped before this one.
    public var dropped: Int?

    public init(id: String, level: String, source: String, text: String, frame: Int, dropped: Int? = nil) {
        self.id = id
        self.level = level
        self.source = source
        self.text = text
        self.frame = frame
        self.dropped = dropped
    }
}

/// `device.screenshot`: the device sent a screenshot unasked.
public struct DeviceScreenshot: Codable, Sendable, Hashable {
    public var id: String
    public var path: String
    public var width: Int
    public var height: Int
    public var frame: Int

    public var result: ScreenshotResult { ScreenshotResult(path: path, width: width, height: height, frame: frame) }
}
