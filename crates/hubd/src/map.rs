//! The contract's shapes made from the internal types of `hub-core`,
//! `hub-daemons` and `hub-setup`. Those types are not the contract: each
//! mapping here is written out, so a change on either side is a compile
//! error or a visible line, never a silent change of the API.

use pocket_hub_api::types as api;
use pocket_hub_api::{ErrorKind, RpcError};
use pocket_hub_core::{DeviceSnapshot, OpError, Operation, PairError, family};
use pocket_hub_daemons::{DaemonState, DaemonStatus, InstalledDaemon, LogLine, LogStream};
use pocket_hub_setup::{InstallReport, InstallTarget, PairingOutcome, ProgramAction, Progress, Stage};

// ---------------------------------------------------------------------------
// The device mutex's holder
// ---------------------------------------------------------------------------

/// What hub-core's mutex shows after a `launching` reply (§8.3 launch hold).
const LAUNCH_HOLD_SUFFIX: &str = " (launching)";

/// The label hub-core keeps for a holder: the contract's holder object as
/// compact JSON, so it comes back whole in the device's `busy` and in the
/// error `busy`.
pub fn holder_label(holder: &api::Holder) -> String {
    serde_json::to_string(holder).expect("a holder serializes")
}

/// The holder object of a hub-core label. A launch hold keeps the holder of
/// the launch. A label Hub did not make (none exist today) is shown as a
/// remote client's name.
pub fn holder_of(label: &str) -> api::Holder {
    let label = label.strip_suffix(LAUNCH_HOLD_SUFFIX).unwrap_or(label);
    serde_json::from_str(label).unwrap_or_else(|_| api::Holder { kind: api::ClientKind::Remote, name: label.to_owned(), method: String::new() })
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

/// `DeviceSnapshot` → `Device` (§2).
///
/// | contract | hub-core |
/// | --- | --- |
/// | `link.carrier` | `link.carrier` (`tcp`, `usb`, `uart`, `ble`) |
/// | `link.since` | `link.linked_at` |
/// | `link.features` | `link.features_in_use`: the groups both sides list |
/// | `busy` | `mutex_holder` (a holder label) and `mutex_since`; `launching` while the label carries the launch hold's mark |
/// | `channels` | `link.channels`, `[]` when not linked |
/// | `program` | `program(snapshot, latest)` |
/// | `supports` | `family::supported(kind)`: the family's operations, by name |
///
/// `linked`, `latest_screenshot` and `last_close` have no place in the
/// contract (the screenshot and the close are events).
pub fn device(snapshot: &DeviceSnapshot, latest: Option<String>) -> api::Device {
    let link = snapshot.link.as_ref();
    api::Device {
        id: snapshot.id.clone(),
        kind: snapshot.kind.clone(),
        supports: family::supported(&snapshot.kind).into_iter().map(|operation| operation.name().to_owned()).collect(),
        name: snapshot.name.clone(),
        model: snapshot.model.clone(),
        target: snapshot.target.clone(),
        host_abi: snapshot.host_abi,
        last_address: snapshot.last_address.clone(),
        last_seen: snapshot.last_seen,
        setup_unconfirmed: snapshot.setup_unconfirmed,
        duplicate_id: snapshot.duplicate_id,
        link: link.map(|link| api::LinkState {
            carrier: carrier_name(link.carrier),
            peer: link.peer.clone(),
            since: link.linked_at,
            payload_limit: link.payload_limit,
            features: link.features_in_use.clone(),
        }),
        status: snapshot.status.clone(),
        apps: snapshot.apps.as_ref().map(|apps| {
            apps.iter().map(|app| api::AppRow { id: app.id.clone(), output: app.output.clone(), version: app.version.clone(), hash: app.hash.clone(), home: app.home }).collect()
        }),
        busy: snapshot.mutex_holder.as_deref().map(|label| api::Busy {
            holder: holder_of(label),
            since: snapshot.mutex_since.unwrap_or(0),
            launching: label.ends_with(LAUNCH_HOLD_SUFFIX),
        }),
        program: program(snapshot, latest),
        channels: link
            .map(|link| link.channels.iter().map(|channel| api::ChannelRow { app: channel.app.clone(), generation: channel.generation, since: channel.since }).collect())
            .unwrap_or_default(),
    }
}

/// `Device.program`: `version` is the `version` of the row the device
/// marks `home` in its last apps report (its own program, §6.2), null
/// before a report, without such a row, or when the row's version is empty;
/// `latest` is the newest release Hub knows, and none for a family whose
/// program Hub does not install (`supports` without `program`); `update` is
/// true when `latest` orders above `version`
/// (`pocket_hub_setup::version_order`: dated versions below SemVer ones; any
/// other form is never an update).
pub fn program(snapshot: &DeviceSnapshot, latest: Option<String>) -> api::ProgramState {
    let latest = latest.filter(|_| family::supports(&snapshot.kind, Operation::Program));
    let version = snapshot.apps.iter().flatten().find(|app| app.home == Some(true)).map(|app| app.version.clone()).filter(|version| !version.is_empty());
    let update = matches!((&latest, &version), (Some(latest), Some(version)) if pocket_hub_setup::is_newer(latest, version));
    api::ProgramState { version, latest, update }
}

pub fn carrier_name(kind: pocket_hub_core::CarrierKind) -> String {
    match kind {
        pocket_hub_core::CarrierKind::Tcp => "tcp",
        pocket_hub_core::CarrierKind::Usb => "usb",
        pocket_hub_core::CarrierKind::Uart => "uart",
        pocket_hub_core::CarrierKind::Ble => "ble",
    }
    .to_owned()
}

/// `device.unlinked.code`: the close code's name, or `lost` when the carrier
/// ended without a `CLOSE` (hub-core's `CloseInfo { by: "carrier", code: None }`).
pub fn unlinked_code(code: Option<&str>) -> String {
    code.unwrap_or("lost").to_owned()
}

/// A sentence from one of hub-core's lower-case phrases.
pub fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut out = String::with_capacity(text.len() + 1);
    let mut chars = text.chars();
    if let Some(first) = chars.next() {
        out.extend(first.to_uppercase());
        out.push_str(chars.as_str());
    }
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// `OpError` → the error of §1.
///
/// | hub-core | contract |
/// | --- | --- |
/// | `Unknown` | `not_found` |
/// | `NotLinked` | `not_linked` |
/// | `Busy { holder }` | `busy` with the holder object |
/// | `Unsupported { feature }` | `link`, `code: "unsupported"` (the link's code for an unlisted group, §6.1) |
/// | `Invalid` | `invalid` |
/// | `Link { code: "not_found" }` | `not_found` (no such app on the device) |
/// | `Link { code: "cancelled" }` | `cancelled` |
/// | `Link { code, extra.phase }` | `link` with `code` and `phase` |
pub fn op_error(error: OpError) -> RpcError {
    match error {
        OpError::Unknown => RpcError::op(ErrorKind::NotFound, "This device is not paired with this Hub."),
        OpError::NotLinked => RpcError::op(ErrorKind::NotLinked, "The device is not connected to this Hub now."),
        OpError::Busy { holder } => {
            let holder = holder_of(&holder);
            let message = if holder.method.is_empty() {
                format!("The device is busy: {} holds it.", holder.name)
            } else {
                format!("The device is busy: {} ({}) is running {}.", holder.name, holder.kind.as_str(), holder.method)
            };
            RpcError::op(ErrorKind::Busy, message).with(|data| data.holder = Some(holder))
        }
        OpError::Unsupported { feature } => {
            RpcError::op(ErrorKind::Link, format!("The device does not support {feature}.")).with(|data| data.code = Some("unsupported".into()))
        }
        OpError::Invalid { message } => RpcError::op(ErrorKind::Invalid, sentence(&message)),
        OpError::Link { code, message, extra } => match code.as_str() {
            "not_found" => RpcError::op(ErrorKind::NotFound, sentence(&format!("the device has no such app: {message}"))),
            "cancelled" => RpcError::op(ErrorKind::Cancelled, "The call was cancelled."),
            _ => {
                let text = if message.is_empty() { format!("the device answered {code}") } else { format!("the device answered {code}: {message}") };
                let phase = extra.get("phase").and_then(|phase| phase.as_str()).map(str::to_owned);
                // `runtime.remove` refuses with a `reason` (`running`, `home`).
                let reason = extra.get("reason").and_then(|reason| reason.as_str()).map(str::to_owned);
                RpcError::op(ErrorKind::Link, sentence(&text)).with(|data| {
                    data.code = Some(code);
                    data.phase = phase;
                    data.reason = reason;
                })
            }
        },
    }
}

/// An operation the device's family does not take (§2 `Device.supports`):
/// `link` with `code` `unsupported`, the error a device gives for a request
/// it does not serve, and nothing is sent. The sentence says what the owner
/// does instead where there is such a thing.
pub fn unsupported(operation: Operation) -> RpcError {
    let message = match operation {
        Operation::Push => "This device takes no applet sent from the computer: install it from the store page on the device itself.",
        Operation::Screenshot => "This device takes no screenshot.",
        Operation::Logs => "This device sends no log.",
        Operation::Program => "Hub does not install this device's software.",
    };
    RpcError::op(ErrorKind::Link, message).with(|data| data.code = Some("unsupported".into()))
}

// ---------------------------------------------------------------------------
// Pairing by a code
// ---------------------------------------------------------------------------

/// hub-core's request → the contract's (§2 PairRequest): `kind` is the
/// family of the target, and `address` the device's address; the carrier's
/// own name of the peer (`ip:port`) has no place in the contract.
pub fn pair_request(request: &pocket_hub_core::PairRequest) -> api::PairRequest {
    api::PairRequest {
        device: request.device.clone(),
        kind: family::kind_of_target(&request.target).to_owned(),
        name: request.name.clone(),
        model: request.model.clone(),
        target: request.target.clone(),
        address: request.address.clone(),
        since: request.since,
        expires: request.expires,
    }
}

/// `PairError` → the error of §1.
///
/// | hub-core | contract |
/// | --- | --- |
/// | `NotACode` | `invalid` |
/// | `NoRequest` | `not_found` |
/// | `WrongCode` | `pairing`, `reason: wrong_code` |
/// | `BadProof` | `pairing`, `reason: bad_proof` |
/// | `Gone` | `pairing`, `reason: gone` |
/// | `Store` | `internal` |
pub fn pair_error(error: PairError) -> RpcError {
    let pairing = |reason: &str, message: &str| RpcError::op(ErrorKind::Pairing, message).with(|data| data.reason = Some(reason.to_owned()));
    match error {
        PairError::NotACode => RpcError::op(ErrorKind::Invalid, "A pairing code is the 6 digits the device shows."),
        PairError::NoRequest => RpcError::op(ErrorKind::NotFound, "No device with this id asks to be paired."),
        PairError::WrongCode => pairing("wrong_code", "The code is not the one the device shows. Nothing was stored."),
        PairError::BadProof => pairing("bad_proof", "The device did not prove that it holds the key. Nothing was stored."),
        PairError::Gone => pairing("gone", "The device left before the pairing finished. Nothing was stored."),
        PairError::Store(why) => RpcError::op(ErrorKind::Internal, sentence(&format!("the device took the pairing and Hub could not store its record: {why}"))),
    }
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

/// hub-setup's stages → the contract's: the look at the card is `reading`,
/// and looking the release up is part of `downloading`.
pub fn setup_stage(stage: Stage) -> api::SetupStage {
    match stage {
        Stage::Connecting => api::SetupStage::Connecting,
        Stage::Checking => api::SetupStage::Reading,
        Stage::Resolving | Stage::Downloading => api::SetupStage::Downloading,
        Stage::Uploading => api::SetupStage::Uploading,
        Stage::Pairing => api::SetupStage::Pairing,
        Stage::Done => api::SetupStage::Done,
    }
}

/// One progress notification of a setup. `done`/`total` are bytes while
/// downloading and uploading and file operations while pairing; they are
/// left out of a step that has no measure.
pub fn setup_progress(progress: &Progress) -> api::SetupProgress {
    let message = match progress.stage {
        Stage::Connecting => "Connecting to the device's storage.",
        Stage::Checking => "Reading what the device holds.",
        Stage::Resolving => "Looking up the release of the 3DS's software.",
        Stage::Downloading => "Downloading the 3DS's software.",
        Stage::Uploading => "Copying the 3DS's software to the device.",
        Stage::Pairing => "Writing this computer's pairing.",
        Stage::Done => "Done.",
    };
    let measured = progress.total.is_some();
    api::SetupProgress {
        stage: setup_stage(progress.stage),
        done: measured.then_some(progress.done),
        total: progress.total,
        message: message.to_owned(),
    }
}

/// hub-setup's report → the contract's report, or the error `setup` when
/// the install failed (§3 Setup: a failed setup is never a report).
///
/// | hub-setup | contract |
/// | --- | --- |
/// | `device_id` | `device` |
/// | `pairing.outcome` `Key`/`Kept`/`Address` | `pairing` `key`/`kept`/`address` |
/// | `program.action` `Installed` | `program.action` `installed` |
/// | `program.action` `AlreadyInstalled`, `KeptUnchecked` | `program.action` `kept` |
/// | `program.release.version`, `.build_info.runtime` | `program.version`, `program.runtime` |
/// | `next_step` | `next` |
/// | `ok: false`, `failed_stage` | error `setup`, `step` (mapped as `setup_stage`) |
/// | `pairing.outcome` `EightComputers` | `reason: companions_full` |
/// | `pairing.unconfirmed` | `device` and `confirmed: false` |
/// | `detail` | `detail` |
/// | a card target that fails to open | `reason: card` |
pub fn install_report(report: &InstallReport, target: &InstallTarget) -> Result<api::InstallReport, RpcError> {
    if !report.ok {
        return Err(setup_error(report, target));
    }
    let pairing = match report.pairing.as_ref().map(|pairing| pairing.outcome) {
        Some(PairingOutcome::Key) => api::PairingResult::Key,
        Some(PairingOutcome::Kept) => api::PairingResult::Kept,
        Some(PairingOutcome::Address) => api::PairingResult::Address,
        other => return Err(RpcError::op(ErrorKind::Internal, format!("The setup ended without a pairing outcome ({other:?})."))),
    };
    let action = match report.program.action {
        ProgramAction::Installed => api::ProgramAction::Installed,
        ProgramAction::AlreadyInstalled | ProgramAction::KeptUnchecked => api::ProgramAction::Kept,
        ProgramAction::NotInstalled => return Err(RpcError::op(ErrorKind::Internal, "The setup ended without a program on the device.")),
    };
    let device = report.device_id.clone().ok_or_else(|| RpcError::op(ErrorKind::Internal, "The setup ended without a device id."))?;
    let release = report.program.release.as_ref();
    Ok(api::InstallReport {
        device,
        pairing,
        program: api::ProgramInfo {
            action,
            version: release.and_then(|release| release.version.clone()),
            runtime: release.and_then(|release| release.build_info.as_ref()).and_then(|info| info.runtime.clone()),
        },
        confirmed: true,
        next: report.next_step.clone(),
    })
}

fn setup_error(report: &InstallReport, target: &InstallTarget) -> RpcError {
    let stage = report.failed_stage.unwrap_or(Stage::Connecting);
    let outcome = report.pairing.as_ref().map(|pairing| pairing.outcome);
    let reason = match stage {
        // A console that does not answer, or a folder that is not its card.
        Stage::Connecting => Some(match target {
            InstallTarget::Ftp { .. } => "unreachable",
            InstallTarget::Card { .. } => "card",
        }),
        Stage::Checking if outcome == Some(PairingOutcome::EightComputers) => Some("companions_full"),
        Stage::Checking => Some("storage"),
        Stage::Resolving | Stage::Downloading => Some("release_unavailable"),
        Stage::Uploading | Stage::Pairing => Some("storage"),
        Stage::Done => None,
    };
    let unconfirmed = report.pairing.as_ref().filter(|pairing| pairing.unconfirmed).and_then(|pairing| pairing.device_id.clone().or_else(|| report.device_id.clone()));
    let message = if report.next_step.is_empty() { "The setup failed.".to_owned() } else { report.next_step.clone() };
    RpcError::op(ErrorKind::Setup, message).with(|data| {
        data.step = Some(setup_stage(stage).as_str().to_owned());
        data.reason = reason.map(str::to_owned);
        // hub-setup's diagnostic text, which holds no key (its crate's rule).
        data.detail = report.detail.clone();
        if let Some(device) = unconfirmed {
            data.device = Some(device);
            data.confirmed = Some(false);
        }
    })
}

// ---------------------------------------------------------------------------
// Daemons
// ---------------------------------------------------------------------------

/// `DaemonStatus` → `Daemon` (§2).
///
/// | contract | hub-daemons |
/// | --- | --- |
/// | `version` | the installed version, else the running process's |
/// | `state` | `state` (same five names) |
/// | `since` | `since`; for a daemon that has not changed state since Hub started, its install time |
/// | `lastExit.signal` | the signal number's name (`SIGTERM`) |
/// | `error` | `last_error` while `failed`, else null |
///
/// `running_version`, `started_at`, `restarts`, `crashes` and `retry_at`
/// have no place in the contract.
pub fn daemon(status: &DaemonStatus, installed: Option<&InstalledDaemon>) -> api::Daemon {
    api::Daemon {
        id: status.id.clone(),
        version: status.version.clone().or_else(|| installed.map(|daemon| daemon.version.clone())).or_else(|| status.running_version.clone()).unwrap_or_default(),
        state: match status.state {
            DaemonState::Stopped => api::DaemonState::Stopped,
            DaemonState::Starting => api::DaemonState::Starting,
            DaemonState::Running => api::DaemonState::Running,
            DaemonState::Stopping => api::DaemonState::Stopping,
            DaemonState::Failed => api::DaemonState::Failed,
        },
        pid: status.pid,
        since: status.since.or_else(|| installed.map(|daemon| daemon.installed_at)),
        channels: status.channels as u32,
        last_exit: status.last_exit.as_ref().map(|exit| api::LastExit { code: exit.code, signal: exit.signal.map(signal_name), at: exit.at }),
        error: if status.state == DaemonState::Failed { status.last_error.clone() } else { None },
    }
}

pub fn log_stream(stream: LogStream) -> &'static str {
    match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
        LogStream::Hub => "hub",
    }
}

