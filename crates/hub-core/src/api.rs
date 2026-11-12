//! What the rest of Hub sees of the endpoint: snapshots, events, the error of
//! a runtime operation, and the two traits other crates implement (the
//! daemon supervisor's `ChannelHost`, and `DeviceRequests` for the device's
//! own system requests).

use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::apps::InstalledApp;
use crate::carrier::CarrierKind;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ---------------------------------------------------------------------------
// Applet channels toward daemons (§7, docs/DAEMON-PROTOCOL.md §5)
// ---------------------------------------------------------------------------

/// A device opened an applet's channel; the supervisor finds (or starts) the
/// applet's daemon and hands back one local connection for this channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelOffer {
    /// The device id, authenticated by the link's handshake.
    pub device: String,
    pub device_name: String,
    pub model: String,
    pub target: String,
    /// The applet id, asserted by the device's runtime (§7.1).
    pub app: String,
    /// 16 hex: the running package's footer hash.
    pub hash: String,
    /// The channel's generation (§7.4), already on disk.
    pub generation: u32,
    /// The device link's payload limit, which is the channel connection's.
    pub payload_limit: u32,
}

/// The daemon's channel connection. The supervisor has read the daemon's
/// `channel.attach`, matched its ticket, and read nothing else; the relay
/// writes `channel.ready` first and then carries the channel's frames.
pub struct ChannelStream {
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    /// Resolves when the supervisor stops the daemon with this channel open
    /// (the owner's stop, an upgrade, Hub quitting): the relay then sends the
    /// daemon `channel.close` `stop` and tells the device `daemon`
    /// (docs/DAEMON-PROTOCOL.md §4).
    pub stop: Option<BoxFuture<'static, ()>>,
}

/// Why no channel connection came. Each maps to the `channel.open` error code
/// of §7.1 that sets the device's retry rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChannelOpenError {
    /// `no_daemon`: no daemon is installed for this applet id.
    #[error("no daemon serves this applet")]
    NoDaemon,
    /// `busy`: the daemon exists but could not start or attach in time.
    #[error("{0}")]
    Busy(String),
    /// `rejected`: Hub's policy refuses the applet id.
    #[error("{0}")]
    Rejected(String),
}

/// The daemon supervisor's side of a channel. `open` is called after the
/// generation is stored; Hub answers the device `busy` when it has not
/// returned within LINK_CHANNEL_OPEN_MS, and **drops the future** when the
/// device cancels the open or the link ends: dropping it is how the
/// supervisor learns that the open is no longer wanted. A stream that comes
/// after the answer is dropped.
pub trait ChannelHost: Send + Sync + 'static {
    fn open(&self, offer: ChannelOffer) -> BoxFuture<'static, Result<ChannelStream, ChannelOpenError>>;
}

// ---------------------------------------------------------------------------
// The device's own system requests (§6.3 to §6.5)
// ---------------------------------------------------------------------------

/// An error `REPLY` (§5.2): `{code, message}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ErrorReply {
    pub code: String,
    pub message: String,
}

impl ErrorReply {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into() }
    }
}

/// One system request of the device, as a `DeviceRequests` handler sees it.
pub struct DeviceCall {
    pub device: String,
    /// The connection the request came on, unique for the endpoint's life:
    /// state a provider keeps per connection (the catalog snapshot of §6.4)
    /// is keyed by it, so a new connection of the same device starts clean.
    pub conn: u64,
    pub target: String,
    pub t: String,
    /// The request's JSON object, `t` included.
    pub object: Map<String, Value>,
    /// The connection's payload limit (§4.4).
    pub payload_limit: u32,
    /// Progress events and response bytes on the request's id. The head step
    /// holds: bytes written before a progress event are refused.
    pub sink: CallSink,
    /// The request's own stream toward Hub (§5.6): nothing is granted until
    /// the handler takes it with the length the request declares.
    pub body: CallBody,
    /// Fires when the device sends `CANCEL`, the link ends, or the group
    /// leaves use (§5.4, §6.1). Hub has answered the device by then.
    pub cancel: CallCancel,
}

