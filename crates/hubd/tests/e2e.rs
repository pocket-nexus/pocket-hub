//! The whole path through the real `pocket-hubd` binary: setup onto a
//! folder that stands in for an SD card, the reference device linking with
//! the pairing that setup wrote, the runtime operations, the device mutex
//! across two clients, the link-channel example service installed through the
//! API and serving the device's applet channel with the real SDK, a second
//! device that pairs by the code it shows as an M5Stack Tab5 does, and the
//! ordered shutdown.
//!
//! The run's lines are the examples of `docs/control-api-examples.json`:
//! with `POCKET_HUB_UPDATE_EXAMPLES=1` the file is written again; without
//! it the test checks that the file holds an example of every method, topic
//! and error this run produced.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use common::*;
use pocket_hub_api::types::*;
use pocket_hub_api::{Client, ErrorKind, PROTOCOL, Transcript, methods};
use pocket_hub_applet_store::test_support::{FakeStore, build_package, icon, public, sign, test_key};
use pocket_hub_setup::test_support::{FakeFtp, FakeHttp, fake_3dsx, github_release};
use serde_json::{Value, json};

const APP: &str = "dev.pocket-nexus.link-channel";
const MIB: usize = 1024 * 1024;

async fn wait_for_socket(socket: &Path) {
    for _ in 0..400 {
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("pocket-hubd did not answer on {}", socket.display());
}

/// The device once `wanted` holds of it, from the `device.changed` events
/// (each carries the whole Device) until a deadline of 30 s: for state that
/// several events build in any order. It makes no call, so the examples
/// file's `devices.get` stays the one the test makes on purpose.
async fn device_where(client: &Client, id: &str, mut wanted: impl FnMut(&Device) -> bool) -> Device {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut last = None;
    loop {
        let Ok(event) = tokio::time::timeout_at(deadline, client.next_event()).await else {
            panic!("the device never reached the state; the last one: {last:?}");
        };
        let event = event.expect("the connection is open");
        if !topic(&event, "device.changed") || event["device"]["id"] != id {
            continue;
        }
        let device: Device = serde_json::from_value(event["device"].clone()).unwrap();
        if wanted(&device) {
            return device;
        }
        last = Some(device);
    }
}

/// The first service log line that contains `needle`, waiting for it.
async fn service_line(client: &Client, needle: &str) -> String {
    for _ in 0..200 {
        let logs: ServiceLogs = client.call_as(methods::SERVICES_LOGS, &ServiceLogsParams { id: APP.into(), tail: Some(1000) }).await.unwrap();
        if let Some(line) = logs.lines.iter().find(|line| line.text.contains(needle)) {
            return line.text.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no service log line contains {needle}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_vertical_slice_through_the_binary() {
    let home = temp_home();
    let work = temp_home();
    let console = FakeFtp::start(&work.path().join("console")).await;
    let applets = FakeStore::start().await;
    let notes = applets.publish(&test_key(1), build_package("dev.pocket-nexus.notes", "1.0.0", "Notes", &[("3ds-dev", "notes"), ("tab5-home", "notes")], 64 * 1024), icon(1), "pocket-nexus", "Notes on the device.");
    // Its details, as the store reads them out of the package: here every
    // member the store may answer, as for a real applet; and sample data.
    applets.set_details(
        &notes.package,
        Some(json!({
            "devices": [
                {"target": "tab5-home", "name": "M5Stack Tab5", "build": {"target": "tab5-home", "hostAbi": 12, "profile": "0".repeat(64), "hash": "0".repeat(16), "sections": []}},
                {"target": "3ds-dev", "name": "Nintendo 3DS", "build": {"target": "3ds-dev", "hostAbi": 12, "profile": "0".repeat(64), "hash": "0".repeat(16), "sections": []}},
            ],
            "needs": ["text.glyphs.baked", "input.buttons", "display.auxiliary", "input.touch.auxiliary"],
            "uses": ["system.agent"],
            "surface": {"touch": true, "size": {"min": [320, 240], "max": [640, 720]}},
            "framework": "solid",
            "api": 1,
        })),
    );
    let notes = applets.set_sample(&notes, &json!({"mock": 1, "routes": [{"url": "https://example.com/notes.json", "text": "[]"}]}));
    let mut forged = applets.publish(&test_key(2), build_package("io.example.forged", "1.0.0", "Forged", &[("3ds-dev", "forged")], 100), icon(2), "someone", "Signed by another key.");
    forged.signature = sign(&test_key(1), &forged);
    applets.set_entry(&forged);
    // Pocket Shell's releases, as GitHub's API describes them.
    let release = fake_3dsx(64 * 1024, 5);
    let releases = FakeHttp::start(github_release("test/shell", "v2026.11.11", "2026.11.11", &release, true)).await;
    let socket = home.path().join("hubd.sock");
    let mut core = tokio::process::Command::new(env!("CARGO_BIN_EXE_pocket-hubd"))
        .env("POCKET_HUB_NAME", HUB_NAME)
        .arg("--home")
        .arg(home.path())
        .args(["--link-port", "0", "--no-beacons", "--service-idle-seconds", "1", "--mutex-wait-ms", "400", "--discover-targets", "127.0.0.1"])
        .arg("--discover-port")
        .arg(console.address.port().to_string())
        .arg("--bun")
        .arg(bun())
        .arg("--store-url")
        .arg(&applets.url)
        .arg("--store-key")
        .arg(public(&test_key(1)))
        .arg("--shell-api")
        .arg(&releases.base)
        .args(["--shell-repo", "test/shell"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("pocket-hubd runs");
    wait_for_socket(&socket).await;

    // --- a hello in another protocol (CONTROL-API.md §1, Hello) -------------
    // Refused with `-32602`, and the refusal says the core's own protocol, so
    // a client of another version learns it without reading the sentence. Its
    // lines are the examples file's `-32602`.
    let refused_transcript = Transcript::default();
    let refused = Client::connect_raw(&socket, Some(refused_transcript.clone())).await.unwrap();
    let error = rpc(refused.call(methods::HELLO, &HelloParams { protocol: 1, client: info(ClientKind::Gui, "Pocket Hub") }).await.unwrap_err());
    assert_eq!((error.code, error.message.as_str()), (-32602, "This Hub speaks protocol 2, not 1."));
    assert_eq!((error.refused_protocol(), error.data.as_ref().and_then(|data| data.protocol), error.kind()), (Some(PROTOCOL), Some(PROTOCOL), None));
    drop(refused);

    // --- hello, the hub ------------------------------------------------------
    let transcript = Transcript::default();
    let client = Client::connect_raw(&socket, Some(transcript.clone())).await.unwrap();
    assert_eq!(rpc(client.call(methods::STATUS, &json!({})).await.unwrap_err()).code, -32600);
    let hello: HelloResult = client.call_as(methods::HELLO, &HelloParams { protocol: PROTOCOL, client: info(ClientKind::Gui, "Pocket Hub") }).await.unwrap();
    assert_eq!(hello.hub.name, HUB_NAME, "a test core never takes the computer's name");
    client.call(methods::SUBSCRIBE, &json!({})).await.unwrap();
    let status: HubStatus = client.call_as(methods::STATUS, &json!({})).await.unwrap();
    // The system Bun's own version: this computer's, for the examples file.
    let bun_version = status.bun.version.clone();
    assert_eq!(status.id, hello.hub.id);
    let port = status.carriers[0].port.unwrap();
    assert_eq!(status.home, home.path().display().to_string());
    client.call(methods::SET_NAME, &NameParams { name: "Studio Test".into() }).await.unwrap();
    let renamed = event(&client, |e| topic(e, "hub.changed")).await;
    assert_eq!(renamed["hub"]["name"], "Studio Test");
    assert_eq!(rpc(client.call("hub.nothing", &json!({})).await.unwrap_err()).code, -32601);
    assert_eq!(rpc(client.call(methods::DEVICES_GET, &json!({})).await.unwrap_err()).code, -32602);
    assert_eq!(kind_of(client.call(methods::DEVICES_GET, &DeviceParams { device: "0123456789abcdef".into() }).await.unwrap_err()), ErrorKind::NotFound);

    // --- setup over ftpd, and onto the card ----------------------------------
    let shell = program(work.path(), "pocket-shell.3dsx", 32 * 1024);
    let found: DiscoverResult = client.call_as(methods::SETUP_DISCOVER, &KindParams { kind: "3ds".into() }).await.unwrap();
    assert_eq!(found.candidates.len(), 1);
    let over_ftp: InstallReport = client
        .call_as(methods::SETUP_INSTALL, &SetupInstallParams { kind: "3ds".into(), host: "127.0.0.1".into(), port: Some(console.address.port()), program: Some(ProgramChoice::Path { path: shell.display().to_string() }) })
        .await
        .unwrap();
    assert_eq!(over_ftp.pairing, PairingResult::Key);
    assert_eq!(kind_of(client.call(methods::SETUP_INSTALL, &SetupInstallParams { kind: "3ds".into(), host: "127.0.0.1".into(), port: Some(1), program: None }).await.unwrap_err()), ErrorKind::Setup);
    client.call(methods::DEVICES_FORGET, &DeviceParams { device: over_ftp.device.clone() }).await.unwrap();

    let card = card(work.path(), "sd");
    let mut setup = client
        .start(methods::SETUP_CARD, &SetupCardParams { kind: "3ds".into(), path: card.display().to_string(), program: Some(ProgramChoice::Path { path: shell.display().to_string() }) })
        .await
        .unwrap();
    let mut stages = Vec::new();
    while let Some(progress) = setup.progress().await {
        stages.push(progress["stage"].as_str().unwrap().to_owned());
    }
    let report: InstallReport = serde_json::from_value(setup.finish().await.unwrap()).unwrap();
    assert_eq!((report.pairing, report.program.action, report.confirmed), (PairingResult::Key, ProgramAction::Installed, true));
    assert!(stages.contains(&"pairing".to_owned()) && stages.last().map(String::as_str) == Some("done"), "{stages:?}");
    assert!(card.join("3ds/pocket-shell.3dsx").is_file());
    let companion = std::fs::read_to_string(link_dir(&card).join(format!("companions/{}.txt", hello.hub.id))).unwrap();
    assert!(companion.contains(&format!("port={port}")), "the pairing names the port in use");
    let id = report.device.clone();

    // Paired, not linked yet.
    assert_eq!(kind_of(client.call(methods::DEVICES_STATUS, &DeviceParams { device: id.clone() }).await.unwrap_err()), ErrorKind::NotLinked);

    // --- the device links with that pairing ----------------------------------
    let mut device = RefDevice::spawn(&link_dir(&card), &hello.hub.id, port).await;
    assert_eq!(device.id, id);
    let linked = event(&client, |e| topic(e, "device.changed") && !e["device"]["link"].is_null()).await;
    assert_eq!(linked["device"]["name"], "Test 3DS");
    assert_eq!(linked["device"]["link"]["carrier"], "tcp");
    assert!(!linked["device"]["setupUnconfirmed"].as_bool().unwrap());
    let status = client.call(methods::DEVICES_STATUS, &DeviceParams { device: id.clone() }).await.unwrap();
    assert_eq!(status["phase"], "booted");
    // Two applets and the launcher's own row: the device's program, older
    // than the release Hub looked up when it linked.
    // The report and the look end in either order: the test waits for the
    // state both leave, not for one event.
    device.send(json!({"cmd": "apps", "count": 2, "home": "2026.11.2"})).await;
    let with_apps = device_where(&client, &id, |d| d.apps.as_ref().is_some_and(|a| a.len() == 3) && d.program.latest.is_some()).await;
    assert_eq!(with_apps.apps.unwrap()[2].output, "app1");
    assert_eq!(with_apps.program, ProgramState { version: Some("2026.11.2".into()), latest: Some("2026.11.11".into()), update: true });
    let list: DeviceList = client.call_as(methods::DEVICES_LIST, &json!({})).await.unwrap();
    assert_eq!(list.devices.len(), 1);
    let got: Device = client.call_as(methods::DEVICES_GET, &DeviceParams { device: "*".into() }).await.unwrap();
    assert_eq!(got, list.devices[0]);

    // --- push with progress, launch, the program, screenshots, logs ----------
    let pocket = package(work.path(), "demo.pocket", 300 * 1024, 1);
    let mut push = client.start(methods::DEVICES_PUSH, &PathParams { device: "*".into(), path: pocket.display().to_string() }).await.unwrap();
    let mut last = None;
    while let Some(progress) = push.progress().await {
        last = Some(serde_json::from_value::<TransferProgress>(progress).unwrap());
    }
    let pushed: PushResult = serde_json::from_value(push.finish().await.unwrap()).unwrap();
    assert_eq!(last.map(|p| (p.done, p.total)), Some((300 * 1024, 300 * 1024)));
    assert_eq!(pushed.app.title, "Demo");
    assert_eq!(device.expect("pushed").await["bytes"], 300 * 1024);
    let not_pocket = work.path().join("notes.txt");
    std::fs::write(&not_pocket, "not a package").unwrap();
    assert_eq!(kind_of(client.call(methods::DEVICES_PUSH, &PathParams { device: "*".into(), path: not_pocket.display().to_string() }).await.unwrap_err()), ErrorKind::Invalid);

    let launched: LaunchResult = client.call_as(methods::DEVICES_LAUNCH, &LaunchParams { device: id.clone(), app: "dev.app.1".into() }).await.unwrap();
    assert_eq!(launched.output, "app1");
    // Deleting an applet by its output name: the device's next report drops
    // its row (link specification §6.2 `runtime.remove`). Its home screen is
    // refused with the device's reason.
    client.call(methods::DEVICES_REMOVE, &RemoveParams { device: id.clone(), app: "app0".into() }).await.unwrap();
    assert_eq!(device.expect("removed").await["id"], "dev.app.0");
    device_where(&client, &id, |d| d.apps.as_ref().is_some_and(|apps| apps.len() == 2 && apps.iter().all(|app| app.id != "dev.app.0"))).await;
    let refused = rpc(client.call(methods::DEVICES_REMOVE, &RemoveParams { device: id.clone(), app: "home".into() }).await.unwrap_err());
    let data = refused.data.unwrap();
    assert_eq!((data.kind.unwrap(), data.code.as_deref(), data.reason.as_deref()), (ErrorKind::Link, Some("rejected"), Some("home")));
    // The latest release, under the name of the program the device runs.
    let mut update = client.start(methods::DEVICES_INSTALL_PROGRAM, &InstallProgramParams { device: id.clone(), program: None, launch: Some(false) }).await.unwrap();
    let mut update_phases = Vec::new();
    while let Some(progress) = update.progress().await {
        update_phases.push(progress["phase"].as_str().unwrap().to_owned());
    }
    let installed: InstallProgramResult = serde_json::from_value(update.finish().await.unwrap()).unwrap();
    assert_eq!(installed, InstallProgramResult { phase: "installed".into(), name: "pocket-shell.3dsx".into(), version: Some("2026.11.11".into()) });
    assert_eq!(update_phases.first().map(String::as_str), Some("downloading"));
    let native = device.expect("native").await;
    assert_eq!((native["name"].as_str(), native["bytes"].as_u64()), (Some("pocket-shell.3dsx"), Some(release.len() as u64)));

    // A tag the release source does not have.
    let missing = rpc(client.call(methods::DEVICES_INSTALL_PROGRAM, &InstallProgramParams { device: id.clone(), program: Some(ProgramChoice::Release { release: "v1999.01.01".into() }), launch: None }).await.unwrap_err());
    let data = missing.data.unwrap();
    assert_eq!((data.kind.unwrap(), data.reason.as_deref()), (ErrorKind::Release, Some("not_found")));

    // With `launch` (the default), the device restarts into the program: the
    // mutex is held (`launching`) until it links again.
    let build = program(work.path(), "pocket-test.3dsx", 8 * 1024);
    let relaunch: InstallProgramResult =
        client.call_as(methods::DEVICES_INSTALL_PROGRAM, &InstallProgramParams { device: id.clone(), program: Some(ProgramChoice::Path { path: build.display().to_string() }), launch: None }).await.unwrap();
    assert_eq!((relaunch.phase.as_str(), relaunch.name.as_str(), relaunch.version.as_deref()), ("launching", "pocket-shell.3dsx", None));
    let holding = event(&client, |e| topic(e, "device.changed") && e["device"]["busy"]["launching"] == true).await;
    assert_eq!(holding["device"]["busy"]["holder"]["method"], "devices.installProgram");
    assert_eq!(kind_of(client.call(methods::DEVICES_LAUNCH, &LaunchParams { device: id.clone(), app: "app0".into() }).await.unwrap_err()), ErrorKind::Busy);
    device.kill().await;
    let mut device = RefDevice::spawn(&link_dir(&card), &hello.hub.id, port).await;
    event(&client, |e| topic(e, "device.changed") && !e["device"]["link"].is_null() && e["device"]["busy"].is_null()).await;

    let shot: ScreenshotResult = client.call_as(methods::DEVICES_SCREENSHOT, &ScreenshotParams { device: id.clone(), path: None }).await.unwrap();
    assert!(shot.path.starts_with(&home.path().join("cache/screenshots").display().to_string()));
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&shot.path).unwrap()));
    let reader = decoder.read_info().expect("a valid PNG");
    assert_eq!((reader.info().width, reader.info().height), (shot.width, shot.height));
    let chosen = work.path().join("shot.png");
    let shot_at: ScreenshotResult = client.call_as(methods::DEVICES_SCREENSHOT, &ScreenshotParams { device: id.clone(), path: Some(chosen.display().to_string()) }).await.unwrap();
    assert_eq!(shot_at.path, chosen.display().to_string());
    assert!(std::fs::read(&chosen).unwrap().starts_with(b"\x89PNG"));

    client.call(methods::DEVICES_LOGS, &LogsParams { device: id.clone(), on: true }).await.unwrap();
    device.send(json!({"cmd": "log", "text": "hello from the device", "level": "warn", "source": "guest", "frame": 42})).await;
    let line = event(&client, |e| topic(e, "device.log")).await;
    assert_eq!((line["text"].as_str(), line["level"].as_str(), line["frame"].as_u64()), (Some("hello from the device"), Some("warn"), Some(42)));
    client.call(methods::DEVICES_LOGS, &LogsParams { device: id.clone(), on: false }).await.unwrap();
    device.send(json!({"cmd": "shot"})).await;
    let unasked = event(&client, |e| topic(e, "device.screenshot")).await;
    assert!(Path::new(unasked["path"].as_str().unwrap()).is_file());

    // --- an applet from the store ---------------------------------------------
    // The install fetches the catalog; the answer for the device then shows
    // the copy it holds (the examples file keeps that fuller answer).
    let mut from_store = client.start(methods::STORE_INSTALL, &StoreInstallParams { device: id.clone(), app: notes.id.clone() }).await.unwrap();
    let mut store_phases = Vec::new();
    while let Some(progress) = from_store.progress().await {
        store_phases.push(progress["phase"].as_str().unwrap().to_owned());
    }
    let from_store: PushResult = serde_json::from_value(from_store.finish().await.unwrap()).unwrap();
    assert_eq!((from_store.app.id.as_str(), from_store.app.output.as_str()), ("dev.pocket-nexus.notes", "notes"));
    assert_eq!(store_phases.first().map(String::as_str), Some("downloading"));
    assert_eq!(device.expect("pushed").await["bytes"].as_u64(), Some(notes.bytes));
    event(&client, |e| topic(e, "store.changed")).await;
    event(&client, |e| topic(e, "device.changed") && e["device"]["apps"].as_array().is_some_and(|apps| apps.iter().any(|a| a["id"] == "dev.pocket-nexus.notes"))).await;
    let for_device: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams { device: Some(id.clone()), refresh: None }).await.unwrap();
    let held = for_device.apps.iter().find(|app| app.id == notes.id).unwrap();
    assert_eq!((held.installed.as_ref().unwrap().as_ref().unwrap().hash.as_str(), held.update, held.cached), (from_store.hash.as_str(), Some(false), true));
    let offer: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams::default()).await.unwrap();
    assert_eq!(offer.apps.len(), 2);
    let details: StoreDetails = client.call_as(methods::STORE_DETAILS, &StoreDetailsParams { app: notes.id.clone() }).await.unwrap();
    assert_eq!((details.id.as_str(), details.version.as_str()), ("dev.pocket-nexus.notes", "1.0.0"));
    assert_eq!(details.devices.iter().map(|device| (device.name.as_str(), device.host_abi)).collect::<Vec<_>>(), [("M5Stack Tab5", Some(12)), ("Nintendo 3DS", Some(12))]);
    assert_eq!(details.surface.and_then(|surface| surface.size).map(|size| size.max), Some([640, 720]));
    let sample: StoreSample = client.call_as(methods::STORE_SAMPLE, &StoreDetailsParams { app: notes.id.clone() }).await.unwrap();
    assert!(Path::new(&sample.package).is_file() && Path::new(&sample.mock).is_file());
    let refused = rpc(client.call(methods::STORE_INSTALL, &StoreInstallParams { device: id.clone(), app: forged.id.clone() }).await.unwrap_err());
    assert_eq!(refused.data.unwrap().reason.as_deref(), Some("rejected"));

    // --- the device mutex across two clients ---------------------------------
    let other_transcript = Transcript::default();
    let other = Client::connect_traced(&socket, info(ClientKind::Cli, "pocket-hub"), Some(other_transcript.clone())).await.unwrap();
    signal(device.pid(), SIGSTOP);
    let big = package(work.path(), "big.pocket", 2 * MIB, 1);
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    let held = client.start(methods::DEVICES_PUSH, &PathParams { device: id.clone(), path: big.display().to_string() }).await.unwrap();
    event(&client, |e| topic(e, "device.changed") && e["device"]["busy"]["since"].as_i64().is_some_and(|since| since >= started)).await;
    let busy = rpc(other.call(methods::DEVICES_LAUNCH, &LaunchParams { device: id.clone(), app: "app0".into() }).await.unwrap_err());
    assert_eq!(busy.data.unwrap().holder, Some(Holder { kind: ClientKind::Gui, name: "Pocket Hub".into(), method: "devices.push".into() }));
    client.cancel(held.id).await.unwrap();
    assert_eq!(kind_of(held.finish().await.unwrap_err()), ErrorKind::Cancelled);
    signal(device.pid(), SIGCONT);
    event(&client, |e| topic(e, "device.changed") && e["device"]["busy"].is_null()).await;

    // --- the link-channel service, installed through the API -------------------
    let runtime: RuntimeInfo = client.call_as(methods::SERVICES_RUNTIME, &RuntimeParams::default()).await.unwrap();
    assert!(runtime.installed);
    let mut install = client.start(methods::SERVICES_INSTALL, &ServicePathParams { path: link_channel_package().display().to_string() }).await.unwrap();
    let mut install_stages = Vec::new();
    while let Some(progress) = install.progress().await {
        install_stages.push(progress["stage"].as_str().unwrap().to_owned());
    }
    let service: Service = serde_json::from_value(install.finish().await.unwrap()).unwrap();
    assert_eq!((service.id.as_str(), service.version.as_str(), service.state), (APP, "0.1.0", ServiceState::Stopped));
    assert_eq!(install_stages, vec!["installing"]);
    // The service's kept lines at a moment whose content is known: the
    // examples file's `services.logs`.
    let kept: ServiceLogs = client.call_as(methods::SERVICES_LOGS, &ServiceLogsParams { id: APP.into(), tail: Some(1) }).await.unwrap();
    assert_eq!(kept.lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>(), ["installed 0.1.0"]);
    assert!(home.path().join(format!("services/{APP}/0.1.0/node_modules/@pocketjs/service")).is_symlink(), "Hub linked its SDK into the package");

    device.send(json!({"cmd": "open", "app": APP})).await;
    device.expect("channel-open").await;
    // The service's and the device's views of the channel, in either order.
    let (mut open, mut running) = (None, None);
    while open.is_none() || running.is_none() {
        let e = event(&client, |e| topic(e, "device.changed") || topic(e, "service.changed")).await;
        if topic(&e, "device.changed") && e["device"]["channels"].as_array().is_some_and(|c| c.len() == 1) {
            open = Some(e);
        } else if topic(&e, "service.changed") && e["service"]["state"] == "running" && e["service"]["channels"] == 1 {
            running = Some(e);
        }
    }
    assert_eq!(open.unwrap()["device"]["channels"][0]["app"], APP);
    let service_pid = running.unwrap()["service"]["pid"].as_u64().unwrap() as u32;

    // A request each way.
    device.send(json!({"cmd": "request", "tag": "echo", "t": "echo", "args": {"text": "hi"}})).await;
    let echo = device.reply("echo").await;
    assert_eq!((echo["ok"].as_bool(), echo["value"]["echo"].as_str()), (Some(true), Some("hi")));
    device.send(json!({"cmd": "emit", "t": "hello", "args": {"from": "device"}})).await;
    let asked = device.expect("applet-request").await;
    assert_eq!(asked["args"], json!({"a": 19, "b": 23}));
    // Events from the service.
    let tick = device.wait(|v| v["ev"] == "ch-event" && v["t"] == "tick").await;
    assert!(tick["args"]["n"].is_u64());
    // 1 MiB each way.
    // In 32 KiB pieces: see the harness's `emitStream` about the reference peer.
    device.send(json!({"cmd": "emitStream", "tag": "note", "t": "note", "args": {}, "bytes": MIB, "chunk": 32 * 1024})).await;
    device.wait(|v| v["ev"] == "stream-sent" && v["tag"] == "note").await;
    device.send(json!({"cmd": "request", "tag": "blob", "t": "blob", "args": {"bytes": MIB}})).await;
    let blob = device.reply("blob").await;
    assert_eq!(blob["ok"], true, "{blob}");
    assert_eq!((blob["bytes"].as_u64(), blob["value"]["bytes"].as_u64()), (Some(MIB as u64), Some(MIB as u64)));
    assert_eq!(blob["sum"], blob["value"]["hash"], "the device read the bytes the service wrote");

    let services: ServiceList = client.call_as(methods::SERVICES_LIST, &json!({})).await.unwrap();
    assert_eq!((services.services[0].state, services.services[0].channels), (ServiceState::Running, 1));
    event(&client, |e| topic(e, "service.log") && e["id"] == APP).await;

    // The applet exits: the channel closes, the service goes idle and stops.
    device.send(json!({"cmd": "exit"})).await;
    event(&client, |e| topic(e, "device.changed") && e["device"]["channels"].as_array().is_some_and(|c| c.is_empty())).await;
    // Polled on a client of its own: the line comes a moment after the
    // close, so the number of polls differs between runs, and on the traced
    // client each one would move the id of every later request in the
    // examples file.
    let closed = service_line(&common::client(&socket, "logs").await, "closed (").await;
    let note = format!("\"bytes\":{MIB},\"hash\":{}", fnv32(&pattern(MIB)));
    assert!(closed.contains(&note), "the service read the device's 1 MiB: {closed}");
    let stopped = event(&client, |e| topic(e, "service.changed") && e["service"]["state"] == "stopped").await;
    assert_eq!(stopped["service"]["channels"], 0);
    assert!(stopped["service"]["lastExit"]["at"].is_i64());
    assert!(gone(service_pid).await);

    // Its owner's start, restart and stop.
    let started: Service = client.call_as(methods::SERVICES_START, &ServiceIdParams { id: APP.into() }).await.unwrap();
    assert_eq!(started.state, ServiceState::Running);
    let restarted: Service = client.call_as(methods::SERVICES_RESTART, &ServiceIdParams { id: APP.into() }).await.unwrap();
    assert_eq!(restarted.state, ServiceState::Running);
    assert_ne!(restarted.pid, started.pid);
    let stopped: Service = client.call_as(methods::SERVICES_STOP, &ServiceIdParams { id: APP.into() }).await.unwrap();
    assert_eq!(stopped.state, ServiceState::Stopped);
    assert!(gone(restarted.pid.unwrap()).await);

    // --- a Tab5 pairs by the code it shows -------------------------------------
    // A store no Hub has set up: the device holds no key file, Hub no record.
    let tab5_dir = work.path().join("tab5-link");
    std::fs::create_dir_all(&tab5_dir).unwrap();
    let tab5_config = json!({"autoConnect": false, "name": "Desk Tab5", "model": "M5Stack Tab5", "target": "tab5-home", "features": ["apps", "runtime", "runtime.remove", "catalog", "install", "net.http"]});
    let mut tab5 = RefDevice::spawn_with(&tab5_dir, &hello.hub.id, port, tab5_config).await;
    let answer = |device: &str, code: &str| PairAnswerParams { device: device.to_owned(), code: code.to_owned() };
    let waiting = |count: usize| move |e: &Value| topic(e, "pairing.changed") && e["requests"].as_array().is_some_and(|requests| requests.len() == count);
    // No device asks yet. (The examples file's `pairing.list` is the one
    // below, with a request in it.)
    assert_eq!(kind_of(client.call(methods::PAIRING_ANSWER, &answer("*", "314159")).await.unwrap_err()), ErrorKind::NotFound);

    // Its owner opens a pairing on it and chooses this Hub.
    tab5.send(json!({"cmd": "pairOpen", "code": "314159"})).await;
    tab5.send(json!({"cmd": "pairAsk"})).await;
    let asked = event(&client, waiting(1)).await;
    let listed: PairRequests = client.call_as(methods::PAIRING_LIST, &json!({})).await.unwrap();
    assert_eq!(json!(listed.requests), asked["requests"]);
    let request = &listed.requests[0];
    assert_eq!((request.device.as_str(), request.kind.as_str(), request.name.as_str(), request.model.as_str(), request.target.as_str()), (tab5.id.as_str(), "tab5", "Desk Tab5", "M5Stack Tab5", "tab5-home"));
    assert_eq!((request.address.as_str(), request.expires - request.since), ("127.0.0.1", 300_000));

    // What is no code is refused before anything is sent. Another code than
    // the device's: nothing is stored, and the device asks again.
    assert_eq!(kind_of(client.call(methods::PAIRING_ANSWER, &answer("*", "31415")).await.unwrap_err()), ErrorKind::Invalid);
    let wrong = rpc(client.call(methods::PAIRING_ANSWER, &answer("*", "271828")).await.unwrap_err()).data.unwrap();
    assert_eq!((wrong.kind.unwrap(), wrong.reason.as_deref()), (ErrorKind::Pairing, Some("wrong_code")));
    event(&client, waiting(1)).await;
    assert_eq!(kind_of(client.call(methods::DEVICES_GET, &DeviceParams { device: tab5.id.clone() }).await.unwrap_err()), ErrorKind::NotFound);

    // Dismissed: the device reads why, and its owner opens a pairing anew.
    client.call(methods::PAIRING_DECLINE, &DeviceParams { device: tab5.id.clone() }).await.unwrap();
    event(&client, waiting(0)).await;
    assert_eq!(kind_of(client.call(methods::PAIRING_DECLINE, &DeviceParams { device: tab5.id.clone() }).await.unwrap_err()), ErrorKind::NotFound);
    tab5.send(json!({"cmd": "pair", "tag": "declined"})).await;
    let state = tab5.wait(|v| v["ev"] == "pair" && v["tag"] == "declined").await;
    assert_eq!((state["pair"]["state"].as_str(), state["pair"]["error"].as_str()), (Some("failed"), Some("that computer dismissed the request")));
    tab5.send(json!({"cmd": "pairOpen", "code": "314159"})).await;
    tab5.send(json!({"cmd": "pairAsk"})).await;
    event(&client, waiting(1)).await;

    // The code: the answer is the device, of the family of its target, and
    // it links by itself with the key file it wrote.
    let paired: Device = client.call_as(methods::PAIRING_ANSWER, &answer("*", "314159")).await.unwrap();
    assert_eq!((paired.id.as_str(), paired.kind.as_str(), paired.name.as_str(), paired.model.as_str(), paired.target.as_str()), (tab5.id.as_str(), "tab5", "Desk Tab5", "M5Stack Tab5", "tab5-home"));
    assert!(paired.supports.is_empty(), "a Tab5 takes no push, screenshot, log or program from Hub");
    let tab5_file = std::fs::read_to_string(tab5_dir.join(format!("companions/{}.txt", hello.hub.id))).unwrap();
    assert!(tab5_file.contains(&format!("port={port}")));
    let linked = device_where(&client, &tab5.id, |d| d.link.is_some() && d.status.is_some()).await;
    assert_eq!(linked.link.unwrap().features, ["runtime", "runtime.remove", "apps", "net.http", "catalog", "install"]);
    assert_eq!(tab5.expect("linked").await["hub"], hello.hub.id.as_str());

    // What its family does not take is refused before anything is sent or fetched.
    for (method, params) in [
        (methods::DEVICES_PUSH, json!({"device": tab5.id, "path": pocket})),
        (methods::DEVICES_SCREENSHOT, json!({"device": tab5.id})),
        (methods::DEVICES_LOGS, json!({"device": tab5.id, "on": true})),
        (methods::DEVICES_INSTALL_PROGRAM, json!({"device": tab5.id})),
        (methods::STORE_INSTALL, json!({"device": tab5.id, "app": notes.id})),
    ] {
        let refused = rpc(client.call(method, &params).await.unwrap_err()).data.unwrap();
        assert_eq!((refused.kind.unwrap(), refused.code.as_deref()), (ErrorKind::Link, Some("unsupported")), "{method}");
    }

    // Its applets come from its own store page, which Hub serves for its
    // target (link specification §6.4): the offer, then a package.
    let for_tab5: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams { device: Some(tab5.id.clone()), refresh: None }).await.unwrap();
    assert_eq!(for_tab5.apps.iter().map(|app| app.id.as_str()).collect::<Vec<_>>(), [notes.id.as_str()]);
    tab5.send(json!({"cmd": "catalog", "tag": "tab5"})).await;
    let rows = tab5.wait(|v| v["ev"] == "catalog" && v["tag"] == "tab5").await;
    assert_eq!((rows["ok"].as_bool(), rows["rows"][0]["id"].as_str()), (Some(true), Some(notes.id.as_str())));
    tab5.send(json!({"cmd": "install", "app": notes.id, "tag": "tab5"})).await;
    let installed = tab5.wait(|v| v["ev"] == "install" && v["tag"] == "tab5").await;
    assert_eq!((installed["ok"].as_bool(), installed["bytes"].as_u64()), (Some(true), Some(notes.bytes)));
    // Its own program is the launcher's row; Hub knows no release to compare it with.
    tab5.send(json!({"cmd": "apps", "count": 1, "home": "0.3.0"})).await;
    let reported = device_where(&client, &tab5.id, |d| d.apps.is_some()).await;
    assert_eq!(reported.program, ProgramState { version: Some("0.3.0".into()), latest: None, update: false });
    // An applet it holds opens and is deleted from the computer, as on a 3DS.
    client.call(methods::DEVICES_LAUNCH, &LaunchParams { device: tab5.id.clone(), app: "dev.app.0".into() }).await.unwrap();
    assert_eq!(tab5.expect("launch").await["app"], "app0");
    client.call(methods::DEVICES_REMOVE, &RemoveParams { device: tab5.id.clone(), app: "dev.app.0".into() }).await.unwrap();
    assert_eq!(tab5.expect("removed").await["id"], "dev.app.0");

    // A setup is for a device whose storage Hub reaches.
    assert_eq!(kind_of(client.call(methods::SETUP_DISCOVER, &KindParams { kind: "tab5".into() }).await.unwrap_err()), ErrorKind::Invalid);
    client.call(methods::DEVICES_FORGET, &DeviceParams { device: tab5.id.clone() }).await.unwrap();
    event(&client, |e| topic(e, "device.removed") && e["id"] == tab5.id.as_str()).await;
    let tab5_id = tab5.id.clone();
    tab5.kill().await;

    // --- forget, remove, shut down -------------------------------------------
    client.call(methods::DEVICES_FORGET, &DeviceParams { device: id.clone() }).await.unwrap();
    // The link's end, then the record's.
    let unlinked = event(&client, |e| topic(e, "device.unlinked")).await;
    assert_eq!((unlinked["id"].as_str(), unlinked["code"].as_str()), (Some(id.as_str()), Some("forgotten")));
    let removed = event(&client, |e| topic(e, "device.removed")).await;
    assert_eq!(removed["id"], id.as_str());
    client.call(methods::SERVICES_REMOVE, &ServiceIdParams { id: APP.into() }).await.unwrap();
    event(&client, |e| topic(e, "service.removed")).await;
    assert_eq!(kind_of(client.call(methods::SERVICES_START, &ServiceIdParams { id: APP.into() }).await.unwrap_err()), ErrorKind::NotFound);

    // A service that cannot start is the error `service`; `purge` removes its data.
    let broken = work.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("pocket.service.json"), r#"{"service":1,"id":"dev.test.broken","version":"1.0.0","entry":"service.ts"}"#).unwrap();
    std::fs::write(broken.join("service.ts"), "process.exit(3);\n").unwrap();
    client.call(methods::SERVICES_INSTALL, &ServicePathParams { path: broken.display().to_string() }).await.unwrap();
    let error = rpc(client.call(methods::SERVICES_START, &ServiceIdParams { id: "dev.test.broken".into() }).await.unwrap_err());
    let data = error.data.unwrap();
    assert_eq!((data.kind.unwrap(), data.id.as_deref()), (ErrorKind::Service, Some("dev.test.broken")));
    assert!(home.path().join("service-data/dev.test.broken").is_dir());
    client.call(methods::SERVICES_REMOVE, &ServiceRemoveParams { id: "dev.test.broken".into(), purge: Some(true) }).await.unwrap();
    assert!(!home.path().join("service-data/dev.test.broken").exists());

    // The keys setup stored, to look for in the log afterwards.
    let store: Value = serde_json::from_str(&std::fs::read_to_string(home.path().join("store/devices.json")).unwrap()).unwrap();
    let mut keys: Vec<String> = store["devices"].as_array().unwrap().iter().map(|d| d["key"].as_str().unwrap().to_owned()).collect();
    keys.push(companion.lines().find_map(|line| line.strip_prefix("key=")).expect("the pairing holds a key").to_owned());
    // The key Hub sent the Tab5 under its code, as the device unmasked and wrote it.
    keys.push(tab5_file.lines().find_map(|line| line.strip_prefix("key=")).expect("the Tab5's key file holds a key").to_owned());

    client.call(methods::SHUTDOWN, &json!({})).await.unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(10), core.wait()).await.expect("the core exits").unwrap();
    assert!(exit.success());
    assert!(!socket.exists(), "the socket is gone");
    assert!(!home.path().join("service.sock").exists());
    for pid in [service_pid, started.pid.unwrap(), restarted.pid.unwrap()] {
        assert!(gone(pid).await);
    }
    device.kill().await;
    // No link key in any line of the log, nor in any answer or event.
    let log = std::fs::read_to_string(home.path().join("logs/hubd.log")).unwrap();
    assert!(log.contains("Pocket Hub core stopped"));
    let lines: Vec<String> = transcript.lines().iter().chain(other_transcript.lines().iter()).chain(refused_transcript.lines().iter()).map(|(_, line)| line.to_string()).collect();
    for key in &keys {
        assert_eq!(key.len(), 64);
        assert!(!log.contains(key.as_str()), "a link key is in hubd.log");
        assert!(lines.iter().all(|line| !line.contains(key.as_str())), "a link key went over the control socket");
    }

    let machine = Machine {
        replacements: vec![
            (Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap().display().to_string(), "/path/to/pocket-hub".into()),
            (bun().display().to_string(), "/usr/local/bin/bun".into()),
            (home.path().display().to_string(), "/tmp/hub-home".into()),
            (work.path().display().to_string(), "/tmp/work".into()),
            (hello.hub.id.clone(), "fedcba9876543210".into()),
            (id.clone(), "0123456789abcdef".into()),
            (over_ftp.device.clone(), "00000000000000f1".into()),
            (tab5_id, "00000000000000a5".into()),
        ],
        bun_version,
    };
    examples(&[refused_transcript, transcript, other_transcript], &machine);
}

