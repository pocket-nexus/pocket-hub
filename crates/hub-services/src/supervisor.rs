//! The supervisor: the service socket, the processes and their channels.
//!
//! One table, behind one lock that is never held across an await, holds what
//! Hub knows of each service: its published status, its log ring, its running
//! process (an *instance*) if any, and the tickets offered to it. Each
//! instance has its own task (`instance.rs`) that owns the child process and
//! the control connection; everything else talks to that task through a
//! command channel and watches its phase.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::channel::{make_stream, ChannelInfo, ChannelLease, ChannelStream};
use crate::config::Config;
use crate::instance::{self, Cmd, Phase, StopReason};
use crate::logs::{LogLine, LogRing, LogStream};
use crate::manifest::valid_id;
use crate::registry::{InstallError, InstallReport, InstalledService, Registry};
use crate::runtime::BunRuntime;
use crate::util::{cut, epoch_ms, random_hex, same_secret, signal_group};
use crate::wire;

/// What the relay knows of a channel a device opened (link specification
/// §7.1), passed to the service in `channel.offer` (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelOffer {
    pub device: String,
    pub device_name: String,
    pub model: String,
    pub target: String,
    pub app: String,
    pub hash: String,
    pub generation: u32,
    pub payload_limit: u32,
}

/// Why a channel was not opened, mapped by the relay onto the device's
/// `REPLY` `ERROR` codes of the same names.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    /// No service is installed for this applet: `no_daemon`, the wire's code
    /// (link specification §7.1), which says daemon until the specification
    /// renames it.
    #[error("no_daemon")]
    NoService,
    /// The service exists but did not take the channel in time; the text says
    /// why, for Hub's log and UI.
    #[error("busy: {0}")]
    Busy(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StartError {
    #[error("{0} is not installed")]
    NotInstalled(String),
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("another Pocket Hub is serving the service socket {0}")]
    SocketInUse(PathBuf),
    #[error("{0} exists and is not a socket")]
    NotASocket(PathBuf),
    #[error("the service socket {0}: {1}")]
    Socket(PathBuf, io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    Stopping,
    /// The last process ended without Hub asking: `lastExit` and `lastError`
    /// say how. A failed start also holds off the next one (`retryAt`).
    Failed,
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    /// Epoch milliseconds.
    pub at: i64,
}

/// One service as a UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub id: String,
    /// The installed (current) version; None once removed.
    pub version: Option<String>,
    /// The version the running process was started from.
    pub running_version: Option<String>,
    pub state: ServiceState,
    /// Epoch milliseconds of when it entered `state`; None for a service that
    /// has not changed state since Hub started.
    pub since: Option<i64>,
    pub pid: Option<u32>,
    /// Epoch milliseconds of the running process's start.
    pub started_at: Option<i64>,
    pub channels: usize,
    /// Starts after the first since Hub started.
    pub restarts: u64,
    /// Processes that ended without Hub asking.
    pub crashes: u64,
    pub last_exit: Option<ExitInfo>,
    pub last_error: Option<String>,
    /// Epoch milliseconds before which a device's open is answered `busy`
    /// without a start, after failed starts.
    pub retry_at: Option<i64>,
}

