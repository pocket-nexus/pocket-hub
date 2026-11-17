//! One service process, from its spawn to its end (§2, §4).
//!
//! The instance's task owns the child and, once the service has said
//! `service.hello`, its control connection. It answers `service.ready`, writes
//! the offers the supervisor gives it, keeps the idle timer, and runs the
//! stop sequence. Whatever ends the process — Hub's stop, the end of the
//! control connection (the lease), or the process's own exit — the task
//! reaps it, kills what is left of its process group, shuts its channel
//! connections down and records how it ended. It never starts the process
//! again: only the next open does.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use pocket_link::FrameDecoder;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::child::InheritNothing;
use crate::logs::{pump, LogStream};
use crate::registry::InstalledService;
use crate::supervisor::{backoff, exit_info, ServiceState, Entry, Inner, Instance};
use crate::util::{epoch_ms, inherited_env, path_with, private_dir, random_hex, signal_group};
use crate::wire;

/// What the supervisor's waiters see of an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Spawned; no `service.hello` yet.
    Starting,
    /// Connected: offers go out.
    Running,
    /// Ending, for the reason given.
    Stopping(String),
    /// Ended, for the reason given; the process is reaped.
    Down(String),
}

pub(crate) enum Cmd {
    /// A `channel.offer` frame to write on the control connection.
    Offer(Bytes),
    Stop(StopReason),
    /// The channels or pending opens changed: check the idle timer.
    Recount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopReason {
    Idle,
    Owner,
    Upgrade,
    Shutdown,
}

impl StopReason {
    fn text(self) -> &'static str {
        match self {
            Self::Idle => "stopped after its idle time",
            Self::Owner => "stopped by its owner",
            Self::Upgrade => "stopped for a new version",
            Self::Shutdown => "stopped as Pocket Hub quits",
        }
    }
}

/// How the process ended.
struct Outcome {
    status: Option<ExitStatus>,
    why: String,
    /// Hub asked for the end.
    requested: Option<StopReason>,
}

/// Spawns `bun run ./<entry>` for `service` (§2) in its own process group and
/// starts its task. Called with the supervisor's table locked; `entry` is
/// the service's.
pub(crate) fn spawn(inner: &Arc<Inner>, entry: &mut Entry, service: &InstalledService, bun: &Path) -> std::io::Result<Instance> {
    private_dir(&service.data)?;
    // Checked again at each start: the link made at install may be gone.
    if let Some(sdk) = &inner.config.sdk
        && let Err(error) = crate::registry::link_sdk(&service.dir, sdk)
    {
        tracing::warn!(app = %service.id, %error, "the service SDK could not be linked into the package");
    }
    let token = random_hex();
    let mut command = Command::new(bun);
    // `./` makes Bun run the file even when package.json has a script of the
    // same name; `--no-install` keeps Bun from fetching packages the
    // lockfile did not pin.
    command
        .arg("--no-install")
        .arg("run")
        .arg(format!("./{}", service.entry))
        .current_dir(&service.dir)
        .env_clear()
        .envs(inherited_env())
        .env("PATH", path_with(bun))
        .env("POCKET_SERVICE_SOCKET", &inner.config.socket)
        .env("POCKET_SERVICE_TOKEN", &token)
        .env("POCKET_SERVICE_ID", &service.id)
        .env("POCKET_SERVICE_DATA", &service.data)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true)
        // No descriptor of Hub's: a service is code Hub did not write.
        .inherit_nothing();
    let mut child = command.spawn()?;
    let pid = child.id().ok_or_else(|| std::io::Error::other("the process ended at once"))?;
    let pumps = vec![
        tokio::spawn(pump(child.stdout.take().expect("piped"), entry.logs.clone(), LogStream::Stdout)),
        tokio::spawn(pump(child.stderr.take().expect("piped"), entry.logs.clone(), LogStream::Stderr)),
    ];
    let serial = inner.next_serial();
    let (phase_tx, phase_rx) = watch::channel(Phase::Starting);
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (hello_tx, hello_rx) = oneshot::channel();

    entry.starts += 1;
    entry.status.restarts = entry.starts - 1;
    entry.status.state = ServiceState::Starting;
    entry.status.pid = Some(pid);
    entry.status.started_at = Some(epoch_ms());
    entry.status.running_version = Some(service.version.clone());
    entry.status.channels = 0;
    entry.status.last_error = None;
    entry.status.retry_at = None;
    entry.logs.push(LogStream::Hub, &format!("starting {} {} (pid {pid})", service.id, service.version), false);

    let idle = inner.config.timings.idle_override.unwrap_or(Duration::from_secs(service.idle_seconds));
    let task = Task {
        inner: inner.clone(),
        app: service.id.clone(),
        version: service.version.clone(),
        data: service.data.clone(),
        serial,
        pid,
        child,
        pumps,
        hello: Some(hello_rx),
        cmd: cmd_rx,
        phase: phase_tx,
        idle,
        started: Instant::now(),
        running: false,
    };
    tokio::spawn(task.run());
    Ok(Instance {
        serial,
        token,
        version: service.version.clone(),
        pid,
        phase: phase_rx,
        cmd: cmd_tx,
        hello: Some(hello_tx),
        channels: Default::default(),
        pending: 0,
        stopping: false,
    })
}

