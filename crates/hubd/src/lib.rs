//! `pocket-hubd`, Pocket Hub's resident core (docs/CONTROL-API.md): the
//! store of paired devices, the Pocket Link endpoint, the service host
//! (`service_host`, mounted) and the control server on
//! `<home>/hubd.sock`, in one process per home.
//!
//! - [`start`] checks the home, takes it, binds the link port and the
//!   sockets, and starts every part; [`Running::run`] serves until a signal
//!   or `hub.shutdown`, then stops them in order.
//! - `core`: the parts and the events the control API publishes.
//! - `server`: the socket, one task per client, the JSON-RPC envelope.
//! - `handlers`: the methods of §3.
//! - `map`: the contract's shapes from the internal types.
//! - `program`: the newest release of each device family's own program.
//! - `logging`: the size-bounded log file.

mod core;
mod handlers;
pub mod logging;
pub mod map;
mod program;
mod server;

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use crate::core::Core;
use pocket_hub_setup::ShellSource;

/// The version `hub.hello` and `hub.status` give.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How a core is started.
#[derive(Debug, Clone)]
pub struct Options {
    pub home: PathBuf,
    /// The TCP link port; None for 8628, `Some(0)` for an ephemeral port.
    /// A port that is taken stops the start.
    pub link_port: Option<u16>,
    /// Send beacons (link specification §2.2).
    pub beacons: bool,
    /// An existing Bun instead of the one Hub downloads.
    pub bun: Option<PathBuf>,
    /// Where `{"release":"latest"}` comes from instead of Pocket Shell's
    /// GitHub releases (a mirror, a test server, a local build).
    pub shell_source: Option<ShellSource>,
    /// The shortest time between two looks for a family's newest program
    /// release (§2 `Device.program`: 6 hours; less in tests).
    pub program_look_interval: Duration,
    /// `setup.discover` probes these addresses instead of the local subnets.
    pub discover_targets: Option<Vec<Ipv4Addr>>,
    /// `setup.discover` probes this port instead of ftpd's 5000.
    pub discover_port: Option<u16>,
    /// The Hub's name when its home is new; None for the computer's name. A
    /// home that has its identity keeps its name.
    pub name: Option<String>,
    /// How long a second caller waits for a device's mutex (§3: 10 s).
    pub mutex_wait: Duration,
    /// The link's silence limit, and the longer limit and Hub's PING interval
    /// while a state-changing request is open (`pocket_hub_core::Timing`:
    /// 10 s, 30 s, 3 s; shorter in tests).
    pub link_silence: Duration,
    pub link_state_silence: Duration,
    pub link_state_ping: Duration,
    /// Replaces every service manifest's `idleSeconds` (development, tests).
    pub service_idle: Option<Duration>,
    /// `net.http` requests for these hosts go to the given servers, private
    /// addresses allowed for those hosts alone (development, tests; link
    /// specification §12.1 `--resolve`).
    pub net_overrides: Vec<pocket_hub_net::ResolveOverride>,
    /// The applet store's base URL (`https://applets.pocket.nexus`; a
    /// mirror or a test store).
    pub store_url: String,
    /// The key whose applets are official, base64url (the store's official
    /// key; a test store's own).
    pub store_key: String,
}

impl Options {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            link_port: None,
            beacons: true,
            bun: None,
            shell_source: None,
            program_look_interval: Duration::from_secs(6 * 60 * 60),
            discover_targets: None,
            discover_port: None,
            name: None,
            mutex_wait: Duration::from_secs(10),
            link_silence: pocket_hub_core::Timing::default().silence,
            link_state_silence: pocket_hub_core::Timing::default().state_silence,
            link_state_ping: pocket_hub_core::Timing::default().state_ping,
            service_idle: None,
            net_overrides: Vec::new(),
            store_url: pocket_hub_applet_store::DEFAULT_URL.to_owned(),
            store_key: pocket_hub_applet_store::OFFICIAL_KEY.to_owned(),
        }
    }
}

/// Why a core did not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// Another core serves this home: not a failure (§1 "Starting and
    /// stopping the core").
    #[error("A Pocket Hub core already runs for {0}.")]
    AlreadyRunning(PathBuf),
    #[error("{0}")]
    Home(String),
    #[error("The link port {port} is in use by another program, so Pocket Hub cannot start. Quit that program, or start Pocket Hub with --link-port <another port>.")]
    PortInUse { port: u16 },
    #[error("Pocket Hub cannot listen on the link port {port}: {error}.")]
    Port { port: u16, error: std::io::Error },
    #[error("{0}")]
    Other(String),
}

/// A started core.
pub struct Running {
    core: Arc<Core>,
}

impl Running {
    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }

    /// Serves until `hub.shutdown` or `stop`, then shuts down in order:
    /// links closed with `going_away`, services stopped, socket removed.
    pub async fn run(self, stop: impl std::future::Future<Output = ()>) {
        tokio::select! {
            _ = self.core.shutdown_requested() => {}
            _ = stop => {}
        }
        self.core.shutdown().await;
    }
}

/// Starts a core on `options.home`.
pub async fn start(options: Options) -> Result<Running, StartError> {
    Ok(Running { core: core::Core::start(options).await? })
}
