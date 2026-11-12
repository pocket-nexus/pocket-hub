import Darwin
import Foundation

/// A core this process started, watched until it exits.
///
/// A core that cannot start writes one line to standard error and exits with a
/// status other than 0 (CONTROL-API.md §1, "Starting and stopping the core").
/// Its standard error goes to a file of this process's temporary folder rather
/// than a pipe: the core outlives the app, and a pipe whose reader has gone
/// would kill it with SIGPIPE at its next write.
public final class LaunchedCore: @unchecked Sendable {
    // @unchecked: `status` is guarded by `lock`; the rest is immutable.
    public let pid: Int32
    public let executable: String
    public let standardError: URL
    private let lock = NSLock()
    private var status: Int32?

    init(pid: Int32, executable: String, standardError: URL) {
        self.pid = pid
        self.executable = executable
        self.standardError = standardError
    }

    /// The exit status once the core has exited; nil while it runs.
    public var exitStatus: Int32? { lock.withLock { status } }

    func exited(_ status: Int32) {
        lock.withLock { self.status = status }
    }

    /// The core's last line on standard error, without its `pocket-hubd: ` prefix.
    public func failureLine() -> String? {
        guard let text = try? String(contentsOf: standardError, encoding: .utf8) else { return nil }
        guard let last = text.split(whereSeparator: \.isNewline).last(where: { !$0.trimmingCharacters(in: .whitespaces).isEmpty })
        else { return nil }
        var line = String(last).trimmingCharacters(in: .whitespaces)
        if line.hasPrefix("pocket-hubd: ") { line.removeFirst("pocket-hubd: ".count) }
        return line
    }

    /// Removes the standard error file (the core keeps writing to it unlinked).
    public func discardOutput() {
        try? FileManager.default.removeItem(at: standardError)
    }
}

/// Finds and starts `pocket-hubd` when nothing answers on the socket.
///
/// The core runs with no argument and stays in the foreground of whatever
/// started it, so it is started detached: its own session, no terminal, not
/// killed by a signal sent to the app's process group. It keeps running when
/// the app quits. It writes its own log to `<home>/logs/hubd.log`.
public struct CoreLauncher: Sendable {
    public enum Outcome: Sendable {
        case started(LaunchedCore)
        case notFound
        case failed(String)
    }

    /// The core's executable; nil when this process has none to start.
    public var executable: @Sendable () -> URL?
    public var arguments: [String]
    public var environment: [String: String]

    public init(executable: @escaping @Sendable () -> URL?, arguments: [String] = [],
                environment: [String: String] = ProcessInfo.processInfo.environment)
    {
        self.executable = executable
        self.arguments = arguments
        self.environment = environment
    }

    /// The app's core: `$POCKET_HUBD` when it is set, else the app bundle's
    /// `Contents/Helpers/pocket-hubd`, as the command line takes `$POCKET_HUBD`
    /// or the `pocket-hubd` beside it. One place each, nothing searched: a
    /// `$POCKET_HUBD` that names no program, or a bundle without its helper,
    /// is reported as a missing core rather than replaced by another one.
    /// The core reads its own settings from the environment
    /// (`POCKET_HUB_HOME`, `POCKET_HUB_LINK_PORT`, …), passed on as is.
    public static func standard(bundle: Bundle = .main,
                                environment: [String: String] = ProcessInfo.processInfo.environment) -> CoreLauncher
    {
        let bundleURL = bundle.bundleURL
        return CoreLauncher(executable: {
            if let explicit = environment["POCKET_HUBD"], !explicit.isEmpty {
                return URL(fileURLWithPath: explicit)
            }
            guard bundleURL.pathExtension == "app" else { return nil }
            return bundleURL.appending(path: "Contents/Helpers/pocket-hubd")
        }, environment: environment)
    }

    /// The core's executable when it is there and can be run.
    public func locate() -> URL? {
        executable().flatMap { FileManager.default.isExecutableFile(atPath: $0.path) ? $0 : nil }
    }

    /// Starts the core. Does not wait for it to listen; the caller retries the
    /// socket and watches `LaunchedCore.exitStatus`.
    public func launch() -> Outcome {
        guard let executable = locate() else { return .notFound }
        let path = executable.path
        let errors = FileManager.default.temporaryDirectory
            .appending(path: "nexus.pocket.hub-core-\(UUID().uuidString.prefix(8)).stderr")

        var attributes = posix_spawnattr_t(nil as OpaquePointer?)
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID))

        var actions = posix_spawn_file_actions_t(nil as OpaquePointer?)
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 1, "/dev/null", O_WRONLY, 0)
        posix_spawn_file_actions_addopen(&actions, 2, errors.path, O_WRONLY | O_CREAT | O_TRUNC, 0o600)

        let argv = [path] + arguments
        let envp = environment.map { "\($0.key)=\($0.value)" }
        var pid: pid_t = 0
        let status = withCStrings(argv) { argvPointers in
            withCStrings(envp) { envPointers in
                posix_spawn(&pid, path, &actions, &attributes, argvPointers, envPointers)
            }
        }
        guard status == 0 else { return .failed(String(cString: strerror(status))) }
        let child = pid
        let core = LaunchedCore(pid: child, executable: path, standardError: errors)
        // Reap the core when it exits (no zombie) and record how it ended.
        Thread.detachNewThread {
            var exitStatus: Int32 = 0
            while waitpid(child, &exitStatus, 0) < 0 && errno == EINTR {}
            let code: Int32 = (exitStatus & 0x7f) == 0 ? (exitStatus >> 8) & 0xff : 128 + (exitStatus & 0x7f)
            core.exited(code)
        }
        return .started(core)
    }
}

private func withCStrings<R>(_ strings: [String], _ body: (UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> R) -> R {
    var pointers = strings.map { strdup($0) } + [nil]
    defer { pointers.forEach { free($0) } }
    return pointers.withUnsafeMutableBufferPointer { body($0.baseAddress!) }
}
