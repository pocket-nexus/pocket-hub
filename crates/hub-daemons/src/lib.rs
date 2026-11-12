//! Pocket Hub's daemons (docs/DAEMON-PROTOCOL.md): the computer side of an
//! applet, run by Hub on Bun.
//!
//! - [`BunRuntime`] finds or downloads the pinned Bun that every daemon runs on.
//! - [`Registry`] installs daemon packages under `<home>/daemons` and keeps
//!   each daemon's data folder under `<home>/daemon-data`.
//! - [`Supervisor`] owns the daemon socket and the processes: it starts a
//!   daemon when a device opens its applet's channel, hands the channel
//!   connection the daemon attaches to the relay, and stops the process when it
//!   has been idle.
//!
//! Section numbers `§n` are those of docs/DAEMON-PROTOCOL.md unless they name
//! the link specification (`docs/POCKET-LINK.md` in PocketJS).

#[cfg(not(unix))]
compile_error!("pocket-hub-daemons runs daemons as Unix process groups over a Unix socket; other platforms are not supported yet");

#[cfg(unix)]
pub mod child;
mod archive;
mod channel;
mod config;
mod instance;
mod logs;
mod manifest;
mod registry;
mod runtime;
mod supervisor;
mod util;
mod wire;

pub use channel::{ChannelInfo, ChannelReader, ChannelStream, ChannelWriter, StopSignal};
pub use config::{Config, Timings};
pub use logs::{LogLine, LogStream};
pub use manifest::{Manifest, ManifestError, MANIFEST_FILE};
pub use registry::{DependencyInstall, InstallError, InstallReport, InstalledDaemon, Registry, SDK_LINK, link_sdk};
pub use runtime::{BunRuntime, Progress, RuntimeError, BUN_VERSION};
pub use supervisor::{
    ChannelOffer, DaemonEvent, DaemonState, DaemonStatus, ExitInfo, OpenError, StartError, Supervisor, SupervisorError,
};

/// The version of the control connection's protocol (§4, `daemon.hello`).
pub const PROTOCOL_VERSION: u64 = 1;
