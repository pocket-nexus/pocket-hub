//! The core's parts and what it publishes.
//!
//! **Events and answers are ordered** (§1): every event a client may see is
//! made and handed to the broadcast under `publication`, and the answers
//! that read the same state (`devices.list`, `devices.get`, `daemons.list`)
//! are made under it too. A client's writer sends every event already in its
//! queue before it sends an answer, so an answer reflects every event written
//! before it, and an event written after an answer is as new as the answer or
//! newer.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pocket_hub_api::types::{self as api, Event};
use pocket_hub_api::{methods, paths};
use pocket_hub_applet_store::{AppletStore, LinkCatalog, LinkConfig, StoreConfig, StoreEvent};
use pocket_hub_core::{Endpoint, EndpointConfig, HubEvent, TcpCarrier, Timing};
use pocket_hub_daemons::{DaemonEvent, DaemonStatus, LogLine, Supervisor};
use pocket_hub_setup::{DeviceFamily, ReleaseFetcher, ShellSource};
use pocket_hub_store::Store;
use serde_json::json;
use tokio::net::UnixListener;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::host::DaemonHost;
use crate::program::Releases;
use crate::{Options, StartError, VERSION, map, sdk, server};

/// Screenshots kept in `<home>/cache/screenshots`.
pub const KEPT_SCREENSHOTS: usize = 50;

/// One event, as the line every subscribed client writes.
pub struct Published {
    pub topic: &'static str,
    pub line: String,
}

pub struct Core {
    pub options: Options,
    pub home: PathBuf,
    pub socket: PathBuf,
    pub store: Arc<Store>,
    pub endpoint: Endpoint,
    pub supervisor: Arc<Supervisor>,
    /// The applet store's client (§3 Store), and the link's catalog and
    /// install served from it (link specification §6.4).
    pub applets: Arc<AppletStore>,
    pub link_catalog: LinkCatalog,
    pub started_at: i64,
    /// The TCP link port in use.
    pub link_port: u16,
    /// The version of the configured Bun as it reports it; None for the
    /// pinned Bun Hub downloads.
    bun_override_version: Option<String>,
    events: broadcast::Sender<Arc<Published>>,
    publication: Mutex<()>,
    /// The newest release of each family's own program (§2 `Device.program`).
    releases: Releases,
    /// Per device, the clients that asked for its logs (§3 `devices.logs`).
    logs_wanted: Mutex<HashMap<String, HashSet<u64>>>,
    shutdown_tx: watch::Sender<bool>,
    shutting_down: AtomicBool,
    next_conn: AtomicU64,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Holds the home's lock file, flocked, for the process's life.
    _home_lock: File,
}

