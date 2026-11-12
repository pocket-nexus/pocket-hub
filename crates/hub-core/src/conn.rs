//! One linked connection after `ready` (§4 to §8): the actor that owns every
//! request, stream and channel of the link.
//!
//! Three tasks serve a connection. The **reader** hands the socket's bytes to
//! the actor and nothing else; the **writer** drains one FIFO queue onto the
//! socket (§8.1) and reports when a stream's `DATA` frame has left it; the
//! **actor** decodes frames, keeps the tables and decides. The actor never
//! waits on the writer: it queues and goes on, so Hub reads a connection
//! whatever the length of its write queue (§8.1). What it can owe is bounded
//! by the open-request bounds, the credit it granted and the event units.
//!
//! A stream producer has at most one `DATA` frame in the queue at a time
//! (the one-chunk rule): it queues the next chunk when the writer reports the
//! previous one written, so a frame queued now waits behind at most one chunk
//! per active stream.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use bytes::Bytes;
use pocket_link::consts::*;
use pocket_link::frame::{self, Decoded, FLAG_APP, FLAG_END, FLAG_ERROR, Frame, FrameDecoder, FrameType, JsonFrame, Side, id_range};
use pocket_link::handshake::parse_features;
use pocket_link::text::encoded_prefix;
use pocket_link::{CloseCode, LinkClose, ResetCode};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::api::{BodyEvent, CallBody, CallCancel, CallSink, CancelCause, ChannelOpenError, ChannelStream, CloseInfo, DeviceCall, ErrorReply, HubEvent, OpError, request_group};
use crate::apps::{AppsAssembler, AppsPage};
use crate::endpoint::Shared;
use crate::reading::{Holdings, Reading, Rule, read_event};
use crate::relay::Channel;
use crate::screenshot::{compose, surfaces_of};

/// What reaches the actor.
pub(crate) enum Input {
    /// Bytes the reader took from the socket.
    Bytes(Bytes),
    /// The socket's read side ended (EOF or an error).
    Eof,
    /// The `DATA` frame of this stream id left the write queue.
    Written(u32),
    Msg(ConnMsg),
}

pub(crate) enum ConnMsg {
    /// A runtime operation of a device handle.
    Op(Op),
    /// The caller of an operation went away: `CANCEL` its request.
    CancelOp(u64),
    /// Close the link: `forgotten`, `going_away`.
    Close(CloseCode, String),
    /// A newer connection of this device reached `ready` (§2.4).
    Retire,
    /// Hub's feature list changed (§6.1).
    SetFeatures(Vec<String>),
    /// A standalone event (id 0) of `group`, sent only while the group is in
    /// use on this connection (`catalog.changed`, §6.4).
    Event { group: &'static str, object: Map<String, Value> },
    /// The owner or Hub's policy ends a channel (§7.5 `policy`).
    CloseChannel(u32),
    /// The supervisor answered a `channel.open`.
    OpenDone { id: u32, generation: u32, result: Result<ChannelStream, ChannelOpenError> },
    /// The generation could not be stored.
    OpenFailed { id: u32, message: String },
    /// The supervisor stops the channel's daemon.
    DaemonStop { key: u64 },
    /// A frame from a daemon's channel connection; None when it closed.
    Daemon { key: u64, frame: Option<Result<Frame, LinkClose>> },
    CallProgress { id: u32, object: Map<String, Value> },
    CallWrite { id: u32, bytes: Bytes, end: bool },
    CallDone { id: u32, result: Result<Value, ErrorReply> },
    /// A handler took its request's body stream, declared `total` bytes.
    TakeBody { id: u32, total: u64 },
    /// A handler read this many bytes of its request's body: the window opens again.
    BodyConsumed { id: u32, bytes: u64 },
}

/// §6.5: `net.http` is in use only on a connection whose payload limit is
/// at least 12,288 bytes; every other group fits any limit.
pub(crate) fn group_fits(group: &str, limit: u32) -> bool {
    group != "net.http" || limit >= HTTP_MIN_PAYLOAD
}

/// Hub's window on a device request's body stream (§5.6: 256 KiB for any
/// device stream at Hub).
pub(crate) const BODY_WINDOW: u64 = WINDOW_HUB as u64;

/// One runtime operation (§6.2).
pub(crate) struct Op {
    pub key: u64,
    /// The request's object, `t` included.
    pub request: Map<String, Value>,
    /// The group the request belongs to (§6.1).
    pub group: &'static str,
    /// The request's stream toward the device (push, `.3dsx`).
    pub body: Option<Bytes>,
    /// Progress events go here.
    pub progress: Option<mpsc::UnboundedSender<Value>>,
    /// The count of body bytes written to the carrier so far goes here.
    pub sent: Option<mpsc::UnboundedSender<u64>>,
    /// `runtime.push`, `runtime.native.install`, `runtime.launch` (§8.3).
    pub state_changing: bool,
    pub done: oneshot::Sender<Result<OpOutcome, OpError>>,
}

pub(crate) struct OpOutcome {
    pub reply: Map<String, Value>,
    /// The progress event that announced the response stream.
    pub head: Option<Map<String, Value>>,
    pub data: Vec<u8>,
}

struct WriteItem {
    bytes: Bytes,
    token: Option<u32>,
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// A request Hub sent to the device.
pub(crate) struct OutRequest {
    pub group: &'static str,
    pub last_frame: Instant,
    pub cancelled_at: Option<Instant>,
    /// What the requester's own side reports when the id is freed without a reply.
    pub cause: &'static str,
    pub kind: OutKind,
}

pub(crate) enum OutKind {
    /// `switch`: a `runtime.launch` of an app, whose `switching` reply opens
    /// the patient window (`Conn::patient`).
    Op { key: u64, state: bool, switch: bool, done: Option<oneshot::Sender<Result<OpOutcome, OpError>>>, progress: Option<mpsc::UnboundedSender<Value>>, head: Option<Map<String, Value>> },
    /// A daemon's request, forwarded: `local` is its id on the channel connection.
    Channel { key: u64, local: u32, early_credit: u64, answered: bool },
}

/// A request of the device that Hub holds.
pub(crate) enum InRequest {
    /// `channel.open` while the daemon starts.
    ChannelOpen { task: JoinHandle<()>, deadline: Instant, app: String, hash: String },
    /// A system request served by `DeviceRequests`. Its handler runs in a
    /// task of its own; `cancel` is the handler's signal (§5.4).
    System { cancel: watch::Sender<Option<CancelCause>>, group: &'static str, head: bool },
    /// An applet's request, forwarded to its daemon.
    Applet { key: u64, head: bool, refused: bool },
}

/// Bytes Hub sends on one stream direction, its own or a daemon's.
pub(crate) struct SendStream {
    pub pending: VecDeque<Bytes>,
    pub pending_len: u64,
    /// Credit the device granted and no queued chunk has used.
    pub credit: u64,
    /// One DATA frame of this stream is in the write queue.
    pub queued: bool,
    /// No more bytes after the pending ones: the last frame carries END.
    pub ending: bool,
    pub end_queued: bool,
    /// The REPLY that goes behind the stream's last byte (§5.2).
    pub trailer: Option<Bytes>,
    /// The channel the stream belongs to.
    pub channel: Option<u64>,
    /// The stream counts toward its channel's bound of 4 (§7.6): from its
    /// opening frame, which for a response is its first DATA, to END or RESET.
    pub counted: bool,
    /// Bytes of the DATA frame in the write queue.
    pub in_flight: u64,
    /// Bytes written to the carrier so far, and where that count goes after
    /// each frame (a push's byte progress).
    pub sent: Option<(u64, mpsc::UnboundedSender<u64>)>,
    /// The same count for a `DeviceRequests` handler's backpressure (`CallSink::send`).
    pub written: Option<watch::Sender<u64>>,
}

impl SendStream {
    pub fn new(channel: Option<u64>) -> Self {
        Self {
            pending: VecDeque::new(),
            pending_len: 0,
            credit: 0,
            queued: false,
            ending: false,
            end_queued: false,
            trailer: None,
            channel,
            counted: true,
            in_flight: 0,
            sent: None,
            written: None,
        }
    }
}

/// Bytes Hub receives on one stream direction.
pub(crate) struct RecvStream {
    /// Granted and not yet received: DATA past it closes the link with `flow`.
    pub outstanding: u64,
    pub received: u64,
    pub sink: RecvSink,
}

pub(crate) enum RecvSink {
    /// Hub keeps the bytes, under a window it refills itself (§5.6: 256 KiB).
    Collect { total: u64, data: Vec<u8>, ended: bool, unsolicited: Option<Map<String, Value>> },
    /// The bytes go to a daemon, which grants the credit.
    Relay { key: u64 },
    /// A device request's body, toward its `DeviceRequests` handler. `total`
    /// is None until the handler takes the stream; an `END` of an empty body
    /// that came before is kept in `early_end` (§5.6).
    Body { tx: mpsc::UnboundedSender<BodyEvent>, total: Option<u64>, early_end: bool, granted: u64, consumed: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Open,
    /// Replaced: only a PONG and a CLOSE are read, until `until` (§2.4).
    Retired { until: Instant },
    /// CLOSE sent: the socket closes when the device closes or at `until` (§4.8).
    Closing { until: Instant },
    Ended,
}

pub(crate) struct Conn {
    pub shared: Arc<Shared>,
    pub device: String,
    pub device_name: String,
    pub model: String,
    pub target: String,
    pub conn: u64,
    pub limit: u32,
    pub tx: mpsc::UnboundedSender<Input>,
    rx: mpsc::UnboundedReceiver<Input>,
    write: Option<mpsc::UnboundedSender<WriteItem>>,
    reader: Option<JoinHandle<()>>,
    decoder: FrameDecoder,
    phase: Phase,
    last_received: Instant,
    next_id: u32,
    pub out: HashMap<u32, OutRequest>,
    pub inr: HashMap<u32, InRequest>,
    pub send: HashMap<u32, SendStream>,
    pub recv: HashMap<u32, RecvStream>,
    pub channels: HashMap<u32, Channel>,
    pub next_key: u64,
    /// Link ids of daemon requests that ended, with their channel: a late
    /// progress event on one still gets its units back (§5.6). The oldest
    /// entries go first past `RELEASED_KEPT`.
    pub released: HashMap<u32, u64>,
    pub released_order: VecDeque<u32>,
    /// Tasks that wait for a daemon's stop signal, by channel key.
    pub stop_watchers: HashMap<u64, JoinHandle<()>>,
    /// State-changing operations waiting for the open one to end (§8.3).
    waiting_state: VecDeque<Op>,
    hub_features: Vec<String>,
    wanted_features: Option<Vec<String>>,
    features_sent_at: Option<Instant>,
    device_features: Vec<String>,
    apps: AppsAssembler,
    /// After a `switching` reply: the patient window lasts until the next
    /// `runtime.status` event or this instant (`patient`).
    switch_until: Option<Instant>,
    /// When Hub sends its next own `PING`; None outside the patient window.
    next_ping: Option<Instant>,
    /// The payload of Hub's own `PING`s. Kept under 2^24 so it never equals
    /// `PING_REPLACED`.
    pings: u32,
}

const PING_REPLACED: u32 = 0x5245_504c;

impl Conn {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        shared: Arc<Shared>,
        hello: &pocket_link::handshake::Hello,
        conn: u64,
        limit: u32,
        hub_features: Vec<String>,
        decoder: FrameDecoder,
        tx: mpsc::UnboundedSender<Input>,
        rx: mpsc::UnboundedReceiver<Input>,
    ) -> Self {
        Self {
            shared,
            device: hello.device.clone(),
            device_name: hello.name.clone(),
            model: hello.model.clone(),
            target: hello.target.clone(),
            conn,
            limit,
            tx,
            rx,
            write: None,
            reader: None,
            decoder,
            phase: Phase::Open,
            last_received: Instant::now(),
            next_id: 1,
            out: HashMap::new(),
            inr: HashMap::new(),
            send: HashMap::new(),
            recv: HashMap::new(),
            channels: HashMap::new(),
            next_key: 1,
            released: HashMap::new(),
            released_order: VecDeque::new(),
            stop_watchers: HashMap::new(),
            waiting_state: VecDeque::new(),
            hub_features,
            wanted_features: None,
            features_sent_at: None,
            device_features: hello.features.clone(),
            apps: AppsAssembler::default(),
            switch_until: None,
            next_ping: None,
            pings: 0,
        }
    }