impl ServiceStatus {
    fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            version: None,
            running_version: None,
            state: ServiceState::Stopped,
            since: None,
            pid: None,
            started_at: None,
            channels: 0,
            restarts: 0,
            crashes: 0,
            last_exit: None,
            last_error: None,
            retry_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServiceEvent {
    /// A service's status changed.
    State(ServiceStatus),
    Installed(InstalledService),
    Removed { id: String },
}

/// Publishes a status that changed, with `since` stamped when its state did.
fn publish_change(events: &broadcast::Sender<ServiceEvent>, before: &ServiceStatus, status: &mut ServiceStatus) {
    if status.state != before.state {
        status.since = Some(epoch_ms());
    }
    if *status != *before {
        let _ = events.send(ServiceEvent::State(status.clone()));
    }
}

/// One channel a process serves: a second descriptor of its socket, to shut
/// it down when the process ends, whoever holds the halves.
pub(crate) struct ChannelSlot {
    socket: std::os::unix::net::UnixStream,
    stop: watch::Sender<bool>,
}

pub(crate) struct Instance {
    pub serial: u64,
    pub token: String,
    pub version: String,
    pub pid: u32,
    pub phase: watch::Receiver<Phase>,
    pub cmd: mpsc::UnboundedSender<Cmd>,
    /// Taken by the connection whose `service.hello` carries the token.
    pub hello: Option<oneshot::Sender<Result<UnixStream, String>>>,
    pub channels: HashMap<u64, ChannelSlot>,
    /// Opens between their wait for this process and their channel.
    pub pending: usize,
    /// A stop was asked for; new opens wait for it to end and start anew.
    pub stopping: bool,
}

impl Instance {
    pub fn idle(&self) -> bool {
        self.channels.is_empty() && self.pending == 0
    }
}

pub(crate) struct Entry {
    pub status: ServiceStatus,
    pub logs: Arc<LogRing>,
    pub instance: Option<Instance>,
    /// Processes started since Hub started.
    pub starts: u64,
    /// Failed starts in a row.
    pub failures: u32,
    pub hold_until: Option<Instant>,
}

struct Ticket {
    app: String,
    serial: u64,
    tx: oneshot::Sender<UnixStream>,
}

#[derive(Default)]
pub(crate) struct Table {
    pub services: HashMap<String, Entry>,
    tickets: HashMap<String, Ticket>,
}

pub(crate) struct Inner {
    pub config: Config,
    pub runtime: BunRuntime,
    pub registry: Registry,
    pub table: Mutex<Table>,
    pub events: broadcast::Sender<ServiceEvent>,
    pub log_lines: broadcast::Sender<LogLine>,
    pub closing: AtomicBool,
    serials: AtomicU64,
    /// The socket file as bound, to remove only our own at shutdown.
    socket_id: (u64, u64),
}

impl Inner {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table.lock().expect("supervisor table")
    }

