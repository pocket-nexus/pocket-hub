//! The endpoint: carriers' accept loops, the handshake (§3.3 to §3.8, §4.1),
//! one link per device with the replacement rule of §2.4, a device's request
//! to be paired by its code (§3.11, `pairing`), and the API the rest of Hub
//! uses.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use pocket_hub_store::Store;
use pocket_link::consts::*;
use pocket_link::frame::{self, Decoded, FrameDecoder, FrameType};
use pocket_link::handshake::{self, Challenge, Clock, Hello, Ready, Refusal, Refused};
use pocket_link::pair::{self, Pair};
use pocket_link::preamble::{self, HUB_RANGE};
use pocket_link::text::new_nonce;
use pocket_link::CloseCode;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::api::{ChannelHost, CloseInfo, DeviceRequests, DeviceSnapshot, HubEvent, LinkInfo, PairError, PairRequest};
use crate::apps::InstalledApp;
use crate::carrier::{CarrierKind, CarrierListener, CarrierStream};
use crate::config::EndpointConfig;
use crate::conn::{Conn, ConnMsg, Input};
use crate::lock::DeviceLock;
use crate::ops::DeviceHandle;
use crate::screenshot::Screenshot;

/// The live connection of a device.
pub(crate) struct Link {
    pub conn: u64,
    pub tx: mpsc::UnboundedSender<Input>,
    pub info: LinkInfo,
}

/// What Hub knows of a device beyond its record.
#[derive(Default)]
pub(crate) struct DeviceState {
    pub link: Option<Link>,
    pub lock: Arc<DeviceLock>,
    pub status: Option<serde_json::Value>,
    pub apps: Option<Vec<InstalledApp>>,
    pub screenshot: Option<Arc<Screenshot>>,
    pub duplicate: bool,
    pub last_close: Option<CloseInfo>,
}

#[derive(Default)]
pub(crate) struct Registry {
    pub devices: HashMap<String, DeviceState>,
    /// Counted replacements per device id (§2.4).
    replacements: HashMap<String, Vec<Instant>>,
}

pub(crate) struct Shared {
    pub store: Arc<Store>,
    pub config: EndpointConfig,
    /// The providers of the device's system requests, each serving the groups it lists.
    pub requests: Vec<Arc<dyn DeviceRequests>>,
    host: RwLock<Option<Arc<dyn ChannelHost>>>,
    pub registry: Mutex<Registry>,
    /// The requests to be paired by a code (§3.11), by device id.
    pub pairing: Mutex<HashMap<String, crate::pairing::Slot>>,
    events: broadcast::Sender<HubEvent>,
    connections: AtomicUsize,
    next_conn: AtomicU64,
    pub next_op: AtomicU64,
}

impl Shared {
    pub fn host(&self) -> Option<Arc<dyn ChannelHost>> {
        self.host.read().unwrap().clone()
    }

    /// The groups Hub serves (§6.1): the runtime operations always (the
    /// removal of an applet among them), `channel` while a channel host is
    /// installed, and those of `DeviceRequests`.
    pub fn hub_features(&self) -> Vec<String> {
        let mut list: Vec<String> = ["runtime", "runtime.native", "runtime.remove", "apps"].iter().map(|s| s.to_string()).collect();
        if self.host().is_some() {
            list.push("channel".into());
        }
        for requests in &self.requests {
            for feature in requests.features() {
                if !list.contains(&feature) && feature != "channel" && list.len() < MAX_FEATURES {
                    list.push(feature);
                }
            }
        }
        list
    }

    pub fn publish(&self, event: HubEvent) {
        let _ = self.events.send(event);
    }

