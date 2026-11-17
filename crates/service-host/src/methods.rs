//! The control API's service methods as a library (docs/CONTROL-API.md §3
//! Services, §4 Events): what a program that serves them mounts into its
//! own control server. `pocket-hubd` is one; a program that runs services
//! on a device with no Hub is meant to be another, from this crate with
//! `hub-core` and `hub-api` alone.
//!
//! - [`ServiceHost::start`] writes the SDK into the home and starts the
//!   supervisor on the service socket.
//! - [`ServiceHost::connector`] is what the link endpoint hands a device's
//!   applet channels to.
//! - [`ServiceHost::call`] answers `services.*`; [`ServiceHost::handles`]
//!   says which methods those are. The server keeps the envelope (hello,
//!   ids, cancel, the client's connection) and hands each call over.
//! - [`ServiceHost::attach`] turns the supervisor's events into
//!   `service.changed`, `service.removed` and `service.log`, through the
//!   server's [`Publisher`].
//!
//! The answers and their errors are the contract's, word for word: they
//! moved here from `pocket-hubd` unchanged.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pocket_hub_api::rpc::{absolute, params, sentence};
use pocket_hub_api::types::{
    self as api, BunInfo, Empty, Event, RuntimeInfo, RuntimeParams, RuntimeProgress, ServiceIdParams, ServiceInstallProgress, ServiceList,
    ServiceLogs, ServiceLogsParams, ServicePathParams, ServiceRemoveParams,
};
use pocket_hub_api::{ErrorKind, RpcError, methods};
use pocket_hub_core::ServiceConnector;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::child::InheritNothing;
use crate::connector::Connector;
use crate::registry::Removed;
use crate::supervisor::EARLIER_SENTENCE;
use crate::{Config, InstallError, InstalledService, LogLine, LogStream, Progress, ServiceEvent, ServiceState, ServiceStatus, StartError, Supervisor};

/// What the program that serves the methods gives them: its event broadcast
/// and the order of its answers (CONTROL-API.md §1, Transport: an answer
/// reflects every event written before it).
pub trait Publisher: Send + Sync + 'static {
    /// Runs `body` holding the lock under which events are published and the
    /// answers that read published state are made. `services.list` is
    /// answered inside it, and each `service.changed` is made inside it.
    fn ordered(&self, body: &mut dyn FnMut());
    /// Hands an event to every subscribed client.
    fn publish(&self, event: Event);
    /// The host's events were lost to a full queue: the program publishes
    /// its state again, every service of [`ServiceHost::services`] among it.
    fn resync(&self);
}

/// Where the host lives and runs.
#[derive(Debug, Clone)]
pub struct HostOptions {
    /// The home folder: `services/`, `service-data/`, `runtimes/` and
    /// `sdk/service` are made in it.
    pub home: PathBuf,
    /// The service socket (SERVICE-PROTOCOL.md §2).
    pub socket: PathBuf,
    /// An existing Bun instead of the one the host downloads.
    pub bun: Option<PathBuf>,
    /// Replaces every manifest's `idleSeconds` (development and tests).
    pub idle_override: Option<Duration>,
}

/// The service host: the supervisor and the control API's service methods.
pub struct ServiceHost {
    supervisor: Arc<Supervisor>,
    /// The version of the configured Bun as it reports it; None for the
    /// pinned Bun the host downloads.
    bun_override_version: Option<String>,
    publisher: OnceLock<Arc<dyn Publisher>>,
    /// Subscribed at the start, so nothing the supervisor says before
    /// [`ServiceHost::attach`] is lost.
    events: Mutex<Option<(broadcast::Receiver<ServiceEvent>, broadcast::Receiver<LogLine>)>>,
}

