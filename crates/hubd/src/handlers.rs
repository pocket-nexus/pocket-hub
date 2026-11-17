//! The methods of §3.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use pocket_hub_api::types::*;
use pocket_hub_api::{ErrorKind, RpcError, methods};
use pocket_hub_applet_store::{StoreError, is_update};
use pocket_hub_core::{DeviceHandle, LaunchTarget, OpError, Operation, family};
use pocket_hub_daemons::{InstallError, StartError};
use pocket_hub_setup::{DeviceFamily, DiscoverOptions, FTPD_PORT, InstallOptions, InstallTarget, InstalledProgram, InstalledPrograms, ReleaseError, ReleaseFetcher, ShellSource};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::core::Core;
use crate::server::{self, Client};
use crate::{VERSION, map};

/// Progress notifications of one call.
struct Progress<'a> {
    client: &'a Client,
    id: &'a Value,
}

impl Progress<'_> {
    fn send(&self, members: impl Serialize) {
        let mut params = serde_json::to_value(members).expect("progress serializes");
        if let Value::Object(map) = &mut params {
            map.insert("id".into(), self.id.clone());
        }
        self.client.send(json!({"jsonrpc": "2.0", "method": methods::PROGRESS, "params": params}).to_string());
    }
}

fn params<T: DeserializeOwned>(method: &str, value: Value) -> Result<T, RpcError> {
    let value = if value.is_null() { json!({}) } else { value };
    serde_json::from_value(value).map_err(|error| RpcError::invalid_params(format!("The parameters of {method} are not usable: {error}.")))
}

fn ok(value: impl Serialize) -> Result<Value, RpcError> {
    Ok(serde_json::to_value(value).expect("results serialize"))
}

fn internal(message: impl Into<String>) -> RpcError {
    RpcError::op(ErrorKind::Internal, message)
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::op(ErrorKind::Invalid, message)
}

/// An absolute path from a parameter.
fn absolute(path: &str) -> Result<PathBuf, RpcError> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(invalid(format!("{} is not an absolute path.", path.display())));
    }
    Ok(path)
}

async fn read_file(path: &Path) -> Result<Vec<u8>, RpcError> {
    tokio::fs::read(path).await.map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => RpcError::op(ErrorKind::NotFound, format!("There is no file at {}.", path.display())),
        _ => internal(format!("{} could not be read: {error}.", path.display())),
    })
}

pub async fn dispatch(core: &Arc<Core>, client: &Client, id: &Value, method: &str, value: Value) -> Result<Value, RpcError> {
    let progress = Progress { client, id };
    match method {
        methods::STATUS => hub_status(core),
        methods::SET_NAME => set_name(core, params(method, value)?),
        methods::SUBSCRIBE => {
            let p: SubscribeParams = params(method, value)?;
            server::subscribe(client, p.topics)?;
            ok(Empty {})
        }
        methods::SHUTDOWN => ok(Empty {}),

        methods::DEVICES_LIST => {
            let _guard = core.publication();
            ok(DeviceList { devices: core.devices() })
        }
        methods::DEVICES_GET => {
            let p: DeviceParams = params(method, value)?;
            let handle = device(core, &p.device)?;
            let _guard = core.publication();
            ok(core.device(handle.id()).ok_or_else(|| map::op_error(OpError::Unknown))?)
        }
        methods::DEVICES_FORGET => {
            let p: DeviceParams = params(method, value)?;
            let handle = device(core, &p.device)?;
            let removed = core.endpoint.forget(handle.id()).await.map_err(|error| internal(format!("The device record could not be deleted: {error}.")))?;
            if !removed {
                return Err(map::op_error(OpError::Unknown));
            }
            core.forget_logs(handle.id());
            ok(Empty {})
        }
        methods::DEVICES_STATUS => {
            let p: DeviceParams = params(method, value)?;
            device(core, &p.device)?.status().await.map_err(map::op_error)
        }
        methods::DEVICES_PUSH => push(core, client, method, params(method, value)?, &progress).await,
        methods::DEVICES_LAUNCH => launch(core, client, method, params(method, value)?).await,
        methods::DEVICES_REMOVE => remove(core, client, method, params(method, value)?).await,
        methods::DEVICES_SCREENSHOT => screenshot(core, params(method, value)?).await,
        methods::DEVICES_LOGS => logs(core, client, params(method, value)?).await,
        methods::DEVICES_INSTALL_PROGRAM => install_program(core, client, method, params(method, value)?, &progress).await,

        methods::SETUP_DISCOVER => discover(core, params(method, value)?).await,
        methods::SETUP_INSTALL => setup_install(core, params(method, value)?, &progress).await,
        methods::SETUP_CARD => setup_card(core, params(method, value)?, &progress).await,

        methods::PAIRING_LIST => {
            let _guard = core.publication();
            ok(PairRequests { requests: core.pair_requests() })
        }
        methods::PAIRING_ANSWER => pair_answer(core, params(method, value)?).await,
        methods::PAIRING_DECLINE => {
            let p: DeviceParams = params(method, value)?;
            let id = pair_request(core, &p.device)?;
            if !core.endpoint.decline_pair(&id) {
                return Err(map::pair_error(pocket_hub_core::PairError::NoRequest));
            }
            ok(Empty {})
        }

        methods::DAEMONS_LIST => {
            let _guard = core.publication();
            ok(DaemonList { daemons: core.daemons() })
        }
        methods::DAEMONS_INSTALL => daemon_install(core, params(method, value)?, &progress).await,
        methods::DAEMONS_REMOVE => {
            let p: DaemonRemoveParams = params(method, value)?;
            let removed = core.supervisor.remove(&p.id, p.purge.unwrap_or(false)).await.map_err(|error| daemon_error(&p.id, format!("The daemon could not be removed: {error}.")))?;
            if !removed {
                return Err(no_daemon(&p.id));
            }
            ok(Empty {})
        }
        methods::DAEMONS_START => {
            let p: DaemonIdParams = params(method, value)?;
            core.supervisor.start(&p.id).await.map_err(|error| start_error(&p.id, error))?;
            daemon_now(core, &p.id)
        }
        methods::DAEMONS_STOP => {
            let p: DaemonIdParams = params(method, value)?;
            installed(core, &p.id)?;
            core.supervisor.stop(&p.id).await;
            daemon_now(core, &p.id)
        }
        methods::DAEMONS_RESTART => {
            let p: DaemonIdParams = params(method, value)?;
            installed(core, &p.id)?;
            core.supervisor.restart(&p.id).await.map_err(|error| start_error(&p.id, error))?;
            daemon_now(core, &p.id)
        }
        methods::DAEMONS_LOGS => {
            let p: DaemonLogsParams = params(method, value)?;
            let lines = core.supervisor.logs(&p.id, None);
            if lines.is_empty() && core.supervisor.status(&p.id).is_none() {
                return Err(no_daemon(&p.id));
            }
            let tail = p.tail.unwrap_or(200);
            let lines = lines[lines.len().saturating_sub(tail)..].iter().map(map::daemon_log_line).collect();
            ok(DaemonLogs { lines })
        }
        methods::DAEMONS_RUNTIME => runtime(core, params(method, value)?, &progress).await,

        methods::STORE_APPS => store_apps(core, params(method, value)?).await,
        methods::STORE_DETAILS => store_details(core, params(method, value)?).await,
        methods::STORE_SAMPLE => store_sample(core, params(method, value)?, &progress).await,
        methods::STORE_INSTALL => store_install(core, client, method, params(method, value)?, &progress).await,
        other => Err(RpcError::method_not_found(other)),
    }
}