    /// An id for one connection, unique for the endpoint's life.
    pub fn next_conn_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::SeqCst)
    }

    pub fn unlinked(&self, device: &str, conn: u64, info: CloseInfo) {
        let mut registry = self.registry.lock().unwrap();
        let Some(state) = registry.devices.get_mut(device) else { return };
        if state.link.as_ref().is_none_or(|link| link.conn != conn) {
            // A replaced connection: the device is linked through the newer one.
            return;
        }
        state.link = None;
        state.last_close = Some(info.clone());
        drop(registry);
        self.publish(HubEvent::Unlinked { device: device.to_owned(), close: info });
    }

    pub fn update_link(&self, device: &str, conn: u64, change: impl FnOnce(&mut LinkInfo)) {
        let mut registry = self.registry.lock().unwrap();
        if let Some(link) = registry.devices.get_mut(device).and_then(|s| s.link.as_mut()).filter(|l| l.conn == conn) {
            change(&mut link.info);
        }
    }

    pub fn counted_replacement(&self, device: &str) {
        self.registry.lock().unwrap().replacements.entry(device.to_owned()).or_default().push(Instant::now());
    }

    pub fn set_status(&self, device: &str, status: serde_json::Value) {
        self.registry.lock().unwrap().devices.entry(device.to_owned()).or_default().status = Some(status.clone());
        self.publish(HubEvent::Status { device: device.to_owned(), status });
    }

    pub fn set_apps(&self, device: &str, apps: Vec<InstalledApp>) {
        self.registry.lock().unwrap().devices.entry(device.to_owned()).or_default().apps = Some(apps.clone());
        self.publish(HubEvent::Apps { device: device.to_owned(), apps });
    }

    pub fn set_screenshot(&self, device: &str, shot: Screenshot) {
        let (frame, width, height) = (shot.frame, shot.width, shot.height);
        self.registry.lock().unwrap().devices.entry(device.to_owned()).or_default().screenshot = Some(Arc::new(shot));
        self.publish(HubEvent::Screenshot { device: device.to_owned(), frame, width, height });
    }

    /// The device's mutex, which tells every subscriber of a change of its holder.
    pub fn lock_of(&self, device: &str) -> Arc<DeviceLock> {
        let lock = self.registry.lock().unwrap().devices.entry(device.to_owned()).or_default().lock.clone();
        let (events, id, weak) = (self.events.clone(), device.to_owned(), Arc::downgrade(&lock));
        lock.observe(Box::new(move || {
            let held = weak.upgrade().and_then(|lock| lock.held());
            let (holder, since) = held.map_or((None, None), |(holder, since)| (Some(holder), Some(since)));
            let _ = events.send(HubEvent::Busy { device: id.clone(), holder, since });
        }));
        lock
    }

    pub fn link_tx(&self, device: &str) -> Option<mpsc::UnboundedSender<Input>> {
        self.registry.lock().unwrap().devices.get(device).and_then(|s| s.link.as_ref()).map(|l| l.tx.clone())
    }

    /// One paired device with its live state.
    pub fn snapshot(&self, record: &pocket_hub_store::DeviceRecord) -> DeviceSnapshot {
        let registry = self.registry.lock().unwrap();
        let state = registry.devices.get(&record.id);
        let held = state.and_then(|s| s.lock.held());
        DeviceSnapshot {
            id: record.id.clone(),
            kind: record.kind.clone(),
            name: record.name.clone(),
            model: record.model.clone(),
            target: record.target.clone(),
            host_abi: record.host_abi,
            last_address: record.last_address.clone(),
            last_seen: record.last_seen,
            setup_unconfirmed: record.setup_unconfirmed,
            duplicate_id: state.is_some_and(|s| s.duplicate),
            linked: state.is_some_and(|s| s.link.is_some()),
            link: state.and_then(|s| s.link.as_ref()).map(|l| l.info.clone()),
            status: state.and_then(|s| s.status.clone()),
            apps: state.and_then(|s| s.apps.clone()),
            mutex_holder: held.as_ref().map(|(holder, _)| holder.clone()),
            mutex_since: held.map(|(_, since)| since),
            latest_screenshot: state.and_then(|s| s.screenshot.as_deref().cloned()).map(|mut shot| {
                shot.png = Vec::new();
                shot
            }),
            last_close: state.and_then(|s| s.last_close.clone()),
        }
    }
}

/// Runs a store call off the async threads: every write fsyncs.
pub(crate) async fn store_call<T: Send + 'static>(store: &Arc<Store>, call: impl FnOnce(&Store) -> T + Send + 'static) -> Option<T> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || call(&store)).await.ok()
}