/// Why Hub ended a device request before its handler answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelCause {
    /// The device sent `CANCEL`; Hub answered `cancelled` (§5.4).
    Cancelled,
    /// The link ended or was replaced (§4.8, §2.4).
    Closed,
    /// The group left use; Hub answered `unsupported` (§6.1).
    Withdrawn,
}

impl CancelCause {
    /// The code a handler reports on its own side for the request.
    pub fn code(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Closed => "closed",
            Self::Withdrawn => "unsupported",
        }
    }
}

/// The cancellation signal of one device request.
///
/// **Hub answers the device itself and frees the request's place in its
/// group's bound at once** (§5.4: the responder stops and answers; §9: the
/// bound counts open requests), so a handler cannot hold a place after a
/// `CANCEL`. The handler's future keeps running for at most
/// `Timing::handler_wind_down` after the signal, long enough to stop what it
/// started (abort a fetch, log the request), and is dropped then: a handler
/// that ignores the signal still leaves nothing running.
#[derive(Clone)]
pub struct CallCancel {
    pub(crate) rx: tokio::sync::watch::Receiver<Option<CancelCause>>,
}

impl CallCancel {
    /// The cause, once the signal fired.
    pub fn cause(&self) -> Option<CancelCause> {
        *self.rx.borrow()
    }

    /// Resolves when the signal fires. A connection that went away without
    /// firing it (its actor dropped) counts as `Closed`.
    pub async fn cancelled(&self) -> CancelCause {
        let mut rx = self.rx.clone();
        loop {
            if let Some(cause) = *rx.borrow_and_update() {
                return cause;
            }
            if rx.changed().await.is_err() {
                return rx.borrow().unwrap_or(CancelCause::Closed);
            }
        }
    }
}

/// Bytes of a response stream a handler may have at Hub, handed over and
/// not yet written to the carrier, before `CallSink::send` waits. Two TCP
/// frames (§4.4): the device's grant is what moves the stream, and a slow
/// device holds the producer back instead of growing Hub's memory.
pub const CALL_HELD_BYTES: u64 = 32 * 1024;

/// Why `CallSink::send` gave up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SinkError {
    /// The response stream is gone: the device cancelled or reset it, or the link ended.
    #[error("the response stream is gone")]
    Gone,
    /// 30 s passed without the device taking a byte of what Hub holds (§5.5).
    #[error("30 s without credit for the response stream")]
    Stalled,
}

/// Where a handler's progress events and response bytes go. The response
/// stream is credited by the device (§5.6); the bytes wait at Hub until it
/// grants, one chunk at a time in the write queue (§8.1).
#[derive(Clone)]
pub struct CallSink {
    pub(crate) id: u32,
    pub(crate) tx: tokio::sync::mpsc::UnboundedSender<crate::conn::Input>,
    /// Bytes of the response stream written to the carrier so far.
    pub(crate) written: tokio::sync::watch::Receiver<u64>,
    /// Bytes handed to the connection so far.
    pub(crate) handed: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(crate) stall: std::time::Duration,
}

impl CallSink {
    /// An `EVENT` on the request's id (`install.head`, `net.head`).
    pub fn progress(&self, object: Map<String, Value>) {
        let _ = self.tx.send(crate::conn::Input::Msg(crate::conn::ConnMsg::CallProgress { id: self.id, object }));
    }

