//! SERVICE-PROTOCOL.md §6: the host connection, on which a service asks its
//! host for a capability, and §7, the one capability this host provides,
//! `store`.
//!
//! One task per connection reads the service's frames, keeps each open
//! request's state, and hands a frame to write to a writer task in order.
//! Store work runs on the blocking pool and comes back as an outcome; a
//! request ends with exactly one `REPLY` unless the connection closes first.
//!
//! **Credit, both ways (§6).** A `store.set`'s body is granted by the host as
//! it takes the bytes, at most 256 KiB outstanding and never past the
//! declared count, so a body that runs over its count is `DATA` past credit,
//! which ends the connection. A `store.get`'s value goes out after its head
//! event, only as the service grants.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use pocket_link::consts::MAX_HANDSHAKE;
use pocket_link::frame::{encode_credit, encode_data, encode_json, encode_reset, parse_json, Decoded, JsonFrame};
use pocket_link::{Frame, FrameDecoder, FrameType, ResetCode};
use serde_json::{json, Map, Value};
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::store::{Meta, StoreError, Stores, MAX_KEY_BYTES, MAX_LIST, MAX_VALUE_BYTES};
use crate::util::cut;
use crate::wire;

/// The payload limit of the host connection (§6).
pub const PAYLOAD_LIMIT: u32 = 65_536;
/// The service's open requests on the connection.
const MAX_OPEN: usize = 64;
/// The service's open store calls.
const MAX_STORE_OPEN: usize = 8;
/// The most a body's credit has outstanding.
const BODY_WINDOW: u64 = 256 * 1024;
/// A store call ends with `timeout` this long after its `REQUEST`.
pub const STORE_CALL_LIMIT: Duration = Duration::from_secs(30);
/// A grant may not raise a stream's credit past this (link specification §5.6).
const MAX_CREDIT: u64 = 16 * 1024 * 1024;
/// Ids whose request ended while credit for their body was outstanding:
/// `DATA` already in flight for them is dropped. The oldest are forgotten
/// past this many.
const MAX_ENDED_BODIES: usize = 1_024;
/// The capabilities of later steps: a call of one is `not_required`, since
/// no manifest this host installs can require it.
const LATER: &[&str] = &["net.fetch", "net.websocket", "image.render", "text.atlas", "media.open", "media.seek", "media.close"];

/// What one host connection serves: the process's applet id, the manifest's
/// `requires`, and its store.
#[derive(Clone)]
pub struct HostSession {
    pub app: String,
    pub requires: Vec<String>,
    pub store: Option<StoreBinding>,
}

/// The store a session's `store.*` calls reach.
#[derive(Clone)]
pub struct StoreBinding {
    pub stores: Arc<Stores>,
    /// The store's folder name (`Stores::name`).
    pub name: String,
    /// The `store.maxBytes` that applies now: the newest installed release's.
    pub max_bytes: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Told after each write that committed.
    pub written: Arc<dyn Fn() + Send + Sync>,
}

/// An error reply: `{code, message}`.
#[derive(Debug, Clone)]
struct Failure {
    code: &'static str,
    message: String,
}

fn failure(code: &'static str, message: impl Into<String>) -> Failure {
    Failure { code, message: message.into() }
}

impl From<StoreError> for Failure {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Format(_) => failure("store_format", error.to_string()),
            StoreError::Quota { .. } => failure("quota", error.to_string()),
            other => failure("internal", other.to_string()),
        }
    }
}

/// What a store call asks for, once its arguments are checked.
enum Call {
    Get { key: Vec<u8> },
    Set { key: Vec<u8>, bytes: u64 },
    Delete { key: Vec<u8> },
    List { prefix: Vec<u8>, after: Option<Vec<u8>>, limit: usize },
    Usage,
}

/// The work a blocking task did, for its request.
enum Outcome {
    Value(Result<Option<Vec<u8>>, Failure>),
    Reply(Result<Value, Failure>),
}

enum State {
    /// `store.set` taking its body.
    Body { key: Vec<u8>, declared: u64, received: Vec<u8>, outstanding: u64 },
    /// On the blocking pool. A write that has started is answered by its own
    /// outcome whatever comes; a read that is cancelled is answered
    /// `cancelled` then, and its outcome is dropped.
    Working { write: bool },
    /// `store.get`'s value going out after its head event.
    Sending { value: Bytes, sent: usize, credit: u64 },
}