    /// Runs the connection until it ends. `reader` and `writer` are the
    /// carrier's halves after the handshake.
    pub(crate) async fn run(mut self, mut reader: Box<dyn tokio::io::AsyncRead + Send + Unpin>, mut writer: Box<dyn tokio::io::AsyncWrite + Send + Unpin>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let input = self.tx.clone();
        self.reader = Some(tokio::spawn(async move {
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buffer).await {
                    Ok(0) | Err(_) => {
                        let _ = input.send(Input::Eof);
                        return;
                    }
                    Ok(n) => {
                        if input.send(Input::Bytes(Bytes::copy_from_slice(&buffer[..n]))).is_err() {
                            return;
                        }
                    }
                }
            }
        }));
        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<WriteItem>();
        self.write = Some(write_tx);
        let written = self.tx.clone();
        tokio::spawn(async move {
            while let Some(item) = write_rx.recv().await {
                if writer.write_all(&item.bytes).await.is_err() {
                    break;
                }
                if write_rx.is_empty() && writer.flush().await.is_err() {
                    break;
                }
                if let Some(token) = item.token {
                    let _ = written.send(Input::Written(token));
                }
            }
            // Every queued frame is out (a CLOSE last): the write side ends.
            let _ = writer.shutdown().await;
        });

        // Frames that came in the segment of `auth` are already in the decoder.
        self.drain_decoder();
        let mut tick = tokio::time::interval(self.shared.config.timing.tick);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        while self.phase != Phase::Ended {
            tokio::select! {
                input = self.rx.recv() => match input {
                    Some(input) => self.input(input),
                    None => break,
                },
                _ = tick.tick() => self.tick(),
            }
        }
        if let Some(reader) = self.reader.take() {
            reader.abort();
        }
        self.write = None;
    }

    fn input(&mut self, input: Input) {
        match input {
            Input::Bytes(bytes) => {
                if matches!(self.phase, Phase::Ended) {
                    return;
                }
                self.decoder.push(&bytes);
                self.drain_decoder();
            }
            Input::Eof => match self.phase {
                Phase::Closing { .. } => self.phase = Phase::Ended,
                Phase::Ended => {}
                Phase::Retired { .. } => {
                    // A replaced connection that ended answered no PING: not counted.
                    self.phase = Phase::Ended;
                }
                Phase::Open => {
                    self.teardown(CloseInfo { by: "carrier", code: None, reason: "the connection ended".into() });
                    self.phase = Phase::Ended;
                }
            },
            Input::Written(id) => self.written(id),
            Input::Msg(message) => self.message(message),
        }
    }

    fn drain_decoder(&mut self) {
        loop {
            if matches!(self.phase, Phase::Ended | Phase::Closing { .. }) {
                return;
            }
            match self.decoder.next() {
                Ok(None) => return,
                Ok(Some(Decoded::Skipped)) => self.last_received = Instant::now(),
                Ok(Some(Decoded::Frame(frame))) => {
                    self.last_received = Instant::now();
                    if let Err(close) = self.frame(frame) {
                        self.close(close.code, &close.reason);
                        return;
                    }
                }
                Err(close) => {
                    self.close(close.code, &close.reason);
                    return;
                }
            }
        }
    }

    // --- writing (§8.1) ------------------------------------------------------

    pub(crate) fn enqueue(&mut self, bytes: Bytes) {
        self.enqueue_token(bytes, None);
    }

    fn enqueue_token(&mut self, bytes: Bytes, token: Option<u32>) {
        if !matches!(self.phase, Phase::Open | Phase::Retired { .. }) {
            return;
        }
        if let Some(write) = &self.write {
            let _ = write.send(WriteItem { bytes, token });
        }
    }