    pub fn next_serial(&self) -> u64 {
        self.serials.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn entry<'a>(&self, table: &'a mut Table, app: &str) -> &'a mut Entry {
        table.services.entry(app.to_owned()).or_insert_with(|| Entry {
            status: ServiceStatus::new(app),
            logs: Arc::new(LogRing::new(app, self.log_lines.clone())),
            instance: None,
            starts: 0,
            failures: 0,
            hold_until: None,
        })
    }

    /// Runs `change` on an existing entry, publishing its status if it
    /// changed: for late notices (a lease, an open) about a service that may
    /// have been removed meanwhile.
    pub fn update_existing(&self, app: &str, change: impl FnOnce(&mut Entry)) {
        let mut table = self.lock();
        if let Some(entry) = table.services.get_mut(app) {
            let before = entry.status.clone();
            change(entry);
            publish_change(&self.events, &before, &mut entry.status);
        }
    }

    /// Runs `change` on an entry and publishes its status if it changed.
    pub fn update<R>(&self, app: &str, change: impl FnOnce(&mut Entry) -> R) -> R {
        let mut table = self.lock();
        let entry = self.entry(&mut table, app);
        let before = entry.status.clone();
        let result = change(entry);
        publish_change(&self.events, &before, &mut entry.status);
        result
    }

    pub fn log(&self, app: &str, text: &str) {
        let ring = self.update(app, |entry| entry.logs.clone());
        ring.push(LogStream::Hub, text, false);
    }

    /// The relay dropped both halves of a channel.
    pub fn channel_ended(&self, app: &str, serial: u64, channel: u64) {
        self.update_existing(app, |entry| {
            if let Some(instance) = entry.instance.as_mut().filter(|instance| instance.serial == serial) {
                instance.channels.remove(&channel);
                entry.status.channels = instance.channels.len();
                let _ = instance.cmd.send(Cmd::Recount);
            }
        });
    }

    /// The instance `serial` has ended: in one step under the lock, its
    /// tickets fail, its channels are shut down so their relays read the
    /// end, it leaves the table, and `record` notes how it ended. An open
    /// that comes after sees the record, the failed-start hold included.
    pub fn finish_instance(&self, app: &str, serial: u64, record: impl FnOnce(&mut Entry)) {
        let mut table = self.lock();
        table.tickets.retain(|_, ticket| !(ticket.app == app && ticket.serial == serial));
        let entry = self.entry(&mut table, app);
        let before = entry.status.clone();
        if let Some(instance) = entry.instance.take_if(|instance| instance.serial == serial) {
            for slot in instance.channels.values() {
                let _ = slot.socket.shutdown(std::net::Shutdown::Both);
                let _ = slot.stop.send(true);
            }
        }
        record(entry);
        publish_change(&self.events, &before, &mut entry.status);
    }

    /// Shuts the channels of `serial` down now (the lease is lost).
    pub fn close_channels(&self, app: &str, serial: u64) {
        let mut table = self.lock();
        if let Some(instance) = self.entry(&mut table, app).instance.as_ref().filter(|instance| instance.serial == serial) {
            for slot in instance.channels.values() {
                let _ = slot.socket.shutdown(std::net::Shutdown::Both);
            }
        }
    }

    /// Tells the relays of `serial`'s channels that Hub is stopping it.
    pub fn signal_stop(&self, app: &str, serial: u64) {
        let mut table = self.lock();
        if let Some(instance) = self.entry(&mut table, app).instance.as_ref().filter(|instance| instance.serial == serial) {
            for slot in instance.channels.values() {
                let _ = slot.stop.send(true);
            }
        }
    }

    /// Sends Stop to the running instance of `app`, and the phase to wait on.
    fn request_stop(&self, app: &str, reason: StopReason) -> Option<watch::Receiver<Phase>> {
        let mut table = self.lock();
        let instance = table.services.get_mut(app)?.instance.as_mut()?;
        instance.stopping = true;
        let _ = instance.cmd.send(Cmd::Stop(reason));
        Some(instance.phase.clone())
    }
}

/// Removes an open's claim on its instance when the open ends, however it
/// ends: the instance may then become idle.
struct Pending<'a> {
    inner: &'a Arc<Inner>,
    app: String,
    serial: u64,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.inner.update_existing(&self.app, |entry| {
            if let Some(instance) = entry.instance.as_mut().filter(|instance| instance.serial == self.serial) {
                instance.pending = instance.pending.saturating_sub(1);
                let _ = instance.cmd.send(Cmd::Recount);
            }
        });
    }
}

/// Removes an unclaimed ticket when its open ends: a later `channel.attach`
/// with it is refused.
struct TicketGuard<'a> {
    inner: &'a Arc<Inner>,
    ticket: String,
}

impl Drop for TicketGuard<'_> {
    fn drop(&mut self) {
        self.inner.lock().tickets.remove(&self.ticket);
    }
}

/// Owns the service socket and every service process.
pub struct Supervisor {
    inner: Arc<Inner>,
    accept: JoinHandle<()>,
}

impl Supervisor {
    /// Binds the socket (mode 0600) and starts accepting. A stale socket
    /// file is replaced; a socket another process still serves is refused.
    pub async fn new(config: Config) -> Result<Self, SupervisorError> {
        let (listener, socket_id) = bind(&config.socket)?;
        let runtime = BunRuntime::new(&config.home, config.bun_path.clone());
        let registry = Registry::new(&config.home, runtime.clone(), config.timings.install).with_sdk(config.sdk.clone());
        let inner = Arc::new(Inner {
            config,
            runtime,
            registry,
            table: Mutex::new(Table::default()),
            events: broadcast::channel(256).0,
            log_lines: broadcast::channel(1024).0,
            closing: AtomicBool::new(false),
            serials: AtomicU64::new(0),
            socket_id,
        });
        let accept = tokio::spawn(accept_loop(Arc::downgrade(&inner), listener));
        Ok(Self { inner, accept })
    }