struct Request {
    t: &'static str,
    started: Instant,
    state: State,
}

struct Conn {
    session: HostSession,
    out: mpsc::UnboundedSender<Bytes>,
    requests: HashMap<u32, Request>,
    /// The last id of the service's requests: they increase (§6).
    last_id: u32,
    /// Ids whose body credit was outstanding when their request ended, with
    /// how much; in arrival order to forget the oldest.
    ended_bodies: HashMap<u32, u64>,
    ended_order: VecDeque<u32>,
    done: mpsc::UnboundedSender<(u32, Outcome)>,
}

/// Serves one host connection after its `host.ready` was written, until it
/// closes or breaks the protocol. Returns why it ended, for the process's
/// log: the connection is part of the process's lease (§6).
pub async fn serve(stream: UnixStream, session: HostSession) -> String {
    let (mut reader, mut writer) = stream.into_split();
    let (out, mut outgoing) = mpsc::unbounded_channel::<Bytes>();
    let writing = tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            if wire::write_all(&mut writer, &frame).await.is_err() {
                return;
            }
        }
    });
    let (done, mut finished) = mpsc::unbounded_channel();
    let mut conn = Conn { session, out, requests: HashMap::new(), last_id: 0, ended_bodies: HashMap::new(), ended_order: VecDeque::new(), done };
    let mut decoder = FrameDecoder::local(PAYLOAD_LIMIT);
    let mut buffer = vec![0u8; 64 * 1024];
    let why = loop {
        let deadline = conn.next_deadline();
        tokio::select! {
            read = reader.read(&mut buffer) => match read {
                Ok(0) | Err(_) => break "the service closed its host connection".to_owned(),
                Ok(n) => {
                    decoder.push(&buffer[..n]);
                    let mut broken = None;
                    loop {
                        match decoder.next() {
                            Ok(Some(Decoded::Frame(frame))) => {
                                if let Err(why) = conn.frame(frame) {
                                    broken = Some(why);
                                    break;
                                }
                            }
                            Ok(Some(Decoded::Skipped)) => {}
                            Ok(None) => break,
                            Err(close) => {
                                broken = Some(format!("a malformed frame: {}", close.reason));
                                break;
                            }
                        }
                    }
                    if let Some(why) = broken {
                        break format!("the host connection broke the protocol: {why}");
                    }
                }
            },
            Some((id, outcome)) = finished.recv() => conn.outcome(id, outcome),
            _ = sleep_until(deadline) => conn.expire(),
        }
        if writing.is_finished() {
            break "the host connection does not take what the host writes".to_owned();
        }
    };
    writing.abort();
    why
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

fn text_arg<'a>(object: &'a Map<String, Value>, name: &str) -> Result<Option<&'a str>, Failure> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(failure("invalid", format!("`{name}` is not a string"))),
    }
}

/// A key's bytes: 1 to 256 bytes of UTF-8 (a JSON string is UTF-8 already),
/// no control character (§7).
fn key_arg(object: &Map<String, Value>, name: &str) -> Result<Vec<u8>, Failure> {
    let key = text_arg(object, name)?.ok_or_else(|| failure("invalid", format!("`{name}` is missing")))?;
    check_key(key, name)?;
    Ok(key.as_bytes().to_vec())
}

fn check_key(key: &str, name: &str) -> Result<(), Failure> {
    if key.is_empty() {
        return Err(failure("invalid", format!("`{name}` is empty")));
    }
    if key.chars().any(char::is_control) {
        return Err(failure("invalid", format!("`{name}` holds a control character")));
    }
    Ok(())
}

/// The limits of a checked call (`too_large`), checked last (§6).
fn limits(call: &Call) -> Result<(), Failure> {
    let key = match call {
        Call::Get { key } | Call::Delete { key } => key,
        Call::Set { key, bytes } => {
            if *bytes > MAX_VALUE_BYTES {
                return Err(failure("too_large", format!("a value of {bytes} bytes is over {MAX_VALUE_BYTES}")));
            }
            key
        }
        Call::List { after: Some(after), .. } => after,
        Call::List { .. } | Call::Usage => return Ok(()),
    };
    if key.len() > MAX_KEY_BYTES {
        return Err(failure("too_large", format!("a key of {} bytes is over {MAX_KEY_BYTES}", key.len())));
    }
    Ok(())
}

