//! The control API of `pocket-hubd` (docs/CONTROL-API.md) in one place, so
//! the server and its clients cannot drift apart:
//!
//! - [`types`]: every object, parameter, result, progress notification,
//!   event and error of the contract, with the exact JSON names.
//! - [`rpc`]: the JSON-RPC 2.0 envelope and the error object.
//! - [`paths`]: Hub's home folder and the socket's path (§1).
//! - [`client`]: a small async client over the socket, for the command line
//!   and the tests.
//!
//! Section numbers `§n` are those of docs/CONTROL-API.md.

pub mod client;
pub mod paths;
pub mod rpc;
pub mod types;

pub use client::{Call, Client, ClientError, Transcript};
pub use rpc::{ErrorData, ErrorKind, RpcError};
pub use types::*;

/// The protocol version a client names in `hub.hello`.
pub const PROTOCOL: u32 = 1;

/// The methods of §3.
pub mod methods {
    pub const HELLO: &str = "hub.hello";
    pub const STATUS: &str = "hub.status";
    pub const SET_NAME: &str = "hub.setName";
    pub const SUBSCRIBE: &str = "hub.subscribe";
    pub const SHUTDOWN: &str = "hub.shutdown";
    pub const DEVICES_LIST: &str = "devices.list";
    pub const DEVICES_GET: &str = "devices.get";
    pub const DEVICES_FORGET: &str = "devices.forget";
    pub const DEVICES_STATUS: &str = "devices.status";
    pub const DEVICES_PUSH: &str = "devices.push";
    pub const DEVICES_LAUNCH: &str = "devices.launch";
    pub const DEVICES_SCREENSHOT: &str = "devices.screenshot";
    pub const DEVICES_LOGS: &str = "devices.logs";
    pub const DEVICES_INSTALL_PROGRAM: &str = "devices.installProgram";
    pub const SETUP_DISCOVER: &str = "setup.discover";
    pub const SETUP_INSTALL: &str = "setup.install";
    pub const SETUP_CARD: &str = "setup.card";
    pub const DAEMONS_LIST: &str = "daemons.list";
    pub const DAEMONS_INSTALL: &str = "daemons.install";
    pub const DAEMONS_REMOVE: &str = "daemons.remove";
    pub const DAEMONS_START: &str = "daemons.start";
    pub const DAEMONS_STOP: &str = "daemons.stop";
    pub const DAEMONS_RESTART: &str = "daemons.restart";
    pub const DAEMONS_LOGS: &str = "daemons.logs";
    pub const DAEMONS_RUNTIME: &str = "daemons.runtime";
    pub const STORE_APPS: &str = "store.apps";
    pub const STORE_INSTALL: &str = "store.install";
    /// The notification that asks Hub to stop a call.
    pub const CANCEL: &str = "cancel";
    /// The notifications Hub sends.
    pub const PROGRESS: &str = "progress";
    pub const EVENT: &str = "event";
}

/// The event topics of §4.
pub mod topics {
    pub const DEVICE_CHANGED: &str = "device.changed";
    pub const DEVICE_REMOVED: &str = "device.removed";
    pub const DEVICE_UNLINKED: &str = "device.unlinked";
    pub const DEVICE_LOG: &str = "device.log";
    pub const DEVICE_SCREENSHOT: &str = "device.screenshot";
    pub const DAEMON_CHANGED: &str = "daemon.changed";
    pub const DAEMON_REMOVED: &str = "daemon.removed";
    pub const DAEMON_LOG: &str = "daemon.log";
    pub const HUB_CHANGED: &str = "hub.changed";
    pub const STORE_CHANGED: &str = "store.changed";
    pub const ALL: [&str; 10] = [DEVICE_CHANGED, DEVICE_REMOVED, DEVICE_UNLINKED, DEVICE_LOG, DEVICE_SCREENSHOT, DAEMON_CHANGED, DAEMON_REMOVED, DAEMON_LOG, HUB_CHANGED, STORE_CHANGED];
}