pub fn daemon_log_line(line: &LogLine) -> api::DaemonLogLine {
    api::DaemonLogLine { at: line.at, stream: log_stream(line.stream).to_owned(), text: line.text.clone() }
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
    fn holders_survive_the_lock_label() {
        let holder = api::Holder { kind: api::ClientKind::Gui, name: "Pocket Hub".into(), method: "devices.push".into() };
        let label = holder_label(&holder);
        assert_eq!(holder_of(&label), holder);
        assert_eq!(holder_of(&format!("{label}{LAUNCH_HOLD_SUFFIX}")), holder);
        let error = op_error(OpError::Busy { holder: label });
        assert_eq!(error.data.as_ref().unwrap().holder.as_ref(), Some(&holder));
        assert_eq!(error.message, "The device is busy: Pocket Hub (gui) is running devices.push.");
    }

    #[test]
    fn stages_signals_and_sentences() {
        assert_eq!(setup_stage(Stage::Checking), api::SetupStage::Reading);
        assert_eq!(setup_stage(Stage::Resolving), api::SetupStage::Downloading);
        assert_eq!(signal_name(libc::SIGTERM), "SIGTERM");
        assert_eq!(signal_name(200), "SIG200");
        assert_eq!(sentence("a package is 1 byte to 24 MiB"), "A package is 1 byte to 24 MiB.");
        assert_eq!(unlinked_code(None), "lost");
        let progress = setup_progress(&Progress { stage: Stage::Checking, done: 0, total: None });
        assert_eq!(serde_json::to_value(&progress).unwrap(), serde_json::json!({"stage": "reading", "message": "Reading what the device holds."}));
    }
}