    /// A JSON frame toward the device. An object over its limit is Hub's own
    /// bug, logged and not sent.
    pub(crate) fn send_json(&mut self, ty: FrameType, id: u32, object: &Value, options: JsonFrame) {
        match frame::encode_json(ty, id, object, options, self.limit) {
            Ok(bytes) => self.enqueue(bytes),
            Err(error) => tracing::warn!(device = %self.device, "a {ty:?} frame was not sent: {error}"),
        }
    }

    pub(crate) fn reply_error(&mut self, id: u32, code: &str, message: &str) {
        let object = json!({"code": code, "message": encoded_prefix(message, ERROR_MESSAGE_BYTES)});
        self.send_json(FrameType::Reply, id, &object, JsonFrame { error: true, ..Default::default() });
    }

    pub(crate) fn send_reset(&mut self, id: u32, code: ResetCode) {
        self.enqueue(frame::encode_reset(id, code));
    }

    pub(crate) fn send_credit(&mut self, id: u32, amount: u64) {
        let mut left = amount;
        while left > 0 {
            let n = left.min(u32::MAX as u64) as u32;
            self.enqueue(frame::encode_credit(id, n));
            left -= n as u64;
        }
    }

    /// The next id of Hub's range (§5.1): never reused on the connection.
    pub(crate) fn take_id(&mut self) -> Result<u32, LinkClose> {
        if self.next_id > ID_DEVICE_MAX {
            return Err(LinkClose::new(CloseCode::GoingAway, "ids exhausted"));
        }
        let id = self.next_id | ID_HUB_BIT;
        self.next_id += 1;
        Ok(id)
    }

    /// Queues the next chunk of a stream: one DATA frame in the queue at a
    /// time, within credit, at most the payload limit (§8.1).
    pub(crate) fn pump(&mut self, id: u32) {
        let limit = self.limit as u64;
        let Some(stream) = self.send.get_mut(&id) else { return };
        if stream.queued || stream.end_queued {
            return;
        }
        if stream.pending_len == 0 {
            if stream.ending {
                // An empty END; the trailer, if any, follows it when it is written.
                stream.end_queued = true;
                stream.queued = true;
                self.enqueue_token(frame::encode_data(id, &[], true), Some(id));
            } else if let Some(trailer) = stream.trailer.take() {
                // A REPLY without END: it goes behind the stream's last byte, and the stream is over.
                self.send.remove(&id);
                self.enqueue(trailer);
            }
            return;
        }
        let n = stream.credit.min(stream.pending_len).min(limit);
        if n == 0 {
            return;
        }
        let mut chunk = Vec::with_capacity(n as usize);
        while (chunk.len() as u64) < n {
            let head = stream.pending.front_mut().expect("pending bytes");
            let take = (n as usize - chunk.len()).min(head.len());
            chunk.extend_from_slice(&head[..take]);
            if take == head.len() {
                stream.pending.pop_front();
            } else {
                *head = head.slice(take..);
            }
        }
        stream.pending_len -= n;
        stream.credit -= n;
        stream.in_flight = n;
        let last = stream.ending && stream.pending_len == 0;
        stream.queued = true;
        if last {
            stream.end_queued = true;
        }
        self.enqueue_token(frame::encode_data(id, &chunk, last), Some(id));
    }

    fn written(&mut self, id: u32) {
        let Some(stream) = self.send.get_mut(&id) else { return };
        stream.queued = false;
        let n = std::mem::take(&mut stream.in_flight);
        if let Some((done, tx)) = &mut stream.sent {
            *done += n;
            let _ = tx.send(*done);
        }
        if let Some(written) = &stream.written {
            written.send_modify(|count| *count += n);
        }
        if stream.end_queued {
            let trailer = stream.trailer.take();
            self.send.remove(&id);
            if let Some(trailer) = trailer {
                self.enqueue(trailer);
            }
            return;
        }
        self.pump(id);
    }

    // --- closing (§4.8) ------------------------------------------------------

    /// Ends the link from Hub's side: CLOSE, nothing written after it, and
    /// the socket closes when the device closes or after the drain time.
    pub(crate) fn close(&mut self, code: CloseCode, reason: &str) {
        match self.phase {
            Phase::Open => {
                self.enqueue(frame::encode_close(code, reason));
                self.write = None;
                self.phase = Phase::Closing { until: Instant::now() + self.shared.config.timing.close_drain };
                self.teardown(CloseInfo { by: "hub", code: Some(code.name().into()), reason: reason.into() });
            }
            Phase::Retired { .. } => {
                self.enqueue(frame::encode_close(code, reason));
                self.write = None;
                self.phase = Phase::Closing { until: Instant::now() + self.shared.config.timing.close_drain };
            }
            Phase::Closing { .. } | Phase::Ended => {}
        }
    }

    /// Every open request, stream and channel fails; nothing is replayed on
    /// the next connection (§4.8). Daemons get one close notice, `link`.
    fn teardown(&mut self, info: CloseInfo) {
        self.end_everything();
        self.shared.unlinked(&self.device, self.conn, info);
    }

    fn end_everything(&mut self) {
        let keys: Vec<u32> = self.channels.keys().copied().collect();
        for id in keys {
            self.close_channel(id, None, Some("link"));
        }
        for op in self.waiting_state.drain(..) {
            let _ = op.done.send(Err(OpError::link("closed", "the connection closed")));
        }
        for (_, request) in self.out.drain() {
            if let OutKind::Op { done: Some(done), .. } = request.kind {
                let _ = done.send(Err(OpError::link("closed", "the connection closed")));
            }
        }
        for (_, request) in self.inr.drain() {
            match request {
                InRequest::ChannelOpen { task, .. } => task.abort(),
                InRequest::System { cancel, .. } => {
                    let _ = cancel.send(Some(CancelCause::Closed));
                }
                InRequest::Applet { .. } => {}
            }
        }
        self.send.clear();
        self.recv.clear();
    }

    // --- time ----------------------------------------------------------------

    fn tick(&mut self) {
        let now = Instant::now();
        let timing = self.shared.config.timing.clone();
        match self.phase {
            Phase::Ended => return,
            Phase::Closing { until } => {
                if now >= until {
                    self.phase = Phase::Ended;
                }
                return;
            }
            Phase::Retired { until } => {
                // A replaced connection that answered nothing in time was half open: not counted.
                if now >= until {
                    self.close(CloseCode::Replaced, "a newer connection of this device is linked");
                }
                return;
            }
            Phase::Open => {}
        }
        let patient = self.patient(now);
        let limit = if patient { timing.state_silence } else { timing.silence };
        if now.duration_since(self.last_received) >= limit {
            let reason = if patient { "no frame within the longer limit of a state-changing request" } else { "10 s without a frame" };
            self.close(CloseCode::Silence, reason);
            return;
        }
        // Hub's own PING during the patient window: the first one interval
        // after the window opens, then one each interval until it ends.
        match (patient, self.next_ping) {
            (false, _) => self.next_ping = None,
            (true, None) => self.next_ping = Some(now + timing.state_ping),
            (true, Some(at)) if now >= at => {
                self.pings = (self.pings + 1) & 0x00ff_ffff;
                self.enqueue(frame::encode_ping(self.pings));
                self.next_ping = Some(now + timing.state_ping);
            }
            (true, Some(_)) => {}
        }
        // §5.5: the 30 s stall; §5.4: the cancel grace.
        let ids: Vec<u32> = self.out.keys().copied().collect();
        for id in ids {
            let request = self.out.get_mut(&id).expect("listed");
            match request.cancelled_at {
                Some(at) if now.duration_since(at) >= timing.cancel_grace => {
                    let request = self.out.remove(&id).expect("listed");
                    self.send.remove(&id);
                    self.recv.remove(&id);
                    self.request_freed(id, request);
                    self.next_state_op();
                }
                None if now.duration_since(request.last_frame) >= timing.request_stall => {
                    request.cancelled_at = Some(now);
                    request.cause = "timeout";
                    self.enqueue(frame::encode_cancel(id));
                    self.send.remove(&id);
                    self.stalled(id);
                }
                _ => {}
            }
        }
        // §7.1: Hub answers `channel.open` within LINK_CHANNEL_OPEN_MS.
        let late: Vec<u32> = self.inr.iter().filter(|(_, r)| matches!(r, InRequest::ChannelOpen { deadline, .. } if now >= *deadline)).map(|(id, _)| *id).collect();
        for id in late {
            if let Some(InRequest::ChannelOpen { task, .. }) = self.inr.remove(&id) {
                task.abort();
            }
            self.reply_error(id, "busy", "the daemon did not start in time");
        }
        self.flush_features();
    }