// ---------------------------------------------------------------------------
// Hub
// ---------------------------------------------------------------------------

fn hub_status(core: &Core) -> Result<Value, RpcError> {
    let runtime = core.supervisor.runtime();
    ok(HubStatus {
        id: core.store.hub_id(),
        name: core.store.hub_name(),
        version: VERSION.to_owned(),
        home: core.home.display().to_string(),
        started_at: core.started_at,
        carriers: core.endpoint.ports().iter().map(|(kind, port)| Carrier { kind: map::carrier_name(*kind), port: *port }).collect(),
        bun: BunInfo { version: core.bun_version(), installed: runtime.available().is_some() },
    })
}

fn set_name(core: &Core, p: NameParams) -> Result<Value, RpcError> {
    let name = p.name.trim();
    if name.is_empty() || pocket_link::text::encoded_bytes(name) > pocket_link::consts::NAME_BYTES {
        return Err(invalid(format!("A name is 1 to {} bytes.", pocket_link::consts::NAME_BYTES)));
    }
    core.store.set_hub_name(name).map_err(|error| internal(format!("The name could not be stored: {error}.")))?;
    core.publish(&pocket_hub_api::Event::HubChanged { hub: core.hub_info() });
    ok(NameParams { name: core.store.hub_name() })
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

/// A device by id, or `"*"` for the only paired device.
fn device(core: &Core, selector: &str) -> Result<DeviceHandle, RpcError> {
    if selector == "*" {
        let records = core.store.devices();
        return match records.as_slice() {
            [] => Err(RpcError::op(ErrorKind::NotFound, "No device is paired with this Hub.")),
            [only] => core.endpoint.device(&only.id).ok_or_else(|| map::op_error(OpError::Unknown)),
            _ => Err(invalid(format!("{} devices are paired with this Hub; name one by its id instead of \"*\".", records.len()))),
        };
    }
    core.endpoint.device(selector).ok_or_else(|| RpcError::op(ErrorKind::NotFound, format!("No device {selector} is paired with this Hub.")))
}

/// Refuses an operation the device's family does not take (§2
/// `Device.supports`), before anything is fetched or sent.
fn require(handle: &DeviceHandle, operation: Operation) -> Result<(), RpcError> {
    let kind = handle.snapshot().ok_or_else(|| map::op_error(OpError::Unknown))?.kind;
    if family::supports(&kind, operation) { Ok(()) } else { Err(map::unsupported(operation)) }
}

fn holder(client: &Client, method: &str) -> String {
    map::holder_label(&Holder { kind: client.info.kind, name: client.info.name.clone(), method: method.to_owned() })
}

/// Runs a transfer (`push`, `.3dsx`), sending `{phase, done, total}`: the
/// device's phase from its progress events, and the bytes Hub has written,
/// at most every 50 ms and at each change of phase.
async fn transfer<F>(progress: &Progress<'_>, total: u64, operation: impl FnOnce(mpsc::UnboundedSender<Value>, mpsc::UnboundedSender<u64>) -> F) -> F::Output
where
    F: std::future::Future,
{
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let (sent_tx, mut sent) = mpsc::unbounded_channel();
    let work = operation(events_tx, sent_tx);
    tokio::pin!(work);
    let mut phase = "sending".to_owned();
    let mut done = 0u64;
    let mut last: Option<(String, u64, Instant)> = None;
    let mut emit = |phase: &str, done: u64, force: bool| {
        let due = match &last {
            None => true,
            Some((last_phase, last_done, at)) => force || last_phase != phase || (*last_done != done && (at.elapsed() >= Duration::from_millis(50) || done == total)),
        };
        if due && last.as_ref().is_none_or(|(p, d, _)| p != phase || *d != done) {
            progress.send(TransferProgress { phase: phase.to_owned(), done, total });
            last = Some((phase.to_owned(), done, Instant::now()));
        }
    };
    emit(&phase, 0, true);
    let output = loop {
        tokio::select! {
            output = &mut work => break output,
            Some(event) = events.recv() => {
                if let Some(next) = event.get("phase").and_then(Value::as_str) {
                    phase = next.to_owned();
                    emit(&phase, done, true);
                }
            }
            Some(count) = sent.recv() => {
                done = count;
                emit(&phase, done, false);
            }
        }
    };
    while let Ok(event) = events.try_recv() {
        if let Some(next) = event.get("phase").and_then(Value::as_str) {
            phase = next.to_owned();
        }
    }
    while let Ok(count) = sent.try_recv() {
        done = count;
    }
    emit(&phase, done, true);
    output
}

async fn push(core: &Core, client: &Client, method: &str, p: PathParams, progress: &Progress<'_>) -> Result<Value, RpcError> {
    let handle = device(core, &p.device)?;
    require(&handle, Operation::Push)?;
    let path = absolute(&p.path)?;
    let bytes = read_file(&path).await?;
    let hash = pocket_hub_core::ops::package_hash(&bytes).map_err(|why| invalid(format!("{} is not a Pocket package: {why}.", path.display())))?;
    let total = bytes.len() as u64;
    let label = holder(client, method);
    let reply = transfer(progress, total, |events, sent| handle.push_counted(&label, Bytes::from(bytes), Some(events), Some(sent))).await.map_err(map::op_error)?;
    let app = reply.get("app").cloned().unwrap_or(Value::Null);
    let text = |key: &str| app.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    ok(PushResult {
        app: PushedApp { id: text("id"), output: text("output"), title: text("title") },
        hash,
        generation: reply.get("generation").and_then(Value::as_u64).unwrap_or(0),
    })
}

async fn launch(core: &Core, client: &Client, method: &str, p: LaunchParams) -> Result<Value, RpcError> {
    let handle = device(core, &p.device)?;
    // An installed applet's id names its output; anything else is an output name.
    let output = handle.apps().and_then(|apps| apps.into_iter().find(|app| app.id == p.app).map(|app| app.output)).unwrap_or_else(|| p.app.clone());
    let reply = handle.launch(&holder(client, method), LaunchTarget::App(output.clone())).await.map_err(map::op_error)?;
    ok(LaunchResult { output: reply.get("output").and_then(Value::as_str).map(str::to_owned).unwrap_or(output) })
}

async fn remove(core: &Core, client: &Client, method: &str, p: RemoveParams) -> Result<Value, RpcError> {
    let handle = device(core, &p.device)?;
    // The link names an applet by its manifest id: an installed applet's
    // output name is turned into its id; anything else is sent as an id.
    let apps = handle.apps().unwrap_or_default();
    let id = if apps.iter().any(|app| app.id == p.app) {
        p.app.clone()
    } else {
        apps.into_iter().find(|app| app.output == p.app).map(|app| app.id).unwrap_or_else(|| p.app.clone())
    };
    handle.remove(&holder(client, method), &id).await.map_err(map::op_error)?;
    ok(Empty {})
}

async fn screenshot(core: &Core, p: ScreenshotParams) -> Result<Value, RpcError> {
    let handle = device(core, &p.device)?;
    require(&handle, Operation::Screenshot)?;
    let path = p.path.as_deref().map(absolute).transpose()?;
    let shot = handle.screenshot().await.map_err(map::op_error)?;
    let written = core.save_screenshot(handle.id(), shot.frame, shot.png, path).await.map_err(|error| internal(format!("The screenshot could not be written: {error}.")))?;
    ok(ScreenshotResult { path: written.display().to_string(), width: shot.width, height: shot.height, frame: shot.frame })
}

async fn logs(core: &Core, client: &Client, p: LogsParams) -> Result<Value, RpcError> {
    let handle = device(core, &p.device)?;
    require(&handle, Operation::Logs)?;
    if p.on {
        // Remembered first: a device that is not linked gets them at its
        // link, as one that relinks does.
        core.want_logs(handle.id(), client.conn);
        match handle.logs(true).await {
            Ok(()) | Err(OpError::NotLinked) => {}
            Err(error) => {
                core.unwant_logs(handle.id(), client.conn);
                return Err(map::op_error(error));
            }
        }
    } else if core.unwant_logs(handle.id(), client.conn) {
        // The last client that wanted them: off on the device, which a
        // device that is not linked already is.
        match handle.logs(false).await {
            Ok(()) | Err(OpError::NotLinked) => {}
            Err(error) => return Err(map::op_error(error)),
        }
    }
    ok(Empty {})
}

/// `devices.installProgram`: replaces the device's own program over the
/// link (`runtime.native.install`, §6.2).
///
/// - **The name on the device is the family's** (`DeviceFamily::
///   program_name`, the file setup installs), never the local file's or the
///   release asset's: the device replaces `sdmc:/3ds/<name>`, so any other
///   name would leave a second program beside the one it runs.
/// - `program` is resolved as setup resolves it (`shell_source`), through
///   the same release source and cache, without the cache's stand-in for a
///   source that does not answer (`ReleaseFetcher::resolve_fresh`): an update
///   that cannot learn the release fails. `fetch` verifies the bytes (size,
///   SHA-256 against `SHA256SUMS`, the `3DSX` magic) before one is sent.
/// - The device is held from the start, the download included, as
///   `store.install` holds it; a `launching` reply turns the hold into the
///   launch hold (until the device links again, at most 60 s).
/// - What was written is noted for the device as setup notes it, so a later
///   setup's "already installed" rule stays true (`record_program`).
async fn install_program(core: &Arc<Core>, client: &Client, method: &str, p: InstallProgramParams, progress: &Progress<'_>) -> Result<Value, RpcError> {
    let handle = device(core, &p.device)?;
    require(&handle, Operation::Program)?;
    let snapshot = handle.snapshot().ok_or_else(|| map::op_error(OpError::Unknown))?;
    let family = DeviceFamily::from_kind(&snapshot.kind).ok_or_else(|| internal(format!("Hub has no program for a device of kind \"{}\".", snapshot.kind)))?;
    let asks_latest = match &p.program {
        None => true,
        Some(ProgramChoice::Release { release }) => release == "latest",
        Some(ProgramChoice::Path { .. }) => false,
    };
    let source = shell_source(core, p.program)?;
    if let ShellSource::File { path } = &source
        && !path.is_file()
    {
        return Err(RpcError::op(ErrorKind::NotFound, format!("There is no file at {}.", path.display())));
    }
    let launch = p.launch.unwrap_or(true);
    let name = family.program_name();

    // The mutex first: a second caller while the device restarts into a
    // program (unlinked, under the launch hold) waits and gets `busy`, as at
    // any other holder; a device that is free and not linked is `not_linked`.
    let guard = handle.hold(&holder(client, method)).await.map_err(map::op_error)?;
    if !handle.snapshot().is_some_and(|snapshot| snapshot.linked) {
        return Err(map::op_error(OpError::NotLinked));
    }
    let fetcher = ReleaseFetcher::new(core.shell_cache(), &source).map_err(|error| release_error(&source, error))?;
    let release = fetcher.resolve_fresh(&source).await.map_err(|error| release_error(&source, error))?;
    // `downloading` when Hub fetches a release: the download's bytes, or once
    // with `done` equal to `total` when its cache holds the verified copy.
    let fetches = matches!(source, ShellSource::GitHub { .. });
    let mut last: Option<(u64, Instant)> = None;
    let bytes = fetcher
        .fetch(&release, &mut |done, _| {
            let due = last.is_none_or(|(shown, at)| shown != done && (done == release.size || at.elapsed() >= Duration::from_millis(50)));
            if fetches && due {
                progress.send(TransferProgress { phase: "downloading".into(), done, total: release.size });
                last = Some((done, Instant::now()));
            }
        })
        .await
        .map_err(|error| release_error(&source, error))?;
    if asks_latest && fetches {
        // The newest release, learned from the source just now.
        core.set_latest_program(family, release.version.clone());
    }

    let total = bytes.len() as u64;
    let outcome = transfer(progress, total, |events, sent| handle.native_install_held(guard, name, Bytes::from(bytes), launch, Some(events), Some(sent))).await;
    // The file is on the device after a reply, and after the error `launch`
    // with `installed: true` (written, and the start failed, §6.2).
    let written = match &outcome {
        Ok(_) => true,
        Err(OpError::Link { code, extra, .. }) => code == "launch" && extra.get("installed").and_then(Value::as_bool) == Some(true),
        Err(_) => false,
    };
    if written {
        record_program(core, handle.id(), family, &release);
    }
    let reply = outcome.map_err(map::op_error)?;
    tracing::info!(device = handle.id(), name, version = release.version.as_deref().unwrap_or(""), origin = %release.origin, launch, "program installed over the link");
    ok(InstallProgramResult { phase: reply.get("phase").and_then(Value::as_str).unwrap_or_default().to_owned(), name: name.to_owned(), version: release.version })
}

/// Notes the build Hub wrote as the device's program, in the note setup
/// keeps (`installed-programs.json`), so setup's "already installed" rule
/// judges the file on the card by what is there now. The link reports no
/// modification time, so the note has none; the rule then rests on the size
/// and the hash, as it does for a storage that reports no time.
fn record_program(core: &Core, device: &str, family: DeviceFamily, release: &pocket_hub_setup::ShellRelease) {
    let note = InstalledProgram { path: family.program_path().to_owned(), sha256: release.sha256.clone(), size: release.size, version: release.version.clone(), modified: None };
    if let Err(error) = InstalledPrograms::new(core.store.dir()).put(device, note) {
        // Costs one more upload at the next setup.
        tracing::warn!(device, %error, "the installed program could not be noted");
    }
}

/// A program that cannot be used, as the error of §1.
///
/// | source | failure | error |
/// | --- | --- | --- |
/// | a local file | not a 3DS program (no `3DSX` header, too large) | `invalid` |
/// | a local file | the `SHA256SUMS` or `build-info.json` beside it name other bytes | `release`, `rejected` |
/// | a release | the source did not answer, answered an HTTP error, or not with a usable release | `release`, `unreachable` |
/// | a release | the tag asked for is not there (HTTP 404) | `release`, `not_found` |
/// | a release | its program fails its checksum or is not a 3DS program | `release`, `rejected` |
/// | either | Hub's own file system (the cache) | `internal` |
///
/// A local file that is not there is `not_found` before this (the handler's
/// own check).
fn release_error(source: &ShellSource, error: ReleaseError) -> RpcError {
    let reason = match (source, &error) {
        (ShellSource::File { path }, ReleaseError::NotAProgram(_)) => return invalid(map::sentence(&format!("{} cannot be installed: {error}", path.display()))),
        (_, ReleaseError::Io(_)) => return internal(map::sentence(&format!("Hub could not read the program: {error}"))),
        (ShellSource::File { .. }, ReleaseError::Checksum | ReleaseError::Invalid(_)) => "rejected",
        (ShellSource::GitHub { .. }, ReleaseError::Checksum | ReleaseError::NotAProgram(_)) => "rejected",
        (ShellSource::GitHub { tag: Some(_), .. }, ReleaseError::Status(404)) => "not_found",
        (_, ReleaseError::Network(_) | ReleaseError::Status(_) | ReleaseError::Invalid(_)) => "unreachable",
    };
    let message = match (source, reason) {
        (ShellSource::File { path }, _) => format!("{} cannot be installed: {error}", path.display()),
        (ShellSource::GitHub { tag: Some(tag), .. }, "not_found") => format!("There is no release {tag} of the 3DS's software"),
        (_, "rejected") => format!("The release of the 3DS's software does not pass its checks, so nothing was sent to the device: {error}"),
        _ => format!("Hub could not get the 3DS's software, so nothing was sent to the device: {error}"),
    };
    RpcError::op(ErrorKind::Release, map::sentence(&message)).with(|data| data.reason = Some(reason.to_owned()))
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// The error `store` with its `reason`; a failure of Hub's own cache is `internal`.
fn store_error(error: StoreError) -> RpcError {
    if let StoreError::Local(message) = error {
        return internal(message);
    }
    let reason = error.reason().to_owned();
    RpcError::op(ErrorKind::Store, error.to_string()).with(|data| data.reason = Some(reason))
}

/// `store.apps`: the catalog Hub holds (fetched again past 10 minutes or
/// with `refresh`); with `device`, the applets that have a variant for its
/// target, each with the copy the device's last apps report names
/// (`installed`, matched by id) and whether the store's version orders above
/// that copy's (`update`, `pocket_hub_applet_store::is_update`).
async fn store_apps(core: &Arc<Core>, p: StoreAppsParams) -> Result<Value, RpcError> {
    // An unknown device fails before the store is asked.
    let handle = p.device.as_deref().map(|selector| device(core, selector)).transpose()?;
    let catalog = core.applets.apps(p.refresh.unwrap_or(false)).await.map_err(store_error)?;
    let (target, held) = match &handle {
        Some(handle) => (Some(handle.snapshot().ok_or_else(|| map::op_error(OpError::Unknown))?.target), handle.apps()),
        None => (None, None),
    };
    let apps = catalog
        .apps
        .iter()
        .filter(|entry| target.as_deref().is_none_or(|target| entry.runs_on(target)))
        .map(|entry| {
            let installed = handle.as_ref().map(|_| held.iter().flatten().find(|app| app.id == entry.id).map(|app| InstalledCopy { version: app.version.clone(), hash: app.hash.clone() }));
            let update = installed.as_ref().map(|copy| copy.as_ref().is_some_and(|copy| is_update(&entry.version, &copy.version)));
            StoreApp {
                id: entry.id.clone(),
                title: entry.title.clone(),
                summary: entry.summary.clone(),
                author: entry.author.clone(),
                version: entry.version.clone(),
                official: entry.official(&core.applets.config().official_key),
                targets: entry.targets.clone(),
                devices: core.applets.held_details(entry).map(|details| details.devices.iter().map(|device| DeviceName { target: device.target.clone(), name: device.name.clone() }).collect()),
                sample: entry.mock.is_some(),
                bytes: entry.bytes,
                icon: core.applets.icon_path(entry).map(|path| path.display().to_string()),
                cached: core.applets.ready(&entry.package).is_some(),
                services: entry.services.clone(),
                hosts: entry.hosts.clone(),
                installed,
                update,
            }
        })
        .collect();
    ok(StoreApps { apps, fetched_at: catalog.fetched_at })
}

/// `store.details`: the store's details of the package the catalog Hub holds
/// names for the applet (fetched again past 10 minutes, as for `store.apps`),
/// which the store client keeps while it holds that catalog. Hub reads no
/// manifest of its own: the store is the one place that does.
async fn store_details(core: &Arc<Core>, p: StoreDetailsParams) -> Result<Value, RpcError> {
    let catalog = core.applets.apps(false).await.map_err(store_error)?;
    let entry = catalog.app(&p.app).ok_or_else(|| RpcError::op(ErrorKind::NotFound, format!("The applet store lists no {}.", p.app)))?;
    let details = core.applets.details(entry).await.map_err(store_error)?;
    ok(StoreDetails {
        id: entry.id.clone(),
        version: entry.version.clone(),
        devices: details.devices.iter().map(|device| BuiltFor { target: device.target.clone(), name: device.name.clone(), host_abi: device.build.as_ref().map(|build| build.host_abi) }).collect(),
        needs: details.needs.clone(),
        uses: details.uses.clone(),
        surface: details.surface.as_ref().map(|surface| AppSurface { touch: surface.touch, size: surface.size.as_ref().map(|size| SurfaceSize { min: size.min, max: size.max }) }),
        framework: details.framework.clone(),
        api: details.api,
    })
}

/// `store.sample`: the verified package and the sample data the store's
/// player runs the applet from, downloaded when Hub's cache lacks them. No
/// device is involved, so no mutex is taken.
async fn store_sample(core: &Arc<Core>, p: StoreDetailsParams, progress: &Progress<'_>) -> Result<Value, RpcError> {
    let catalog = core.applets.apps(false).await.map_err(store_error)?;
    let entry = catalog
        .app(&p.app)
        .filter(|entry| entry.mock.is_some())
        .ok_or_else(|| RpcError::op(ErrorKind::NotFound, format!("The applet store has no sample data for {}.", p.app)))?;
    let mut last: Option<(u64, Instant)> = None;
    core.applets
        .package(entry, |done, total| {
            let due = last.is_none_or(|(shown, at)| shown != done && (done == total || at.elapsed() >= Duration::from_millis(50)));
            if due {
                progress.send(TransferProgress { phase: "downloading".into(), done, total });
                last = Some((done, Instant::now()));
            }
        })
        .await
        .map_err(store_error)?;
    let mock = core.applets.sample(entry).await.map_err(store_error)?.ok_or_else(|| internal(format!("The sample data of {} is not named by the catalog.", entry.id)))?;
    ok(StoreSample { package: core.applets.package_path(&entry.package).display().to_string(), mock: mock.display().to_string() })
}

/// `store.install`: the applet's package from the cache, or downloaded and
/// verified first (`downloading`), then pushed as `devices.push` does, all
/// under the device mutex.
async fn store_install(core: &Arc<Core>, client: &Client, method: &str, p: StoreInstallParams, progress: &Progress<'_>) -> Result<Value, RpcError> {
    let handle = device(core, &p.device)?;
    require(&handle, Operation::Push)?;
    let snapshot = handle.snapshot().ok_or_else(|| map::op_error(OpError::Unknown))?;
    if !snapshot.linked {
        return Err(map::op_error(OpError::NotLinked));
    }
    let catalog = core.applets.apps(false).await.map_err(store_error)?;
    let entry = catalog
        .app(&p.app)
        .filter(|entry| entry.runs_on(&snapshot.target))
        .cloned()
        .ok_or_else(|| RpcError::op(ErrorKind::NotFound, format!("The applet store offers no {} for this device ({}).", p.app, snapshot.target)))?;
    let label = holder(client, method);
    let guard = handle.hold(&label).await.map_err(map::op_error)?;
    let mut last: Option<(u64, Instant)> = None;
    let facts = core
        .applets
        .package(&entry, |done, total| {
            let due = last.is_none_or(|(shown, at)| shown != done && (done == total || at.elapsed() >= Duration::from_millis(50)));
            if due {
                progress.send(TransferProgress { phase: "downloading".into(), done, total });
                last = Some((done, Instant::now()));
            }
        })
        .await
        .map_err(store_error)?;
    let bytes = read_file(&core.applets.package_path(&entry.package)).await?;
    let hash = pocket_hub_core::ops::package_hash(&bytes).map_err(|why| internal(format!("The cached package of {} cannot be read: {why}.", entry.id)))?;
    if hash != facts.hash {
        core.applets.invalidate(&entry.package);
        return Err(internal(format!("The cached package of {} changed since it was verified; install it again.", entry.id)));
    }
    let total = bytes.len() as u64;
    let reply = transfer(progress, total, |events, sent| handle.push_held(&guard, Bytes::from(bytes), Some(events), Some(sent))).await.map_err(map::op_error)?;
    drop(guard);
    let app = reply.get("app").cloned().unwrap_or(Value::Null);
    let text = |key: &str| app.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    ok(PushResult { app: PushedApp { id: text("id"), output: text("output"), title: text("title") }, hash, generation: reply.get("generation").and_then(Value::as_u64).unwrap_or(0) })
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

/// A setup writes onto a device's storage (§3 Setup), which Hub reaches for
/// a 3DS alone. A Tab5 is named apart: its owner is told the way it does pair.
fn check_kind(kind: &str) -> Result<(), RpcError> {
    match kind {
        family::KIND_3DS => Ok(()),
        family::KIND_TAB5 => Err(invalid("An M5Stack Tab5 is not set up from the computer: it pairs by the code it shows (pairing.answer).")),
        _ => Err(invalid(format!("Hub cannot set up a device of kind \"{kind}\"; the one kind it sets up is \"3ds\"."))),
    }
}

async fn discover(core: &Core, p: KindParams) -> Result<Value, RpcError> {
    check_kind(&p.kind)?;
    let options = DiscoverOptions {
        port: core.options.discover_port.unwrap_or(FTPD_PORT),
        targets: core.options.discover_targets.clone(),
        // §3: it answers within about 2 s.
        deadline: Duration::from_millis(1800),
        ..DiscoverOptions::default()
    };
    let mut found = pocket_hub_setup::discover(&options).await;
    // ftpd's own banners first, as a client offers them.
    found.sort_by_key(|candidate| !candidate.ftpd);
    ok(DiscoverResult { candidates: found.into_iter().map(|c| Candidate { address: c.address.to_string(), port: c.port, banner: c.banner, ftpd: c.ftpd }).collect() })
}

/// The program a setup or an update installs. `latest` and a tag come from the
/// releases this core is configured with (Pocket Shell's GitHub releases, a
/// mirror); a core started with a local file as its release has no tags.
fn shell_source(core: &Core, program: Option<ProgramChoice>) -> Result<ShellSource, RpcError> {
    let configured = core.shell_source();
    match program {
        None => Ok(configured),
        Some(ProgramChoice::Release { release }) if release == "latest" => Ok(configured),
        Some(ProgramChoice::Release { release }) => {
            if release.trim().is_empty() || release.contains(['/', '\\']) {
                return Err(invalid(format!("\"{release}\" is not a release tag.")));
            }
            match configured {
                ShellSource::GitHub { api, repo, .. } => Ok(ShellSource::GitHub { api, repo, tag: Some(release) }),
                ShellSource::File { path } => Err(invalid(format!("This Hub takes the 3DS's software from {} and has no release {release}.", path.display()))),
            }
        }
        Some(ProgramChoice::Path { path }) => Ok(ShellSource::File { path: absolute(&path)? }),
    }
}

async fn setup_install(core: &Core, p: SetupInstallParams, progress: &Progress<'_>) -> Result<Value, RpcError> {
    check_kind(&p.kind)?;
    let port = p.port.unwrap_or(FTPD_PORT);
    let address = match p.host.parse::<IpAddr>() {
        Ok(ip) => SocketAddr::new(ip, port),
        Err(_) => tokio::net::lookup_host((p.host.as_str(), port))
            .await
            .ok()
            .and_then(|mut found| found.next())
            .ok_or_else(|| invalid(format!("{} is not an address this computer can find.", p.host)))?,
    };
    let source = shell_source(core, p.program)?;
    run_setup(core, InstallTarget::Ftp { address }, source, progress).await
}

async fn setup_card(core: &Core, p: SetupCardParams, progress: &Progress<'_>) -> Result<Value, RpcError> {
    check_kind(&p.kind)?;
    let root = absolute(&p.path)?;
    let source = shell_source(core, p.program)?;
    run_setup(core, InstallTarget::Card { root }, source, progress).await
}

async fn run_setup(core: &Core, target: InstallTarget, source: ShellSource, progress: &Progress<'_>) -> Result<Value, RpcError> {
    let mut options = InstallOptions::new(core.shell_cache());
    // The port this core listens on is the one the pairing names.
    options.hub_port = core.link_port;
    options.source = source;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let store = core.store.clone();
    let work = {
        let target = target.clone();
        async move { pocket_hub_setup::install(&target, &store, &options, &mut |step| drop(tx.send(step))).await }
    };
    tokio::pin!(work);
    let mut last: Option<(SetupStage, Instant)> = None;
    let mut forward = |step: &pocket_hub_setup::Progress| {
        let shown = map::setup_progress(step);
        let due = last.as_ref().is_none_or(|(stage, at)| *stage != shown.stage || at.elapsed() >= Duration::from_millis(50) || shown.done == shown.total);
        if due {
            last = Some((shown.stage, Instant::now()));
            progress.send(shown);
        }
    };
    let report = loop {
        tokio::select! {
            report = &mut work => break report,
            Some(step) = rx.recv() => forward(&step),
        }
    };
    while let Ok(step) = rx.try_recv() {
        forward(&step);
    }
    let result = map::install_report(&report, &target);
    // A new record, or a record whose `setupUnconfirmed` changed.
    if let Some(id) = &report.device_id {
        core.publish_device(id);
    }
    tracing::info!(device = report.device_id.as_deref().unwrap_or(""), ok = report.ok, via = report.via, detail = report.detail.as_deref().unwrap_or(""), "setup");
    ok(result?)
}

// ---------------------------------------------------------------------------
// Pairing by a code the device shows
// ---------------------------------------------------------------------------

/// A request by its device id, or `"*"` for the only one that waits.
fn pair_request(core: &Core, selector: &str) -> Result<String, RpcError> {
    if selector != "*" {
        return Ok(selector.to_owned());
    }
    let waiting = core.endpoint.pair_requests();
    match waiting.as_slice() {
        [] => Err(RpcError::op(ErrorKind::NotFound, "No device asks to be paired.")),
        [only] => Ok(only.device.clone()),
        _ => Err(invalid(format!("{} devices ask to be paired; name one by its id instead of \"*\".", waiting.len()))),
    }
}

/// `pairing.answer`: Hub's owner entered the code the device shows (link
/// specification §3.11). The endpoint mints the key, sends it under the
/// code, and stores the record when the device's proof verifies; the answer
/// is the Device that record makes. No key is in it, nor in the error of a
/// pairing that did not complete.
async fn pair_answer(core: &Arc<Core>, p: PairAnswerParams) -> Result<Value, RpcError> {
    let id = pair_request(core, &p.device)?;
    core.endpoint.pair(&id, &p.code).await.map_err(map::pair_error)?;
    let _guard = core.publication();
    ok(core.device(&id).ok_or_else(|| internal("The device was paired and its record is gone."))?)
}

// ---------------------------------------------------------------------------
// Daemons
// ---------------------------------------------------------------------------

fn no_daemon(id: &str) -> RpcError {
    RpcError::op(ErrorKind::NotFound, format!("No daemon {id} is installed."))
}

fn installed(core: &Core, id: &str) -> Result<(), RpcError> {
    core.supervisor.status(id).map(|_| ()).ok_or_else(|| no_daemon(id))
}

fn daemon_now(core: &Core, id: &str) -> Result<Value, RpcError> {
    ok(core.daemon(id).ok_or_else(|| no_daemon(id))?)
}

/// The error `daemon` for `id`.
fn daemon_error(id: &str, message: impl Into<String>) -> RpcError {
    let id = id.to_owned();
    RpcError::op(ErrorKind::Daemon, message).with(|data| data.id = Some(id))
}

fn start_error(id: &str, error: StartError) -> RpcError {
    match error {
        StartError::NotInstalled(id) => no_daemon(&id),
        StartError::Failed(why) => daemon_error(id, map::sentence(&format!("the daemon did not start: {why}"))),
    }
}

/// Downloads Bun when it is missing, with `{done, total}` going to `each`.
async fn ensure_bun(core: &Core, mut each: impl FnMut(pocket_hub_daemons::Progress)) -> Result<PathBuf, RpcError> {
    let runtime = core.supervisor.runtime();
    if let Some(path) = runtime.available() {
        return Ok(path);
    }
    runtime.ensure_with(&mut each).await.map_err(|error| internal(map::sentence(&format!("Bun could not be installed: {error}"))))
}

async fn daemon_install(core: &Core, p: DaemonPathParams, progress: &Progress<'_>) -> Result<Value, RpcError> {
    let path = absolute(&p.path)?;
    if !path.exists() {
        return Err(RpcError::op(ErrorKind::NotFound, format!("There is nothing at {}.", path.display())));
    }
    // A daemon runs on Bun: the first install downloads it.
    let mut shown = None;
    let source = path.display().to_string();
    ensure_bun(core, |step| {
        let message = match step.total {
            Some(total) if total > 0 => format!("Downloading Bun: {}%.", step.done * 100 / total),
            _ => format!("Downloading Bun: {} KiB.", step.done / 1024),
        };
        if shown.as_ref() != Some(&message) {
            progress.send(DaemonInstallProgress { stage: "runtime".into(), message: message.clone() });
            shown = Some(message);
        }
    })
    .await
    .map_err(|error| RpcError::op(ErrorKind::Daemon, error.message))?;
    progress.send(DaemonInstallProgress { stage: "installing".into(), message: format!("Installing the daemon package {}.", path.display()) });
    // A package that is not one is an unusable argument; a package that
    // could not be installed (its dependencies, the disk) is the daemon's.
    let report = core.supervisor.install(&path).await.map_err(|error| match error {
        InstallError::Source(_) | InstallError::Archive(_) | InstallError::Manifest(_) => invalid(map::sentence(&error.to_string())),
        other => RpcError::op(ErrorKind::Daemon, map::sentence(&format!("{source} could not be installed: {other}"))),
    })?;
    daemon_now(core, &report.daemon.id)
}

async fn runtime(core: &Core, p: RuntimeParams, progress: &Progress<'_>) -> Result<Value, RpcError> {
    let runtime = core.supervisor.runtime();
    if p.install.unwrap_or(false) {
        let mut last = None;
        ensure_bun(core, |step| {
            let tick = (step.done / (256 * 1024), step.done == step.total.unwrap_or(u64::MAX));
            if last != Some(tick) {
                last = Some(tick);
                progress.send(RuntimeProgress { done: step.done, total: step.total });
            }
        })
        .await?;
    }
    let path = runtime.available();
    ok(RuntimeInfo {
        version: core.bun_version(),
        installed: path.is_some(),
        path: path.unwrap_or_else(|| runtime.install_path()).display().to_string(),
    })
}