struct Task {
    inner: Arc<Inner>,
    app: String,
    version: String,
    data: PathBuf,
    serial: u64,
    pid: u32,
    child: Child,
    pumps: Vec<JoinHandle<()>>,
    hello: Option<oneshot::Receiver<Result<UnixStream, String>>>,
    cmd: mpsc::UnboundedReceiver<Cmd>,
    phase: watch::Sender<Phase>,
    idle: Duration,
    started: Instant,
    /// The service reached `service.ready`.
    running: bool,
}

impl Task {
    async fn run(mut self) {
        let outcome = match self.starting().await {
            Ok(control) => self.serving(control).await,
            Err(outcome) => outcome,
        };
        self.finish(outcome).await;
    }

    fn log(&self, text: &str) {
        self.inner.log(&self.app, text);
    }

    fn timings(&self) -> &crate::config::Timings {
        &self.inner.config.timings
    }

    /// From the spawn to `service.ready`.
    async fn starting(&mut self) -> Result<UnixStream, Outcome> {
        let deadline = tokio::time::sleep(self.timings().start);
        tokio::pin!(deadline);
        let mut hello = self.hello.take().expect("starting runs once");
        loop {
            tokio::select! {
                status = self.child.wait() => {
                    return Err(Outcome { status: status.ok(), why: "the process exited before its service.hello".into(), requested: None });
                }
                answer = &mut hello => {
                    let mut stream = match answer {
                        Ok(Ok(stream)) => stream,
                        Ok(Err(reason)) => return Err(self.end(format!("service.hello refused: {reason}"), None).await),
                        Err(_) => return Err(self.end("the supervisor dropped the process".into(), None).await),
                    };
                    let ready = json!({ "t": "service.ready", "app": self.app, "data": self.data.to_string_lossy() });
                    let frame = pocket_link::frame::encode_json(
                        pocket_link::FrameType::Handshake, 0, &ready, Default::default(), pocket_link::consts::MAX_HANDSHAKE,
                    );
                    let written = match frame {
                        Ok(frame) => tokio::time::timeout(Duration::from_secs(5), wire::write_all(&mut stream, &frame))
                            .await
                            .map_err(|_| "timed out".to_owned())
                            .and_then(|result| result.map_err(|error| error.to_string())),
                        Err(error) => Err(error.0),
                    };
                    if let Err(error) = written {
                        return Err(self.end(format!("writing service.ready: {error}"), None).await);
                    }
                    return Ok(stream);
                }
                _ = &mut deadline => {
                    let why = format!("no service.hello within {:.1} s of the start", self.timings().start.as_secs_f32());
                    return Err(self.end(why, None).await);
                }
                cmd = self.cmd.recv() => match cmd {
                    Some(Cmd::Stop(reason)) => return Err(self.end(reason.text().into(), Some(reason)).await),
                    Some(_) => {}
                    None => return Err(self.end(StopReason::Shutdown.text().into(), Some(StopReason::Shutdown)).await),
                },
            }
        }
    }