    /// A request of Hub's stalled: its caller learns `timeout` now (§5.4).
    fn stalled(&mut self, id: u32) {
        let Some(request) = self.out.get_mut(&id) else { return };
        match &mut request.kind {
            OutKind::Op { done, .. } => {
                if let Some(done) = done.take() {
                    let _ = done.send(Err(OpError::link("timeout", "30 s without a frame for the request")));
                }
            }
            OutKind::Channel { key, local, answered, .. } => {
                let (key, local) = (*key, *local);
                *answered = true;
                self.daemon_reply_error(key, local, "timeout", "30 s without a frame for the request");
            }
        }
    }

    /// A cancelled request freed without a reply after the grace (§5.4).
    fn request_freed(&mut self, wire: u32, request: OutRequest) {
        match request.kind {
            OutKind::Op { done, .. } => {
                if let Some(done) = done {
                    let _ = done.send(Err(OpError::link(request.cause, "no reply after CANCEL")));
                }
            }
            OutKind::Channel { key, local, answered, .. } => {
                if !answered {
                    self.daemon_reply_error(key, local, request.cause, "the applet's host sent no reply after CANCEL");
                }
                self.released(wire, key);
                if let Some(channel) = self.channels.values_mut().find(|c| c.key == key) {
                    channel.unmap(wire);
                }
            }
        }
    }

    // --- messages ------------------------------------------------------------

    fn message(&mut self, message: ConnMsg) {
        if matches!(self.phase, Phase::Ended | Phase::Closing { .. }) {
            if let ConnMsg::Op(op) = message {
                let _ = op.done.send(Err(OpError::link("closed", "the connection closed")));
            }
            return;
        }
        if let Phase::Retired { .. } = self.phase {
            if let ConnMsg::Op(op) = message {
                let _ = op.done.send(Err(OpError::link("closed", "the connection was replaced")));
            }
            return;
        }
        match message {
            ConnMsg::Op(op) => self.start_op(op),
            ConnMsg::CancelOp(key) => self.cancel_op(key),
            ConnMsg::Close(code, reason) => self.close(code, &reason),
            ConnMsg::Retire => self.retire(),
            ConnMsg::SetFeatures(list) => {
                self.wanted_features = Some(list);
                self.flush_features();
            }
            ConnMsg::Event { group, object } => {
                if self.in_use(group) {
                    self.send_json(FrameType::Event, 0, &Value::Object(object), JsonFrame::default());
                }
            }
            ConnMsg::CloseChannel(id) => self.close_channel(id, Some("policy"), Some("policy")),
            ConnMsg::OpenDone { id, generation, result } => self.open_done(id, generation, result),
            ConnMsg::OpenFailed { id, message } => {
                if let Some(InRequest::ChannelOpen { .. }) = self.inr.remove(&id) {
                    self.reply_error(id, "busy", &message);
                }
            }
            ConnMsg::Daemon { key, frame } => self.daemon_frame(key, frame),
            ConnMsg::DaemonStop { key } => {
                if let Some(id) = self.channels.values().find(|c| c.key == key).map(|c| c.id) {
                    self.close_channel(id, Some("daemon"), Some("stop"));
                }
            }
            ConnMsg::CallProgress { id, object } => {
                if let Some(InRequest::System { head, .. }) = self.inr.get_mut(&id) {
                    *head = true;
                    self.send_json(FrameType::Event, id, &Value::Object(object), JsonFrame::default());
                }
            }
            ConnMsg::CallWrite { id, bytes, end } => self.call_write(id, bytes, end),
            ConnMsg::CallDone { id, result } => self.call_done(id, result),
            ConnMsg::TakeBody { id, total } => self.take_body(id, total),
            ConnMsg::BodyConsumed { id, bytes } => {
                if let Some(RecvStream { sink: RecvSink::Body { consumed, .. }, .. }) = self.recv.get_mut(&id) {
                    *consumed += bytes;
                    self.grant_body(id);
                }
            }
        }
    }

    /// §2.4: the older connection's requests, streams and channels end as at a
    /// connection loss; only its socket stays open, for the PONG.
    fn retire(&mut self) {
        self.end_everything();
        self.phase = Phase::Retired { until: Instant::now() + self.shared.config.timing.replaced_ping };
        self.enqueue(frame::encode_ping(PING_REPLACED));
    }

    /// The patient window: Hub's silence limit is `Timing::state_silence`
    /// (30 s) instead of `silence` (10 s), and Hub sends its own `PING` every
    /// `Timing::state_ping` (3 s), which §4.6 allows.
    ///
    /// **This is Hub's deliberate deviation from §4.6, for one known device
    /// behaviour.** When an applet is installed on a 3DS for the first time,
    /// the program creates the slot's three folders on the SD card between
    /// two link pumps of its main loop, and a new folder took about 4.2 s on
    /// the card measured (12.6 s in all, a New 3DS XL, 2026-11-12): the
    /// console sends and reads nothing for more than 10 s. A reinstall and a
    /// launch of an installed applet do not, since the folders exist. With
    /// the 10 s limit Hub closed with `silence` while the device was
    /// installing, and the push failed though the applet was in place. The
    /// device reads its socket before it checks its own silence timer
    /// (`engine/host-services/link2.c`), so Hub's PINGs, waiting in the
    /// socket when it resumes, keep the device from closing in turn.
    ///
    /// PocketJS #690 moves that work off the console's main loop (longest
    /// gap measured with it: 641 ms). The window stays for consoles that run
    /// an earlier Pocket Shell, and goes when none is left.
    ///
    /// The window is open:
    /// - while a state-changing request of Hub (`runtime.push`,
    ///   `runtime.native.install`, `runtime.launch`) is in `out`: from the
    ///   moment its REQUEST is queued (`start_op`) until its REPLY, the end
    ///   of a cancelled one's grace, or the connection's end;
    /// - after a `switching` reply to a `runtime.launch` of an app, which the
    ///   device sends before the switch: the first boot of an applet that
    ///   keeps files creates two more folders (its data folders) and compiles
    ///   its bundle, which was not measured and can pass 10 s on the same
    ///   arithmetic. Until the device's next `runtime.status` event or
    ///   `state_silence` after the reply, whichever comes first.
    ///
    /// Everywhere else (device requests, channels, an idle link) the limit is
    /// §4.6's and Hub sends no PING of its own. The request's own 30 s stall
    /// (§5.5) is unchanged.
    fn patient(&mut self, now: Instant) -> bool {
        if self.switch_until.is_some_and(|until| now >= until) {
            self.switch_until = None;
        }
        self.switch_until.is_some() || self.state_request_open()
    }

    /// True while a state-changing request of Hub is open on the wire,
    /// cancelled ones included until their REPLY or the cancel grace (§8.3).
    fn state_request_open(&self) -> bool {
        self.out.values().any(|r| matches!(r.kind, OutKind::Op { state: true, .. }))
    }

    /// Sends the next state-changing operation that waited for the wire (§8.3).
    fn next_state_op(&mut self) {
        while !self.state_request_open() {
            let Some(op) = self.waiting_state.pop_front() else { return };
            self.start_op(op);
        }
    }