/// What makes a run this computer's and this moment's, replaced in the
/// examples file so that whoever regenerates it gets the same bytes.
struct Machine {
    /// Exact text and its stand-in: the repository's folder, the Bun binary,
    /// the run's temporary home and work folders, and the ids made in the run.
    replacements: Vec<(String, String)>,
    /// The system Bun's version, which the core reports for `--bun`.
    bun_version: String,
}

/// Every time in the file: 2026-01-01 00:00:00 UTC.
const FIXED_TIME: i64 = 1_767_225_600_000;

impl Machine {
    /// A string with this run's paths and ids replaced, and the numbers that
    /// differ between runs fixed: a pid in a service's log line, a loopback
    /// port (a peer, a test server), a screenshot's time in its file name.
    fn text(&self, text: &str) -> String {
        if text == self.bun_version {
            return pocket_hub_services::BUN_VERSION.to_owned();
        }
        let mut text = text.to_owned();
        for (from, to) in &self.replacements {
            text = text.replace(from.as_str(), to);
        }
        let text = number_after(&text, "pid ", "4242");
        let text = number_after(&text, "127.0.0.1:", "50000");
        number_after(&text, "/cache/screenshots/", &FIXED_TIME.to_string())
    }

    /// The value with `text` applied to every string, and the members whose
    /// numbers differ between runs fixed: times (`since`, `lastSeen`,
    /// `startedAt`, `fetchedAt`, `at`, and `expires`, a pairing request's 5
    /// minutes after its `since`), `pid`, and `port` (8628 for a carrier,
    /// 5000, ftpd's, for a console).
    fn value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(items) => items.iter_mut().for_each(|item| self.value(item)),
            Value::Object(map) => {
                // A carrier is `{kind, port}`; a setup's `{kind, host, port}` is a console's.
                let carrier = map.len() == 2 && map.contains_key("kind") && map.contains_key("port");
                for (key, member) in map.iter_mut() {
                    match (key.as_str(), member.is_number()) {
                        ("since" | "lastSeen" | "startedAt" | "fetchedAt" | "at", true) => *member = json!(FIXED_TIME),
                        ("expires", true) => *member = json!(FIXED_TIME + 300_000),
                        ("pid", true) => *member = json!(4242),
                        ("port", true) => *member = json!(if carrier { 8628 } else { 5000 }),
                        _ => self.value(member),
                    }
                }
            }
            _ => {}
        }
    }
}