// ---------------------------------------------------------------------------
// The endpoint
// ---------------------------------------------------------------------------

/// Sends standalone events to linked devices; cheap to clone.
#[derive(Clone)]
pub struct DeviceNotifier {
    shared: Arc<Shared>,
}

impl DeviceNotifier {
    /// Sends a standalone event (id 0) of `group` to every linked device
    /// whose connection has the group in use (§5.3, §6.1), and whose target
    /// is one of `targets` when it is given: `catalog.changed` goes to the
    /// devices whose offer changed (§6.4). A device whose list does not name
    /// the group gets nothing, as §6.1 asks of a group that is not in use.
    pub fn notify(&self, group: &'static str, object: serde_json::Map<String, serde_json::Value>, targets: Option<&[String]>) {
        let records = self.shared.store.devices();
        let registry = self.shared.registry.lock().unwrap();
        for (id, state) in &registry.devices {
            let Some(link) = state.link.as_ref() else { continue };
            if let Some(targets) = targets {
                let target = records.iter().find(|record| &record.id == id).map(|record| record.target.as_str()).unwrap_or("");
                if !targets.iter().any(|wanted| wanted == target) {
                    continue;
                }
            }
            let _ = link.tx.send(Input::Msg(ConnMsg::Event { group, object: object.clone() }));
        }
    }
}

/// Hub's Pocket Link endpoint.
pub struct Endpoint {
    shared: Arc<Shared>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    ports: Vec<(CarrierKind, Option<u16>)>,
}

/// What an endpoint is started with.
pub struct EndpointBuilder {
    store: Arc<Store>,
    config: EndpointConfig,
    listeners: Vec<Box<dyn CarrierListener>>,
    host: Option<Arc<dyn ChannelHost>>,
    requests: Vec<Arc<dyn DeviceRequests>>,
}

impl EndpointBuilder {
    pub fn config(mut self, config: EndpointConfig) -> Self {
        self.config = config;
        self
    }
    pub fn listener(mut self, listener: Box<dyn CarrierListener>) -> Self {
        self.listeners.push(listener);
        self
    }
    /// Routes applet channels to services; Hub lists `channel` while one is set.
    pub fn channel_host(mut self, host: Arc<dyn ChannelHost>) -> Self {
        self.host = Some(host);
        self
    }
    /// Serves the device's system requests of the groups `requests` lists.
    /// Called again, it adds a provider; a group two providers list goes to
    /// the first one.
    pub fn device_requests(mut self, requests: Arc<dyn DeviceRequests>) -> Self {
        self.requests.push(requests);
        self
    }

    /// Runs the accept loops and, when configured, the beacon sender.
    pub fn start(self) -> Endpoint {
        let (events, _) = broadcast::channel(1024);
        let shared = Arc::new(Shared {
            store: self.store,
            config: self.config,
            requests: self.requests,
            host: RwLock::new(self.host),
            registry: Mutex::new(Registry::default()),
            pairing: Mutex::new(HashMap::new()),
            events,
            connections: AtomicUsize::new(0),
            next_conn: AtomicU64::new(1),
            next_op: AtomicU64::new(1),
        });
        let mut tasks = Vec::new();
        let mut ports = Vec::new();
        let mut beacon_port = None;
        for mut listener in self.listeners {
            ports.push((listener.kind(), listener.port()));
            if beacon_port.is_none() && listener.kind() == CarrierKind::Tcp {
                beacon_port = listener.port();
            }
            let shared = shared.clone();
            tasks.push(tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok(stream) => {
                            tokio::spawn(connection(shared.clone(), stream));
                        }
                        Err(error) => {
                            tracing::warn!("a carrier's accept failed: {error}");
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }));
        }
        if shared.config.beacons
            && let Some(port) = beacon_port
        {
            tasks.push(tokio::spawn(crate::beacon::run(shared.store.clone(), port, shared.config.beacon_port)));
        }
        Endpoint { shared, tasks: Mutex::new(tasks), ports }
    }
}