    pub fn runtime(&self) -> &BunRuntime {
        &self.inner.runtime
    }

    pub fn registry(&self) -> &Registry {
        &self.inner.registry
    }

    pub fn socket(&self) -> &Path {
        &self.inner.config.socket
    }

    /// Status changes, installs and removals.
    pub fn subscribe(&self) -> broadcast::Receiver<ServiceEvent> {
        self.inner.events.subscribe()
    }

    /// Every service's log lines as they come.
    pub fn subscribe_logs(&self) -> broadcast::Receiver<LogLine> {
        self.inner.log_lines.subscribe()
    }

    /// The kept log lines of `id`, oldest first; with `after`, those after
    /// that sequence number.
    pub fn logs(&self, id: &str, after: Option<u64>) -> Vec<LogLine> {
        let table = self.inner.lock();
        table.services.get(id).map(|entry| entry.logs.lines(after)).unwrap_or_default()
    }

    /// The status of `id`, installed or with a process.
    pub fn status(&self, id: &str) -> Option<ServiceStatus> {
        let installed = self.inner.registry.get(id);
        let table = self.inner.lock();
        let mut status = match (table.services.get(id), &installed) {
            (Some(entry), _) => entry.status.clone(),
            (None, Some(_)) => ServiceStatus::new(id),
            (None, None) => return None,
        };
        status.version = installed.map(|service| service.version);
        Some(status)
    }

    /// Every installed service's status, and any other with a process, by id.
    pub fn statuses(&self) -> Vec<ServiceStatus> {
        let mut ids: Vec<String> = self.inner.registry.list().into_iter().map(|service| service.id).collect();
        ids.extend(self.inner.lock().services.iter().filter(|(_, entry)| entry.instance.is_some()).map(|(id, _)| id.clone()));
        ids.sort();
        ids.dedup();
        ids.iter().filter_map(|id| self.status(id)).collect()
    }

    /// Installs a package (`Registry::install`). A process of an older
    /// version keeps running while it has channels and the new version takes
    /// effect at its next start; one with no channel is stopped at once, so
    /// the next open starts the new version. A new package clears the hold
    /// after failed starts.
    pub async fn install(&self, source: &Path) -> Result<InstallReport, InstallError> {
        let running: Vec<String> = {
            let table = self.inner.lock();
            table.services.values().filter_map(|entry| entry.instance.as_ref().map(|instance| instance.version.clone())).collect()
        };
        let report = self.inner.registry.install(source, &running).await?;
        let id = report.service.id.clone();
        let version = report.service.version.clone();
        let idle_stop = self.inner.update(&id, |entry| {
            entry.status.version = Some(version.clone());
            entry.failures = 0;
            entry.hold_until = None;
            entry.status.retry_at = None;
            if entry.status.state == ServiceState::Failed {
                entry.status.state = ServiceState::Stopped;
            }
            let instance = entry.instance.as_mut()?;
            if !instance.idle() || instance.stopping {
                return None;
            }
            instance.stopping = true;
            let _ = instance.cmd.send(Cmd::Stop(StopReason::Upgrade));
            Some(instance.phase.clone())
        });
        self.inner.log(&id, &format!("installed {version}"));
        let _ = self.inner.events.send(ServiceEvent::Installed(report.service.clone()));
        if let Some(mut phase) = idle_stop {
            let _ = phase.wait_for(|phase| matches!(phase, Phase::Down(_))).await;
        }
        Ok(report)
    }

