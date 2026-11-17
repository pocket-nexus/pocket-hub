//! What the integration tests share: a Hub on loopback with a temporary
//! store, the reference device under Bun driven by JSON lines, a raw device
//! written with the codec for the cases the reference never produces, and a
//! fake service host.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use pocket_hub_core::{
    BoxFuture, ChannelHost, ChannelOffer, ChannelOpenError, ChannelStream, Endpoint, EndpointConfig, HubEvent, TcpCarrier, Timing,
};
use pocket_hub_store::{DeviceRecord, Store};
use pocket_link::frame::{self, Decoded, Dialect, Frame, FrameDecoder, FrameType};
use pocket_link::pairing::{CompanionFile, format_companion_file, format_device_file};
use pocket_link::text::{new_key, new_link_id};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, mpsc};

pub const WAIT: Duration = Duration::from_secs(10);

pub fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pocket-hub-core-{label}-{}", new_link_id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Timers short enough for tests; each test lowers the one it exercises further.
pub fn fast_config() -> EndpointConfig {
    EndpointConfig {
        timing: Timing { tick: Duration::from_millis(20), mutex_wait: Duration::from_secs(5), ..Timing::default() },
        beacons: false,
        ..EndpointConfig::default()
    }
}

pub struct Hub {
    pub endpoint: Endpoint,
    pub store: Arc<Store>,
    pub port: u16,
    pub events: broadcast::Receiver<HubEvent>,
    pub dir: PathBuf,
}

impl Hub {
    pub async fn start(config: EndpointConfig, host: Option<Arc<dyn ChannelHost>>) -> Hub {
        let dir = temp_dir("store");
        let store = Arc::new(Store::open(dir.join("store")).unwrap());
        let carrier = TcpCarrier::bind(Some(0)).await.unwrap();
        let mut builder = Endpoint::builder(store.clone()).config(config).listener(Box::new(carrier));
        if let Some(host) = host {
            builder = builder.channel_host(host);
        }
        let endpoint = builder.start();
        let port = endpoint.ports()[0].1.unwrap();
        let events = endpoint.subscribe();
        Hub { endpoint, store, port, events, dir }
    }

    /// Writes a pairing as Hub's setup does: `device.txt` and the key file in a
    /// fresh device folder, the same key in the store. Returns the folder and the id.
    pub fn pair(&self) -> (PathBuf, String, String) {
        let dir = self.dir.join(format!("device-{}", new_link_id()));
        std::fs::create_dir_all(dir.join("companions")).unwrap();
        let id = new_link_id();
        let key = new_key();
        std::fs::write(dir.join("device.txt"), format_device_file(&id)).unwrap();
        let file = CompanionFile { key: key.clone(), name: self.store.hub_name(), address: "127.0.0.1".into(), port: self.port };
        std::fs::write(dir.join("companions").join(format!("{}.txt", self.store.hub_id())), format_companion_file(&file)).unwrap();
        self.store
            .put_device(DeviceRecord {
                id: id.clone(),
                kind: "3ds".into(),
                name: String::new(),
                model: String::new(),
                key: key.clone(),
                target: String::new(),
                host_abi: 0,
                last_address: None,
                last_seen: None,
                setup_unconfirmed: true,
            })
            .unwrap();
        (dir, id, key)
    }

    /// The next event that matches, within `WAIT`.
    pub async fn event(&mut self, mut matches: impl FnMut(&HubEvent) -> bool) -> HubEvent {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let event = tokio::time::timeout_at(deadline, self.events.recv()).await.expect("no matching hub event in time").expect("events");
            if matches(&event) {
                return event;
            }
        }
    }

    pub async fn linked(&mut self, device: &str) {
        let device = device.to_owned();
        self.event(move |e| matches!(e, HubEvent::Linked { device: d, .. } if *d == device)).await;
    }
}

// ---------------------------------------------------------------------------
// The reference device under Bun
// ---------------------------------------------------------------------------

pub struct Device {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::UnboundedReceiver<Value>,
    seen: VecDeque<Value>,
    pub id: String,
}