impl Endpoint {
    pub fn builder(store: Arc<Store>) -> EndpointBuilder {
        EndpointBuilder { store, config: EndpointConfig::default(), listeners: Vec::new(), host: None, requests: Vec::new() }
    }

    /// The carriers listened on, with the port each one's beacon names.
    pub fn ports(&self) -> &[(CarrierKind, Option<u16>)] {
        &self.ports
    }

    /// Every paired device with its live state.
    pub fn devices(&self) -> Vec<DeviceSnapshot> {
        self.shared.store.devices().iter().map(|record| self.shared.snapshot(record)).collect()
    }

    /// A paired device's handle, for its runtime operations.
    pub fn device(&self, id: &str) -> Option<DeviceHandle> {
        self.shared.store.device(id)?;
        Some(DeviceHandle::new(self.shared.clone(), id.to_owned()))
    }

    /// Events of every device, from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.shared.events.subscribe()
    }

    /// The groups Hub lists now (§6.1).
    pub fn features(&self) -> Vec<String> {
        self.shared.hub_features()
    }

    /// Installs or removes the channel host. Every linked device gets a
    /// `features` event (§6.1); removing it closes every channel with
    /// `feature` first.
    pub fn set_channel_host(&self, host: Option<Arc<dyn ChannelHost>>) {
        *self.shared.host.write().unwrap() = host;
        let features = self.shared.hub_features();
        let registry = self.shared.registry.lock().unwrap();
        for link in registry.devices.values().filter_map(|s| s.link.as_ref()) {
            let _ = link.tx.send(Input::Msg(ConnMsg::SetFeatures(features.clone())));
        }
    }

    /// Sends a standalone event to linked devices: `DeviceNotifier::notify`.
    pub fn notify(&self, group: &'static str, object: serde_json::Map<String, serde_json::Value>, targets: Option<&[String]>) {
        self.notifier().notify(group, object, targets);
    }

    /// A handle that sends standalone events to the linked devices, for a
    /// part of Hub that does not own the endpoint (a `DeviceRequests`
    /// provider announcing `catalog.changed`).
    pub fn notifier(&self) -> DeviceNotifier {
        DeviceNotifier { shared: self.shared.clone() }
    }

    // --- pairing by a code the device shows (§3.11) ---------------------------

    /// The devices that ask to be paired and wait for their code, oldest
    /// first. `HubEvent::PairRequests` says when this changed.
    pub fn pair_requests(&self) -> Vec<PairRequest> {
        crate::pairing::requests(&self.shared)
    }

    /// Hub's owner entered `code`, the 6 digits `device` shows: Hub mints a
    /// link key and sends it under the code. Returns once the device proved
    /// that it holds the key and its record is stored (`HubEvent::Paired`);
    /// the device then links by the handshake of §3.3. **A wrong code stores
    /// nothing**, and a key Hub held for that device id stays as it was.
    pub async fn pair(&self, device: &str, code: &str) -> Result<(), PairError> {
        crate::pairing::offer(&self.shared, device, code).await
    }

    /// Hub's owner dismissed the request of `device`: it reads `refused`
    /// `declined`. False when no request of that id waits for a code.
    pub fn decline_pair(&self, device: &str) -> bool {
        crate::pairing::decline(&self.shared, device)
    }

    /// §3.7 Forgetting on Hub: the record goes, and the link closes with
    /// `forgotten`; the device's next connection gets `unknown_device`.
    pub async fn forget(&self, id: &str) -> std::io::Result<bool> {
        let owned = id.to_owned();
        let removed = store_call(&self.shared.store, move |store| store.remove_device(&owned)).await.unwrap_or_else(|| Err(std::io::Error::other("the store task failed")))?;
        let link = self.shared.registry.lock().unwrap().devices.remove(id).and_then(|s| s.link);
        if let Some(link) = link {
            let reason = "the owner forgot this device";
            let _ = link.tx.send(Input::Msg(ConnMsg::Close(CloseCode::Forgotten, reason.into())));
            // The device's state is gone, so the connection's own end finds
            // nothing to report: the link's end is told here.
            self.shared.publish(HubEvent::Unlinked {
                device: id.to_owned(),
                close: CloseInfo { by: "hub", code: Some(CloseCode::Forgotten.name().into()), reason: reason.into() },
            });
        }
        if removed {
            self.shared.publish(HubEvent::Forgotten { device: id.to_owned() });
        }
        Ok(removed)
    }

    /// Hub quits: every link closes with `going_away`, and so does every
    /// request to be paired; the listeners and the beacon stop. Returns when
    /// the links have closed or the close drain has passed.
    pub async fn shutdown(&self) {
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        crate::pairing::close_all(&self.shared);
        let links: Vec<_> = self.shared.registry.lock().unwrap().devices.values().filter_map(|s| s.link.as_ref().map(|l| l.tx.clone())).collect();
        for tx in &links {
            let _ = tx.send(Input::Msg(ConnMsg::Close(CloseCode::GoingAway, "Hub quits".into())));
        }
        let deadline = Instant::now() + self.shared.config.timing.close_drain + std::time::Duration::from_millis(200);
        while Instant::now() < deadline && links.iter().any(|tx| !tx.is_closed()) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// One connection: the preamble, the handshake, then the link
// ---------------------------------------------------------------------------

struct Counted(Arc<Shared>);
impl Drop for Counted {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) async fn write(writer: &mut (dyn AsyncWrite + Send + Unpin), bytes: &[u8]) -> std::io::Result<()> {
    writer.write_all(bytes).await?;
    writer.flush().await
}

async fn connection(shared: Arc<Shared>, stream: CarrierStream) {
    shared.connections.fetch_add(1, Ordering::SeqCst);
    let _counted = Counted(shared.clone());
    let CarrierStream { kind, mut reader, mut writer, max_payload, peer, peer_ip } = stream;
    let timing = shared.config.timing.clone();
    let admitted = match tokio::time::timeout(timing.handshake, admit(&shared, &mut *reader, &mut *writer, &peer)).await {
        Ok(Some(admitted)) => admitted,
        Ok(None) => {
            let _ = writer.shutdown().await;
            return;
        }
        Err(_) => {
            let _ = write(&mut *writer, &frame::encode_close(CloseCode::HandshakeTimeout, "6 s without completing the handshake")).await;
            let _ = writer.shutdown().await;
            return;
        }
    };
    let (hello, mut decoder) = match admitted {
        Admitted::Link(hello, decoder) => (hello, decoder),
        Admitted::Pair(asked, decoder) => {
            // §3.11: the request waits for Hub's owner, outside the 6 s of a
            // handshake, and this connection never becomes a link: the device
            // connects again with `hello` once it holds the key.
            crate::pairing::serve(shared.clone(), asked, decoder, &mut *reader, &mut *writer, &peer, peer_ip).await;
            let _ = writer.shutdown().await;
            return;
        }
    };

    // On `ready`: the record takes what the device said and where it is.
    let now_ms = chrono::Utc::now().timestamp_millis();
    let address = peer_ip.map(|ip| ip.to_string());
    {
        let seen = hello.clone();
        let updated = store_call(&shared.store, move |store| {
            store.update_device(&seen.device, |record| {
                record.name = seen.name.clone();
                record.model = seen.model.clone();
                record.target = seen.target.clone();
                record.host_abi = seen.host_abi;
                if address.is_some() {
                    record.last_address = address.clone();
                }
                record.last_seen = Some(now_ms);
                record.setup_unconfirmed = false;
            })
        })
        .await;
        if !matches!(updated, Some(Ok(true))) {
            tracing::warn!(device = %hello.device, "the device record was not updated at ready");
        }
    }
    let features = shared.hub_features();
    let limit = handshake::payload_limit(hello.max_payload, max_payload);
    let local = chrono::Local::now();
    let clock = Clock { utc: now_ms, local: now_ms + i64::from(local.offset().local_minus_utc()) * 1000 };
    let ready = match handshake::encode(&Ready::new(&shared.store.hub_name(), features.clone(), max_payload, clock)) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!("ready could not be encoded: {error}");
            return;
        }
    };
    if write(&mut *writer, &ready).await.is_err() {
        return;
    }
    decoder.ready(limit);

    let conn_id = shared.next_conn_id();
    let (tx, rx) = mpsc::unbounded_channel();
    let in_use: Vec<String> = features.iter().filter(|f| hello.features.contains(f) && crate::conn::group_fits(f, limit)).cloned().collect();
    let info = LinkInfo {
        carrier: kind,
        peer: peer.clone(),
        payload_limit: limit,
        device_features: hello.features.clone(),
        hub_features: features.clone(),
        features_in_use: in_use,
        channels: Vec::new(),
        linked_at: now_ms,
    };
    {
        let mut registry = shared.registry.lock().unwrap();
        let state = registry.devices.entry(hello.device.clone()).or_default();
        // One link per device: the older connection ends its work now (§2.4).
        if let Some(older) = state.link.take() {
            let _ = older.tx.send(Input::Msg(ConnMsg::Retire));
        }
        state.link = Some(Link { conn: conn_id, tx: tx.clone(), info: info.clone() });
        // The device linked again: a launch hold ends (§8.3).
        state.lock.relinked();
    }
    shared.publish(HubEvent::Linked { device: hello.device.clone(), link: info });
    Conn::new(shared.clone(), &hello, conn_id, limit, features, decoder, tx, rx).run(reader, writer).await;
}