pub fn epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Takes `<home>/hubd.lock` with `flock`: one core per home, decided by the
/// kernel, so two cores started together cannot both pass the socket check.
fn lock_home(home: &Path) -> Result<Option<File>, std::io::Error> {
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(home.join("hubd.lock"))?;
    // SAFETY: flock on a descriptor this function owns; no memory is passed.
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if taken {
        return Ok(Some(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(None) } else { Err(error) }
}

impl Core {
    pub(crate) async fn start(options: Options) -> Result<Arc<Core>, StartError> {
        let home = options.home.clone();
        let socket = paths::checked_socket_path(&home).map_err(StartError::Home)?;
        let home_error = |error: std::io::Error| StartError::Home(format!("Pocket Hub cannot use its home folder {}: {error}.", home.display()));
        fs::create_dir_all(&home).map_err(home_error)?;
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).map_err(home_error)?;
        let Some(home_lock) = lock_home(&home).map_err(home_error)? else {
            return Err(StartError::AlreadyRunning(home));
        };
        if fs::symlink_metadata(&socket).is_ok() {
            if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
                return Err(StartError::AlreadyRunning(home));
            }
            // Ours by the lock, and nobody answers on it: a crashed core's.
            fs::remove_file(&socket).map_err(home_error)?;
        }

        let wanted = options.link_port.unwrap_or(pocket_link::consts::PORT);
        if wanted != 0 && port_answers(wanted).await {
            return Err(StartError::PortInUse { port: wanted });
        }
        let carrier = TcpCarrier::bind(options.link_port).await.map_err(|error| match error.kind() {
            std::io::ErrorKind::AddrInUse => StartError::PortInUse { port: wanted },
            _ => StartError::Port { port: wanted, error },
        })?;

        let store = Arc::new(Store::open_named(home.join("store"), options.name.as_deref()).map_err(|error| StartError::Other(format!("Pocket Hub cannot open its store: {error}.")))?);
        let sdk = sdk::materialize(&home).map_err(|error| StartError::Other(format!("Pocket Hub cannot write the daemon SDK into its home: {error}.")))?;
        let mut config = pocket_hub_daemons::Config::new(&home, home.join("daemon.sock"));
        config.bun_path = options.bun.clone();
        config.sdk = Some(sdk);
        config.timings.idle_override = options.daemon_idle;
        let supervisor = Arc::new(Supervisor::new(config).await.map_err(|error| StartError::Other(format!("Pocket Hub cannot start its daemon supervisor: {error}.")))?);

        let endpoint_config = EndpointConfig { timing: Timing {
                mutex_wait: options.mutex_wait,
                silence: options.link_silence,
                state_silence: options.link_state_silence,
                state_ping: options.link_state_ping,
                ..Timing::default()
            }, beacons: options.beacons, ..EndpointConfig::default() };
        // net.http (§6.5): Hub performs an applet's HTTP requests for a
        // device, under the destination policy, with the environment's or the
        // system's proxy settings.
        let net = pocket_hub_net::NetProvider::new(pocket_hub_net::NetConfig { overrides: options.net_overrides.clone(), ..pocket_hub_net::NetConfig::default() });
        // The applet store: its cache under the home, the packages of the
        // paired devices' targets kept ready for their store pages.
        let mut store_config = StoreConfig::new(&options.store_url, home.join("cache").join("store"));
        store_config.official_key = options.store_key.clone();
        let records = store.clone();
        store_config.wanted_targets = Arc::new(move || {
            let mut targets: Vec<String> = records.devices().into_iter().map(|record| record.target).filter(|target| !target.is_empty()).collect();
            targets.sort();
            targets.dedup();
            targets
        });
        let applets = AppletStore::open(store_config).map_err(|error| StartError::Other(format!("Pocket Hub cannot start its applet store client: {error}.")))?;
        let link_catalog = LinkCatalog::new(applets.clone(), LinkConfig::default());
        let endpoint = Endpoint::builder(store.clone())
            .config(endpoint_config)
            .listener(Box::new(carrier))
            .channel_host(Arc::new(DaemonHost(supervisor.clone())))
            .device_requests(Arc::new(net))
            .device_requests(Arc::new(link_catalog.clone()))
            .start();
        // §6.4: `catalog.changed` to the linked devices whose offer changed.
        let notifier = endpoint.notifier();
        link_catalog.set_notifier(move |targets| {
            let mut event = serde_json::Map::new();
            event.insert("t".into(), json!("catalog.changed"));
            notifier.notify("catalog", event, Some(&targets));
        });
        let store_events = applets.subscribe();
        let link_port = endpoint.ports().iter().find_map(|(_, port)| *port).unwrap_or(wanted);
        let hub_events = endpoint.subscribe();
        let daemon_events = supervisor.subscribe();
        let daemon_logs = supervisor.subscribe_logs();

        let listener = UnixListener::bind(&socket).map_err(|error| StartError::Other(format!("Pocket Hub cannot listen on {}: {error}.", socket.display())))?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(home_error)?;

        let bun_override_version = options.bun.as_deref().and_then(reported_version);
        let releases = Releases::new(options.program_look_interval);
        let core = Arc::new(Core {
            options,
            home,
            socket,
            store,
            endpoint,
            supervisor,
            applets,
            link_catalog,
            started_at: epoch_ms(),
            link_port,
            bun_override_version,
            events: broadcast::channel(4096).0,
            publication: Mutex::new(()),
            releases,
            logs_wanted: Mutex::new(HashMap::new()),
            shutdown_tx: watch::channel(false).0,
            shutting_down: AtomicBool::new(false),
            next_conn: AtomicU64::new(1),
            tasks: Mutex::new(Vec::new()),
            _home_lock: home_lock,
        });
        let pump = tokio::spawn(pump(core.clone(), hub_events, daemon_events, daemon_logs));
        let accept = tokio::spawn(server::accept_loop(core.clone(), listener));
        let offer = core.link_catalog.start();
        let store_pump = tokio::spawn(store_pump(core.clone(), store_events));
        core.tasks.lock().unwrap().extend([pump, accept, offer, store_pump]);
        tracing::info!(home = %core.home.display(), socket = %core.socket.display(), link_port, version = VERSION, "Pocket Hub core started");
        Ok(core)
    }

    pub fn hub_info(&self) -> api::HubInfo {
        api::HubInfo { id: self.store.hub_id(), name: self.store.hub_name(), version: VERSION.to_owned() }
    }

    /// The Bun version daemons run on: the configured Bun's own, else the pinned one.
    pub fn bun_version(&self) -> String {
        self.bun_override_version.clone().unwrap_or_else(|| self.supervisor.runtime().version().to_owned())
    }

    pub fn next_conn(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::SeqCst)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Published>> {
        self.events.subscribe()
    }

    /// Held while an answer that reads published state is made.
    pub fn publication(&self) -> MutexGuard<'_, ()> {
        self.publication.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Hands an event to every connection. Callers that publish state hold
    /// `publication` across reading the state and this call.
    pub fn publish(&self, event: &Event) {
        let line = json!({"jsonrpc": "2.0", "method": methods::EVENT, "params": event}).to_string();
        let _ = self.events.send(Arc::new(Published { topic: event.topic(), line }));
    }

    pub fn device(&self, id: &str) -> Option<api::Device> {
        self.endpoint.device(id)?.snapshot().map(|snapshot| map::device(&snapshot, self.latest_program(&snapshot.kind)))
    }

    pub fn devices(&self) -> Vec<api::Device> {
        self.endpoint.devices().iter().map(|snapshot| map::device(snapshot, self.latest_program(&snapshot.kind))).collect()
    }

    /// The devices that ask to be paired and wait for their code (§3 Pairing).
    pub fn pair_requests(&self) -> Vec<api::PairRequest> {
        self.endpoint.pair_requests().iter().map(map::pair_request).collect()
    }

    /// `pairing.changed` with the requests as they are now.
    fn publish_pair_requests(&self) {
        let _guard = self.publication();
        self.publish(&Event::PairingChanged { requests: self.pair_requests() });
    }

    // --- the device's own program (§2 Device.program) -------------------------

    /// The newest release Hub knows of the program of devices of `kind`.
    fn latest_program(&self, kind: &str) -> Option<String> {
        self.releases.latest(DeviceFamily::from_kind(kind)?)
    }

    /// Where the program's releases come from: Pocket Shell's GitHub
    /// releases, or what the core was started with (`--shell-api`,
    /// `--shell-repo`, `--shell-file`). One source: the 3DS is the one family.
    pub fn shell_source(&self) -> ShellSource {
        self.options.shell_source.clone().unwrap_or_default()
    }

    /// The program's download cache, shared by setup and updates.
    pub fn shell_cache(&self) -> PathBuf {
        self.home.join("cache").join("shell")
    }

    /// Notes the newest release's version for `family`; when it changed,
    /// every device of the family is published again (`device.changed`), in
    /// order with the answers that read it.
    pub fn set_latest_program(&self, family: DeviceFamily, version: Option<String>) {
        let _guard = self.publication();
        if !self.releases.set(family, version.clone()) {
            return;
        }
        tracing::info!(kind = family.kind(), latest = version.as_deref().unwrap_or(""), "the newest release of the device's program");
        for device in self.devices().into_iter().filter(|device| device.kind == family.kind()) {
            self.publish(&Event::DeviceChanged { device: Box::new(device) });
        }
    }

    /// Looks the newest release of the program of devices of `kind` up, when
    /// a look is due (`program`'s rules), in a task of its own. A failed look
    /// is logged and changes nothing.
    fn look_for_program(self: &Arc<Self>, kind: &str) {
        let Some(family) = DeviceFamily::from_kind(kind) else { return };
        if !self.releases.begin(family) {
            return;
        }
        let core = self.clone();
        tokio::spawn(async move {
            let source = core.shell_source();
            let found = match ReleaseFetcher::new(core.shell_cache(), &source) {
                Ok(fetcher) => fetcher.resolve_fresh(&source).await,
                Err(error) => Err(error),
            };
            match found {
                Ok(release) => core.set_latest_program(family, release.version),
                Err(error) => tracing::warn!(kind = family.kind(), %error, "the newest release of the device's program could not be looked up; Hub keeps the one it knew"),
            }
            core.releases.finish(family);
        });
    }

    /// `device.changed` with the device as it is now.
    pub fn publish_device(&self, id: &str) {
        let _guard = self.publication();
        if let Some(device) = self.device(id) {
            self.publish(&Event::DeviceChanged { device: Box::new(device) });
        }
    }

    pub fn daemon(&self, id: &str) -> Option<api::Daemon> {
        let status = self.supervisor.status(id)?;
        Some(map::daemon(&status, self.supervisor.registry().get(id).as_ref()))
    }

    pub fn daemons(&self) -> Vec<api::Daemon> {
        self.supervisor.statuses().iter().map(|status| map::daemon(status, self.supervisor.registry().get(&status.id).as_ref())).collect()
    }

    /// `daemon.changed` with the daemon as it is now (`fallback` when it is
    /// no longer known, as after a removal with its process still ending).
    fn publish_daemon(&self, id: &str, fallback: Option<&DaemonStatus>) {
        let _guard = self.publication();
        let daemon = self.daemon(id).or_else(|| fallback.map(|status| map::daemon(status, None)));
        if let Some(daemon) = daemon {
            self.publish(&Event::DaemonChanged { daemon });
        }
    }

    /// Every device, the pairing requests and every daemon again, after
    /// events were lost to a full queue.
    pub fn resync(&self) {
        let _guard = self.publication();
        for device in self.devices() {
            self.publish(&Event::DeviceChanged { device: Box::new(device) });
        }
        self.publish(&Event::PairingChanged { requests: self.pair_requests() });
        for daemon in self.daemons() {
            self.publish(&Event::DaemonChanged { daemon });
        }
    }

    // --- device logs, kept on per client (§3 devices.logs) ------------------

    /// Notes that `conn` wants `device`'s logs.
    pub fn want_logs(&self, device: &str, conn: u64) {
        self.logs_wanted.lock().unwrap().entry(device.to_owned()).or_default().insert(conn);
    }

    /// Notes that `conn` no longer wants them; true when no client does now.
    pub fn unwant_logs(&self, device: &str, conn: u64) -> bool {
        let mut wanted = self.logs_wanted.lock().unwrap();
        let Some(clients) = wanted.get_mut(device) else { return true };
        clients.remove(&conn);
        let empty = clients.is_empty();
        if empty {
            wanted.remove(device);
        }
        empty
    }

    fn logs_wanted(&self, device: &str) -> bool {
        self.logs_wanted.lock().unwrap().get(device).is_some_and(|clients| !clients.is_empty())
    }

    /// The device is forgotten: no client's wish for its logs is kept.
    pub fn forget_logs(&self, device: &str) {
        self.logs_wanted.lock().unwrap().remove(device);
    }

    /// A client left: the logs it alone kept on go off.
    pub fn client_gone(&self, conn: u64) {
        let emptied: Vec<String> = {
            let mut wanted = self.logs_wanted.lock().unwrap();
            let mut emptied = Vec::new();
            wanted.retain(|device, clients| {
                if clients.remove(&conn) && clients.is_empty() {
                    emptied.push(device.clone());
                    return false;
                }
                !clients.is_empty()
            });
            emptied
        };
        for device in emptied {
            if let Some(handle) = self.endpoint.device(&device) {
                tokio::spawn(async move {
                    let _ = handle.logs(false).await;
                });
            }
        }
    }

    // --- screenshots ---------------------------------------------------------

    pub fn screenshot_cache(&self) -> PathBuf {
        self.home.join("cache").join("screenshots")
    }

    /// Writes a PNG at `path`, or into the cache (which keeps the newest
    /// `KEPT_SCREENSHOTS` files) without one. Returns where it went.
    pub async fn save_screenshot(&self, device: &str, frame: u64, png: Vec<u8>, path: Option<PathBuf>) -> std::io::Result<PathBuf> {
        let cache = self.screenshot_cache();
        let (path, prune) = match path {
            Some(path) => (path, false),
            // The time first, zero-padded: names sort as they were taken.
            None => (cache.join(format!("{:013}-{device}-{frame}.png", epoch_ms())), true),
        };
        let target = path.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, &png)?;
            if prune {
                prune_cache(&cache, KEPT_SCREENSHOTS);
            }
            Ok(())
        })
        .await
        .map_err(std::io::Error::other)??;
        Ok(path)
    }

    // --- shutdown ------------------------------------------------------------

    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub async fn shutdown_requested(&self) {
        let mut rx = self.shutdown_tx.subscribe();
        let _ = rx.wait_for(|stop| *stop).await;
    }

    /// The links close with `going_away`, the daemons stop, then the socket
    /// goes. Runs once.
    pub async fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::info!("Pocket Hub core shutting down");
        self.endpoint.shutdown().await;
        self.supervisor.shutdown().await;
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        let _ = fs::remove_file(&self.socket);
        tracing::info!("Pocket Hub core stopped");
    }
}