/// `text` with every run of digits right after `prefix` replaced by `with`.
fn number_after(text: &str, prefix: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(prefix) {
        let (before, after) = rest.split_at(at + prefix.len());
        out.push_str(before);
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            out.push_str(with);
        }
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// The progress notifications kept for a method's example: the last one of
/// each phase (or stage), in the order the phases came. The last one ends a
/// phase with its byte count at `total` or at a step without a count; a
/// count that is neither 0 nor `total` (where the carrier happened to be
/// when the device's first event came) is written as 0.
fn progress_example(notifications: &[Value]) -> Vec<Value> {
    let mut kept: Vec<(String, Value)> = Vec::new();
    for line in notifications {
        let params = &line["params"];
        let phase = params.get("phase").or_else(|| params.get("stage")).and_then(Value::as_str).unwrap_or("").to_owned();
        match kept.iter_mut().find(|(seen, _)| *seen == phase) {
            Some((_, last)) => *last = line.clone(),
            None => kept.push((phase, line.clone())),
        }
    }
    kept.into_iter()
        .map(|(_, mut line)| {
            let params = &mut line["params"];
            if let (Some(done), Some(total)) = (params["done"].as_u64(), params["total"].as_u64())
                && done != 0
                && done != total
            {
                params["done"] = json!(0);
            }
            line
        })
        .collect()
}

/// The first JSON pointer at which `a` and `b` differ.
fn first_difference(a: &Value, b: &Value, at: String) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter().find_map(|key| match (x.get(key), y.get(key)) {
                (Some(x), Some(y)) => first_difference(x, y, format!("{at}/{key}")),
                _ => Some(format!("{at}/{key}")),
            })
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x.iter().zip(y).enumerate().find_map(|(i, (x, y))| first_difference(x, y, format!("{at}/{i}"))),
        _ => (a != b).then_some(at),
    }
}