/// A store call's arguments, checked in §6's order: the shape (`invalid`)
/// here, the limits (`too_large`) after the capability and the bound.
fn store_call(t: &str, object: &Map<String, Value>, body: bool) -> Result<Call, Failure> {
    let wants_body = t == "store.set";
    if body != wants_body {
        return Err(failure("invalid", if wants_body { "store.set carries its value as the request's body" } else { "this request takes no body" }));
    }
    Ok(match t {
        "store.get" => Call::Get { key: key_arg(object, "key")? },
        "store.delete" => Call::Delete { key: key_arg(object, "key")? },
        "store.set" => {
            let key = key_arg(object, "key")?;
            let bytes = object.get("bytes").and_then(Value::as_u64).ok_or_else(|| failure("invalid", "`bytes` is not a count of bytes"))?;
            Call::Set { key, bytes }
        }
        "store.list" => {
            let prefix = text_arg(object, "prefix")?.unwrap_or("");
            if prefix.chars().any(char::is_control) || prefix.len() > MAX_KEY_BYTES {
                return Err(failure("invalid", "`prefix` is not the start of a key"));
            }
            let after = text_arg(object, "after")?;
            if let Some(after) = after {
                check_key(after, "after")?;
            }
            let limit = match object.get("limit") {
                None | Some(Value::Null) => MAX_LIST,
                Some(value) => match value.as_u64() {
                    Some(limit) if (1..=MAX_LIST as u64).contains(&limit) => limit as usize,
                    _ => return Err(failure("invalid", format!("`limit` is not 1 to {MAX_LIST}"))),
                },
            };
            Call::List { prefix: prefix.as_bytes().to_vec(), after: after.map(|after| after.as_bytes().to_vec()), limit }
        }
        "store.usage" => Call::Usage,
        _ => unreachable!("a store call"),
    })
}

impl Conn {
    fn send(&self, frame: Bytes) {
        let _ = self.out.send(frame);
    }

