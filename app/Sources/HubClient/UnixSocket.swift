import Darwin
import Foundation

/// A connected Unix domain stream socket.
///
/// Reading happens on a dedicated thread doing blocking `read(2)`, so no
/// actor or the main thread ever waits on the socket; chunks are delivered in
/// order through `chunks`. Writes go through a serial queue, in order, and
/// never on the caller's thread.
public final class UnixSocketStream: @unchecked Sendable {
    // @unchecked: `fd` is immutable; `closed` and the descriptor's lifetime are
    // guarded by `lock`. The reader thread is the only one that closes `fd`.
    public enum SocketError: Error, Equatable, CustomStringConvertible {
        /// Nothing listens at the path (no file, or a stale socket file).
        case nobodyListening(path: String, errno: Int32)
        case pathTooLong(String)
        case system(call: String, errno: Int32)
        case closed

        public var description: String {
            switch self {
            case .nobodyListening(let path, let e): "nothing listens at \(path) (\(String(cString: strerror(e))))"
            case .pathTooLong(let path): "socket path too long: \(path)"
            case .system(let call, let e): "\(call): \(String(cString: strerror(e)))"
            case .closed: "socket closed"
            }
        }
    }

    public let chunks: AsyncStream<Data>
    private let continuation: AsyncStream<Data>.Continuation
    private let fd: Int32
    private let lock = NSLock()
    private var closed = false
    private var shutDown = false
    private let writeQueue = DispatchQueue(label: "nexus.pocket.hub.socket.write")

    /// Wraps a connected descriptor and starts reading it.
    public init(adopting fd: Int32) {
        self.fd = fd
        var noSigPipe: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))
        // A peer that stops reading must not hold a writer forever.
        var timeout = timeval(tv_sec: 10, tv_usec: 0)
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
        (chunks, continuation) = AsyncStream.makeStream(of: Data.self, bufferingPolicy: .unbounded)
        let thread = Thread { [self] in self.readLoop() }
        thread.name = "nexus.pocket.hub.socket.read"
        thread.start()
    }

    /// Connects to the socket at `path`.
    public static func connect(path: String) throws -> UnixSocketStream {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw SocketError.system(call: "socket", errno: errno) }
        var address = try sockaddr_un(path: path)
        let result = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        if result != 0 {
            let error = errno
            Darwin.close(fd)
            if error == ENOENT || error == ECONNREFUSED {
                throw SocketError.nobodyListening(path: path, errno: error)
            }
            throw SocketError.system(call: "connect", errno: error)
        }
        return UnixSocketStream(adopting: fd)
    }

    /// Writes bytes in order; `completion` reports failure (the stream is then shut down).
    public func send(_ data: Data, completion: (@Sendable (Error?) -> Void)? = nil) {
        writeQueue.async { [self] in
            let error = self.writeAll(data)
            if error != nil { self.close() }
            completion?(error)
        }
    }

    /// Stops both directions. The reader thread then ends `chunks` and releases the descriptor.
    public func close() {
        lock.lock()
        defer { lock.unlock() }
        guard !closed, !shutDown else { return }
        shutDown = true
        Darwin.shutdown(fd, SHUT_RDWR)
    }

    private func writeAll(_ data: Data) -> Error? {
        lock.lock()
        defer { lock.unlock() }
        if closed || shutDown { return SocketError.closed }
        return data.withUnsafeBytes { raw -> Error? in
            guard var pointer = raw.baseAddress else { return nil }
            var remaining = raw.count
            while remaining > 0 {
                let n = Darwin.write(fd, pointer, remaining)
                if n < 0 {
                    if errno == EINTR { continue }
                    return SocketError.system(call: "write", errno: errno)
                }
                pointer += n
                remaining -= n
            }
            return nil
        }
    }

    private func readLoop() {
        let size = 64 * 1024
        let buffer = UnsafeMutableRawPointer.allocate(byteCount: size, alignment: 1)
        defer { buffer.deallocate() }
        while true {
            let n = Darwin.read(fd, buffer, size)
            if n > 0 {
                continuation.yield(Data(bytes: buffer, count: n))
            } else if n < 0 && errno == EINTR {
                continue
            } else {
                break
            }
        }
        lock.lock()
        closed = true
        Darwin.close(fd)
        lock.unlock()
        continuation.finish()
    }
}