/// Every request with its answer, every progress shape, event topic and
/// error kind of the run: one example of each, with what is this computer's
/// and this run's replaced (`Machine`). The file is checked whole: a run on
/// any computer on any day makes the same bytes.
fn examples(transcripts: &[Transcript], machine: &Machine) {
    let mut methods_seen: BTreeMap<String, Value> = BTreeMap::new();
    let mut errors: BTreeMap<String, Value> = BTreeMap::new();
    let mut events: BTreeMap<String, Value> = BTreeMap::new();
    let mut notifications: BTreeMap<String, Value> = BTreeMap::new();
    for transcript in transcripts {
        let lines = transcript.lines();
        let mut requests: BTreeMap<String, Value> = BTreeMap::new();
        let mut progress: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (sent, line) in &lines {
            let method = line.get("method").and_then(Value::as_str);
            match (sent, method) {
                (true, Some(_)) if line.get("id").is_some() => {
                    requests.insert(line["id"].to_string(), line.clone());
                }
                (true, Some(method)) => {
                    notifications.entry(method.to_owned()).or_insert_with(|| line.clone());
                }
                (false, Some("progress")) => {
                    let key = line["params"]["id"].to_string();
                    if let Some(request) = requests.get(&key) {
                        let method = request["method"].as_str().unwrap().to_owned();
                        // The first call of each method only: its example.
                        if !methods_seen.contains_key(&method) {
                            progress.entry(format!("{method} {key}")).or_default().push(line.clone());
                        }
                    }
                }
                (false, Some("event")) => {
                    // The fullest example of each topic: a device with its link, status, apps, busy and channels.
                    let richness = |line: &Value| {
                        let device = &line["params"]["device"];
                        ["link", "status", "apps", "busy"].iter().filter(|key| !device[**key].is_null()).count() + usize::from(device["channels"].as_array().is_some_and(|c| !c.is_empty()))
                    };
                    let topic = line["params"]["topic"].as_str().unwrap().to_owned();
                    if events.get(&topic).is_none_or(|kept| richness(line) > richness(kept)) {
                        events.insert(topic, line.clone());
                    }
                }
                (false, None) => {
                    let key = line["id"].to_string();
                    let Some(request) = requests.get(&key) else { continue };
                    let method = request["method"].as_str().unwrap().to_owned();
                    if let Some(error) = line.get("error") {
                        let kind = error["data"]["kind"].as_str().map(str::to_owned).unwrap_or_else(|| error["code"].to_string());
                        errors.entry(kind).or_insert_with(|| json!({"request": request, "response": line}));
                    } else {
                        methods_seen.entry(method.clone()).or_insert_with(|| {
                            let mut example = json!({"request": request, "response": line});
                            if let Some(progress) = progress.get(&format!("{method} {key}")) {
                                example["progress"] = json!(progress_example(progress));
                            }
                            example
                        });
                    }
                }
                _ => {}
            }
        }
    }
    let mut file = json!({
        "about": "Lines pocket-hubd wrote and read in crates/hubd/tests/e2e.rs (POCKET_HUB_UPDATE_EXAMPLES=1 cargo test -p pocket-hubd --test e2e writes this file again). One example per method (request, answer, and the last progress notification of each phase), notification a client sends, event topic and error kind or code. What belongs to the computer and the run that made it is replaced: paths (the repository is /path/to/pocket-hub, Hub's home /tmp/hub-home, the test's files /tmp/work), ids made in the run, times (2026-01-01), pids, loopback ports and byte counts between 0 and the total.",
        "methods": methods_seen,
        "notifications": notifications,
        "events": events,
        "errors": errors,
    });
    machine.value(&mut file);
    let text = serde_json::to_string_pretty(&file).unwrap() + "\n";
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/control-api-examples.json");
    if std::env::var_os("POCKET_HUB_UPDATE_EXAMPLES").is_some() {
        std::fs::write(&path, text).unwrap();
        return;
    }
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("docs/control-api-examples.json exists")).unwrap();
    // Every kept example decodes as the shared types say.
    for (topic, line) in kept["events"].as_object().unwrap() {
        serde_json::from_value::<pocket_hub_api::Event>(line["params"].clone()).unwrap_or_else(|error| panic!("{topic}: {error}"));
    }
    if let Some(at) = first_difference(&kept, &file, String::new()) {
        panic!("docs/control-api-examples.json is out of date at {at}; run with POCKET_HUB_UPDATE_EXAMPLES=1");
    }
}
