import Foundation

/// A failed operation, as Hub reports it: `error.code` 1 with `error.data.kind`
/// (CONTROL-API.md §1, "Errors").
public struct HubOperationError: Error, Sendable, Equatable {
    public enum Kind: Sendable, Hashable {
        case notFound
        case notLinked
        case busy
        case link
        case cancelled
        case setup
        case invalid
        case daemon
        /// The release of a device's program could not be used.
        case release
        case store
        /// A pairing by a code did not complete.
        case pairing
        case `internal`
        /// A kind this client does not know; treated like `internal`.
        case other(String)

        public init(rawValue: String) {
            switch rawValue {
            case "not_found": self = .notFound
            case "not_linked": self = .notLinked
            case "busy": self = .busy
            case "link": self = .link
            case "cancelled": self = .cancelled
            case "setup": self = .setup
            case "invalid": self = .invalid
            case "daemon": self = .daemon
            case "release": self = .release
            case "store": self = .store
            case "pairing": self = .pairing
            case "internal": self = .internal
            default: self = .other(rawValue)
            }
        }

        public var rawValue: String {
            switch self {
            case .notFound: "not_found"
            case .notLinked: "not_linked"
            case .busy: "busy"
            case .link: "link"
            case .cancelled: "cancelled"
            case .setup: "setup"
            case .invalid: "invalid"
            case .daemon: "daemon"
            case .release: "release"
            case .store: "store"
            case .pairing: "pairing"
            case .internal: "internal"
            case .other(let raw): raw
            }
        }
    }

    public var kind: Kind
    /// One sentence for the owner, as Hub wrote it (not localized by the client).
    public var message: String
    /// `busy`: who holds the device.
    public var holder: MutexHolder?
    /// `link`: the link's error code, or `closed`, `timeout`.
    public var linkCode: String?
    /// `link`: the phase the device gave, when it gave one.
    public var phase: String?
    /// `setup`: the `stage` the setup failed in.
    public var step: String?
    /// `setup`: `unreachable`, `companions_full`, `release_unavailable`, `storage` or `card`;
    /// `release`: `unreachable`, `not_found` or `rejected`;
    /// `store`: `unreachable` or `rejected`;
    /// `pairing`: `wrong_code`, `bad_proof` or `gone`.
    public var reason: String?
    /// `setup`: the failing step in the setup's own words, for a log.
    public var detail: String?
    /// `setup`: the device a key was stored for when its readback did not return
    /// (then `confirmed` is false); `daemon`: the daemon's id.
    public var device: String?
    /// `setup`: false when a key was stored and its final readback did not return.
    public var confirmed: Bool?
    /// `daemon`: the daemon concerned.
    public var daemonId: String?
    /// The whole `error.data`, for members this client does not model.
    public var data: JSONValue?

    public init(kind: Kind, message: String, holder: MutexHolder? = nil, linkCode: String? = nil,
                phase: String? = nil, step: String? = nil, reason: String? = nil, detail: String? = nil,
                device: String? = nil, confirmed: Bool? = nil, daemonId: String? = nil, data: JSONValue? = nil)
    {
        self.kind = kind
        self.message = message
        self.holder = holder
        self.linkCode = linkCode
        self.phase = phase
        self.step = step
        self.reason = reason
        self.detail = detail
        self.device = device
        self.confirmed = confirmed
        self.daemonId = daemonId
        self.data = data
    }
}

/// Every way a call to Hub can fail.
public enum HubError: Error, Sendable, Equatable {
    /// Hub ran the call and the operation failed (`error.code` 1).
    case operation(HubOperationError)
    /// Hub refused the call as malformed (`-32600`, `-32601`, `-32602`) or
    /// answered with another JSON-RPC code. A client bug or a version mismatch.
    case rpc(code: Int, message: String)
    /// No connection to Hub right now.
    case notConnected
    /// The connection closed before the answer arrived. The operation may or
    /// may not have happened.
    case disconnected
    /// Hub answered something this client cannot read.
    case invalidResponse(String)
    /// Hub speaks another protocol version.
    case incompatibleProtocol(hub: Int, client: Int)

    /// The operation error's kind, when this is one.
    public var kind: HubOperationError.Kind? {
        if case .operation(let error) = self { return error.kind }
        return nil
    }

    public var isCancelled: Bool { kind == .cancelled }

    /// Builds the error for a JSON-RPC `error` member.
    init(rpcError: JSONValue) {
        let code = rpcError["code"]?.intValue ?? 0
        let message = rpcError["message"]?.stringValue ?? ""
        guard code == 1 else {
            self = .rpc(code: code, message: message)
            return
        }
        let data = rpcError["data"]
        let kind = data?["kind"]?.stringValue.map(HubOperationError.Kind.init(rawValue:)) ?? .internal
        let holder = data?["holder"].flatMap { try? $0.decode(MutexHolder.self) }
        self = .operation(HubOperationError(
            kind: kind,
            message: message,
            holder: holder,
            linkCode: data?["code"]?.stringValue,
            phase: data?["phase"]?.stringValue,
            step: data?["step"]?.stringValue,
            reason: data?["reason"]?.stringValue,
            detail: data?["detail"]?.stringValue,
            device: data?["device"]?.stringValue,
            confirmed: data?["confirmed"]?.boolValue,
            daemonId: kind == .daemon ? data?["id"]?.stringValue : nil,
            data: data))
    }
}