impl ServiceHost {
    /// Writes the SDK under the home and starts the supervisor on the
    /// service socket. The error is the one sentence a program shows when it
    /// cannot start.
    pub async fn start(options: HostOptions) -> Result<Arc<ServiceHost>, String> {
        let sdk = crate::sdk::materialize(&options.home).map_err(|error| format!("Pocket Hub cannot write the service SDK into its home: {error}."))?;
        let mut config = Config::new(&options.home, &options.socket);
        config.bun_path = options.bun.clone();
        config.sdk = Some(sdk);
        config.timings.idle_override = options.idle_override;
        let supervisor = Arc::new(Supervisor::new(config).await.map_err(|error| format!("Pocket Hub cannot start its service supervisor: {error}."))?);
        let events = Some((supervisor.subscribe(), supervisor.subscribe_logs()));
        Ok(Arc::new(ServiceHost {
            bun_override_version: options.bun.as_deref().and_then(reported_version),
            supervisor,
            publisher: OnceLock::new(),
            events: Mutex::new(events),
        }))
    }

    /// What the link endpoint hands a device's applet channels to.
    pub fn connector(&self) -> Arc<dyn ServiceConnector> {
        Arc::new(Connector(self.supervisor.clone()))
    }

    /// Publishes the supervisor's events through `publisher` from now on, and
    /// orders `services.list` with them. Returns the task that does it; the
    /// program aborts it when it stops. Called once.
    pub fn attach(self: &Arc<Self>, publisher: Arc<dyn Publisher>) -> JoinHandle<()> {
        let _ = self.publisher.set(publisher);
        let (states, logs) = self.events.lock().unwrap().take().expect("attach is called once");
        tokio::spawn(pump(self.clone(), states, logs))
    }

    /// Whether `method` is one of the methods [`ServiceHost::call`] answers.
    pub fn handles(method: &str) -> bool {
        matches!(
            method,
            methods::SERVICES_LIST
                | methods::SERVICES_INSTALL
                | methods::SERVICES_REMOVE
                | methods::SERVICES_START
                | methods::SERVICES_STOP
                | methods::SERVICES_RESTART
                | methods::SERVICES_LOGS
                | methods::SERVICES_RUNTIME
        )
    }

    /// Answers one of the service methods. `progress` gets the members of
    /// each progress notification of the call (the server adds the call's id).
    pub async fn call(&self, method: &str, value: Value, progress: &(dyn Fn(Value) + Sync)) -> Result<Value, RpcError> {
        match method {
            methods::SERVICES_LIST => {
                let mut services = Vec::new();
                self.ordered(&mut || services = self.services());
                ok(ServiceList { services })
            }
            methods::SERVICES_INSTALL => self.install(params(method, value)?, progress).await,
            methods::SERVICES_REMOVE => {
                let p: ServiceRemoveParams = params(method, value)?;
                let removed = self.supervisor.remove(&p.id, p.release.as_deref(), p.purge.unwrap_or(false)).await.map_err(|error| service_error(&p.id, format!("The service could not be removed: {error}.")))?;
                if removed == Removed::Nothing {
                    return Err(no_release(&p.id, p.release.as_deref()));
                }
                ok(Empty {})
            }
            methods::SERVICES_START => {
                let p: ServiceIdParams = params(method, value)?;
                self.release_named(&p)?;
                self.supervisor.start(&p.id).await.map_err(|error| start_error(&p.id, error))?;
                self.service_now(&p.id)
            }
            methods::SERVICES_STOP => {
                let p: ServiceIdParams = params(method, value)?;
                self.installed(&p.id)?;
                self.release_named(&p)?;
                self.supervisor.stop(&p.id).await;
                self.service_now(&p.id)
            }
            methods::SERVICES_RESTART => {
                let p: ServiceIdParams = params(method, value)?;
                self.installed(&p.id)?;
                self.release_named(&p)?;
                self.supervisor.restart(&p.id).await.map_err(|error| start_error(&p.id, error))?;
                self.service_now(&p.id)
            }
            methods::SERVICES_LOGS => {
                let p: ServiceLogsParams = params(method, value)?;
                let lines = self.supervisor.logs(&p.id, None);
                if lines.is_empty() && self.supervisor.status(&p.id).is_none() {
                    return Err(no_service(&p.id));
                }
                let tail = p.tail.unwrap_or(200);
                let lines = lines[lines.len().saturating_sub(tail)..].iter().map(service_log_line).collect();
                ok(ServiceLogs { lines })
            }
            methods::SERVICES_RUNTIME => self.runtime(params(method, value)?, progress).await,
            other => Err(RpcError::method_not_found(other)),
        }
    }