    /// Stops `id`'s process, then removes its packages, and its data folder
    /// with `purge_data`. False when it was not installed.
    pub async fn remove(&self, id: &str, purge_data: bool) -> io::Result<bool> {
        self.stop(id).await;
        let removed = self.inner.registry.remove(id, purge_data).await?;
        {
            let mut table = self.inner.lock();
            if table.services.get(id).is_some_and(|entry| entry.instance.is_none()) {
                table.services.remove(id);
            }
        }
        if removed {
            let _ = self.inner.events.send(ServiceEvent::Removed { id: id.to_owned() });
        }
        Ok(removed)
    }

    /// Opens a channel to `offer.app`'s service (§4, §5): starts the process
    /// if needed, offers the channel with a fresh ticket and returns the
    /// connection that attaches with it, positioned right after its
    /// `channel.attach`. Answers within `Timings::open_budget`.
    pub async fn open_channel(&self, offer: ChannelOffer) -> Result<ChannelStream, OpenError> {
        let budget = self.inner.config.timings.open_budget;
        match tokio::time::timeout(budget, self.open(offer)).await {
            Ok(result) => result,
            Err(_) => Err(OpenError::Busy(format!("the service did not take the channel within {} s", budget.as_secs_f32()))),
        }
    }

    async fn open(&self, offer: ChannelOffer) -> Result<ChannelStream, OpenError> {
        let inner = &self.inner;
        if !valid_id(&offer.app) {
            return Err(OpenError::NoService);
        }
        let Some(service) = inner.registry.get(&offer.app) else {
            return Err(OpenError::NoService);
        };
        let (serial, phase, cmd) = self.claim(&service, false).await.map_err(OpenError::Busy)?;
        let _pending = Pending { inner, app: offer.app.clone(), serial };
        let mut phase = phase;
        let ready = phase.wait_for(|phase| !matches!(phase, Phase::Starting)).await.map(|phase| phase.clone());
        match ready {
            Ok(Phase::Running) => {}
            Ok(Phase::Stopping(why) | Phase::Down(why)) => return Err(OpenError::Busy(why)),
            _ => return Err(OpenError::Busy("the service ended".into())),
        }

        let ticket = random_hex();
        let (tx, rx) = oneshot::channel();
        inner.lock().tickets.insert(ticket.clone(), Ticket { app: offer.app.clone(), serial, tx });
        let _ticket = TicketGuard { inner, ticket: ticket.clone() };
        let message = json!({
            "t": "channel.offer",
            "ticket": ticket,
            "device": offer.device,
            "deviceName": offer.device_name,
            "model": offer.model,
            "target": offer.target,
            "app": offer.app,
            "hash": offer.hash,
            "generation": offer.generation,
            "payloadLimit": offer.payload_limit,
        });
        let frame = wire::event(&message).map_err(|error| OpenError::Busy(format!("channel.offer: {error}")))?;
        if cmd.send(Cmd::Offer(frame)).is_err() {
            return Err(OpenError::Busy("the service ended".into()));
        }
        let attach = inner.config.timings.attach;
        let stream = tokio::select! {
            stream = rx => stream.map_err(|_| OpenError::Busy("the service ended before it attached".into()))?,
            _ = tokio::time::sleep(attach) => {
                inner.log(&offer.app, &format!("no channel.attach within {} s of channel.offer", attach.as_secs_f32()));
                return Err(OpenError::Busy(format!("the service did not attach within {} s", attach.as_secs_f32())));
            }
        };

        // A second descriptor of the socket stays with the supervisor, to
        // shut the connection down if the process ends.
        let std_stream = stream.into_std().map_err(|error| OpenError::Busy(error.to_string()))?;
        let socket = std_stream.try_clone().map_err(|error| OpenError::Busy(error.to_string()))?;
        let stream = UnixStream::from_std(std_stream).map_err(|error| OpenError::Busy(error.to_string()))?;
        let channel = inner.next_serial();
        let (stop_tx, stop_rx) = watch::channel(false);
        let info = inner.update(&offer.app, |entry| {
            let instance = entry.instance.as_mut().filter(|instance| instance.serial == serial)?;
            instance.channels.insert(channel, ChannelSlot { socket, stop: stop_tx });
            entry.status.channels = instance.channels.len();
            Some(ChannelInfo { app: offer.app.clone(), version: instance.version.clone(), pid: instance.pid, channel })
        });
        let Some(info) = info else {
            return Err(OpenError::Busy("the service ended as it attached".into()));
        };
        let lease = ChannelLease { inner: Arc::downgrade(inner), app: offer.app.clone(), serial, channel };
        Ok(make_stream(stream, lease, info, stop_rx))
    }