/// What a connection's first `HANDSHAKE` made of it.
enum Admitted {
    /// A paired device, its `auth` verified: the `hello` and the decoder,
    /// which may hold bytes that followed `auth`.
    Link(Hello, FrameDecoder),
    /// A device that asks to be paired by the code it shows (§3.11): its
    /// `pair`, and the decoder with what followed it.
    Pair(Pair, FrameDecoder),
}

/// The preamble exchange, then the handshake up to a verified `auth`, or a
/// `pair` in place of the `hello`.
async fn admit(shared: &Arc<Shared>, reader: &mut (dyn AsyncRead + Send + Unpin), writer: &mut (dyn AsyncWrite + Send + Unpin), peer: &str) -> Option<Admitted> {
    // §4.1: Hub reads nothing after the device's preamble until it has sent its own.
    let mut theirs = [0u8; PREAMBLE_BYTES];
    reader.read_exact(&mut theirs).await.ok()?;
    let theirs = preamble::decode(&theirs)?;
    let (answer, selected) = preamble::hub_answer(HUB_RANGE, theirs);
    write(writer, &answer).await.ok()?;
    if selected == 0 {
        return None;
    }
    let mut decoder = FrameDecoder::new();
    let refuse = |code: Refusal, message: &str, device: Option<&str>| {
        shared.publish(HubEvent::Refused { device: device.map(str::to_owned), peer: peer.to_owned(), code: code.code().to_owned(), message: message.to_owned() });
        handshake::encode(&Refused::new(code, message)).expect("a refusal fits a handshake frame")
    };

    let object = next_handshake(&mut decoder, reader, writer, shared, peer).await?;
    // §3.11: a connection whose first `HANDSHAKE` is `pair` is a request to
    // be paired. It counts among the connections of §2.4.
    if object.as_ref().and_then(|object| object.get("t")).and_then(|t| t.as_str()) == Some("pair") {
        let asked = match pair::parse_pair(object.as_ref()) {
            Ok(asked) => asked,
            Err(field) => {
                let _ = write(writer, &refuse(Refusal::Malformed, &format!("pair: {field}"), None)).await;
                return None;
            }
        };
        if shared.connections.load(Ordering::SeqCst) > shared.config.max_connections {
            let _ = write(writer, &refuse(Refusal::Busy, "Hub is at its connection limit", Some(&asked.device))).await;
            return None;
        }
        return Some(Admitted::Pair(asked, decoder));
    }
    let hello = match handshake::parse_hello(object.as_ref()) {
        Ok(hello) => hello,
        Err(field) => {
            let device = object.as_ref().and_then(|o| o.get("device")).and_then(|v| v.as_str()).map(str::to_owned);
            let _ = write(writer, &refuse(Refusal::Malformed, &format!("hello: {field}"), device.as_deref())).await;
            return None;
        }
    };
    if shared.connections.load(Ordering::SeqCst) > shared.config.max_connections {
        let _ = write(writer, &refuse(Refusal::Busy, "Hub is at its connection limit", Some(&hello.device))).await;
        return None;
    }
    // §2.4: after 3 counted replacements in 30 s, the next hello is refused `busy`.
    let duplicate = {
        let mut registry = shared.registry.lock().unwrap();
        let window = shared.config.timing.replaced_window;
        let counted = registry.replacements.entry(hello.device.clone()).or_default();
        counted.retain(|at| at.elapsed() < window);
        let duplicate = counted.len() >= REPLACED_COUNT;
        if duplicate {
            // The count starts again after the hold.
            counted.clear();
            registry.devices.entry(hello.device.clone()).or_default().duplicate = true;
        }
        duplicate
    };
    if duplicate {
        shared.publish(HubEvent::DuplicateId { device: hello.device.clone() });
        let _ = write(writer, &refuse(Refusal::Busy, "two consoles use this device id", Some(&hello.device))).await;
        return None;
    }
    let key = shared.store.device(&hello.device).map(|record| record.key).filter(|key| pocket_link::text::is_key(key));
    let (Some(key), true) = (key, hello.companion == shared.store.hub_id()) else {
        let _ = write(writer, &refuse(Refusal::UnknownDevice, "this computer has to set the device up", Some(&hello.device))).await;
        return None;
    };
    let nonce = new_nonce();
    let hub_id = shared.store.hub_id();
    let proof = handshake::companion_proof(&key, &hello.nonce, &nonce, &hello.device, &hub_id)?;
    write(writer, &handshake::encode(&Challenge::new(nonce.clone(), proof)).ok()?).await.ok()?;

    let object = next_handshake(&mut decoder, reader, writer, shared, peer).await?;
    let expected = handshake::console_proof(&key, &hello.nonce, &nonce, &hello.device, &hub_id)?;
    match handshake::parse_auth(object.as_ref()) {
        Some(proof) if handshake::proof_equals(&proof, &expected) => Some(Admitted::Link(hello, decoder)),
        _ => {
            let _ = write(writer, &refuse(Refusal::BadProof, "the proof did not verify", Some(&hello.device))).await;
            None
        }
    }
}

