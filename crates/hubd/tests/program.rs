//! The device's own program through the real `pocket-hubd` binary: updates
//! over the link (`devices.installProgram`) from a GitHub-shaped release
//! server on loopback and from local files, and `Device.program` as links,
//! updates and the release's looks change it.
//!
//! The release's asset is `pocket-shell-v<date>.3dsx`; what the reference
//! device receives is always `pocket-shell.3dsx`, the program it runs.

mod common;

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use common::*;
use pocket_hub_api::types::*;
use pocket_hub_api::{Client, ErrorKind, methods};
use pocket_hub_setup::release::sha256_hex;
use pocket_hub_setup::test_support::{FakeHttp, fake_3dsx, github_release};
use serde_json::{Value, json};

const REPO: &str = "test/shell";
const LATEST: &str = "/repos/test/shell/releases/latest";

async fn wait_for_socket(socket: &Path) {
    for _ in 0..400 {
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("pocket-hubd did not answer on {}", socket.display());
}

/// The Device's `program` now.
async fn program_of(client: &Client, id: &str) -> ProgramState {
    client.call_as::<Device>(methods::DEVICES_GET, &DeviceParams { device: id.into() }).await.unwrap().program
}

/// The kind and reason of an update's error.
async fn refusal(client: &Client, params: &InstallProgramParams) -> (ErrorKind, Option<String>, String) {
    let error = rpc(client.call(methods::DEVICES_INSTALL_PROGRAM, params).await.unwrap_err());
    let data = error.data.expect("an operation error");
    (data.kind.unwrap(), data.reason, error.message)
}

fn from_path(id: &str, path: &Path) -> InstallProgramParams {
    InstallProgramParams { device: id.into(), program: Some(ProgramChoice::Path { path: path.display().to_string() }), launch: None }
}

fn from_release(id: &str, tag: &str) -> InstallProgramParams {
    InstallProgramParams { device: id.into(), program: Some(ProgramChoice::Release { release: tag.into() }), launch: None }
}

/// Runs an update to its end: the phases of its progress and its answer.
async fn update(client: &Client, params: &InstallProgramParams) -> (Vec<TransferProgress>, Result<InstallProgramResult, pocket_hub_api::ClientError>) {
    let mut call = client.start(methods::DEVICES_INSTALL_PROGRAM, params).await.unwrap();
    let mut seen = Vec::new();
    while let Some(progress) = call.progress().await {
        seen.push(serde_json::from_value::<TransferProgress>(progress).unwrap());
    }
    (seen, call.finish().await.map(|value| serde_json::from_value(value).unwrap()))
}

fn phases(progress: &[TransferProgress]) -> Vec<&str> {
    let mut phases: Vec<&str> = progress.iter().map(|p| p.phase.as_str()).collect();
    phases.dedup();
    phases
}

/// The device took the program's `launching` reply: it closed with
/// `restart`, Hub holds it (`launching`) and a second caller is `busy`;
/// told to connect, it links again and the hold ends.
async fn restart_and_relink(client: &Client, other: &Client, device: &mut RefDevice, id: &str) {
    let unlinked = event(client, |e| topic(e, "device.unlinked")).await;
    assert_eq!(unlinked["code"], "restart");
    let holding = client.call_as::<Device>(methods::DEVICES_GET, &DeviceParams { device: id.into() }).await.unwrap();
    let busy = holding.busy.expect("held while the device restarts");
    assert!(busy.launching);
    assert_eq!(busy.holder.method, "devices.installProgram");
    let refused = rpc(other.call(methods::DEVICES_INSTALL_PROGRAM, &InstallProgramParams { device: id.into(), program: None, launch: None }).await.unwrap_err());
    let data = refused.data.unwrap();
    assert_eq!((data.kind.unwrap(), data.holder.map(|h| h.method)), (ErrorKind::Busy, Some("devices.installProgram".to_owned())));
    device.send(json!({"cmd": "connect"})).await;
    event(client, |e| topic(e, "device.changed") && !e["device"]["link"].is_null() && e["device"]["busy"].is_null()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn updates_replace_the_program_the_device_runs() {
    let home = temp_home();
    let work = temp_home();
    let first = fake_3dsx(300 * 1024, 11);
    let releases = FakeHttp::start([github_release(REPO, "v2026.11.11", "2026.11.11", &first, true), github_release(REPO, "v2026.11.02", "2026.11.02", &fake_3dsx(4096, 3), false)].concat()).await;
    let socket = home.path().join("hubd.sock");
    let mut core = tokio::process::Command::new(env!("CARGO_BIN_EXE_pocket-hubd"))
        .env("POCKET_HUB_NAME", HUB_NAME)
        .arg("--home")
        .arg(home.path())
        .args(["--link-port", "0", "--no-beacons", "--mutex-wait-ms", "300", "--store-url", NO_STORE, "--shell-repo", REPO])
        // Every link looks again: the test sees each look.
        .args(["--program-look-ms", "0"])
        .arg("--shell-api")
        .arg(&releases.base)
        .arg("--bun")
        .arg(bun())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("pocket-hubd runs");
    wait_for_socket(&socket).await;
    let client = client(&socket, "Pocket Hub").await;
    let other = Client::connect(&socket, info(ClientKind::Cli, "pocket-hub")).await.unwrap();
    client.call(methods::SUBSCRIBE, &json!({})).await.unwrap();
    let status: HubStatus = client.call_as(methods::STATUS, &json!({})).await.unwrap();
    let port = status.carriers[0].port.unwrap();

    // A device set up from a local build, not linked yet: Hub has not looked.
    let card = card(work.path(), "sd");
    let shell = program(work.path(), "pocket-shell.3dsx", 4096);
    let report: InstallReport = client
        .call_as(methods::SETUP_CARD, &SetupCardParams { kind: "3ds".into(), path: card.display().to_string(), program: Some(ProgramChoice::Path { path: shell.display().to_string() }) })
        .await
        .unwrap();
    let id = report.device.clone();
    assert_eq!(program_of(&client, &id).await, ProgramState { version: None, latest: None, update: false });
    assert_eq!(releases.hits(LATEST), 0);
    // Not linked: nothing is fetched.
    assert_eq!(kind_of(client.call(methods::DEVICES_INSTALL_PROGRAM, &InstallProgramParams { device: id.clone(), program: None, launch: None }).await.unwrap_err()), ErrorKind::NotLinked);

    // --- the link: Hub looks, the device reports its version --------------------
    let mut device = RefDevice::spawn(&link_dir(&card), &status.id, port).await;
    let looked = event(&client, |e| topic(e, "device.changed") && e["device"]["program"]["latest"] == "2026.11.11").await;
    assert_eq!(looked["device"]["program"], json!({"version": null, "latest": "2026.11.11", "update": false}));
    device.send(json!({"cmd": "apps", "count": 1, "home": "2026.11.2"})).await;
    let reported = event(&client, |e| topic(e, "device.changed") && !e["device"]["program"]["version"].is_null()).await;
    assert_eq!(reported["device"]["program"], json!({"version": "2026.11.2", "latest": "2026.11.11", "update": true}));

    // --- the latest release, launched ---------------------------------------
    // Downloaded, verified, sent as the program the device runs.
    let (progress, done) = update(&client, &InstallProgramParams { device: "*".into(), program: None, launch: None }).await;
    let done = done.unwrap();
    assert_eq!(done, InstallProgramResult { phase: "launching".into(), name: "pocket-shell.3dsx".into(), version: Some("2026.11.11".into()) });
    assert_eq!(phases(&progress), ["downloading", "sending", "receiving"]);
    let downloaded = progress.iter().rfind(|p| p.phase == "downloading").unwrap();
    assert_eq!((downloaded.done, downloaded.total), (first.len() as u64, first.len() as u64));
    let sent = progress.last().unwrap();
    assert_eq!((sent.done, sent.total), (first.len() as u64, first.len() as u64));
    let native = device.expect("native").await;
    assert_eq!((native["name"].as_str(), native["launch"].as_bool(), native["bytes"].as_u64()), (Some("pocket-shell.3dsx"), Some(true), Some(first.len() as u64)));
    assert_eq!(native["sum"].as_u64().unwrap() as u32, fnv32(&first), "the device received the release's bytes");
    restart_and_relink(&client, &other, &mut device, &id).await;
    // The new program reports its version: no update now.
    device.send(json!({"cmd": "apps", "count": 1, "home": "2026.11.11"})).await;
    let current = event(&client, |e| topic(e, "device.changed") && e["device"]["program"]["version"] == "2026.11.11").await;
    assert_eq!(current["device"]["program"], json!({"version": "2026.11.11", "latest": "2026.11.11", "update": false}));
    // What Hub wrote is noted for the device as setup notes it.
    let notes: Value = serde_json::from_str(&std::fs::read_to_string(home.path().join("store/installed-programs.json")).unwrap()).unwrap();
    assert_eq!(notes[&id]["path"], "3ds/pocket-shell.3dsx");
    assert_eq!(notes[&id]["sha256"], sha256_hex(&first));
    assert_eq!(notes[&id]["version"], "2026.11.11");

    // --- a local build of any name, not launched -------------------------------
    let build = program(work.path(), "pocket-shell-dev-build_42.3dsx", 70_000);
    let (progress, done) = update(&client, &InstallProgramParams { device: id.clone(), program: Some(ProgramChoice::Path { path: build.display().to_string() }), launch: Some(false) }).await;
    assert_eq!(done.unwrap(), InstallProgramResult { phase: "installed".into(), name: "pocket-shell.3dsx".into(), version: None });
    assert_eq!(phases(&progress), ["sending", "receiving"], "nothing is downloaded for a local file");
    let native = device.expect("native").await;
    assert_eq!((native["name"].as_str(), native["launch"].as_bool(), native["bytes"].as_u64()), (Some("pocket-shell.3dsx"), Some(false), Some(70_000)));
    assert!(program_of(&client, &id).await.version.is_some(), "the device runs what it ran");
    let missing = work.path().join("nowhere.3dsx");
    assert_eq!(refusal(&client, &from_path(&id, &missing)).await.0, ErrorKind::NotFound);
    let not_a_program = work.path().join("notes.3dsx");
    std::fs::write(&not_a_program, vec![b'x'; 4096]).unwrap();
    assert_eq!(refusal(&client, &from_path(&id, &not_a_program)).await.0, ErrorKind::Invalid);
    // A local build whose SHA256SUMS names other bytes: the release kind.
    let folder = work.path().join("checked");
    std::fs::create_dir_all(&folder).unwrap();
    let checked = program(&folder, "pocket-shell-v9.3dsx", 4096);
    std::fs::write(folder.join("SHA256SUMS"), format!("{}  pocket-shell-v9.3dsx\n", "0".repeat(64))).unwrap();
    assert_eq!(refusal(&client, &from_path(&id, &checked)).await.0, ErrorKind::Release);
    assert_eq!(refusal(&client, &from_path(&id, &checked)).await.1.as_deref(), Some("rejected"));

    // --- a release by its tag; the cached one is not downloaded again ----------
    let (progress, done) = update(&client, &InstallProgramParams { device: id.clone(), program: Some(ProgramChoice::Release { release: "v2026.11.02".into() }), launch: Some(false) }).await;
    assert_eq!(done.unwrap().version.as_deref(), Some("2026.11.02"));
    assert_eq!(device.expect("native").await["name"], "pocket-shell.3dsx");
    assert_eq!(phases(&progress)[0], "downloading");
    let (progress, _) = update(&client, &InstallProgramParams { device: id.clone(), program: Some(ProgramChoice::Release { release: "v2026.11.11".into() }), launch: Some(false) }).await;
    let downloading: Vec<&TransferProgress> = progress.iter().filter(|p| p.phase == "downloading").collect();
    assert_eq!(downloading.len(), 1, "once, from the cache");
    assert_eq!((downloading[0].done, downloading[0].total), (first.len() as u64, first.len() as u64));
    assert_eq!(releases.hits("/dl/v2026.11.11/pocket-shell-v2026.11.11.3dsx"), 1);
    device.expect("native").await;
    // A tag the source does not have.
    let (kind, reason, message) = refusal(&client, &from_release(&id, "v1999.01.01")).await;
    assert_eq!((kind, reason.as_deref(), message.as_str()), (ErrorKind::Release, Some("not_found"), "There is no release v1999.01.01 of the 3DS's software."));
    // A release whose program is not the bytes its SHA256SUMS names.
    for (path, body) in github_release(REPO, "v2026.10.01", "2026.10.01", &fake_3dsx(4096, 30), false) {
        releases.set(&path, body);
    }
    releases.set("/dl/v2026.10.01/pocket-shell-v2026.10.01.3dsx", fake_3dsx(4096, 31));
    assert_eq!(refusal(&client, &from_release(&id, "v2026.10.01")).await.1.as_deref(), Some("rejected"));

    // --- a newer release: the next link's look finds it ------------------------
    let newer = fake_3dsx(8192, 12);
    for (path, body) in github_release(REPO, "v2026.12.01", "2026.12.01", &newer, true) {
        releases.set(&path, body);
    }
    update(&client, &InstallProgramParams { device: id.clone(), program: Some(ProgramChoice::Path { path: build.display().to_string() }), launch: Some(true) }).await.1.unwrap();
    device.expect("native").await;
    restart_and_relink(&client, &other, &mut device, &id).await;
    let found = event(&client, |e| topic(e, "device.changed") && e["device"]["program"]["latest"] == "2026.12.01").await;
    assert_eq!(found["device"]["program"], json!({"version": "2026.11.11", "latest": "2026.12.01", "update": true}));

    // --- the release source fails ----------------------------------------------
    // An update that cannot get its release sends nothing and holds nothing.
    releases.fail(Some(503));
    let (kind, reason, message) = refusal(&client, &InstallProgramParams { device: id.clone(), program: None, launch: None }).await;
    assert_eq!((kind, reason.as_deref()), (ErrorKind::Release, Some("unreachable")));
    assert!(message.contains("Hub could not get the 3DS's software, so nothing was sent to the device") && message.contains("HTTP 503"), "{message}");
    assert!(client.call_as::<Device>(methods::DEVICES_GET, &DeviceParams { device: id.clone() }).await.unwrap().busy.is_none());
    // A look that fails at a link leaves `latest` as it was and is logged.
    let looks = releases.hits(LATEST);
    update(&client, &InstallProgramParams { device: id.clone(), program: Some(ProgramChoice::Path { path: build.display().to_string() }), launch: None }).await.1.unwrap();
    let native = device.expect("native").await;
    assert_eq!(native["name"], "pocket-shell.3dsx");
    restart_and_relink(&client, &other, &mut device, &id).await;
    let log = home.path().join("logs/hubd.log");
    for _ in 0..200 {
        if releases.hits(LATEST) > looks && std::fs::read_to_string(&log).unwrap_or_default().contains("could not be looked up") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(releases.hits(LATEST) > looks, "the link looked");
    assert!(std::fs::read_to_string(&log).unwrap().contains("the newest release of the device's program could not be looked up"));
    assert_eq!(program_of(&client, &id).await, ProgramState { version: Some("2026.11.11".into()), latest: Some("2026.12.01".into()), update: true });

    client.call(methods::SHUTDOWN, &json!({})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), core.wait()).await.expect("the core exits").unwrap();
    device.kill().await;
}