/// A listening Unix domain socket. Used by the mock core; the app never listens.
public final class UnixSocketListener: @unchecked Sendable {
    // @unchecked: `fd` is immutable and `stopped` is guarded by `lock`.
    public let path: String
    private let fd: Int32
    private let lock = NSLock()
    private var stopped = false
    private let wakePipe: (read: Int32, write: Int32)

    /// Binds and listens at `path`, replacing a stale socket file. The socket
    /// is made 0600 as the contract requires of `hubd.sock`.
    public init(path: String) throws {
        self.path = path
        unlink(path)
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw UnixSocketStream.SocketError.system(call: "socket", errno: errno) }
        var address = try sockaddr_un(path: path)
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0 else {
            let error = errno
            Darwin.close(fd)
            throw UnixSocketStream.SocketError.system(call: "bind", errno: error)
        }
        chmod(path, 0o600)
        guard listen(fd, 16) == 0 else {
            let error = errno
            Darwin.close(fd)
            throw UnixSocketStream.SocketError.system(call: "listen", errno: error)
        }
        var pipeEnds: [Int32] = [0, 0]
        guard pipe(&pipeEnds) == 0 else {
            let error = errno
            Darwin.close(fd)
            throw UnixSocketStream.SocketError.system(call: "pipe", errno: error)
        }
        self.fd = fd
        self.wakePipe = (pipeEnds[0], pipeEnds[1])
    }

    /// Accepts connections on a dedicated thread until `stop()`.
    public func run(onConnection: @escaping @Sendable (UnixSocketStream) -> Void) {
        let thread = Thread { [self] in
            // poll(2) on the socket and a wake pipe: macOS does not reliably wake
            // a thread blocked in accept(2) when another thread shuts the socket.
            var fds = [pollfd(fd: self.fd, events: Int16(POLLIN), revents: 0),
                       pollfd(fd: self.wakePipe.read, events: Int16(POLLIN), revents: 0)]
            while true {
                let ready = poll(&fds, 2, -1)
                if ready < 0 {
                    if errno == EINTR { continue }
                    break
                }
                if fds[1].revents != 0 { break }
                if fds[0].revents & Int16(POLLIN) != 0 {
                    let client = accept(self.fd, nil, nil)
                    if client >= 0 { onConnection(UnixSocketStream(adopting: client)) }
                }
            }
            Darwin.close(self.fd)
            Darwin.close(self.wakePipe.read)
            Darwin.close(self.wakePipe.write)
        }
        thread.name = "nexus.pocket.hub.socket.accept"
        thread.start()
    }

    /// Stops accepting and removes the socket file. Open connections stay open.
    public func stop() {
        lock.lock()
        defer { lock.unlock() }
        guard !stopped else { return }
        stopped = true
        unlink(path)
        var byte: UInt8 = 1
        _ = Darwin.write(wakePipe.write, &byte, 1)
    }
}

extension sockaddr_un {
    /// The longest path a `sockaddr_un` holds on macOS: `sun_path` is 104 bytes
    /// including the terminating NUL.
    public static let maximumPathLength = 103

    init(path: String) throws {
        self.init()
        sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        guard bytes.count <= Self.maximumPathLength else {
            throw UnixSocketStream.SocketError.pathTooLong(path)
        }
        withUnsafeMutableBytes(of: &sun_path) { raw in
            raw.copyBytes(from: bytes)
            raw[bytes.count] = 0
        }
        sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    }
}