    /// A process of `service` that takes new channels: the running one, or a
    /// new one once a stopping one has ended. Counted as pending until the
    /// caller's `Pending` drops. `owner` starts even while failed starts hold
    /// the service off.
    async fn claim(
        &self,
        service: &InstalledService,
        owner: bool,
    ) -> Result<(u64, watch::Receiver<Phase>, mpsc::UnboundedSender<Cmd>), String> {
        let inner = &self.inner;
        let app = service.id.as_str();
        let bun = match inner.runtime.available() {
            Some(bun) => bun,
            // The download goes on in its own task when this open gives up.
            None => inner.runtime.ensure().await.map_err(|error| error.to_string())?,
        };
        loop {
            enum Next {
                Claimed(u64, watch::Receiver<Phase>, mpsc::UnboundedSender<Cmd>),
                WaitDown(watch::Receiver<Phase>),
            }
            let next = inner.update(app, |entry| -> Result<Next, String> {
                // Read under the table's lock, which `shutdown` takes after
                // setting it: no process starts that `shutdown` misses.
                if inner.closing.load(Ordering::SeqCst) {
                    return Err("Pocket Hub is quitting".into());
                }
                if let Some(instance) = entry.instance.as_mut() {
                    if instance.stopping {
                        return Ok(Next::WaitDown(instance.phase.clone()));
                    }
                    instance.pending += 1;
                    return Ok(Next::Claimed(instance.serial, instance.phase.clone(), instance.cmd.clone()));
                }
                if !owner && let Some(until) = entry.hold_until {
                    let now = Instant::now();
                    if until > now {
                        let why = entry.status.last_error.clone().unwrap_or_else(|| "the service failed to start".into());
                        return Err(format!("{why}; Hub starts it again in {:.1} s", (until - now).as_secs_f32()));
                    }
                }
                let instance = instance::spawn(inner, entry, service, &bun).map_err(|error| format!("starting the service: {error}"))?;
                let claimed = Next::Claimed(instance.serial, instance.phase.clone(), instance.cmd.clone());
                entry.instance = Some(instance);
                if let Some(instance) = entry.instance.as_mut() {
                    instance.pending += 1;
                }
                Ok(claimed)
            })?;
            match next {
                Next::Claimed(serial, phase, cmd) => return Ok((serial, phase, cmd)),
                Next::WaitDown(mut phase) => {
                    let _ = phase.wait_for(|phase| matches!(phase, Phase::Down(_))).await;
                }
            }
        }
    }

    /// Starts `id`'s process if it is not running, even while failed starts
    /// hold it off, and waits until it is connected. With no channel it
    /// stops again after its idle time, like any other.
    pub async fn start(&self, id: &str) -> Result<ServiceStatus, StartError> {
        let service = self.inner.registry.get(id).ok_or_else(|| StartError::NotInstalled(id.to_owned()))?;
        let (serial, mut phase, _) = self.claim(&service, true).await.map_err(StartError::Failed)?;
        let pending = Pending { inner: &self.inner, app: id.to_owned(), serial };
        let ready = phase.wait_for(|phase| !matches!(phase, Phase::Starting)).await.map(|phase| phase.clone());
        drop(pending);
        match ready {
            Ok(Phase::Running) => Ok(self.status(id).expect("installed")),
            Ok(Phase::Stopping(why) | Phase::Down(why)) => Err(StartError::Failed(why)),
            _ => Err(StartError::Failed("the service ended".into())),
        }
    }