    /// Response bytes, after a progress event, with backpressure: returns
    /// once Hub holds at most `CALL_HELD_BYTES` of this stream not yet
    /// written to the carrier. `Stalled` after 30 s in which the device took
    /// nothing while Hub held bytes (§5.5: the sender of a request's stream
    /// gives up, and the request ends with `timeout`).
    pub async fn send(&self, bytes: Bytes) -> Result<(), SinkError> {
        use std::sync::atomic::Ordering;
        let n = bytes.len() as u64;
        let handed = self.handed.fetch_add(n, Ordering::SeqCst) + n;
        if self.tx.send(crate::conn::Input::Msg(crate::conn::ConnMsg::CallWrite { id: self.id, bytes, end: false })).is_err() {
            return Err(SinkError::Gone);
        }
        let mut written = self.written.clone();
        loop {
            if handed.saturating_sub(*written.borrow_and_update()) <= CALL_HELD_BYTES {
                return Ok(());
            }
            match tokio::time::timeout(self.stall, written.changed()).await {
                Err(_) => return Err(SinkError::Stalled),
                Ok(Err(_)) => return Err(SinkError::Gone),
                Ok(Ok(())) => {}
            }
        }
    }

    /// Response bytes without backpressure: the whole buffer waits at Hub
    /// for credit. For small, bounded responses only.
    pub fn write(&self, bytes: Bytes) {
        use std::sync::atomic::Ordering;
        self.handed.fetch_add(bytes.len() as u64, Ordering::SeqCst);
        let _ = self.tx.send(crate::conn::Input::Msg(crate::conn::ConnMsg::CallWrite { id: self.id, bytes, end: false }));
    }

    /// The response stream's `END`. The handler's reply is queued after it.
    pub fn end(&self) {
        let _ = self.tx.send(crate::conn::Input::Msg(crate::conn::ConnMsg::CallWrite { id: self.id, bytes: Bytes::new(), end: true }));
    }
}

/// What the connection hands the body reader.
pub(crate) enum BodyEvent {
    Data(Bytes),
    End,
    Failed(ErrorReply),
}

/// The request's stream toward Hub (§5.6), as its handler reads it.
///
/// Credit starts at 0. `take` names the length the request declares; Hub
/// then grants up to its 256 KiB window (§5.6 "Recommended windows") and
/// refills it as the handler reads. An `END` of an empty body that arrived
/// before `take` is kept (§5.6: the responder keeps what arrives for an open
/// request before it has taken the request's stream).
pub struct CallBody {
    pub(crate) id: u32,
    pub(crate) tx: tokio::sync::mpsc::UnboundedSender<crate::conn::Input>,
    pub(crate) rx: tokio::sync::mpsc::UnboundedReceiver<BodyEvent>,
    pub(crate) taken: bool,
    pub(crate) done: bool,
    pub(crate) stall: std::time::Duration,
}

impl CallBody {
    /// Takes the stream, declared `total` bytes long: Hub grants credit now.
    /// A second call does nothing.
    pub fn take(&mut self, total: u64) {
        if self.taken {
            return;
        }
        self.taken = true;
        let _ = self.tx.send(crate::conn::Input::Msg(crate::conn::ConnMsg::TakeBody { id: self.id, total }));
    }

    /// The next bytes, or None after the `END` of a body of exactly the
    /// declared length. Errors: `transfer` when the body ends short or the
    /// device resets it, `timeout` after 30 s without a `DATA` frame (§5.5),
    /// `cancelled` when the request ended meanwhile.
    pub async fn next(&mut self) -> Result<Option<Bytes>, ErrorReply> {
        if self.done {
            return Ok(None);
        }
        if !self.taken {
            return Err(ErrorReply::new("internal", "the body was read before it was taken"));
        }
        match tokio::time::timeout(self.stall, self.rx.recv()).await {
            Err(_) => Err(ErrorReply::new("timeout", "30 s without DATA on the request's body")),
            Ok(None) => Err(ErrorReply::new("cancelled", "the request ended")),
            Ok(Some(BodyEvent::Data(bytes))) => {
                let _ = self.tx.send(crate::conn::Input::Msg(crate::conn::ConnMsg::BodyConsumed { id: self.id, bytes: bytes.len() as u64 }));
                Ok(Some(bytes))
            }
            Ok(Some(BodyEvent::End)) => {
                self.done = true;
                Ok(None)
            }
            Ok(Some(BodyEvent::Failed(error))) => {
                self.done = true;
                Err(error)
            }
        }
    }

