//! Where the services live and how long Hub waits for them.

use std::path::PathBuf;
use std::time::Duration;

/// The supervisor's configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Hub's home folder: `runtimes/`, `services/` and `service-data/` are made
    /// in it. It should be mode 0700, as Hub's own files are.
    pub home: PathBuf,
    /// The service socket (§2, `POCKET_SERVICE_SOCKET`). Keep it short:
    /// macOS takes at most 103 bytes of path for a Unix socket.
    pub socket: PathBuf,
    /// An existing Bun to use instead of the one Hub downloads (development
    /// and tests use the system `bun`).
    pub bun_path: Option<PathBuf>,
    /// The folder of Hub's own copy of the service SDK (`@pocketjs/service`:
    /// its `package.json` and sources). When set, a package whose
    /// `node_modules` holds no `@pocketjs/service` of its own gets a link to
    /// it at install and again before each start (§1 "Hub provides the SDK").
    pub sdk: Option<PathBuf>,
    pub timings: Timings,
}

impl Config {
    pub fn new(home: impl Into<PathBuf>, socket: impl Into<PathBuf>) -> Self {
        Self { home: home.into(), socket: socket.into(), bun_path: None, sdk: None, timings: Timings::default() }
    }
}

/// Every wait of the supervisor. The defaults keep a start on demand inside
/// the device's LINK_CHANNEL_OPEN_MS (20 s, link specification §7.1); tests
/// shorten them.
#[derive(Debug, Clone)]
pub struct Timings {
    /// The whole of `open_channel`, after which it answers `Busy`. Below the
    /// 20 s the relay has, so the relay's own answer is never late.
    pub open_budget: Duration,
    /// From spawning the process to its `service.hello`.
    pub start: Duration,
    /// From `channel.offer` to the service's `channel.attach` (§4: 10 s).
    pub attach: Duration,
    /// For the first frame of any connection on the socket.
    pub first_frame: Duration,
    /// From `service.stop` to `SIGTERM` (§4: 5 s).
    pub stop_grace: Duration,
    /// From `SIGTERM` to `SIGKILL` (§4: 5 s).
    pub term_grace: Duration,
    /// Replaces every manifest's `idleSeconds` when set (tests).
    pub idle_override: Option<Duration>,
    /// A process that ends on its own sooner than this after its start is a
    /// failed start, which holds off the next start (`failure_backoff`).
    pub crash_window: Duration,
    /// After the first failed start in a row, no start for this long; the wait
    /// doubles at each further one up to `failure_backoff_max`. The same rule
    /// as the device's after `busy` (link specification §7.1).
    pub failure_backoff: Duration,
    pub failure_backoff_max: Duration,
    /// `bun install --frozen-lockfile` of a package.
    pub install: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            open_budget: Duration::from_secs(19),
            start: Duration::from_secs(8),
            attach: Duration::from_secs(10),
            first_frame: Duration::from_secs(10),
            stop_grace: Duration::from_secs(5),
            term_grace: Duration::from_secs(5),
            idle_override: None,
            crash_window: Duration::from_secs(10),
            failure_backoff: Duration::from_secs(2),
            failure_backoff_max: Duration::from_secs(60),
            install: Duration::from_secs(300),
        }
    }
}