    /// Stops `id`'s process (§4: `service.stop`, then `SIGTERM`, then
    /// `SIGKILL`) and waits until it has ended. Clears a failed state.
    pub async fn stop(&self, id: &str) {
        if let Some(mut phase) = self.inner.request_stop(id, StopReason::Owner) {
            let _ = phase.wait_for(|phase| matches!(phase, Phase::Down(_))).await;
        }
        self.inner.update_existing(id, |entry| {
            if entry.instance.is_none() {
                entry.failures = 0;
                entry.hold_until = None;
                entry.status.retry_at = None;
                entry.status.state = ServiceState::Stopped;
            }
        });
    }

    pub async fn restart(&self, id: &str) -> Result<ServiceStatus, StartError> {
        self.stop(id).await;
        self.start(id).await
    }

    /// Stops every process and the socket, for Hub quitting. Opens after this
    /// are answered `Busy`.
    pub async fn shutdown(&self) {
        self.inner.closing.store(true, Ordering::SeqCst);
        self.accept.abort();
        let ids: Vec<String> = self.inner.lock().services.keys().cloned().collect();
        let phases: Vec<_> = ids.iter().filter_map(|id| self.inner.request_stop(id, StopReason::Shutdown)).collect();
        for mut phase in phases {
            let _ = phase.wait_for(|phase| matches!(phase, Phase::Down(_))).await;
        }
        self.inner.lock().tickets.clear();
        remove_socket(&self.inner.config.socket, self.inner.socket_id);
    }
}

impl Drop for Supervisor {
    /// The last resort when `shutdown` was not awaited: no service outlives
    /// its supervisor.
    fn drop(&mut self) {
        self.accept.abort();
        self.inner.closing.store(true, Ordering::SeqCst);
        let table = self.inner.lock();
        for entry in table.services.values() {
            if let Some(instance) = &entry.instance {
                signal_group(instance.pid, libc::SIGKILL);
            }
        }
        drop(table);
        remove_socket(&self.inner.config.socket, self.inner.socket_id);
    }
}

/// Binds the socket through a temporary name, set to 0600 before it is
/// renamed into place, so it is never reachable with looser permissions.
fn bind(path: &Path) -> Result<(UnixListener, (u64, u64)), SupervisorError> {
    let socket_error = |error| SupervisorError::Socket(path.to_path_buf(), error);
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(SupervisorError::NotASocket(path.to_path_buf()));
        }
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(SupervisorError::SocketInUse(path.to_path_buf()));
        }
        // Nobody answers: a Hub that ended without removing it.
        fs::remove_file(path).map_err(socket_error)?;
    }
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let temp = parent.join(format!(".hub-{}", &random_hex()[..8]));
    let listener = std::os::unix::net::UnixListener::bind(&temp).map_err(socket_error)?;
    let placed = (|| {
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
        fs::rename(&temp, path)?;
        listener.set_nonblocking(true)?;
        let meta = fs::symlink_metadata(path)?;
        Ok::<_, io::Error>((meta.dev(), meta.ino()))
    })();
    let socket_id = match placed {
        Ok(id) => id,
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(socket_error(error));
        }
    };
    let listener = UnixListener::from_std(listener).map_err(socket_error)?;
    Ok((listener, socket_id))
}

fn remove_socket(path: &Path, id: (u64, u64)) {
    if fs::symlink_metadata(path).is_ok_and(|meta| (meta.dev(), meta.ino()) == id) {
        let _ = fs::remove_file(path);
    }
}