    /// Takes the stream and reads it whole: `total` bytes, then `END`.
    pub async fn read_all(&mut self, total: u64) -> Result<Vec<u8>, ErrorReply> {
        self.take(total);
        let mut out = Vec::with_capacity(total.min(crate::conn::BODY_WINDOW) as usize);
        while let Some(bytes) = self.next().await? {
            out.extend_from_slice(&bytes);
        }
        Ok(out)
    }
}

/// The device-initiated system requests other than `channel.open`
/// (`catalog`, `install`, `net.http`, `devtools.open`): other crates serve
/// them through this trait without touching the connection code. Several
/// providers may be installed, each serving the groups it lists; a group
/// no provider lists is not in Hub's `ready.features` and its requests are
/// answered `unsupported` (§6.1).
///
/// What the connection does for every handler: the bounds of §9 per group
/// (`busy` past them), `unsupported` for `net.http` on a connection under
/// 12,288 bytes (§6.5), the response stream's credit (an early `CREDIT` kept,
/// §5.6), the reply queued behind the stream's `END` (§5.2), the request's
/// body stream (`CallBody`), and the answer to a `CANCEL` (`CallCancel`).
pub trait DeviceRequests: Send + Sync + 'static {
    /// The groups served (§6.1), listed in `ready.features`.
    fn features(&self) -> Vec<String>;
    /// Answers one request of a listed group. On `call.cancel` the handler
    /// stops at once; its result is then discarded.
    fn handle(&self, call: DeviceCall) -> BoxFuture<'static, Result<Value, ErrorReply>>;
}

/// The group a device system request belongs to (§6.1).
pub fn request_group(t: &str) -> Option<&'static str> {
    Some(match t {
        "catalog" => "catalog",
        "install" => "install",
        "net.http" => "net.http",
        "devtools.open" => "devtools",
        "channel.open" => "channel",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Runtime operations
// ---------------------------------------------------------------------------

/// Why a runtime operation did not succeed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OpError {
    /// The device is not paired with this Hub.
    #[error("the device is not paired")]
    Unknown,
    /// The device is paired and not linked now.
    #[error("the device is not linked")]
    NotLinked,
    /// The device mutex is held (§8.3): its holder, as Hub's status shows it.
    #[error("the device is busy: held by {holder}")]
    Busy { holder: String },
    /// The device does not list the group the operation needs (§6.1).
    #[error("the device does not serve {feature}")]
    Unsupported { feature: String },
    /// Hub refused the operation's input before sending anything.
    #[error("{message}")]
    Invalid { message: String },
    /// The request ended without success: an error `REPLY` of the device
    /// (`rejected`, `busy`, `transfer`, `storage`, `launch`, …), or what Hub
    /// reports on its own side: `closed`, `timeout`, `cancelled` (§5.2, §5.4).
    #[error("{code}: {message}")]
    Link {
        code: String,
        message: String,
        /// Fields of the error reply beyond `code` and `message` (`phase`, `installed`).
        #[serde(skip_serializing_if = "Map::is_empty")]
        extra: Map<String, Value>,
    },
}

impl OpError {
    pub(crate) fn link(code: &str, message: impl Into<String>) -> Self {
        Self::Link { code: code.to_owned(), message: message.into(), extra: Map::new() }
    }
    /// The code of a `Link` error.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Link { code, .. } => Some(code),
            _ => None,
        }
    }
}

/// What `runtime.launch` starts (§6.2): one of the two, never both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    /// A `.3dsx` name under `sdmc:/3ds/`.
    Native(String),
    /// An installed applet's output name.
    App(String),
}

// ---------------------------------------------------------------------------
// Snapshots and events
// ---------------------------------------------------------------------------

/// An open applet channel, as the device status shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelInfo {
    pub channel: u32,
    pub app: String,
    pub hash: String,
    pub generation: u32,
    /// Epoch milliseconds of the channel's opening.
    pub since: i64,
}