    fn start_op(&mut self, op: Op) {
        // §8.3: at most one state-changing request open per device on the wire.
        // The device mutex keeps local callers apart; this keeps a cancelled
        // request's successor off the wire until the cancelled one has ended.
        if op.state_changing && self.state_request_open() {
            self.waiting_state.push_back(op);
            return;
        }
        if !self.in_use(op.group) {
            let _ = op.done.send(Err(OpError::Unsupported { feature: op.group.to_owned() }));
            return;
        }
        let switch = op.request.get("t").and_then(Value::as_str) == Some("runtime.launch") && op.request.contains_key("app");
        let id = match self.take_id() {
            Ok(id) => id,
            Err(close) => {
                let _ = op.done.send(Err(OpError::link("closed", "ids exhausted")));
                self.close(close.code, &close.reason);
                return;
            }
        };
        self.send_json(FrameType::Request, id, &Value::Object(op.request), JsonFrame::default());
        if let Some(body) = op.body {
            let mut stream = SendStream::new(None);
            stream.sent = op.sent.map(|tx| (0, tx));
            stream.pending_len = body.len() as u64;
            if !body.is_empty() {
                stream.pending.push_back(body);
            }
            stream.ending = true;
            self.send.insert(id, stream);
        }
        self.out.insert(
            id,
            OutRequest {
                group: op.group,
                last_frame: Instant::now(),
                cancelled_at: None,
                cause: "cancelled",
                kind: OutKind::Op { key: op.key, state: op.state_changing, switch, done: Some(op.done), progress: op.progress, head: None },
            },
        );
    }

    fn cancel_op(&mut self, key: u64) {
        if let Some(at) = self.waiting_state.iter().position(|op| op.key == key) {
            // Never sent: nothing to cancel on the wire.
            self.waiting_state.remove(at);
            return;
        }
        let Some((&id, _)) = self.out.iter().find(|(_, r)| matches!(r.kind, OutKind::Op { key: k, .. } if k == key)) else { return };
        let request = self.out.get_mut(&id).expect("found");
        if request.cancelled_at.is_none() {
            request.cancelled_at = Some(Instant::now());
            request.cause = "cancelled";
            if let OutKind::Op { done, .. } = &mut request.kind {
                *done = None;
            }
            self.enqueue(frame::encode_cancel(id));
            // The requester stops its own body and discards what still comes (§5.4).
            self.send.remove(&id);
            self.recv.remove(&id);
        }
    }

    // --- features (§6.1) -----------------------------------------------------

    /// A group is in use when both lists name it (§6.1), and `net.http` only
    /// on a connection whose payload limit is at least 12,288 bytes, whatever
    /// the lists say (§6.5: its request and `net.head` at their limits do not
    /// fit a smaller frame).
    pub(crate) fn in_use(&self, group: &str) -> bool {
        self.hub_features.iter().any(|f| f == group) && self.device_features.iter().any(|f| f == group) && self.fits(group)
    }

    fn fits(&self, group: &str) -> bool {
        group_fits(group, self.limit)
    }

    fn features_in_use(&self) -> Vec<String> {
        self.hub_features.iter().filter(|f| self.device_features.contains(f) && self.fits(f)).cloned().collect()
    }

    /// At most one `features` event a second; a change inside the second
    /// goes out at its end, with the list as it stands then.
    fn flush_features(&mut self) {
        let Some(next) = &self.wanted_features else { return };
        if self.features_sent_at.is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(FEATURES_INTERVAL_MS)) {
            return;
        }
        let next = next.clone();
        self.wanted_features = None;
        if next.len() == self.hub_features.len() && next.iter().all(|f| self.hub_features.contains(f)) {
            return;
        }
        // The side that drops a group ends its own part in it first, and
        // queues all of it before its `features` event.
        let dropped: Vec<String> = self.hub_features.iter().filter(|f| !next.contains(f)).cloned().collect();
        for group in &dropped {
            self.drop_group(group);
        }
        self.hub_features = next.clone();
        self.features_sent_at = Some(Instant::now());
        self.send_json(FrameType::Event, 0, &json!({"t": "features", "features": next}), JsonFrame::default());
        self.publish_features();
    }

    /// Hub stops serving `group` (§6.1 steps 1 to 4).
    fn drop_group(&mut self, group: &str) {
        let peer: Vec<u32> = self
            .inr
            .iter()
            .filter(|(_, r)| match r {
                InRequest::ChannelOpen { .. } => group == "channel",
                InRequest::System { group: g, .. } => *g == group,
                InRequest::Applet { .. } => false,
            })
            .map(|(id, _)| *id)
            .collect();
        for id in peer {
            match self.inr.remove(&id) {
                Some(InRequest::ChannelOpen { task, .. }) => task.abort(),
                Some(InRequest::System { cancel, .. }) => {
                    let _ = cancel.send(Some(CancelCause::Withdrawn));
                }
                _ => {}
            }
            self.send.remove(&id);
            self.recv.remove(&id);
            self.reply_error(id, "unsupported", &format!("{group} left use"));
        }
        let own: Vec<u32> = self.out.iter().filter(|(_, r)| r.group == group && r.cancelled_at.is_none()).map(|(id, _)| *id).collect();
        for id in own {
            let request = self.out.get_mut(&id).expect("listed");
            request.cancelled_at = Some(Instant::now());
            if let OutKind::Op { done, .. } = &mut request.kind
                && let Some(done) = done.take()
            {
                let _ = done.send(Err(OpError::link("unsupported", format!("{group} left use"))));
            }
            self.enqueue(frame::encode_cancel(id));
            self.send.remove(&id);
        }
        if group == "channel" {
            let ids: Vec<u32> = self.channels.keys().copied().collect();
            for id in ids {
                self.close_channel(id, Some("feature"), Some("feature"));
            }
        }
    }

    fn on_features(&mut self, object: &Map<String, Value>) -> Result<(), LinkClose> {
        let list = parse_features(object.get("features")).ok_or_else(|| LinkClose::protocol("a features event without a valid list"))?;
        let before = self.features_in_use();
        self.device_features = list;
        let after = self.features_in_use();
        // A group that leaves use: the device ended its part before its event;
        // what crosses it is ended here.
        for group in before.iter().filter(|g| !after.contains(g)) {
            if group == "channel" {
                let ids: Vec<u32> = self.channels.keys().copied().collect();
                for id in ids {
                    self.close_channel(id, None, Some("feature"));
                }
            }
        }
        self.publish_features();
        Ok(())
    }

    fn publish_features(&self) {
        let in_use = self.features_in_use();
        self.shared.update_link(&self.device, self.conn, |link| {
            link.device_features = self.device_features.clone();
            link.hub_features = self.hub_features.clone();
            link.features_in_use = in_use.clone();
        });
        self.shared.publish(HubEvent::Features { device: self.device.clone(), device_features: self.device_features.clone(), features_in_use: in_use });
    }

    // --- reading (§4.7, §5) --------------------------------------------------

    fn frame(&mut self, frame: Frame) -> Result<(), LinkClose> {
        if let Phase::Retired { .. } = self.phase {
            // Only the PONG that tells a live console from a half-open connection matters now.
            match frame.ty {
                FrameType::Pong => {
                    if frame.payload[..4] == PING_REPLACED.to_le_bytes() {
                        self.shared.counted_replacement(&self.device);
                        self.close(CloseCode::Replaced, "a newer connection of this device is linked");
                    }
                }
                FrameType::Close => self.phase = Phase::Ended,
                _ => {}
            }
            return Ok(());
        }
        match frame.ty {
            FrameType::Ping => self.enqueue(frame::encode_pong(&frame.payload)),
            FrameType::Pong => {}
            FrameType::Close => {
                let (code, reason) = frame::parse_close(&frame.payload);
                self.teardown(CloseInfo { by: "device", code: Some(code.name().into()), reason });
                self.write = None;
                self.phase = Phase::Ended;
            }
            FrameType::Handshake => return Err(LinkClose::protocol("HANDSHAKE after ready")),
            FrameType::Request => self.on_request(frame)?,
            FrameType::Reply => self.on_reply(frame),
            FrameType::Event => self.on_event(frame)?,
            FrameType::Cancel => self.on_cancel(frame.id),
            FrameType::Data => self.on_data(frame)?,
            FrameType::Credit => self.on_credit(frame.id, frame::parse_credit(&frame.payload) as u64)?,
            FrameType::Reset => self.on_reset(frame.id, frame::parse_reset(&frame.payload)),
        }
        Ok(())
    }

    fn id_in_use(&self, id: u32) -> bool {
        self.out.contains_key(&id) || self.inr.contains_key(&id) || self.send.contains_key(&id) || self.recv.contains_key(&id) || self.channels.contains_key(&id)
    }

