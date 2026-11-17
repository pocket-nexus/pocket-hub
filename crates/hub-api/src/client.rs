//! A client of the control socket: requests with their progress, `cancel`,
//! and the events after `hub.subscribe`.
//!
//! One task reads the socket and routes each line: an answer to the call
//! that waits for it, a `progress` notification to that call's progress
//! queue, an `event` to the event queue. Writes take a lock, one line at a
//! time.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::rpc::RpcError;
use crate::types::{ClientInfo, Event, HelloParams, HelloResult};
use crate::{PROTOCOL, methods};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No core answers on the socket.
    #[error("no Pocket Hub core answers on {path}: {error}")]
    Unreachable { path: String, error: std::io::Error },
    /// The connection ended before the answer.
    #[error("the connection to the Pocket Hub core closed")]
    Closed,
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error("an answer could not be read: {0}")]
    Decode(String),
}

/// Every line sent and received, in order: the tests keep examples of the
/// contract from it.
#[derive(Clone, Default)]
pub struct Transcript(Arc<Mutex<Vec<(bool, Value)>>>);

impl Transcript {
    /// `(sent, line)` pairs, oldest first; `sent` is true for the client's lines.
    pub fn lines(&self) -> Vec<(bool, Value)> {
        self.0.lock().unwrap().clone()
    }
    fn push(&self, sent: bool, value: Value) {
        self.0.lock().unwrap().push((sent, value));
    }
}

struct Waiting {
    progress: mpsc::UnboundedSender<Value>,
    done: oneshot::Sender<Result<Value, RpcError>>,
}

type Table = Arc<Mutex<HashMap<u64, Waiting>>>;

pub struct Client {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    next_id: AtomicU64,
    waiting: Table,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<Value>>,
    transcript: Option<Transcript>,
    reader: JoinHandle<()>,
    hello: Option<HelloResult>,
    /// True once the core's side of the connection has ended.
    closed: watch::Receiver<bool>,
}

/// One request in flight.
pub struct Call {
    pub id: u64,
    progress: mpsc::UnboundedReceiver<Value>,
    done: oneshot::Receiver<Result<Value, RpcError>>,
}

impl Call {
    /// The next progress notification's params (without `id`), or None once
    /// the call has answered.
    pub async fn progress(&mut self) -> Option<Value> {
        self.progress.recv().await
    }

    /// The answer, handing each progress notification to `on_progress` first.
    pub async fn finish_with(mut self, mut on_progress: impl FnMut(Value)) -> Result<Value, ClientError> {
        while let Some(progress) = self.progress.recv().await {
            on_progress(progress);
        }
        match self.done.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(ClientError::Rpc(error)),
            Err(_) => Err(ClientError::Closed),
        }
    }

    pub async fn finish(self) -> Result<Value, ClientError> {
        self.finish_with(|_| {}).await
    }
}

impl Client {
    /// Connects and says hello.
    pub async fn connect(socket: &Path, client: ClientInfo) -> Result<Client, ClientError> {
        Self::connect_traced(socket, client, None).await
    }

    pub async fn connect_traced(socket: &Path, client: ClientInfo, transcript: Option<Transcript>) -> Result<Client, ClientError> {
        Self::connect_in(socket, client, PROTOCOL, transcript).await
    }

    /// Connects and says hello in `protocol`. Another protocol than
    /// [`PROTOCOL`] is for the two frozen methods alone, `hub.hello` and
    /// `hub.shutdown` (CONTROL-API.md §1, Hello): nothing else is ever called
    /// in another version's dialect.
    pub async fn connect_in(socket: &Path, client: ClientInfo, protocol: u32, transcript: Option<Transcript>) -> Result<Client, ClientError> {
        let mut connection = Self::connect_raw(socket, transcript).await?;
        let hello = connection.call(methods::HELLO, &HelloParams { protocol, client }).await?;
        connection.hello = Some(serde_json::from_value(hello).map_err(|error| ClientError::Decode(error.to_string()))?);
        Ok(connection)
    }

    /// Connects without a hello (the tests of the hello rule use it).
    pub async fn connect_raw(socket: &Path, transcript: Option<Transcript>) -> Result<Client, ClientError> {
        let stream = UnixStream::connect(socket).await.map_err(|error| ClientError::Unreachable { path: socket.display().to_string(), error })?;
        let (read, write) = stream.into_split();
        let waiting: Table = Arc::default();
        let (events_tx, events) = mpsc::unbounded_channel();
        let transcript_for_reader = transcript.clone();
        let table = waiting.clone();
        let (closed_tx, closed) = watch::channel(false);
        let reader = tokio::spawn(async move {
            read_loop(BufReader::new(read), waiting, events_tx, transcript_for_reader).await;
            let _ = closed_tx.send(true);
        });
        Ok(Client {
            writer: tokio::sync::Mutex::new(write),
            next_id: AtomicU64::new(1),
            waiting: table,
            events: tokio::sync::Mutex::new(events),
            transcript,
            reader,
            hello: None,
            closed,
        })
    }