/// The next HANDSHAKE frame's object. Before `ready` only HANDSHAKE, PING,
/// PONG and CLOSE are valid (§3.3); the decoder closes on anything else.
async fn next_handshake(
    decoder: &mut FrameDecoder,
    reader: &mut (dyn AsyncRead + Send + Unpin),
    writer: &mut (dyn AsyncWrite + Send + Unpin),
    shared: &Arc<Shared>,
    peer: &str,
) -> Option<Option<serde_json::Map<String, serde_json::Value>>> {
    let mut buffer = vec![0u8; 4096];
    loop {
        match decoder.next() {
            Ok(Some(Decoded::Frame(frame))) => match frame.ty {
                FrameType::Handshake => return Some(frame::parse_json(&frame).object),
                FrameType::Ping => write(writer, &frame::encode_pong(&frame.payload)).await.ok()?,
                FrameType::Close => {
                    // The device ended the handshake: `auth` when Hub's proof did not verify (§3.8).
                    let (code, reason) = frame::parse_close(&frame.payload);
                    shared.publish(HubEvent::Refused { device: None, peer: peer.to_owned(), code: code.name().to_owned(), message: reason });
                    return None;
                }
                _ => {}
            },
            Ok(Some(Decoded::Skipped)) => {}
            Ok(None) => {
                let n = reader.read(&mut buffer).await.ok()?;
                if n == 0 {
                    return None;
                }
                decoder.push(&buffer[..n]);
            }
            Err(close) => {
                let _ = write(writer, &frame::encode_close(close.code, &close.reason)).await;
                return None;
            }
        }
    }
}