    /// Every installed service, as the contract's `Service`.
    pub fn services(&self) -> Vec<api::Service> {
        self.supervisor.statuses().iter().map(|status| service(status, self.supervisor.registry().get(&status.id).as_ref())).collect()
    }

    /// One installed service, as the contract's `Service`.
    pub fn service(&self, id: &str) -> Option<api::Service> {
        let status = self.supervisor.status(id)?;
        Some(service(&status, self.supervisor.registry().get(id).as_ref()))
    }

    /// The Bun services run on, as `hub.status` gives it: the configured
    /// Bun's own version, else the pinned one, and whether it is there.
    pub fn bun(&self) -> BunInfo {
        BunInfo { version: self.bun_version(), installed: self.supervisor.runtime().available().is_some() }
    }

    /// Stops every service (SERVICE-PROTOCOL.md §4) and closes the socket.
    pub async fn shutdown(&self) {
        self.supervisor.shutdown().await;
    }

    fn bun_version(&self) -> String {
        self.bun_override_version.clone().unwrap_or_else(|| self.supervisor.runtime().version().to_owned())
    }

    fn ordered(&self, body: &mut dyn FnMut()) {
        match self.publisher.get() {
            Some(publisher) => publisher.ordered(body),
            None => body(),
        }
    }

    fn installed(&self, id: &str) -> Result<(), RpcError> {
        self.supervisor.status(id).map(|_| ()).ok_or_else(|| no_service(id))
    }

    /// A `release` the call names must be the installed one. An id with no
    /// release (one an earlier Pocket Hub installed) has none to name.
    fn release_named(&self, p: &ServiceIdParams) -> Result<(), RpcError> {
        let Some(release) = &p.release else { return Ok(()) };
        match self.supervisor.registry().release(&p.id, release) {
            Some(_) => Ok(()),
            None => Err(no_release(&p.id, Some(release))),
        }
    }

    fn service_now(&self, id: &str) -> Result<Value, RpcError> {
        ok(self.service(id).ok_or_else(|| no_service(id))?)
    }

    /// `service.changed` with the service as it is now (`fallback` when it is
    /// no longer known, as after a removal with its process still ending).
    fn publish_service(&self, publisher: &dyn Publisher, id: &str, fallback: Option<&ServiceStatus>) {
        publisher.ordered(&mut || {
            let service = self.service(id).or_else(|| fallback.map(|status| service(status, None)));
            if let Some(service) = service {
                publisher.publish(Event::ServiceChanged { service });
            }
        });
    }

    /// Downloads Bun when it is missing, with `{done, total}` going to `each`.
    async fn ensure_bun(&self, mut each: impl FnMut(Progress)) -> Result<PathBuf, RpcError> {
        let runtime = self.supervisor.runtime();
        if let Some(path) = runtime.available() {
            return Ok(path);
        }
        runtime.ensure_with(&mut each).await.map_err(|error| RpcError::op(ErrorKind::Internal, sentence(&format!("Bun could not be installed: {error}"))))
    }