    /// The hub's answer to the hello.
    pub fn hello(&self) -> Option<&HelloResult> {
        self.hello.as_ref()
    }

    /// Writes one line as it is (a test sends malformed ones).
    pub async fn send_line(&self, line: &str) -> std::io::Result<()> {
        if let Some(transcript) = &self.transcript
            && let Ok(value) = serde_json::from_str(line)
        {
            transcript.push(true, value);
        }
        let mut writer = self.writer.lock().await;
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await
    }

    async fn send(&self, value: Value) -> Result<(), ClientError> {
        self.send_line(&value.to_string()).await.map_err(|_| ClientError::Closed)
    }

    /// Sends a request and returns it in flight.
    pub async fn start(&self, method: &str, params: &impl Serialize) -> Result<Call, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (progress_tx, progress) = mpsc::unbounded_channel();
        let (done_tx, done) = oneshot::channel();
        self.waiting.lock().unwrap().insert(id, Waiting { progress: progress_tx, done: done_tx });
        let params = serde_json::to_value(params).map_err(|error| ClientError::Decode(error.to_string()))?;
        let request = match params {
            Value::Null => json!({"jsonrpc": "2.0", "id": id, "method": method}),
            params => json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        };
        if let Err(error) = self.send(request).await {
            self.waiting.lock().unwrap().remove(&id);
            return Err(error);
        }
        Ok(Call { id, progress, done })
    }

    /// A request's answer.
    pub async fn call(&self, method: &str, params: &impl Serialize) -> Result<Value, ClientError> {
        self.start(method, params).await?.finish().await
    }

    /// A request's answer, decoded.
    pub async fn call_as<T: DeserializeOwned>(&self, method: &str, params: &impl Serialize) -> Result<T, ClientError> {
        let value = self.call(method, params).await?;
        serde_json::from_value(value).map_err(|error| ClientError::Decode(error.to_string()))
    }

    /// Asks Hub to stop the call `id`.
    pub async fn cancel(&self, id: u64) -> Result<(), ClientError> {
        self.send(json!({"jsonrpc": "2.0", "method": methods::CANCEL, "params": {"id": id}})).await
    }

    /// The next event's params (`topic` included), or None when the
    /// connection ended.
    pub async fn next_event(&self) -> Option<Value> {
        self.events.lock().await.recv().await
    }

    /// The next event, decoded.
    pub async fn next_event_typed(&self) -> Option<Result<Event, ClientError>> {
        let value = self.next_event().await?;
        Some(serde_json::from_value(value).map_err(|error| ClientError::Decode(error.to_string())))
    }

    /// Resolves when the core has ended the connection (after
    /// `hub.shutdown`, this and the socket file's removal are what say the
    /// core is gone).
    pub async fn closed(&self) {
        let mut closed = self.closed.clone();
        let _ = closed.wait_for(|closed| *closed).await;
    }

    /// An event already received, without waiting.
    pub fn try_event(&self) -> Option<Value> {
        self.events.try_lock().ok()?.try_recv().ok()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

async fn read_loop(
    mut reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    waiting: Table,
    events: mpsc::UnboundedSender<Value>,
    transcript: Option<Transcript>,
) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(transcript) = &transcript {
            transcript.push(false, value.clone());
        }
        match value.get("method").and_then(Value::as_str) {
            Some(methods::PROGRESS) => {
                let mut params = value.get("params").cloned().unwrap_or(Value::Null);
                let id = params.get("id").and_then(Value::as_u64);
                if let Value::Object(map) = &mut params {
                    map.remove("id");
                }
                if let Some(id) = id
                    && let Some(waiting) = waiting.lock().unwrap().get(&id)
                {
                    let _ = waiting.progress.send(params);
                }
            }
            Some(methods::EVENT) => {
                let _ = events.send(value.get("params").cloned().unwrap_or(Value::Null));
            }
            Some(_) => {}
            None => {
                let Some(id) = value.get("id").and_then(Value::as_u64) else { continue };
                let Some(entry) = waiting.lock().unwrap().remove(&id) else { continue };
                let outcome = match value.get("error") {
                    Some(error) => Err(serde_json::from_value(error.clone()).unwrap_or_else(|_| RpcError::protocol(0, error.to_string()))),
                    None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                };
                // The progress queue closes with the answer, after every
                // notification that came before it.
                let Waiting { progress, done } = entry;
                drop(progress);
                let _ = done.send(outcome);
            }
        }
    }
    // The connection ended: every call waiting is answered `Closed` by the
    // drop of its sender.
    waiting.lock().unwrap().clear();
}