/// True when a program already accepts connections on `port` on this
/// computer. The listener binds the wildcard address with `SO_REUSEADDR`,
/// which on macOS succeeds beside a program that holds the port on one
/// address only (the loopback, or one interface): devices would then reach
/// either program. A connection to each local address finds that holder.
async fn port_answers(port: u16) -> bool {
    let mut addresses = vec![std::net::Ipv4Addr::LOCALHOST];
    addresses.extend(pocket_hub_core::beacon::interface_addresses());
    let probes = addresses.into_iter().map(|address| async move {
        matches!(tokio::time::timeout(std::time::Duration::from_millis(300), tokio::net::TcpStream::connect((address, port))).await, Ok(Ok(_)))
    });
    futures_any(probes).await
}

/// True when any of the probes is.
async fn futures_any(probes: impl Iterator<Item = impl std::future::Future<Output = bool> + Send + 'static>) -> bool {
    let tasks: Vec<_> = probes.map(tokio::spawn).collect();
    let mut any = false;
    for task in tasks {
        any |= task.await.unwrap_or(false);
    }
    any
}

/// `<bun> --version`, for a Bun Hub did not download.
fn reported_version(bun: &Path) -> Option<String> {
    use pocket_hub_daemons::child::InheritNothing;
    let output = std::process::Command::new(bun).arg("--version").stdin(std::process::Stdio::null()).inherit_nothing().output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !text.trim().is_empty()).then(|| text.trim().to_owned())
}