async fn accept_loop(inner: std::sync::Weak<Inner>, listener: UnixListener) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(error) => {
                tracing::warn!(%error, "accepting on the service socket");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Some(inner) = inner.upgrade() else { return };
        tokio::spawn(serve(inner, stream));
    }
}

/// The first frame of a connection says what it is (§4, §5).
async fn serve(inner: Arc<Inner>, mut stream: UnixStream) {
    let first = match tokio::time::timeout(inner.config.timings.first_frame, wire::read_handshake(&mut stream)).await {
        Ok(Ok(first)) => first,
        Ok(Err(error)) => {
            tracing::debug!(%error, "a service socket connection without its opening HANDSHAKE");
            return;
        }
        Err(_) => return,
    };
    let object = first.object.clone().unwrap_or_default();
    let text = |key: &str| object.get(key).and_then(|value| value.as_str()).unwrap_or("").to_owned();
    match first.t() {
        Some("service.hello") => {
            let app = text("app");
            let token = text("token");
            let protocol = object.get("protocol").and_then(|value| value.as_u64());
            hello(&inner, stream, &app, &token, protocol).await;
        }
        Some("channel.attach") => attach(&inner, stream, &text("ticket")).await,
        other => tracing::debug!(t = ?other, "a service socket connection opened with an unknown HANDSHAKE"),
    }
}

async fn refuse(mut stream: UnixStream, t: &str, message: &str) {
    let frame = wire::handshake(&json!({ "t": t, "message": cut(message, 200) }));
    let _ = wire::write_all(&mut stream, &frame).await;
    let _ = tokio::io::AsyncWriteExt::shutdown(&mut stream).await;
}

/// §4: a `service.hello` must carry the token of a process Hub started and is
/// waiting for. The instance answers `service.ready` itself.
async fn hello(inner: &Arc<Inner>, stream: UnixStream, app: &str, token: &str, protocol: Option<u64>) {
    let waiting = {
        let mut table = inner.lock();
        table.services.get_mut(app).and_then(|entry| {
            let instance = entry.instance.as_mut()?;
            (instance.hello.is_some() && same_secret(&instance.token, token)).then(|| instance.hello.take()).flatten()
        })
    };
    let Some(waiting) = waiting else {
        refuse(stream, "service.refused", "unknown token").await;
        return;
    };
    if protocol != Some(crate::PROTOCOL_VERSION) {
        let message = format!("protocol {} is not supported; this Hub speaks protocol {}", protocol.map_or("?".into(), |p| p.to_string()), crate::PROTOCOL_VERSION);
        let _ = waiting.send(Err(message.clone()));
        refuse(stream, "service.refused", &message).await;
        return;
    }
    if let Err(Ok(stream)) = waiting.send(Ok(stream)) {
        refuse(stream, "service.refused", "the process is no longer expected").await;
    }
}

/// §5: a channel connection names its offer's ticket. Each ticket is good for
/// one connection while its open waits.
async fn attach(inner: &Arc<Inner>, stream: UnixStream, ticket: &str) {
    let claimed = inner.lock().tickets.remove(ticket);
    let Some(claimed) = claimed else {
        refuse(stream, "channel.refused", "unknown or expired ticket").await;
        return;
    };
    if let Err(stream) = claimed.tx.send(stream) {
        refuse(stream, "channel.refused", "unknown or expired ticket").await;
    }
}

pub(crate) fn backoff(timings: &crate::config::Timings, failures: u32) -> Duration {
    let factor = 1u32.checked_shl(failures.saturating_sub(1)).unwrap_or(u32::MAX);
    timings.failure_backoff.saturating_mul(factor).min(timings.failure_backoff_max)
}

pub(crate) fn exit_info(status: Option<std::process::ExitStatus>) -> ExitInfo {
    use std::os::unix::process::ExitStatusExt;
    ExitInfo { code: status.and_then(|s| s.code()), signal: status.and_then(|s| s.signal()), at: epoch_ms() }
}