/// The live link of a device.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    pub carrier: CarrierKind,
    /// Where the device is, as text for the owner.
    pub peer: String,
    /// The connection's payload limit (§4.4).
    pub payload_limit: u32,
    /// The device's current list and Hub's, and the groups both name (§6.1).
    pub device_features: Vec<String>,
    pub hub_features: Vec<String>,
    pub features_in_use: Vec<String>,
    pub channels: Vec<ChannelInfo>,
    /// Epoch milliseconds of `ready`.
    pub linked_at: i64,
}

/// How a link ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseInfo {
    /// `hub`: Hub sent `CLOSE`; `device`: the device did; `carrier`: the byte stream ended.
    pub by: &'static str,
    /// The close code's name (§4.8), when a `CLOSE` was sent or read.
    pub code: Option<String>,
    pub reason: String,
}

/// One paired device with its live state. No key appears here.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub model: String,
    pub target: String,
    pub host_abi: u32,
    pub last_address: Option<String>,
    pub last_seen: Option<i64>,
    /// A setup has no matching final readback and no `ready` settled it (§3.2).
    pub setup_unconfirmed: bool,
    /// Two consoles were seen to share this device id (§2.4).
    pub duplicate_id: bool,
    pub linked: bool,
    pub link: Option<LinkInfo>,
    /// The last `runtime.status` the device sent or answered.
    pub status: Option<Value>,
    /// The installed applets of its last complete `runtime.apps` report.
    pub apps: Option<Vec<InstalledApp>>,
    /// The device mutex's holder (§8.3).
    pub mutex_holder: Option<String>,
    /// Epoch milliseconds of when the holder took the mutex.
    pub mutex_since: Option<i64>,
    /// The latest unsolicited screenshot, without its pixels.
    pub latest_screenshot: Option<crate::screenshot::Screenshot>,
    pub last_close: Option<CloseInfo>,
}

/// Something that happened on the endpoint, for every subscriber.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum HubEvent {
    #[serde(rename_all = "camelCase")]
    Linked { device: String, link: LinkInfo },
    #[serde(rename_all = "camelCase")]
    Unlinked { device: String, close: CloseInfo },
    /// A handshake ended in `refused`, or the device closed it (`auth`).
    #[serde(rename_all = "camelCase")]
    Refused { device: Option<String>, peer: String, code: String, message: String },
    #[serde(rename_all = "camelCase")]
    Features { device: String, device_features: Vec<String>, features_in_use: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Status { device: String, status: Value },
    #[serde(rename_all = "camelCase")]
    Apps { device: String, apps: Vec<InstalledApp> },
    #[serde(rename_all = "camelCase")]
    Log {
        device: String,
        level: String,
        source: String,
        text: String,
        frame: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        dropped: Option<u64>,
    },
    /// An unsolicited screenshot arrived whole and is the device's latest.
    #[serde(rename_all = "camelCase")]
    Screenshot { device: String, frame: u64, width: u32, height: u32 },
    #[serde(rename_all = "camelCase")]
    ChannelOpened { device: String, channel: ChannelInfo },
    #[serde(rename_all = "camelCase")]
    ChannelClosed {
        device: String,
        channel: ChannelInfo,
        /// The reason the daemon was given (§7.5, docs/DAEMON-PROTOCOL.md §5).
        reason: String,
    },
    /// A device's `channel.open` was refused (§7.1).
    #[serde(rename_all = "camelCase")]
    ChannelRefused { device: String, app: String, code: String },
    #[serde(rename_all = "camelCase")]
    DuplicateId { device: String },
    /// The device mutex was taken, released, or turned into a launch hold
    /// (§8.3): its holder now, None when it is free.
    #[serde(rename_all = "camelCase")]
    Busy { device: String, holder: Option<String>, since: Option<i64> },
    #[serde(rename_all = "camelCase")]
    Forgotten { device: String },
}