fn prune_cache(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png")).collect();
    if names.len() <= keep {
        return;
    }
    names.sort();
    for old in &names[..names.len() - keep] {
        let _ = fs::remove_file(old);
    }
}

/// Turns the endpoint's and the supervisor's events into the contract's.
async fn pump(
    core: Arc<Core>,
    mut hub: broadcast::Receiver<HubEvent>,
    mut daemons: broadcast::Receiver<DaemonEvent>,
    mut logs: broadcast::Receiver<LogLine>,
) {
    use broadcast::error::RecvError;
    loop {
        tokio::select! {
            event = hub.recv() => match event {
                Ok(event) => on_hub_event(&core, event).await,
                Err(RecvError::Lagged(_)) => core.resync(),
                Err(RecvError::Closed) => return,
            },
            event = daemons.recv() => match event {
                Ok(DaemonEvent::State(status)) => core.publish_daemon(&status.id.clone(), Some(&status)),
                Ok(DaemonEvent::Installed(daemon)) => core.publish_daemon(&daemon.id, None),
                Ok(DaemonEvent::Removed { id }) => core.publish(&Event::DaemonRemoved { id }),
                Err(RecvError::Lagged(_)) => core.resync(),
                Err(RecvError::Closed) => return,
            },
            line = logs.recv() => match line {
                Ok(line) => core.publish(&Event::DaemonLog { id: line.app.clone(), at: line.at, stream: map::log_stream(line.stream).to_owned(), text: line.text }),
                Err(RecvError::Lagged(missed)) => tracing::warn!(missed, "daemon log lines were dropped from the control API"),
                Err(RecvError::Closed) => return,
            },
        }
    }
}