    fn reply(&mut self, id: u32, result: Result<Value, Failure>) {
        let Some(request) = self.requests.remove(&id) else { return };
        let millis = request.started.elapsed().as_millis() as u64;
        let frame = match &result {
            Ok(value) => {
                tracing::info!(service = %self.session.app, t = request.t, outcome = "ok", millis, "service host request");
                encode_json(FrameType::Reply, id, value, JsonFrame::default(), PAYLOAD_LIMIT)
            }
            Err(failure) => {
                tracing::info!(service = %self.session.app, t = request.t, outcome = failure.code, millis, "service host request");
                // §6: `message` is at most 240 bytes, a sentence for the service's log.
                let error = json!({ "code": failure.code, "message": cut(&failure.message, 240) });
                encode_json(FrameType::Reply, id, &error, JsonFrame { error: true, ..Default::default() }, PAYLOAD_LIMIT)
            }
        };
        match frame {
            Ok(frame) => self.send(frame),
            Err(error) => tracing::warn!(service = %self.session.app, %error, "a reply did not fit its frame"),
        }
        if let State::Body { outstanding, .. } = request.state
            && outstanding > 0
        {
            self.ended_bodies.insert(id, outstanding);
            self.ended_order.push_back(id);
            while self.ended_order.len() > MAX_ENDED_BODIES {
                if let Some(old) = self.ended_order.pop_front() {
                    self.ended_bodies.remove(&old);
                }
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.requests
            .values()
            .filter(|request| !matches!(request.state, State::Working { write: true, .. }))
            .map(|request| request.started + STORE_CALL_LIMIT)
            .min()
    }

    /// §6: a store call ends at 30 s. A write whose body is complete is not
    /// ended: its commit decides the reply.
    fn expire(&mut self) {
        let now = Instant::now();
        let late: Vec<u32> = self
            .requests
            .iter()
            .filter(|(_, request)| !matches!(request.state, State::Working { write: true, .. }) && request.started + STORE_CALL_LIMIT <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in late {
            self.abandon_streams(id);
            self.reply(id, Err(failure("timeout", format!("the call did not finish within {} s", STORE_CALL_LIMIT.as_secs()))));
        }
    }

    /// RESET `cancelled` for a stream of `id` still open, either way.
    fn abandon_streams(&mut self, id: u32) {
        if matches!(self.requests.get(&id).map(|request| &request.state), Some(State::Body { .. } | State::Sending { .. })) {
            self.send(encode_reset(id, ResetCode::Cancelled));
        }
    }

    fn frame(&mut self, frame: Frame) -> Result<(), String> {
        match frame.ty {
            FrameType::Request => self.request(frame),
            FrameType::Data => self.data(frame),
            FrameType::Credit => self.credit(frame),
            FrameType::Cancel => {
                self.cancel(frame.id);
                Ok(())
            }
            FrameType::Reset => {
                self.reset(frame.id);
                Ok(())
            }
            // A HANDSHAKE after host.ready, and an EVENT (the service has
            // none to send), are ignored as on the control connection.
            FrameType::Handshake | FrameType::Event => Ok(()),
            FrameType::Reply => Err("a REPLY, but the host has asked nothing".into()),
            FrameType::Ping | FrameType::Pong | FrameType::Close => Err(format!("{:?}, which this connection does not carry", frame.ty)),
        }
    }

    fn request(&mut self, frame: Frame) -> Result<(), String> {
        let id = frame.id;
        if id == 0 || id > pocket_link::consts::ID_DEVICE_MAX {
            return Err(format!("a request id {id:#x} outside the service's range"));
        }
        if id <= self.last_id {
            return Err(format!("the request id {id} after {}: ids are not reused", self.last_id));
        }
        self.last_id = id;
        let body = frame.flags & pocket_link::frame::FLAG_BODY != 0;
        let json = parse_json(&frame);
        let t = json.t().map(str::to_owned);
        let object = json.object.unwrap_or_default();
        // The request is open from here, so its reply is the one REPLY.
        let name: &'static str = match t.as_deref() {
            Some("store.get") => "store.get",
            Some("store.set") => "store.set",
            Some("store.delete") => "store.delete",
            Some("store.list") => "store.list",
            Some("store.usage") => "store.usage",
            Some(other) => LATER.iter().find(|later| **later == other).copied().unwrap_or("unknown"),
            None => "unknown",
        };
        if self.requests.len() >= MAX_OPEN {
            self.requests.insert(id, Request { t: name, started: Instant::now(), state: State::Working { write: false } });
            self.reply(id, Err(failure("busy", format!("{MAX_OPEN} requests are open on this connection"))));
            return Ok(());
        }
        let open_store = self.requests.values().filter(|request| request.t.starts_with("store.")).count();
        self.requests.insert(id, Request { t: name, started: Instant::now(), state: State::Working { write: false } });
        let checked = (|| -> Result<Call, Failure> {
            let Some(t) = t.as_deref() else { return Err(failure("invalid", "the request is not a JSON object with a string `t`")) };
            if LATER.contains(&t) {
                return Err(failure("not_required", format!("{t}: this host provides no `{}` capability, so no manifest can require it", t.split('.').next().unwrap_or(t))));
            }
            if !t.starts_with("store.") || name == "unknown" {
                return Err(failure("invalid", format!("no request {t:?} on the host connection")));
            }
            let call = store_call(t, &object, body)?;
            if !self.session.requires.iter().any(|name| name == "store") || self.session.store.is_none() {
                return Err(failure("not_required", "the manifest's requires does not name store"));
            }
            if open_store >= MAX_STORE_OPEN {
                return Err(failure("busy", format!("{MAX_STORE_OPEN} store calls are open")));
            }
            limits(&call)?;
            Ok(call)
        })();
        match checked {
            Err(failure) => self.reply(id, Err(failure)),
            Ok(Call::Set { key, bytes }) => {
                let grant = bytes.min(BODY_WINDOW);
                if let Some(request) = self.requests.get_mut(&id) {
                    request.state = State::Body { key, declared: bytes, received: Vec::with_capacity(bytes as usize), outstanding: grant };
                }
                if grant > 0 {
                    self.send(encode_credit(id, grant as u32));
                }
            }
            Ok(call) => self.work(id, call),
        }
        Ok(())
    }

    /// Runs a checked call on the blocking pool.
    fn work(&mut self, id: u32, call: Call) {
        let binding = self.session.store.clone().expect("checked");
        let write = matches!(call, Call::Delete { .. });
        if let Some(request) = self.requests.get_mut(&id) {
            request.state = State::Working { write };
        }
        let done = self.done.clone();
        tokio::task::spawn_blocking(move || {
            let store = binding.stores.get(&binding.name);
            let outcome = match call {
                Call::Get { key } => Outcome::Value(store.and_then(|store| store.get(&key)).map_err(Failure::from)),
                Call::Delete { key } => Outcome::Reply(
                    store
                        .and_then(|store| store.delete(&key))
                        .map(|existed| {
                            if existed {
                                (binding.written)();
                            }
                            json!({ "existed": existed })
                        })
                        .map_err(Failure::from),
                ),
                Call::List { prefix, after, limit } => Outcome::Reply(
                    store
                        .and_then(|store| store.list(&prefix, after.as_deref(), limit))
                        .map(|(keys, more)| json!({ "keys": keys.into_iter().map(|key| String::from_utf8_lossy(&key).into_owned()).collect::<Vec<_>>(), "more": more }))
                        .map_err(Failure::from),
                ),
                Call::Usage => Outcome::Reply(
                    store
                        .and_then(|store| store.meta())
                        .map(|Meta { bytes, keys }| json!({ "bytes": bytes, "keys": keys, "maxBytes": (binding.max_bytes)() }))
                        .map_err(Failure::from),
                ),
                Call::Set { .. } => unreachable!("a set runs from its body"),
            };
            let _ = done.send((id, outcome));
        });
    }

    /// The last byte of a set's body came: the transaction runs, and its
    /// outcome is the reply whatever comes meanwhile (§7, `CANCEL`).
    fn commit(&mut self, id: u32, key: Vec<u8>, value: Vec<u8>) {
        let binding = self.session.store.clone().expect("checked");
        if let Some(request) = self.requests.get_mut(&id) {
            request.state = State::Working { write: true };
        }
        let done = self.done.clone();
        tokio::task::spawn_blocking(move || {
            let max = (binding.max_bytes)();
            let result = binding.stores.get(&binding.name).and_then(|store| store.set(&key, &value, max));
            if result.is_ok() {
                (binding.written)();
            }
            let _ = done.send((id, Outcome::Reply(result.map(|()| json!({})).map_err(Failure::from))));
        });
    }

    fn outcome(&mut self, id: u32, outcome: Outcome) {
        // A read cancelled meanwhile was answered then.
        if !matches!(self.requests.get(&id).map(|request| &request.state), Some(State::Working { .. })) {
            return;
        }
        match outcome {
            Outcome::Reply(result) => self.reply(id, result),
            Outcome::Value(Err(failure)) => self.reply(id, Err(failure)),
            Outcome::Value(Ok(None)) => self.reply(id, Ok(json!({ "absent": true }))),
            Outcome::Value(Ok(Some(value))) => {
                // The head step: the head event before any byte (§6).
                let bytes = value.len();
                match encode_json(FrameType::Event, id, &json!({ "t": "store.head", "bytes": bytes }), JsonFrame::default(), PAYLOAD_LIMIT) {
                    Ok(frame) => self.send(frame),
                    Err(error) => return self.reply(id, Err(failure("internal", error.0))),
                }
                if let Some(request) = self.requests.get_mut(&id) {
                    request.state = State::Sending { value: Bytes::from(value), sent: 0, credit: 0 };
                }
                self.pump(id);
            }
        }
    }

    /// Sends what a `store.get`'s credit allows; the reply after `END`.
    fn pump(&mut self, id: u32) {
        let Some(Request { state: State::Sending { value, sent, credit }, .. }) = self.requests.get_mut(&id) else { return };
        let mut frames = Vec::new();
        if value.is_empty() {
            frames.push(encode_data(id, &[], true));
        }
        while *sent < value.len() && *credit > 0 {
            let n = (value.len() - *sent).min(*credit as usize).min(PAYLOAD_LIMIT as usize);
            let end = *sent + n == value.len();
            frames.push(encode_data(id, &value[*sent..*sent + n], end));
            *sent += n;
            *credit -= n as u64;
        }
        let complete = *sent == value.len();
        let bytes = value.len();
        for frame in frames {
            self.send(frame);
        }
        if complete {
            self.reply(id, Ok(json!({ "bytes": bytes })));
        }
    }

    fn credit(&mut self, frame: Frame) -> Result<(), String> {
        let amount = u64::from(pocket_link::frame::parse_credit(&frame.payload));
        let id = frame.id;
        if let Some(Request { state: State::Sending { credit, .. }, .. }) = self.requests.get_mut(&id) {
            if *credit + amount > MAX_CREDIT {
                return Err(format!("CREDIT past {MAX_CREDIT} bytes on {id}"));
            }
            *credit += amount;
            self.pump(id);
        }
        // A CREDIT for a stream that has ended is late, not wrong.
        Ok(())
    }

    fn data(&mut self, frame: Frame) -> Result<(), String> {
        let id = frame.id;
        let length = frame.payload.len() as u64;
        let end = frame.flags & pocket_link::frame::FLAG_END != 0;
        let Some(Request { state: State::Body { key, declared, received, outstanding }, .. }) = self.requests.get_mut(&id) else {
            // In flight when its request ended: dropped within what was granted.
            if let Some(left) = self.ended_bodies.get_mut(&id) {
                if length > *left {
                    return Err(format!("DATA past the credit granted on {id}"));
                }
                *left -= length;
                if end || *left == 0 {
                    self.ended_bodies.remove(&id);
                }
                return Ok(());
            }
            // No credit was ever granted to it: only an empty END fits.
            return if length == 0 { Ok(()) } else { Err(format!("DATA past the credit granted on {id}")) };
        };
        if length > *outstanding {
            return Err(format!("DATA past the credit granted on {id}"));
        }
        *outstanding -= length;
        received.extend_from_slice(&frame.payload);
        let got = received.len() as u64;
        let declared = *declared;
        if end {
            if got != declared {
                return {
                    self.reply(id, Err(failure("invalid", format!("the body ended at {got} bytes, not the {declared} declared"))));
                    Ok(())
                };
            }
            let key = std::mem::take(key);
            let value = std::mem::take(received);
            self.commit(id, key, value);
            return Ok(());
        }
        // Credit follows what was taken, never past the declared count.
        let due = (declared - got).min(BODY_WINDOW).saturating_sub(*outstanding);
        if due > 0 && (due >= BODY_WINDOW / 4 || *outstanding == 0) {
            *outstanding += due;
            self.send(encode_credit(id, due as u32));
        }
        Ok(())
    }

    /// §7 `CANCEL`: before the body's last byte nothing is written and the
    /// reply is `cancelled`; after it the write completes and the set's own
    /// reply is sent. A read is answered `cancelled` at once.
    fn cancel(&mut self, id: u32) {
        match self.requests.get(&id).map(|request| &request.state) {
            None | Some(State::Working { write: true, .. }) => {}
            Some(State::Working { write: false, .. }) => self.reply(id, Err(failure("cancelled", "the service cancelled the call"))),
            Some(State::Body { .. } | State::Sending { .. }) => {
                self.abandon_streams(id);
                self.reply(id, Err(failure("cancelled", "the service cancelled the call")));
            }
        }
    }

    /// The service abandoned a stream of `id`: its body, or the value it
    /// was reading. The call ends `cancelled`, unless its write has started.
    fn reset(&mut self, id: u32) {
        self.ended_bodies.remove(&id);
        if matches!(self.requests.get(&id).map(|request| &request.state), Some(State::Body { .. } | State::Sending { .. })) {
            if let Some(Request { state: State::Body { outstanding, .. }, .. }) = self.requests.get_mut(&id) {
                // Nothing more comes on a stream its sender reset.
                *outstanding = 0;
            }
            self.reply(id, Err(failure("cancelled", "the service reset the call's stream")));
        }
    }
}

/// The `host.ready` handshake: the payload limit, the manifest's `requires`
/// and what this host provides (§6).
pub fn ready_frame(requires: &[String]) -> Bytes {
    let ready = json!({ "t": "host.ready", "payloadLimit": PAYLOAD_LIMIT, "requires": requires, "provides": crate::manifest::PROVIDED });
    encode_json(FrameType::Handshake, 0, &ready, JsonFrame::default(), MAX_HANDSHAKE).expect("host.ready within 1,024 bytes")
}
