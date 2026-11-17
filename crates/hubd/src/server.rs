//! The control socket: one reader and one writer task per client, and one
//! task per call.
//!
//! - **Hello first** (§1): a request before `hub.hello` is answered `-32600`
//!   and the connection stays open; a second hello is refused the same way.
//! - Each call runs in its own task, racing its cancel signal: a `cancel`
//!   notification, or the client's disconnection (which drops every signal),
//!   ends it with `cancelled`, and dropping the call's future is what stops
//!   the work (hub-core cancels the device request and releases the mutex).
//! - **The writer** sends answers, progress and events in one order: before
//!   each answer it writes the events already queued for the client (see
//!   `core` for why that makes answers and events consistent).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use pocket_hub_api::rpc::{self, RpcError};
use pocket_hub_api::types::{ClientInfo, HelloParams, HelloResult};
use pocket_hub_api::{ErrorKind, PROTOCOL, methods, topics};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::core::{Core, Published};
use crate::handlers;

/// The longest line a client may send.
const MAX_LINE_BYTES: usize = 1024 * 1024;

pub async fn accept_loop(core: Arc<Core>, listener: UnixListener) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(core.clone(), stream));
            }
            Err(error) => {
                tracing::warn!(%error, "the control socket's accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

/// What the writer writes.
pub enum Out {
    Line(String),
    /// A line, then a signal once it is flushed (the answer to `hub.shutdown`).
    Flushed(String, oneshot::Sender<()>),
}

/// The topics a client subscribed to: None before `hub.subscribe`.
type Topics = Arc<Mutex<Option<HashSet<String>>>>;

/// What a call's task knows of its client.
#[derive(Clone)]
pub struct Client {
    pub conn: u64,
    pub info: ClientInfo,
    pub out: mpsc::UnboundedSender<Out>,
    pub topics: Topics,
}

impl Client {
    pub fn send(&self, line: String) {
        let _ = self.out.send(Out::Line(line));
    }
}

fn answer(id: &Value, result: Result<Value, RpcError>) -> String {
    match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}).to_string(),
    }
}

async fn connection(core: Arc<Core>, stream: UnixStream) {
    let conn = core.next_conn();
    let (read, write) = stream.into_split();
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let topics: Topics = Arc::default();
    let writer = tokio::spawn(write_loop(core.clone(), write, out_rx, core.subscribe(), topics.clone()));
    // Each open call's cancel signal, by its id's JSON text.
    let calls: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>> = Arc::default();
    let mut client: Option<Client> = None;
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader).take(MAX_LINE_BYTES as u64 + 1).read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() > MAX_LINE_BYTES && !line.ends_with(b"\n") {
            let _ = out_tx.send(Out::Line(answer(&Value::Null, Err(RpcError::invalid_request("A request is longer than 1 MiB.")))));
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(_) => {
                let _ = out_tx.send(Out::Line(answer(&Value::Null, Err(RpcError::protocol(rpc::PARSE_ERROR, "The line is not JSON.")))));
                continue;
            }
        };
        let id = value.get("id").cloned();
        let method = value.get("method").and_then(Value::as_str).map(str::to_owned);
        let well_formed = value.get("jsonrpc").and_then(Value::as_str) == Some("2.0") && method.is_some() && id.as_ref().is_none_or(|id| id.is_number() || id.is_string());
        if !well_formed {
            let _ = out_tx.send(Out::Line(answer(id.as_ref().unwrap_or(&Value::Null), Err(RpcError::invalid_request("The line is not a JSON-RPC 2.0 request.")))));
            continue;
        }
        let method = method.expect("checked");
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            // A notification: `cancel` is the one a client sends. One for an
            // id that names no open call is ignored.
            if method == methods::CANCEL
                && let Some(target) = params.get("id")
                && let Some(signal) = calls.lock().unwrap().remove(&target.to_string())
            {
                let _ = signal.send(());
            }
            continue;
        };

        if method == methods::HELLO {
            let result = match &client {
                Some(_) => Err(RpcError::invalid_request("hub.hello was already sent on this connection.")),
                None => hello(&core, params).map(|(info, result)| {
                    client = Some(Client { conn, info, out: out_tx.clone(), topics: topics.clone() });
                    result
                }),
            };
            let _ = out_tx.send(Out::Line(answer(&id, result)));
            continue;
        }
        let Some(client) = client.clone() else {
            let _ = out_tx.send(Out::Line(answer(&id, Err(RpcError::invalid_request("The first request must be hub.hello.")))));
            continue;
        };
        let key = id.to_string();
        let (signal, cancelled) = oneshot::channel::<()>();
        {
            let mut open = calls.lock().unwrap();
            if open.contains_key(&key) {
                drop(open);
                let _ = out_tx.send(Out::Line(answer(&id, Err(RpcError::invalid_request(format!("The id {key} names a call that is still open."))))));
                continue;
            }
            open.insert(key.clone(), signal);
        }
        let (core, calls) = (core.clone(), calls.clone());
        tokio::spawn(async move {
            let result = tokio::select! {
                result = handlers::dispatch(&core, &client, &id, &method, params) => result,
                _ = cancelled => Err(RpcError::op(ErrorKind::Cancelled, "The call was cancelled.")),
            };
            calls.lock().unwrap().remove(&key);
            if method == methods::SHUTDOWN && result.is_ok() {
                // The answer is written before the socket closes (§1).
                let (flushed, written) = oneshot::channel();
                let _ = client.out.send(Out::Flushed(answer(&id, result), flushed));
                let _ = tokio::time::timeout(std::time::Duration::from_secs(2), written).await;
                core.request_shutdown();
            } else {
                client.send(answer(&id, result));
            }
        });
    }
    // The client went away: its calls are cancelled by the drop of their
    // signals, and what it held is released.
    calls.lock().unwrap().clear();
    core.client_gone(conn);
    drop(out_tx);
    drop(client);
    let _ = writer.await;
}