impl Device {
    pub async fn spawn(hub: &Hub, dir: &std::path::Path, extra: Value) -> Device {
        let mut config = json!({"dir": dir, "hub": hub.store.hub_id(), "address": "127.0.0.1", "port": hub.port});
        if let (Value::Object(config), Value::Object(extra)) = (&mut config, extra) {
            config.extend(extra);
        }
        // From any crate that includes this module (hub-net shares it).
        let harness = concat!(env!("CARGO_MANIFEST_DIR"), "/../hub-core/tests/harness/device.ts");
        let mut child = Command::new("bun")
            .arg(harness)
            .arg(config.to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("bun runs the reference device");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        let _ = tx.send(value);
                    }
                    Err(_) => eprintln!("device: {line}"),
                }
            }
        });
        let mut device = Device { child, stdin, lines, seen: VecDeque::new(), id: String::new() };
        let ready = device.expect("ready").await;
        device.id = ready["device"].as_str().unwrap().to_owned();
        device
    }

    pub async fn send(&mut self, command: Value) {
        let mut line = command.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    /// The next event satisfying `matches`, from the events already seen or
    /// the ones to come; the others stay for later calls.
    pub async fn wait(&mut self, mut matches: impl FnMut(&Value) -> bool) -> Value {
        if let Some(index) = self.seen.iter().position(&mut matches) {
            return self.seen.remove(index).unwrap();
        }
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let value = tokio::time::timeout_at(deadline, self.lines.recv()).await.unwrap_or_else(|_| panic!("no matching device event in time; seen: {:?}", self.seen)).expect("the device runs");
            if matches(&value) {
                return value;
            }
            self.seen.push_back(value);
        }
    }

    pub async fn expect(&mut self, ev: &str) -> Value {
        let ev = ev.to_owned();
        self.wait(move |v| v["ev"] == ev.as_str()).await
    }

    /// The reply of a channel request sent with this tag.
    pub async fn reply(&mut self, tag: &str) -> Value {
        let tag = tag.to_owned();
        self.wait(move |v| v["ev"] == "reply" && v["tag"] == tag.as_str()).await
    }

    /// True when no event satisfying `matches` arrives within `within`.
    pub async fn quiet(&mut self, within: Duration, mut matches: impl FnMut(&Value) -> bool) -> bool {
        if self.seen.iter().any(&mut matches) {
            return false;
        }
        let deadline = tokio::time::Instant::now() + within;
        while let Ok(Some(value)) = tokio::time::timeout_at(deadline, self.lines.recv()).await {
            if matches(&value) {
                return false;
            }
            self.seen.push_back(value);
        }
        true
    }

    pub fn pid(&self) -> u32 {
        self.child.id().expect("the device runs")
    }

    pub async fn kill(mut self) {
        let _ = self.child.kill().await;
    }
}

/// The byte pattern of the harness: byte i of a stream.
pub fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|i| ((i * 7 + 13) & 0xff) as u8).collect()
}

/// FNV-1a 32, as the harness computes it.
pub fn fnv32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

// ---------------------------------------------------------------------------
// A raw device: the codec by hand
// ---------------------------------------------------------------------------

pub struct Raw {
    pub reader: tokio::net::tcp::OwnedReadHalf,
    pub writer: tokio::net::tcp::OwnedWriteHalf,
    pub decoder: FrameDecoder,
}

impl Raw {
    pub async fn connect(port: u16) -> Raw {
        let socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        socket.set_nodelay(true).unwrap();
        let (reader, writer) = socket.into_split();
        Raw { reader, writer, decoder: FrameDecoder::new() }
    }

