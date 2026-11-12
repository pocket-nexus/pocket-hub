//! Pocket Hub's Pocket Link endpoint (docs/POCKET-LINK.md in PocketJS, §12.3).
//!
//! - `carrier`: the byte streams devices reach Hub on (TCP in phase one).
//! - `beacon`: Hub's UDP beacons (§2.2).
//! - `endpoint`: the accept loops, the handshake (§3, §4.1), one link per
//!   device (§2.4), and the API: `Endpoint`, its snapshots and events.
//! - `conn`: one link after `ready`: the write queue, requests, events,
//!   streams and credit, keepalive and closing (§4 to §8).
//! - `relay`: applet channels joined to daemons (§7, docs/DAEMON-PROTOCOL.md §5).
//! - `ops`: the runtime operations as methods of a `DeviceHandle` (§6.2).
//! - `lock`: the device mutex (§8.3).
//! - `apps`, `screenshot`, `reading`: `runtime.apps` paging, the screenshot
//!   PNG, and the four steps of reading an event (§5.3).

pub mod api;
pub mod apps;
pub mod beacon;
pub mod carrier;
pub mod config;
mod conn;
pub mod endpoint;
pub mod lock;
pub mod ops;
pub mod reading;
mod relay;
pub mod screenshot;

pub use api::{
    BoxFuture, CALL_HELD_BYTES, CallBody, CallCancel, CallSink, CancelCause, ChannelHost, ChannelInfo, ChannelOffer, ChannelOpenError, ChannelStream, CloseInfo, DeviceCall, DeviceRequests,
    DeviceSnapshot, ErrorReply, HubEvent, LaunchTarget, LinkInfo, OpError, SinkError,
};
pub use apps::InstalledApp;
pub use carrier::{CarrierKind, CarrierListener, CarrierStream, TcpCarrier};
pub use config::{EndpointConfig, Timing};
pub use endpoint::{DeviceNotifier, Endpoint, EndpointBuilder};
pub use ops::DeviceHandle;
pub use screenshot::Screenshot;