fn hello(core: &Core, params: Value) -> Result<(ClientInfo, Value), RpcError> {
    let hello: HelloParams = serde_json::from_value(params).map_err(|error| RpcError::invalid_params(format!("hub.hello takes {{protocol, client: {{kind, name, version}}}}: {error}.")))?;
    if hello.protocol != PROTOCOL {
        return Err(RpcError::other_protocol(PROTOCOL, hello.protocol));
    }
    let result = HelloResult { protocol: PROTOCOL, hub: core.hub_info() };
    Ok((hello.client, serde_json::to_value(result).expect("serializes")))
}

async fn write_loop(core: Arc<Core>, mut writer: OwnedWriteHalf, mut out: mpsc::UnboundedReceiver<Out>, mut events: broadcast::Receiver<Arc<Published>>, topics: Topics) {
    use broadcast::error::{RecvError, TryRecvError};
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let mut flushed = None;
        tokio::select! {
            biased;
            message = out.recv() => {
                let Some(message) = message else { break };
                // Every event queued before this answer goes first.
                loop {
                    match events.try_recv() {
                        Ok(event) => queue_event(&event, &topics, &mut buffer),
                        Err(TryRecvError::Lagged(_)) => resync(&core, &topics, &mut buffer),
                        Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                    }
                }
                let line = match message {
                    Out::Line(line) => line,
                    Out::Flushed(line, signal) => {
                        flushed = Some(signal);
                        line
                    }
                };
                buffer.extend_from_slice(line.as_bytes());
                buffer.push(b'\n');
            }
            event = events.recv() => match event {
                Ok(event) => queue_event(&event, &topics, &mut buffer),
                Err(RecvError::Lagged(_)) => resync(&core, &topics, &mut buffer),
                Err(RecvError::Closed) => {}
            },
        }
        if buffer.is_empty() {
            continue;
        }
        if writer.write_all(&buffer).await.is_err() || writer.flush().await.is_err() {
            break;
        }
        if let Some(signal) = flushed {
            let _ = signal.send(());
        }
    }
    let _ = writer.shutdown().await;
}

fn wanted(topics: &Topics, topic: &str) -> bool {
    topics.lock().unwrap().as_ref().is_some_and(|set| set.contains(topic))
}

fn queue_event(event: &Published, topics: &Topics, buffer: &mut Vec<u8>) {
    if wanted(topics, event.topic) {
        buffer.extend_from_slice(event.line.as_bytes());
        buffer.push(b'\n');
    }
}

/// This client lost events to a full queue: every device, the pairing
/// requests and every service again, as they are now, which leaves it holding
/// the truth, and `store.changed` for the catalog held, since a lost one
/// would leave it with an old offer.
fn resync(core: &Core, topics: &Topics, buffer: &mut Vec<u8>) {
    tracing::warn!("a client fell behind on events; it gets every device, pairing request and service again");
    let _guard = core.publication();
    let mut add = |event: pocket_hub_api::Event| {
        if wanted(topics, event.topic()) {
            buffer.extend_from_slice(json!({"jsonrpc": "2.0", "method": methods::EVENT, "params": event}).to_string().as_bytes());
            buffer.push(b'\n');
        }
    };
    for device in core.devices() {
        add(pocket_hub_api::Event::DeviceChanged { device: Box::new(device) });
    }
    add(pocket_hub_api::Event::PairingChanged { requests: core.pair_requests() });
    for service in core.services.services() {
        add(pocket_hub_api::Event::ServiceChanged { service });
    }
    if let Some(catalog) = core.applets.held() {
        add(pocket_hub_api::Event::StoreChanged { fetched_at: catalog.fetched_at });
    }
}

/// `hub.subscribe`: the topic list, replacing any earlier one.
pub fn subscribe(client: &Client, list: Option<Vec<String>>) -> Result<(), RpcError> {
    let set: HashSet<String> = match list {
        None => topics::ALL.iter().map(|topic| topic.to_string()).collect(),
        Some(list) => {
            if let Some(unknown) = list.iter().find(|topic| !topics::ALL.contains(&topic.as_str())) {
                return Err(RpcError::invalid_params(format!("There is no topic {unknown}.")));
            }
            list.into_iter().collect()
        }
    };
    *client.topics.lock().unwrap() = Some(set);
    Ok(())
}