    fn on_request(&mut self, frame: Frame) -> Result<(), LinkClose> {
        let id = frame.id;
        if id_range(id) != Some(Side::Device) || self.id_in_use(id) {
            return Err(LinkClose::protocol("a REQUEST id is not a fresh id of the device"));
        }
        if frame.flags & FLAG_APP != 0 {
            return self.applet_request(frame);
        }
        let parsed = frame::parse_json(&frame);
        let Some(object) = parsed.object else {
            self.reply_error(id, "malformed", "not a JSON object with a string t");
            return Ok(());
        };
        let Some(t) = object.get("t").and_then(Value::as_str).map(str::to_owned) else {
            self.reply_error(id, "malformed", "not a JSON object with a string t");
            return Ok(());
        };
        let Some(group) = request_group(&t) else {
            self.reply_error(id, "unknown_message", &format!("unknown request {t}"));
            return Ok(());
        };
        if !self.in_use(group) {
            let why = if self.fits(group) { format!("{group} is not in use") } else { format!("{group} is in use from a payload limit of {HTTP_MIN_PAYLOAD}; this connection's is {}", self.limit) };
            self.reply_error(id, "unsupported", &why);
            return Ok(());
        }
        if t == "channel.open" {
            self.channel_open(id, &object);
            return Ok(());
        }
        let Some(handler) = self.shared.requests.iter().find(|r| r.features().iter().any(|f| f == group)).cloned() else {
            self.reply_error(id, "unsupported", &format!("{group} is not served"));
            return Ok(());
        };
        // §9: the bounds on device requests open at Hub.
        let open = |g: &[&str]| self.inr.values().filter(|r| matches!(r, InRequest::System { group, .. } if g.contains(group))).count();
        let full = match group {
            "net.http" => open(&["net.http"]) >= HTTP_MAX_OPEN,
            "catalog" | "install" => open(&["catalog", "install"]) >= MAX_CATALOG_REQUESTS,
            "devtools" => open(&["devtools"]) >= 1,
            _ => false,
        };
        if full {
            self.reply_error(id, "busy", &format!("too many {group} requests are open"));
            return Ok(());
        }
        let timing = &self.shared.config.timing;
        // The response stream exists from the request on, so a CREDIT the
        // device sends with the request is kept (§5.6, §6.5); it opens on the
        // wire with the handler's first progress event.
        let (written_tx, written_rx) = watch::channel(0u64);
        let mut response = SendStream::new(None);
        response.written = Some(written_tx);
        self.send.insert(id, response);
        // The request's body: nothing granted until the handler takes it.
        let (body_tx, body_rx) = mpsc::unbounded_channel();
        self.recv.insert(id, RecvStream { outstanding: 0, received: 0, sink: RecvSink::Body { tx: body_tx, total: None, early_end: false, granted: 0, consumed: 0 } });
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = DeviceCall {
            device: self.device.clone(),
            conn: self.conn,
            target: self.target.clone(),
            t,
            object,
            payload_limit: self.limit,
            sink: CallSink { id, tx: self.tx.clone(), written: written_rx, handed: Default::default(), stall: timing.request_stall },
            body: CallBody { id, tx: self.tx.clone(), rx: body_rx, taken: false, done: false, stall: timing.request_stall },
            cancel: CallCancel { rx: cancel_rx.clone() },
        };
        let tx = self.tx.clone();
        let wind_down = timing.handler_wind_down;
        let mut future = handler.handle(call);
        let signal = CallCancel { rx: cancel_rx };
        tokio::spawn(async move {
            tokio::select! {
                result = &mut future => {
                    let _ = tx.send(Input::Msg(ConnMsg::CallDone { id, result }));
                }
                // Hub has answered the device: the handler gets a moment to
                // stop what it started, then its future is dropped (§5.4).
                _ = signal.cancelled() => {
                    let _ = tokio::time::timeout(wind_down, future).await;
                }
            }
        });
        self.inr.insert(id, InRequest::System { cancel: cancel_tx, group, head: false });
        Ok(())
    }

    /// A handler took its request's body (§5.6).
    fn take_body(&mut self, id: u32, declared: u64) {
        let Some(RecvStream { sink: RecvSink::Body { tx, total, early_end, .. }, .. }) = self.recv.get_mut(&id) else { return };
        if total.is_some() {
            return;
        }
        *total = Some(declared);
        if *early_end {
            // The END of an empty body came before the handler took it.
            let event = if declared == 0 { BodyEvent::End } else { BodyEvent::Failed(ErrorReply::new("transfer", format!("the body ended at 0 of {declared} bytes"))) };
            let _ = tx.send(event);
            self.recv.remove(&id);
            return;
        }
        self.grant_body(id);
    }

    /// Grants a request's body what Hub's window has free, up to its declared length.
    fn grant_body(&mut self, id: u32) {
        let Some(stream) = self.recv.get_mut(&id) else { return };
        let RecvSink::Body { total: Some(total), granted, consumed, .. } = &mut stream.sink else { return };
        let room = (*total - *granted).min(BODY_WINDOW - (*granted - *consumed).min(BODY_WINDOW));
        if room == 0 {
            return;
        }
        *granted += room;
        stream.outstanding += room;
        self.send_credit(id, room);
    }

    fn call_write(&mut self, id: u32, bytes: Bytes, end: bool) {
        let Some(InRequest::System { head, .. }) = self.inr.get(&id) else { return };
        if !*head {
            // The head step: bytes before a progress event never reach the wire (§7.2).
            tracing::warn!(device = %self.device, "a handler wrote response bytes before a progress event");
            return;
        }
        let stream = self.send.entry(id).or_insert_with(|| SendStream::new(None));
        if stream.ending {
            return;
        }
        stream.pending_len += bytes.len() as u64;
        if !bytes.is_empty() {
            stream.pending.push_back(bytes);
        }
        stream.ending = end;
        self.pump(id);
    }