    pub async fn write(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).await.unwrap();
    }

    pub async fn read_exact(&mut self, n: usize) -> Option<Vec<u8>> {
        let mut out = vec![0u8; n];
        tokio::time::timeout(WAIT, self.reader.read_exact(&mut out)).await.ok()?.ok()?;
        Some(out)
    }

    /// The next frame, or None when the connection ends.
    pub async fn frame(&mut self) -> Option<Frame> {
        let mut buffer = vec![0u8; 65536];
        loop {
            match self.decoder.next() {
                Ok(Some(Decoded::Frame(frame))) => return Some(frame),
                Ok(Some(Decoded::Skipped)) => continue,
                Ok(None) => {}
                Err(close) => panic!("hub sent a bad frame: {close}"),
            }
            let n = tokio::time::timeout(WAIT, self.reader.read(&mut buffer)).await.ok()?.ok()?;
            if n == 0 {
                return None;
            }
            self.decoder.push(&buffer[..n]);
        }
    }

    /// Reads until the connection ends; true when it ended within `within`.
    pub async fn ends_within(&mut self, within: Duration) -> bool {
        let mut buffer = vec![0u8; 65536];
        let deadline = tokio::time::Instant::now() + within;
        loop {
            match tokio::time::timeout_at(deadline, self.reader.read(&mut buffer)).await {
                Err(_) => return false,
                Ok(Ok(0)) | Ok(Err(_)) => return true,
                Ok(Ok(n)) => self.decoder.push(&buffer[..n]),
            }
        }
    }

    pub fn hello(device: &str, companion: &str, nonce: &str, max_payload: u32) -> Value {
        Raw::hello_with(device, companion, nonce, max_payload, &["runtime", "apps", "channel"])
    }

    pub fn hello_with(device: &str, companion: &str, nonce: &str, max_payload: u32, features: &[&str]) -> Value {
        json!({"t":"hello","device":device,"companion":companion,"nonce":nonce,"name":"Raw","model":"Raw 3DS","target":"3ds-dev","hostAbi":7,"features":features,"maxPayload":max_payload})
    }

    /// The whole handshake of a version-2 device; returns the `ready` object.
    pub async fn admit(port: u16, device: &str, companion: &str, key: &str) -> (Raw, Value) {
        Raw::admit_with(port, device, companion, key, &["runtime", "apps", "channel"]).await
    }

    /// The handshake with a feature list of the test's choosing.
    pub async fn admit_with(port: u16, device: &str, companion: &str, key: &str, features: &[&str]) -> (Raw, Value) {
        let mut raw = Raw::connect(port).await;
        let nonce = pocket_link::text::new_nonce();
        let mut first = pocket_link::preamble::encode(pocket_link::preamble::Preamble { highest: 2, lowest: 2, selected: 0 }).to_vec();
        first.extend_from_slice(&pocket_link::handshake::encode(&Raw::hello_with(device, companion, &nonce, 16384, features)).unwrap());
        raw.write(&first).await;
        let answer = raw.read_exact(8).await.unwrap();
        assert_eq!(answer[6], 2);
        let challenge = frame::parse_json(&raw.frame().await.unwrap()).object.unwrap();
        assert_eq!(challenge["t"], "challenge");
        let hub_nonce = challenge["nonce"].as_str().unwrap();
        let expected = pocket_link::handshake::companion_proof(key, &nonce, hub_nonce, device, companion).unwrap();
        assert_eq!(challenge["proof"].as_str().unwrap(), expected);
        let proof = pocket_link::handshake::console_proof(key, &nonce, hub_nonce, device, companion).unwrap();
        raw.write(&pocket_link::handshake::encode(&json!({"t":"auth","proof":proof})).unwrap()).await;
        let ready = frame::parse_json(&raw.frame().await.unwrap()).object.unwrap();
        assert_eq!(ready["t"], "ready");
        raw.decoder.ready(16384);
        (raw, Value::Object(ready))
    }
}

// ---------------------------------------------------------------------------
// A fake service host
// ---------------------------------------------------------------------------

