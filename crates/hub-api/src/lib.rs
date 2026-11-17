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

/// The protocol version a client names in `hub.hello` (CONTROL-API.md §1,
/// Hello). 1 was Pocket Hub 0.2.1's and earlier, before services took their
/// names; the core refuses it like any other version.
pub const PROTOCOL: u32 = 2;

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
    pub const DEVICES_REMOVE: &str = "devices.remove";
    pub const DEVICES_SCREENSHOT: &str = "devices.screenshot";
    pub const DEVICES_LOGS: &str = "devices.logs";
    pub const DEVICES_INSTALL_PROGRAM: &str = "devices.installProgram";
    pub const SETUP_DISCOVER: &str = "setup.discover";
    pub const SETUP_INSTALL: &str = "setup.install";
    pub const SETUP_CARD: &str = "setup.card";
    pub const PAIRING_LIST: &str = "pairing.list";
    pub const PAIRING_ANSWER: &str = "pairing.answer";
    pub const PAIRING_DECLINE: &str = "pairing.decline";
    pub const SERVICES_LIST: &str = "services.list";
    pub const SERVICES_INSTALL: &str = "services.install";
    pub const SERVICES_REMOVE: &str = "services.remove";
    pub const SERVICES_START: &str = "services.start";
    pub const SERVICES_STOP: &str = "services.stop";
    pub const SERVICES_RESTART: &str = "services.restart";
    pub const SERVICES_LOGS: &str = "services.logs";
    pub const SERVICES_RUNTIME: &str = "services.runtime";
    pub const STORE_APPS: &str = "store.apps";
    pub const STORE_DETAILS: &str = "store.details";
    pub const STORE_SAMPLE: &str = "store.sample";
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
    pub const PAIRING_CHANGED: &str = "pairing.changed";
    pub const SERVICE_CHANGED: &str = "service.changed";
    pub const SERVICE_REMOVED: &str = "service.removed";
    pub const SERVICE_LOG: &str = "service.log";
    pub const HUB_CHANGED: &str = "hub.changed";
    pub const STORE_CHANGED: &str = "store.changed";
    pub const ALL: [&str; 11] =
        [DEVICE_CHANGED, DEVICE_REMOVED, DEVICE_UNLINKED, DEVICE_LOG, DEVICE_SCREENSHOT, PAIRING_CHANGED, SERVICE_CHANGED, SERVICE_REMOVED, SERVICE_LOG, HUB_CHANGED, STORE_CHANGED];
}