    async fn install(&self, p: ServicePathParams, progress: &(dyn Fn(Value) + Sync)) -> Result<Value, RpcError> {
        let path = absolute(&p.path)?;
        if !path.exists() {
            return Err(RpcError::op(ErrorKind::NotFound, format!("There is nothing at {}.", path.display())));
        }
        // A service runs on Bun: the first install downloads it.
        let mut shown = None;
        let source = path.display().to_string();
        self.ensure_bun(|step| {
            let message = match step.total {
                Some(total) if total > 0 => format!("Downloading Bun: {}%.", step.done * 100 / total),
                _ => format!("Downloading Bun: {} KiB.", step.done / 1024),
            };
            if shown.as_ref() != Some(&message) {
                send(progress, ServiceInstallProgress { stage: "runtime".into(), message: message.clone() });
                shown = Some(message);
            }
        })
        .await
        .map_err(|error| RpcError::op(ErrorKind::Service, error.message).with(|data| data.reason = Some("runtime".into())))?;
        send(progress, ServiceInstallProgress { stage: "installing".into(), message: format!("Installing the service package {}.", path.display()) });
        // A package that is not one is an unusable argument; a package that
        // could not be installed (its dependencies, the disk) is the service's.
        // A package that is not one is an unusable argument; a manifest the
        // host refuses is the service's (SERVICE-PROTOCOL.md §1), its field
        // in `detail`; a package that could not be installed (its
        // dependencies, the disk) is the service's too.
        let report = self.supervisor.install(&path).await.map_err(|error| match error {
            InstallError::Source(_) | InstallError::Archive(_) => RpcError::op(ErrorKind::Invalid, sentence(&error.to_string())),
            InstallError::Manifest(manifest) => RpcError::op(ErrorKind::Service, sentence(&manifest.to_string())).with(|data| {
                data.reason = Some("manifest".into());
                data.detail = manifest.field();
            }),
            InstallError::Runtime(runtime) => RpcError::op(ErrorKind::Service, sentence(&format!("{source} could not be installed: {runtime}"))).with(|data| data.reason = Some("runtime".into())),
            other => RpcError::op(ErrorKind::Service, sentence(&format!("{source} could not be installed: {other}"))),
        })?;
        self.service_now(&report.service.id)
    }

    async fn runtime(&self, p: RuntimeParams, progress: &(dyn Fn(Value) + Sync)) -> Result<Value, RpcError> {
        let runtime = self.supervisor.runtime();
        if p.install.unwrap_or(false) {
            let mut last = None;
            self.ensure_bun(|step| {
                let tick = (step.done / (256 * 1024), step.done == step.total.unwrap_or(u64::MAX));
                if last != Some(tick) {
                    last = Some(tick);
                    send(progress, RuntimeProgress { done: step.done, total: step.total });
                }
            })
            .await?;
        }
        let path = runtime.available();
        ok(RuntimeInfo {
            version: self.bun_version(),
            installed: path.is_some(),
            path: path.unwrap_or_else(|| runtime.install_path()).display().to_string(),
        })
    }
}

/// Turns the supervisor's events into the contract's.
async fn pump(host: Arc<ServiceHost>, mut states: broadcast::Receiver<ServiceEvent>, mut logs: broadcast::Receiver<LogLine>) {
    use broadcast::error::RecvError;
    let publisher = host.publisher.get().expect("attached").clone();
    loop {
        tokio::select! {
            event = states.recv() => match event {
                Ok(ServiceEvent::State(status)) => host.publish_service(&*publisher, &status.id.clone(), Some(&status)),
                Ok(ServiceEvent::Installed(service)) => host.publish_service(&*publisher, &service.id, None),
                Ok(ServiceEvent::Removed { id, release }) => publisher.publish(Event::ServiceRemoved { id, release }),
                Err(RecvError::Lagged(_)) => publisher.resync(),
                Err(RecvError::Closed) => return,
            },
            line = logs.recv() => match line {
                Ok(line) => publisher.publish(Event::ServiceLog { id: line.app.clone(), release: line.release.clone(), at: line.at, stream: log_stream(line.stream).to_owned(), text: line.text }),
                Err(RecvError::Lagged(missed)) => tracing::warn!(missed, "service log lines were dropped from the control API"),
                Err(RecvError::Closed) => return,
            },
        }
    }
}

fn ok(value: impl Serialize) -> Result<Value, RpcError> {
    Ok(serde_json::to_value(value).expect("results serialize"))
}

fn send(progress: &(dyn Fn(Value) + Sync), members: impl Serialize) {
    progress(serde_json::to_value(members).expect("progress serializes"));
}

fn no_service(id: &str) -> RpcError {
    RpcError::op(ErrorKind::NotFound, format!("No service {id} is installed."))
}

/// `not_found` for an id, or for one release of it.
fn no_release(id: &str, release: Option<&str>) -> RpcError {
    match release {
        None => no_service(id),
        Some(release) => RpcError::op(ErrorKind::NotFound, format!("No release {release} of the service {id} is installed.")),
    }
}