/// The service's end of one channel connection, in the local dialect.
pub struct Service {
    pub offer: ChannelOffer,
    reader: ReadHalf<DuplexStream>,
    writer: WriteHalf<DuplexStream>,
    decoder: FrameDecoder,
    pub channel: u32,
    pub message_limit: u64,
    next_id: u32,
    /// The supervisor's stop signal for this channel.
    pub stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Service {
    /// Reads `channel.ready`, which the relay writes first.
    async fn attach(offer: ChannelOffer, stream: DuplexStream, stop: tokio::sync::oneshot::Sender<()>) -> Service {
        let (reader, writer) = tokio::io::split(stream);
        let limit = offer.payload_limit;
        let mut service = Service { offer, reader, writer, decoder: FrameDecoder::local(limit), channel: 0, message_limit: 0, next_id: 1, stop: Some(stop) };
        let ready = service.frame().await.expect("channel.ready");
        assert_eq!(ready.ty, FrameType::Handshake);
        let object = frame::parse_json(&ready).object.unwrap();
        assert_eq!(object["t"], "channel.ready");
        service.channel = object["channel"].as_u64().unwrap() as u32;
        service.message_limit = object["messageLimit"].as_u64().unwrap();
        service
    }

    pub async fn frame(&mut self) -> Option<Frame> {
        self.frame_within(WAIT).await
    }

    pub async fn frame_within(&mut self, within: Duration) -> Option<Frame> {
        let mut buffer = vec![0u8; 65536];
        let deadline = tokio::time::Instant::now() + within;
        loop {
            match self.decoder.next() {
                Ok(Some(Decoded::Frame(frame))) => return Some(frame),
                Ok(Some(Decoded::Skipped)) => continue,
                Ok(None) => {}
                Err(close) => panic!("hub sent the service a bad frame: {close}"),
            }
            let n = tokio::time::timeout_at(deadline, self.reader.read(&mut buffer)).await.ok()?.ok()?;
            if n == 0 {
                return None;
            }
            self.decoder.push(&buffer[..n]);
        }
    }

    /// The next frame that is not a CREDIT (credit is checked where it matters).
    pub async fn next_non_credit(&mut self) -> Frame {
        loop {
            let frame = self.frame().await.expect("a frame from hub");
            if frame.ty != FrameType::Credit {
                return frame;
            }
        }
    }

    pub async fn send(&mut self, ty: FrameType, flags: u8, id: u32, payload: &[u8]) {
        let bytes = frame::encode_in(Dialect::Local, ty, flags, id, payload, u32::MAX).unwrap();
        self.writer.write_all(&bytes).await.unwrap();
    }

    pub async fn send_json(&mut self, ty: FrameType, flags: u8, id: u32, object: &Value) {
        self.send(ty, flags, id, object.to_string().as_bytes()).await;
    }

    /// Several frames in one write.
    pub async fn send_all(&mut self, frames: &[(FrameType, u8, u32, Vec<u8>)]) {
        let mut bytes = Vec::new();
        for (ty, flags, id, payload) in frames {
            bytes.extend_from_slice(&frame::encode_in(Dialect::Local, *ty, *flags, *id, payload, u32::MAX).unwrap());
        }
        self.writer.write_all(&bytes).await.unwrap();
    }

    pub async fn credit(&mut self, id: u32, amount: u32) {
        self.send(FrameType::Credit, 0, id, &amount.to_le_bytes()).await;
    }

    /// The next id of the service's own counter (Hub's range).
    pub fn take_id(&mut self) -> u32 {
        let id = self.next_id | 0x8000_0000;
        self.next_id += 1;
        id
    }

    /// The service closes its socket.
    pub async fn hang_up(mut self) {
        let _ = self.writer.shutdown().await;
    }
}

pub enum HostNotice {
    Attached(Box<Service>),
    /// An open's future was dropped before it finished (a cancelled open).
    Dropped(String),
    /// A `slow.service` open reached the host and waits.
    Opening(String),
}

/// A service host that answers by applet id: `no.service`, `busy.service`,
/// `rejected.app`, `slow.service` (never answers), and any other id attaches a
/// service whose end the test gets on `notices`.
pub struct FakeHost {
    notices: mpsc::UnboundedSender<HostNotice>,
}

impl FakeHost {
    pub fn new() -> (Arc<FakeHost>, mpsc::UnboundedReceiver<HostNotice>) {
        let (notices, rx) = mpsc::unbounded_channel();
        (Arc::new(FakeHost { notices }), rx)
    }
}

struct DropNotice(Option<(mpsc::UnboundedSender<HostNotice>, String)>);
impl Drop for DropNotice {
    fn drop(&mut self) {
        if let Some((tx, app)) = self.0.take() {
            let _ = tx.send(HostNotice::Dropped(app));
        }
    }
}

impl ChannelHost for FakeHost {
    fn open(&self, offer: ChannelOffer) -> BoxFuture<'static, Result<ChannelStream, ChannelOpenError>> {
        let notices = self.notices.clone();
        Box::pin(async move {
            match offer.app.as_str() {
                "no.service" => return Err(ChannelOpenError::NoService),
                "busy.service" => return Err(ChannelOpenError::Busy("the service did not start".into())),
                "rejected.app" => return Err(ChannelOpenError::Rejected("policy".into())),
                "slow.service" => {
                    let _ = notices.send(HostNotice::Opening(offer.app.clone()));
                    let mut notice = DropNotice(Some((notices.clone(), offer.app.clone())));
                    std::future::pending::<()>().await;
                    notice.0 = None;
                    unreachable!()
                }
                _ => {}
            }
            let (hub_side, service_side) = tokio::io::duplex(4 * 1024 * 1024);
            let (reader, writer) = tokio::io::split(hub_side);
            let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
            tokio::spawn(async move {
                let service = Service::attach(offer, service_side, stop_tx).await;
                let _ = notices.send(HostNotice::Attached(Box::new(service)));
            });
            // Resolves on the test's signal; a service end dropped without one never stops.
            let stop: BoxFuture<'static, ()> = Box::pin(async move {
                if stop_rx.await.is_err() {
                    std::future::pending::<()>().await;
                }
            });
            Ok(ChannelStream { reader: Box::new(reader), writer: Box::new(writer), stop: Some(stop) })
        })
    }
}

pub async fn next_service(notices: &mut mpsc::UnboundedReceiver<HostNotice>) -> Service {
    loop {
        match tokio::time::timeout(WAIT, notices.recv()).await.expect("a service attaches in time").expect("notices") {
            HostNotice::Attached(service) => return *service,
            HostNotice::Dropped(_) | HostNotice::Opening(_) => continue,
        }
    }
}

pub fn object(frame: &Frame) -> serde_json::Map<String, Value> {
    serde_json::from_slice::<Value>(&frame.payload).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

pub fn bytes(value: &[u8]) -> Bytes {
    Bytes::copy_from_slice(value)
}