    /// From `service.ready` to the end.
    async fn serving(&mut self, control: UnixStream) -> Outcome {
        self.running = true;
        self.inner.update(&self.app, |entry| entry.status.state = ServiceState::Running);
        let _ = self.phase.send(Phase::Running);
        self.log("connected");

        let (mut reader, mut writer) = control.into_split();
        let mut decoder = FrameDecoder::local(wire::CONTROL_LIMIT);
        let mut buffer = vec![0u8; 4096];
        let idle = tokio::time::sleep(Duration::from_secs(86_400));
        tokio::pin!(idle);
        let mut armed = self.arm(idle.as_mut());
        loop {
            tokio::select! {
                status = self.child.wait() => {
                    return Outcome { status: status.ok(), why: "the process exited".into(), requested: None };
                }
                read = reader.read(&mut buffer) => match read {
                    Ok(0) | Err(_) => return self.lease_lost("the service closed its control connection").await,
                    Ok(n) => {
                        // §4 defines nothing the service sends after its
                        // hello; a valid frame is ignored, a broken one
                        // ends the lease.
                        decoder.push(&buffer[..n]);
                        loop {
                            match decoder.next() {
                                Ok(Some(_)) => {}
                                Ok(None) => break,
                                Err(close) => {
                                    let why = format!("a malformed frame on the control connection: {}", close.reason);
                                    return self.lease_lost(&why).await;
                                }
                            }
                        }
                    }
                },
                cmd = self.cmd.recv() => match cmd {
                    Some(Cmd::Offer(frame)) => {
                        let written = tokio::time::timeout(Duration::from_secs(5), wire::write_all(&mut writer, &frame)).await;
                        if !matches!(written, Ok(Ok(()))) {
                            return self.lease_lost("the control connection does not take channel.offer").await;
                        }
                    }
                    Some(Cmd::Recount) => armed = self.arm(idle.as_mut()),
                    Some(Cmd::Stop(reason)) => return self.stop(reason, &mut writer).await,
                    None => return self.stop(StopReason::Shutdown, &mut writer).await,
                },
                _ = &mut idle, if armed => {
                    if self.claim_idle_stop() {
                        return self.stop(StopReason::Idle, &mut writer).await;
                    }
                    armed = false;
                }
            }
        }
    }

    fn is_idle(&self) -> bool {
        let table = self.inner.lock();
        table.services.get(&self.app).and_then(|entry| entry.instance.as_ref()).is_some_and(|instance| instance.serial == self.serial && instance.idle())
    }

    /// Whether the instance is still idle, marking it stopping in the same
    /// step: an open that comes after waits for a new process rather than
    /// taking this one as it stops.
    fn claim_idle_stop(&self) -> bool {
        let mut table = self.inner.lock();
        let instance = table.services.get_mut(&self.app).and_then(|entry| entry.instance.as_mut());
        match instance {
            Some(instance) if instance.serial == self.serial && instance.idle() && !instance.stopping => {
                instance.stopping = true;
                true
            }
            _ => false,
        }
    }

    /// Sets the idle timer `idle` from now when no channel is open and no
    /// open waits; whether it is armed.
    fn arm(&self, idle: std::pin::Pin<&mut tokio::time::Sleep>) -> bool {
        if self.is_idle() {
            idle.reset(tokio::time::Instant::now() + self.idle);
            true
        } else {
            false
        }
    }

    /// Marks the instance as ending: new opens wait for its end and start a
    /// new process.
    fn ending(&self, why: &str) {
        let serial = self.serial;
        self.inner.update(&self.app, |entry| {
            if let Some(instance) = entry.instance.as_mut().filter(|instance| instance.serial == serial) {
                instance.stopping = true;
                entry.status.state = ServiceState::Stopping;
            }
        });
        let _ = self.phase.send(Phase::Stopping(why.to_owned()));
    }

    /// §4: `service.stop`, then `SIGTERM` after `stop_grace`, then `SIGKILL`
    /// after `term_grace`.
    async fn stop(&mut self, reason: StopReason, writer: &mut OwnedWriteHalf) -> Outcome {
        self.ending(reason.text());
        self.inner.signal_stop(&self.app, self.serial);
        self.log(&format!("sending service.stop: {}", reason.text()));
        if let Ok(frame) = wire::event(&json!({ "t": "service.stop" })) {
            let _ = tokio::time::timeout(Duration::from_secs(1), wire::write_all(writer, &frame)).await;
        }
        let status = self.ladder(Some(self.timings().stop_grace)).await;
        Outcome { status, why: reason.text().into(), requested: Some(reason) }
    }

