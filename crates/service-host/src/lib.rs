//! Pocket Hub's services (docs/SERVICE-PROTOCOL.md): the computer side of an
//! applet, run by Hub on Bun.
//!
//! - [`BunRuntime`] finds or downloads the pinned Bun that every service runs on.
//! - [`Registry`] installs service packages under `<home>/services` and keeps
//!   each service's data folder under `<home>/service-data`.
//! - [`Supervisor`] owns the service socket and the processes: it starts a
//!   service when a device opens its applet's channel, hands the channel
//!   connection the service attaches to the relay, and stops the process when it
//!   has been idle.
//!
//! Section numbers `§n` are those of docs/SERVICE-PROTOCOL.md unless they name
//! the link specification (`docs/POCKET-LINK.md` in PocketJS).

#[cfg(not(unix))]
compile_error!("the service host runs services as Unix process groups over a Unix socket; other platforms are not supported yet");

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
pub use registry::{DependencyInstall, InstallError, InstallReport, InstalledService, Registry, SDK_LINK, link_sdk};
pub use runtime::{BunRuntime, Progress, RuntimeError, BUN_VERSION};
pub use supervisor::{
    ChannelOffer, ServiceEvent, ServiceState, ServiceStatus, ExitInfo, OpenError, StartError, Supervisor, SupervisorError,
};

/// The version of the control connection's protocol (§4, `service.hello`).
pub const PROTOCOL_VERSION: u64 = 2;