/// The error `service` for `id`.
fn service_error(id: &str, message: impl Into<String>) -> RpcError {
    let id = id.to_owned();
    RpcError::op(ErrorKind::Service, message).with(|data| data.id = Some(id))
}

fn start_error(id: &str, error: StartError) -> RpcError {
    match error {
        StartError::NotInstalled(id) => no_service(&id),
        StartError::Earlier(_) => service_error(id, EARLIER_SENTENCE).with(|data| data.reason = Some("earlier".into())),
        StartError::Failed(why) => service_error(id, sentence(&format!("the service did not start: {why}"))),
    }
}

/// `<bun> --version`, for a Bun the host did not download.
fn reported_version(bun: &Path) -> Option<String> {
    let output = std::process::Command::new(bun).arg("--version").stdin(std::process::Stdio::null()).inherit_nothing().output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !text.trim().is_empty()).then(|| text.trim().to_owned())
}

/// `ServiceStatus` → `Service` (§2).
///
/// | contract | supervisor |
/// | --- | --- |
/// | `release` | the installed release, else the release a removed service still runs; null for an earlier one |
/// | `requires`, `hosts` | the installed release's manifest; empty for an earlier one |
/// | `state` | `state` (same five names) |
/// | `since` | `since`; for a service that has not changed state since Hub started, its install time |
/// | `lastExit.signal` | the signal number's name (`SIGTERM`) |
/// | `error` | `last_error` while `failed`, else null |
/// | `reason` | `earlier` for an id an earlier Pocket Hub installed |
///
/// `started_at`, `restarts`, `crashes` and `retry_at` have no place in the
/// contract.
pub fn service(status: &ServiceStatus, installed: Option<&InstalledService>) -> api::Service {
    api::Service {
        id: status.id.clone(),
        release: status.release.clone().or_else(|| installed.map(|service| service.release.clone())).or_else(|| status.running_release.clone()),
        requires: installed.map(|service| service.requires.clone()).unwrap_or_default(),
        hosts: installed.map(|service| service.hosts.clone()).unwrap_or_default(),
        state: match status.state {
            ServiceState::Stopped => api::ServiceState::Stopped,
            ServiceState::Starting => api::ServiceState::Starting,
            ServiceState::Running => api::ServiceState::Running,
            ServiceState::Stopping => api::ServiceState::Stopping,
            ServiceState::Failed => api::ServiceState::Failed,
        },
        pid: status.pid,
        since: status.since.or_else(|| installed.map(|service| service.installed_at)),
        channels: status.channels as u32,
        last_exit: status.last_exit.as_ref().map(|exit| api::LastExit { code: exit.code, signal: exit.signal.map(signal_name), at: exit.at }),
        error: if status.state == ServiceState::Failed { status.last_error.clone() } else { None },
        reason: status.earlier.then(|| "earlier".to_owned()),
    }
}

fn log_stream(stream: LogStream) -> &'static str {
    match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
        LogStream::Hub => "hub",
    }
}

fn service_log_line(line: &LogLine) -> api::ServiceLogLine {
    api::ServiceLogLine { at: line.at, stream: log_stream(line.stream).to_owned(), text: line.text.clone() }
}

/// A signal's name, `SIG<number>` for one this table does not hold.
pub fn signal_name(signal: i32) -> String {
    let name = match signal {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGILL => "SIGILL",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGABRT => "SIGABRT",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGKILL => "SIGKILL",
        libc::SIGUSR1 => "SIGUSR1",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGUSR2 => "SIGUSR2",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGALRM => "SIGALRM",
        libc::SIGTERM => "SIGTERM",
        libc::SIGCHLD => "SIGCHLD",
        libc::SIGCONT => "SIGCONT",
        libc::SIGSTOP => "SIGSTOP",
        libc::SIGTSTP => "SIGTSTP",
        libc::SIGXCPU => "SIGXCPU",
        libc::SIGXFSZ => "SIGXFSZ",
        libc::SIGSYS => "SIGSYS",
        other => return format!("SIG{other}"),
    };
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_names() {
        assert_eq!(signal_name(libc::SIGTERM), "SIGTERM");
        assert_eq!(signal_name(200), "SIG200");
    }
}