    /// The lease is gone (§4): the channels close now and the process is
    /// ended with `SIGTERM`, then `SIGKILL`.
    async fn lease_lost(&mut self, why: &str) -> Outcome {
        self.ending(why);
        self.inner.close_channels(&self.app, self.serial);
        self.log(why);
        let status = self.ladder(None).await;
        Outcome { status, why: why.to_owned(), requested: None }
    }

    /// Ends a process that has no control connection.
    async fn end(&mut self, why: String, requested: Option<StopReason>) -> Outcome {
        self.ending(&why);
        self.log(&why);
        let status = self.ladder(None).await;
        Outcome { status, why, requested }
    }

    /// Waits `first` for the process to exit on its own, then sends the
    /// group `SIGTERM`, then `SIGKILL`.
    async fn ladder(&mut self, first: Option<Duration>) -> Option<ExitStatus> {
        if let Some(first) = first
            && let Ok(status) = tokio::time::timeout(first, self.child.wait()).await
        {
            return status.ok();
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            return Some(status);
        }
        self.log("sending SIGTERM");
        signal_group(self.pid, libc::SIGTERM);
        if let Ok(status) = tokio::time::timeout(self.timings().term_grace, self.child.wait()).await {
            return status.ok();
        }
        self.log("sending SIGKILL");
        signal_group(self.pid, libc::SIGKILL);
        self.child.wait().await.ok()
    }

    async fn finish(mut self, outcome: Outcome) {
        // What is left of the group (the service's own children) goes with it.
        signal_group(self.pid, libc::SIGKILL);
        // The log is complete before the state says the process ended. A
        // pipe a stray descendant still holds is not waited for long.
        for pump in self.pumps.drain(..) {
            let _ = tokio::time::timeout(Duration::from_secs(1), pump).await;
        }
        let ended = match outcome.status {
            Some(status) => match (status.code(), status.signal()) {
                (Some(code), _) => format!("exited with code {code}"),
                (None, Some(signal)) => format!("ended by signal {signal}"),
                _ => "ended".to_owned(),
            },
            None => "ended".to_owned(),
        };
        self.log(&format!("{ended}: {}", outcome.why));

        let timings = self.timings().clone();
        let quick = !self.running || self.started.elapsed() < timings.crash_window;
        let crashed = outcome.requested.is_none();
        let why = if self.running { format!("{}: {ended}", outcome.why) } else { format!("{} ({ended})", outcome.why) };
        let status = outcome.status;
        let serial = self.serial;
        let message = why.clone();
        self.inner.finish_instance(&self.app, serial, move |entry| {
            entry.status.pid = None;
            entry.status.channels = 0;
            entry.status.started_at = None;
            entry.status.running_version = None;
            entry.status.last_exit = Some(exit_info(status));
            if crashed {
                entry.status.crashes += 1;
                entry.status.state = ServiceState::Failed;
                entry.status.last_error = Some(message);
                if quick {
                    entry.failures += 1;
                    let hold = backoff(&timings, entry.failures);
                    entry.hold_until = Some(Instant::now() + hold);
                    entry.status.retry_at = Some(epoch_ms() + hold.as_millis() as i64);
                } else {
                    entry.failures = 0;
                    entry.hold_until = None;
                    entry.status.retry_at = None;
                }
            } else {
                entry.status.state = ServiceState::Stopped;
                entry.failures = 0;
                entry.hold_until = None;
                entry.status.retry_at = None;
            }
        });
        let _ = self.phase.send(Phase::Down(why));

        // A version replaced while this process ran is removed now.
        if let Some(current) = self.inner.registry.get(&self.app)
            && current.version != self.version
        {
            let running: Vec<String> = {
                let table = self.inner.lock();
                table.services.get(&self.app).and_then(|entry| entry.instance.as_ref()).map(|instance| instance.version.clone()).into_iter().collect()
            };
            let mut keep = running;
            keep.push(current.version);
            self.inner.registry.prune(&self.app, &keep).await;
        }
    }
}