    fn call_done(&mut self, id: u32, result: Result<Value, ErrorReply>) {
        let Some(InRequest::System { head, .. }) = self.inr.remove(&id) else { return };
        if matches!(self.recv.get(&id), Some(RecvStream { sink: RecvSink::Body { .. }, .. })) {
            // What the device still sends of the body is not taken (§4.7: an id Hub no longer holds).
            self.recv.remove(&id);
        }
        if !head {
            // No progress event opened the response stream: the REPLY alone.
            self.send.remove(&id);
        }
        let (object, error) = match result {
            Ok(Value::Object(map)) => (Value::Object(map), false),
            Ok(_) => (json!({}), false),
            Err(e) => (json!({"code": e.code, "message": encoded_prefix(&e.message, ERROR_MESSAGE_BYTES)}), true),
        };
        let reply = match frame::encode_json(FrameType::Reply, id, &object, JsonFrame { error, ..Default::default() }, self.limit) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.send.remove(&id);
                self.reply_error(id, "internal", "the reply is over the payload limit");
                return;
            }
        };
        // The REPLY is queued after every DATA frame of the request (§5.2).
        match self.send.get_mut(&id) {
            Some(stream) => {
                stream.ending = true;
                stream.trailer = Some(reply);
                self.pump(id);
            }
            None => self.enqueue(reply),
        }
    }

    fn on_reply(&mut self, frame: Frame) {
        let id = frame.id;
        let Some(request) = self.out.get(&id) else { return };
        if let OutKind::Channel { .. } = request.kind {
            self.channel_reply(frame);
            return;
        }
        let request = self.out.remove(&id).expect("held");
        self.send.remove(&id);
        let collected = self.recv.remove(&id);
        self.next_state_op();
        let OutKind::Op { done, head, switch, .. } = request.kind else { return };
        let parsed = frame::parse_json(&frame);
        let object = parsed.object.unwrap_or_default();
        // The app switch comes after this reply, and its first boot is the
        // stall (`patient`); a cancelled launch that was answered switches too.
        if switch && frame.flags & FLAG_ERROR == 0 && object.get("phase").and_then(Value::as_str) == Some("switching") {
            self.switch_until = Some(Instant::now() + self.shared.config.timing.state_silence);
        }
        let Some(done) = done else { return };
        if frame.flags & FLAG_ERROR != 0 {
            let mut extra = object.clone();
            let code = extra.remove("code").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "internal".into());
            let message = extra.remove("message").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            let _ = done.send(Err(OpError::Link { code, message, extra }));
            return;
        }
        let mut data = Vec::new();
        if let Some(stream) = collected
            && let RecvSink::Collect { total, data: bytes, ended, .. } = stream.sink
        {
            if !ended || bytes.len() as u64 != total {
                let _ = done.send(Err(OpError::link("transfer", format!("the stream carried {} bytes of {total}", bytes.len()))));
                return;
            }
            data = bytes;
        }
        let _ = done.send(Ok(OpOutcome { reply: object, head, data }));
    }

    fn on_cancel(&mut self, id: u32) {
        match self.inr.get(&id) {
            Some(InRequest::ChannelOpen { .. }) => {
                if let Some(InRequest::ChannelOpen { task, .. }) = self.inr.remove(&id) {
                    // Dropping the open's future tells the supervisor; a stream that still comes is dropped.
                    task.abort();
                }
                self.reply_error(id, "cancelled", "the device cancelled the open");
            }
            Some(InRequest::System { .. }) => {
                // §5.4: the responder stops, drops what it would still send
                // on the request's stream, and answers `cancelled`.
                if let Some(InRequest::System { cancel, .. }) = self.inr.remove(&id) {
                    let _ = cancel.send(Some(CancelCause::Cancelled));
                }
                self.send.remove(&id);
                self.recv.remove(&id);
                self.reply_error(id, "cancelled", "cancelled");
            }
            Some(InRequest::Applet { .. }) => self.applet_cancel(id),
            None => {}
        }
    }

    fn on_event(&mut self, frame: Frame) -> Result<(), LinkClose> {
        let app = frame.flags & FLAG_APP != 0;
        let parsed = frame::parse_json(&frame);
        let t = parsed.t().map(str::to_owned);
        let id = frame.id;
        let reading = read_event(id, app, t.as_deref(), &*self);
        match reading {
            Reading::Close(why) => Err(LinkClose::protocol(why)),
            Reading::Ignore => {
                // A late progress event of a daemon request Hub has released:
                // dropped, and its units go back to the device.
                if !app && let Some(&key) = self.released.get(&id) {
                    self.late_device_event(key, parsed.raw.len());
                }
                Ok(())
            }
            Reading::ResetRefused => {
                self.send_reset(id, ResetCode::Refused);
                Ok(())
            }
            Reading::AppProgress => {
                self.device_progress(id, parsed.raw, t.is_some());
                Ok(())
            }
            Reading::App { opens_stream } => {
                let channel = parsed.channel.unwrap_or(0);
                self.applet_event(channel, id, parsed.raw, t.is_some(), opens_stream);
                Ok(())
            }
            Reading::System { rule, opens_stream } => {
                let object = parsed.object.unwrap_or_default();
                self.system_event(id, rule, opens_stream, t.as_deref().unwrap_or(""), object)
            }
        }
    }

    fn system_event(&mut self, id: u32, rule: Rule, opens_stream: bool, t: &str, object: Map<String, Value>) -> Result<(), LinkClose> {
        if t == "features" {
            return self.on_features(&object);
        }
        if rule == Rule::Channel {
            let reason = object.get("reason").and_then(Value::as_str).unwrap_or("closed").to_owned();
            self.close_channel(id, None, Some(&reason));
            return Ok(());
        }
        if !opens_stream && id != 0 {
            // Progress of one of Hub's requests.
            let Some(request) = self.out.get_mut(&id) else { return Ok(()) };
            request.last_frame = Instant::now();
            if request.cancelled_at.is_some() {
                return Ok(());
            }
            let OutKind::Op { progress, head, .. } = &mut request.kind else { return Ok(()) };
            if let Some(progress) = progress {
                let _ = progress.send(Value::Object(object.clone()));
            }
            if t == "runtime.screenshot" && head.is_none() {
                *head = Some(object.clone());
                let total = surfaces_of(&object).map(|(_, total)| total).unwrap_or(0);
                self.recv.insert(id, RecvStream { outstanding: 0, received: 0, sink: RecvSink::Collect { total, data: Vec::new(), ended: false, unsolicited: None } });
                self.grant_collect(id);
            }
            return Ok(());
        }
        match t {
            "runtime.status" if self.in_use("runtime") => {
                // The device reports again after an app switch: the patient
                // window of the `switching` reply ends (`patient`).
                self.switch_until = None;
                self.shared.set_status(&self.device, Value::Object(object));
            }
            "runtime.apps" if self.in_use("apps") => {
                if let Some(apps) = self.apps.page(AppsPage::from_event(&object))? {
                    self.shared.set_apps(&self.device, apps);
                }
            }
            "log" if self.in_use("runtime") => {
                let text = |k: &str| object.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                self.shared.publish(HubEvent::Log {
                    device: self.device.clone(),
                    level: text("level"),
                    source: text("source"),
                    text: text("text"),
                    frame: object.get("frame").and_then(Value::as_u64).unwrap_or(0),
                    dropped: object.get("dropped").and_then(Value::as_u64),
                });
            }
            "runtime.screenshot" if opens_stream => {
                // An unsolicited screenshot: kept as the device's latest, or refused.
                let total = surfaces_of(&object).map(|(_, total)| total);
                match total {
                    Some(total) if self.in_use("runtime") && self.shared.config.keep_screenshots && total > 0 => {
                        self.recv.insert(id, RecvStream { outstanding: 0, received: 0, sink: RecvSink::Collect { total, data: Vec::new(), ended: false, unsolicited: Some(object) } });
                        self.grant_collect(id);
                    }
                    _ => self.send_reset(id, ResetCode::Refused),
                }
            }
            _ if opens_stream => {
                // A stream of a group Hub does not serve here (the agent's).
                self.send_reset(id, ResetCode::Refused);
            }
            _ => {}
        }
        Ok(())
    }

    /// Grants a collecting stream what its 256 KiB window has free, refilled
    /// at half a window so a long stream costs a CREDIT every few frames.
    fn grant_collect(&mut self, id: u32) {
        let Some(stream) = self.recv.get_mut(&id) else { return };
        let RecvSink::Collect { total, ended, .. } = &stream.sink else { return };
        if *ended {
            return;
        }
        let window = WINDOW_HUB as u64;
        let left = total.saturating_sub(stream.received + stream.outstanding);
        let room = (window - stream.outstanding.min(window)).min(left);
        if room == 0 || (stream.outstanding > 0 && room < window / 2 && room < left) {
            return;
        }
        stream.outstanding += room;
        self.send_credit(id, room);
    }

    fn on_data(&mut self, frame: Frame) -> Result<(), LinkClose> {
        let id = frame.id;
        let end = frame.flags & FLAG_END != 0;
        if let Some(request) = self.out.get_mut(&id) {
            request.last_frame = Instant::now();
            if request.cancelled_at.is_some() {
                return Ok(());
            }
        }
        let Some(stream) = self.recv.get_mut(&id) else { return Ok(()) };
        let n = frame.payload.len() as u64;
        if n > stream.outstanding {
            return Err(LinkClose::new(CloseCode::Flow, "DATA past credit"));
        }
        stream.outstanding -= n;
        stream.received += n;
        match &mut stream.sink {
            RecvSink::Body { tx, total, early_end, .. } => {
                let Some(total) = *total else {
                    // Before the handler took the body only an empty END can
                    // come (no credit was granted): kept for `take_body`.
                    if end {
                        *early_end = true;
                    }
                    return Ok(());
                };
                if n > 0 {
                    let _ = tx.send(BodyEvent::Data(frame.payload));
                }
                if end {
                    // §5.6: a direction declared with `bytes: N` ends with END
                    // when N bytes have been sent; anything else is `transfer`.
                    let event = if stream.received == total { BodyEvent::End } else { BodyEvent::Failed(ErrorReply::new("transfer", format!("the body ended at {} of {total} bytes", stream.received))) };
                    let _ = tx.send(event);
                    self.recv.remove(&id);
                }
            }
            RecvSink::Relay { key } => {
                let key = *key;
                if end {
                    self.recv.remove(&id);
                }
                self.relay_data(key, id, frame.payload, end);
            }
            RecvSink::Collect { total, data, ended, unsolicited } => {
                data.extend_from_slice(&frame.payload);
                if stream.received > *total {
                    // Longer than declared: a `transfer` error at the reply, a `length` reset for an event's stream.
                    if unsolicited.is_some() {
                        self.recv.remove(&id);
                        self.send_reset(id, ResetCode::Length);
                    }
                    return Ok(());
                }
                if end {
                    *ended = true;
                    if let Some(head) = unsolicited.take() {
                        let pixels = std::mem::take(data);
                        let complete = stream.received == *total;
                        self.recv.remove(&id);
                        if complete {
                            self.keep_screenshot(head, pixels);
                        }
                    }
                    return Ok(());
                }
                self.grant_collect(id);
            }
        }
        Ok(())
    }

    fn keep_screenshot(&mut self, head: Map<String, Value>, pixels: Vec<u8>) {
        let frame = head.get("frame").and_then(Value::as_u64).unwrap_or(0);
        let format = head.get("format").and_then(Value::as_str).unwrap_or("").to_owned();
        let Some((surfaces, _)) = surfaces_of(&head) else { return };
        let device = self.device.clone();
        let shared = self.shared.clone();
        // PNG encoding takes a few milliseconds: off the actor.
        tokio::task::spawn_blocking(move || match compose(frame, &format, surfaces, &pixels) {
            Ok(shot) => shared.set_screenshot(&device, shot),
            Err(error) => tracing::warn!(device = %device, "an unsolicited screenshot was not kept: {error}"),
        });
    }

    fn on_credit(&mut self, id: u32, amount: u64) -> Result<(), LinkClose> {
        if let Some(channel) = self.channels.get(&id) {
            let key = channel.key;
            self.units_from_device(key, amount);
            return Ok(());
        }
        if let Some(request) = self.out.get_mut(&id) {
            request.last_frame = Instant::now();
        }
        if let Some(stream) = self.send.get_mut(&id) {
            if stream.credit + amount > MAX_CREDIT {
                return Err(LinkClose::new(CloseCode::Flow, "a grant over 16 MiB outstanding"));
            }
            stream.credit += amount;
            if let Some(key) = stream.channel {
                self.relay_credit_to_daemon(key, id, amount);
            }
            self.pump(id);
        }
        Ok(())
    }

    fn on_reset(&mut self, id: u32, code: Option<ResetCode>) {
        let owner = self.send.get(&id).and_then(|s| s.channel).or_else(|| match self.recv.get(&id) {
            Some(RecvStream { sink: RecvSink::Relay { key }, .. }) => Some(*key),
            _ => None,
        });
        let unsolicited = matches!(self.recv.get(&id), Some(RecvStream { sink: RecvSink::Collect { unsolicited: Some(_), .. }, .. }));
        // RESET abandons both directions; a request still ends with its REPLY.
        let trailer = self.send.remove(&id).and_then(|s| s.trailer);
        if let Some(trailer) = trailer {
            self.enqueue(trailer);
        }
        if unsolicited || owner.is_some() {
            self.recv.remove(&id);
        } else if let Some(RecvStream { sink: RecvSink::Body { tx, .. }, .. }) = self.recv.get(&id) {
            let _ = tx.send(BodyEvent::Failed(ErrorReply::new("transfer", "the device reset the request's stream")));
            self.recv.remove(&id);
        } else if let Some(stream) = self.recv.get_mut(&id)
            && let RecvSink::Collect { ended, .. } = &mut stream.sink
        {
            // A request's response stream: the REPLY reports it (`transfer`).
            *ended = false;
            stream.outstanding = 0;
        }
        if let Some(key) = owner {
            self.relay_reset_to_daemon(key, id, code.unwrap_or(ResetCode::Cancelled));
        } else if let Some(InRequest::Applet { key, .. }) = self.inr.get(&id) {
            let key = *key;
            self.relay_reset_to_daemon(key, id, code.unwrap_or(ResetCode::Cancelled));
        } else if let Some(OutRequest { kind: OutKind::Channel { key, .. }, .. }) = self.out.get(&id) {
            let key = *key;
            self.relay_reset_to_daemon(key, id, code.unwrap_or(ResetCode::Cancelled));
        }
    }

    // --- channel.open (§7.1) -------------------------------------------------

    fn channel_open(&mut self, id: u32, object: &Map<String, Value>) {
        let app = object.get("app").and_then(Value::as_str).unwrap_or("");
        let hash = object.get("hash").and_then(Value::as_str).unwrap_or("");
        if app.is_empty() || pocket_link::text::encoded_bytes(app) > APP_ID_BYTES || !pocket_link::text::is_hash(hash) {
            self.reply_error(id, "malformed", "channel.open needs an app of 1 to 128 bytes and a 16-hex hash");
            return;
        }
        let Some(host) = self.shared.host() else {
            self.reply_error(id, "unsupported", "Hub routes no channels");
            return;
        };
        let opening = self.inr.values().filter(|r| matches!(r, InRequest::ChannelOpen { .. })).count();
        if self.channels.len() + opening >= MAX_CHANNELS {
            self.reply_error(id, "busy", "the connection has 4 channels");
            self.shared.publish(HubEvent::ChannelRefused { device: self.device.clone(), app: app.to_owned(), code: "busy".into() });
            return;
        }
        let tx = self.tx.clone();
        let store = self.shared.store.clone();
        let mut offer = crate::api::ChannelOffer {
            device: self.device.clone(),
            device_name: self.device_name.clone(),
            model: self.model.clone(),
            target: self.target.clone(),
            app: app.to_owned(),
            hash: hash.to_owned(),
            generation: 0,
            payload_limit: self.limit,
        };
        let task = tokio::spawn(async move {
            // §7.4: the generation is on disk before the REPLY that carries it.
            let generation = match tokio::task::spawn_blocking(move || store.next_generation()).await {
                Ok(Ok(generation)) => generation,
                Ok(Err(error)) => {
                    let _ = tx.send(Input::Msg(ConnMsg::OpenFailed { id, message: format!("the generation could not be stored: {error}") }));
                    return;
                }
                Err(_) => return,
            };
            offer.generation = generation;
            let result = host.open(offer).await;
            let _ = tx.send(Input::Msg(ConnMsg::OpenDone { id, generation, result }));
        });
        self.inr.insert(id, InRequest::ChannelOpen { task, deadline: Instant::now() + self.shared.config.timing.channel_open, app: app.to_owned(), hash: hash.to_owned() });
    }

    fn open_done(&mut self, id: u32, generation: u32, result: Result<ChannelStream, ChannelOpenError>) {
        // An answer already went out (busy at 20 s, cancelled): the stream is dropped.
        if !matches!(self.inr.get(&id), Some(InRequest::ChannelOpen { .. })) {
            return;
        }
        let Some(InRequest::ChannelOpen { app, hash, .. }) = self.inr.remove(&id) else { return };
        match result {
            Err(error) => {
                let code = match &error {
                    ChannelOpenError::NoDaemon => "no_daemon",
                    ChannelOpenError::Busy(_) => "busy",
                    ChannelOpenError::Rejected(_) => "rejected",
                };
                self.reply_error(id, code, &error.to_string());
                self.shared.publish(HubEvent::ChannelRefused { device: self.device.clone(), app, code: code.into() });
            }
            Ok(stream) => self.channel_accepted(id, app, hash, generation, stream),
        }
    }

    pub(crate) fn publish_channels(&self) {
        let list: Vec<crate::api::ChannelInfo> = self.channels.values().map(Channel::info).collect();
        self.shared.update_link(&self.device, self.conn, |link| link.channels = list.clone());
    }
}

impl Holdings for Conn {
    fn channel_request(&self, id: u32) -> bool {
        matches!(self.inr.get(&id), Some(InRequest::Applet { .. })) || matches!(self.out.get(&id), Some(OutRequest { kind: OutKind::Channel { .. }, .. }))
    }
    fn own_request(&self, id: u32) -> bool {
        self.out.contains_key(&id)
    }
    fn channel(&self, id: u32) -> bool {
        self.channels.contains_key(&id)
    }
    fn in_use(&self, id: u32) -> bool {
        self.id_in_use(id)
    }
}