/// `store.changed` when the catalog Hub holds, or what it has cached of it,
/// changed: another catalog, icons that arrived, a package verified into the
/// cache or found changed there (`StoreApp.cached`).
async fn store_pump(core: Arc<Core>, mut events: broadcast::Receiver<StoreEvent>) {
    use broadcast::error::RecvError;
    loop {
        match events.recv().await {
            Ok(StoreEvent::Catalog { fetched_at }) => core.publish(&Event::StoreChanged { fetched_at }),
            Ok(StoreEvent::Packages) | Err(RecvError::Lagged(_)) => {
                if let Some(catalog) = core.applets.held() {
                    core.publish(&Event::StoreChanged { fetched_at: catalog.fetched_at });
                }
            }
            Err(RecvError::Closed) => return,
        }
    }
}

async fn on_hub_event(core: &Arc<Core>, event: HubEvent) {
    match event {
        HubEvent::Linked { device, .. } => {
            tracing::info!(%device, "device linked");
            core.publish_device(&device);
            if let Some(kind) = core.endpoint.device(&device).and_then(|handle| handle.snapshot()).map(|snapshot| snapshot.kind) {
                core.look_for_program(&kind);
            }
            // Logs are off at every new connection; Hub keeps them on for
            // the clients that asked.
            if core.logs_wanted(&device)
                && let Some(handle) = core.endpoint.device(&device)
            {
                tokio::spawn(async move {
                    if let Err(error) = handle.logs(true).await {
                        tracing::warn!(device = %handle.id(), %error, "logs could not be turned on again after the link");
                    }
                });
            }
        }
        HubEvent::Unlinked { device, close } => {
            tracing::info!(%device, by = close.by, code = close.code.as_deref().unwrap_or("lost"), reason = %close.reason, "device unlinked");
            {
                let _guard = core.publication();
                core.publish(&Event::DeviceUnlinked { id: device.clone(), code: map::unlinked_code(close.code.as_deref()), reason: close.reason });
            }
            core.publish_device(&device);
        }
        HubEvent::Status { device, .. }
        | HubEvent::Apps { device, .. }
        | HubEvent::Features { device, .. }
        | HubEvent::ChannelOpened { device, .. }
        | HubEvent::ChannelClosed { device, .. }
        | HubEvent::DuplicateId { device }
        | HubEvent::Busy { device, .. } => core.publish_device(&device),
        HubEvent::Log { device, level, source, text, frame, dropped } => core.publish(&Event::DeviceLog { id: device, level, source, text, frame, dropped }),
        HubEvent::Screenshot { device, frame, width, height } => {
            let Some(shot) = core.endpoint.device(&device).and_then(|handle| handle.latest_screenshot()) else { return };
            match core.save_screenshot(&device, frame, shot.png.clone(), None).await {
                Ok(path) => core.publish(&Event::DeviceScreenshot { id: device, path: path.display().to_string(), width, height, frame }),
                Err(error) => tracing::warn!(%device, %error, "the device's screenshot could not be written"),
            }
        }
        HubEvent::Forgotten { device } => core.publish(&Event::DeviceRemoved { id: device }),
        HubEvent::PairRequests => core.publish_pair_requests(),
        HubEvent::Paired { device } => {
            tracing::info!(%device, "device paired by the code it shows");
            core.publish_device(&device);
        }
        HubEvent::Refused { device, peer, code, message } => {
            tracing::info!(device = device.as_deref().unwrap_or(""), %peer, %code, %message, "a device's handshake was refused");
        }
        HubEvent::ChannelRefused { device, app, code } => tracing::info!(%device, %app, %code, "a channel.open was refused"),
    }
}
