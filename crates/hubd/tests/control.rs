//! The control API's rules (docs/CONTROL-API.md) against a core in this
//! process: hello first, protocol errors, device selection, progress,
//! cancelling, the device mutex across clients, events against lists, and
//! the start rules (one core per home, a taken port, a long home).

mod common;

use std::collections::HashMap;
use std::time::Duration;

use common::*;
use pocket_hub_api::rpc::{INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR};
use pocket_hub_api::types::*;
use pocket_hub_api::{Client, ErrorKind, PROTOCOL, methods};
use pocket_hubd::StartError;
use serde_json::{Value, json};

fn chrono_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

/// Pairs a fresh card through `setup.card`; returns the card and the report.
async fn pair_card(client: &Client, dir: &std::path::Path, name: &str) -> (std::path::PathBuf, InstallReport) {
    let card = card(dir, name);
    let program = program(dir, "shell.3dsx", 4096);
    let report: InstallReport = client
        .call_as(methods::SETUP_CARD, &SetupCardParams { kind: "3ds".into(), path: card.display().to_string(), program: Some(ProgramChoice::Path { path: program.display().to_string() }) })
        .await
        .expect("the card is set up");
    (card, report)
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_first_and_protocol_errors() {
    let home = temp_home();
    let core = InProcess::start(test_options(home.path())).await;
    let raw = Client::connect_raw(&core.socket, None).await.unwrap();

    // Before hello: -32600, and the connection stays open.
    let error = rpc(raw.call(methods::STATUS, &json!({})).await.unwrap_err());
    assert_eq!(error.code, INVALID_REQUEST);
    let error = rpc(raw.call(methods::DEVICES_LIST, &json!({})).await.unwrap_err());
    assert_eq!(error.code, INVALID_REQUEST);
    let hello = raw.call(methods::HELLO, &HelloParams { protocol: PROTOCOL, client: info(ClientKind::Cli, "raw") }).await.unwrap();
    assert_eq!(hello["protocol"], PROTOCOL);
    assert_eq!(hello["hub"]["version"], pocket_hubd::VERSION);
    assert_eq!(rpc(raw.call(methods::HELLO, &HelloParams { protocol: PROTOCOL, client: info(ClientKind::Cli, "raw") }).await.unwrap_err()).code, INVALID_REQUEST);

    assert_eq!(rpc(raw.call("devices.nothing", &json!({})).await.unwrap_err()).code, METHOD_NOT_FOUND);
    assert_eq!(rpc(raw.call(methods::DEVICES_GET, &json!({"device": 7})).await.unwrap_err()).code, INVALID_PARAMS);
    assert_eq!(rpc(raw.call(methods::SUBSCRIBE, &json!({"topics": ["no.such"]})).await.unwrap_err()).code, INVALID_PARAMS);
    // A cancel for an id no call has is ignored; a line that is not JSON is a parse error.
    raw.cancel(999).await.unwrap();
    raw.send_line("{not json").await.unwrap();
    assert_eq!(raw.call(methods::DEVICES_LIST, &json!({})).await.unwrap(), json!({"devices": []}));
    let status: HubStatus = raw.call_as(methods::STATUS, &json!({})).await.unwrap();
    assert_eq!(status.carriers, vec![Carrier { kind: "tcp".into(), port: Some(core.core.link_port) }]);
    assert_ne!(core.core.link_port, 0);

    // A hello of another protocol, or with a bad kind, is refused. Protocol 1
    // is Pocket Hub 0.2.2's and earlier, before services took their names:
    // one sentence that names both versions.
    let other = Client::connect_raw(&core.socket, None).await.unwrap();
    let old = rpc(other.call(methods::HELLO, &json!({"protocol": 1, "client": {"kind": "gui", "name": "Pocket Hub", "version": "0.2.2"}})).await.unwrap_err());
    assert_eq!((old.code, old.message.as_str()), (INVALID_PARAMS, "This Hub speaks protocol 2, not 1."));
    assert_eq!(rpc(other.call(methods::HELLO, &json!({"protocol": 3, "client": {"kind": "cli", "name": "x", "version": "1"}})).await.unwrap_err()).code, INVALID_PARAMS);
    assert_eq!(rpc(other.call(methods::HELLO, &json!({"protocol": PROTOCOL, "client": {"kind": "robot", "name": "x", "version": "1"}})).await.unwrap_err()).code, INVALID_PARAMS);

    // The raw parse error answer, read off a socket by hand.
    {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let mut stream = tokio::net::UnixStream::connect(&core.socket).await.unwrap();
        stream.write_all(b"[1,2\n").await.unwrap();
        let mut line = String::new();
        BufReader::new(&mut stream).read_line(&mut line).await.unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(answer["error"]["code"], PARSE_ERROR);
        assert_eq!(answer["id"], Value::Null);
    }
    core.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn device_selection_with_zero_one_and_two_devices() {
    let home = temp_home();
    let work = temp_home();
    let core = InProcess::start(test_options(home.path())).await;
    let client = client(&core.socket, "selection").await;
    let star = DeviceParams { device: "*".into() };

    assert_eq!(kind_of(client.call(methods::DEVICES_GET, &star).await.unwrap_err()), ErrorKind::NotFound);
    assert_eq!(kind_of(client.call(methods::DEVICES_GET, &DeviceParams { device: "0123456789abcdef".into() }).await.unwrap_err()), ErrorKind::NotFound);

    let (_, first) = pair_card(&client, work.path(), "card-1").await;
    assert_eq!(first.pairing, PairingResult::Key);
    assert_eq!(first.program.action, ProgramAction::Installed);
    assert!(first.confirmed);
    let device: Device = client.call_as(methods::DEVICES_GET, &star).await.unwrap();
    assert_eq!(device.id, first.device);
    assert!(device.link.is_none() && device.busy.is_none() && device.channels.is_empty());
    // Paired and not linked.
    assert_eq!(kind_of(client.call(methods::DEVICES_STATUS, &star).await.unwrap_err()), ErrorKind::NotLinked);
    assert_eq!(kind_of(client.call(methods::DEVICES_SCREENSHOT, &ScreenshotParams { device: "*".into(), path: None }).await.unwrap_err()), ErrorKind::NotLinked);
    // An unusable argument: a relative path.
    assert_eq!(kind_of(client.call(methods::DEVICES_PUSH, &PathParams { device: "*".into(), path: "relative.pocket".into() }).await.unwrap_err()), ErrorKind::Invalid);

    let (_, second) = pair_card(&client, work.path(), "card-2").await;
    assert_ne!(second.device, first.device);
    let error = rpc(client.call(methods::DEVICES_GET, &star).await.unwrap_err());
    assert_eq!(error.kind(), Some(ErrorKind::Invalid));
    assert!(error.message.contains("2 devices"), "{}", error.message);
    let list: DeviceList = client.call_as(methods::DEVICES_LIST, &json!({})).await.unwrap();
    assert_eq!(list.devices.len(), 2);

    client.call(methods::DEVICES_FORGET, &DeviceParams { device: second.device.clone() }).await.unwrap();
    assert_eq!(kind_of(client.call(methods::DEVICES_FORGET, &DeviceParams { device: second.device }).await.unwrap_err()), ErrorKind::NotFound);
    let device: Device = client.call_as(methods::DEVICES_GET, &star).await.unwrap();
    assert_eq!(device.id, first.device);

    // A setup that fails is the error `setup`, never a report.
    let error = rpc(client
        .call(methods::SETUP_CARD, &SetupCardParams { kind: "3ds".into(), path: work.path().join("no-card").display().to_string(), program: None })
        .await
        .unwrap_err());
    assert_eq!(error.kind(), Some(ErrorKind::Setup));
    let data = error.data.as_ref().unwrap();
    assert_eq!((data.step.as_deref(), data.reason.as_deref()), (Some("connecting"), Some("card")));
    assert!(data.detail.as_deref().is_some_and(|detail| !detail.is_empty()), "{data:?}");
    assert_eq!(kind_of(client.call(methods::SETUP_CARD, &json!({"kind": "tab5", "path": "/x"})).await.unwrap_err()), ErrorKind::Invalid);
    core.stop().await;
}

/// The device mutex across clients: the holder is the first client's
/// `{kind, name, method}`, a second state-changing call waits and gets
/// `busy` naming it, `cancel` ends a call with `cancelled`, and a client
/// that disconnects releases what it held.
#[tokio::test(flavor = "multi_thread")]
async fn progress_cancel_busy_and_disconnect() {
    let home = temp_home();
    let work = temp_home();
    let core = InProcess::start(test_options(home.path())).await;
    let first = client(&core.socket, "first").await;
    let (card, report) = pair_card(&first, work.path(), "card").await;
    first.call(methods::SUBSCRIBE, &SubscribeParams { topics: Some(vec!["device.changed".into()]) }).await.unwrap();
    let mut device = RefDevice::spawn(&link_dir(&card), &core.core.store.hub_id(), core.core.link_port).await;
    event(&first, |e| topic(e, "device.changed") && !e["device"]["link"].is_null()).await;

    // A push with progress: the phase and the bytes, up to the total.
    let small = package(work.path(), "small.pocket", 200 * 1024, 1);
    let mut call = first.start(methods::DEVICES_PUSH, &PathParams { device: report.device.clone(), path: small.display().to_string() }).await.unwrap();
    let mut seen = Vec::new();
    while let Some(progress) = call.progress().await {
        seen.push(serde_json::from_value::<TransferProgress>(progress).unwrap());
    }
    let pushed: PushResult = serde_json::from_value(call.finish().await.unwrap()).unwrap();
    assert_eq!(pushed.app.id, "dev.demo");
    assert_eq!(seen.first().unwrap().done, 0);
    assert_eq!(seen.last().unwrap().done, 200 * 1024);
    assert!(seen.iter().all(|p| p.total == 200 * 1024));
    let phases: Vec<&str> = seen.iter().map(|p| p.phase.as_str()).collect();
    assert!(phases.contains(&"receiving") && phases.contains(&"staged"), "{phases:?}");
    device.expect("pushed").await;

    // The device stops answering (SIGSTOP keeps its socket open and silent):
    // a push holds the mutex.
    signal(device.pid(), SIGSTOP);
    let big = package(work.path(), "big.pocket", 2 * 1024 * 1024, 1);
    let started = chrono_now();
    let held = first.start(methods::DEVICES_PUSH, &PathParams { device: "*".into(), path: big.display().to_string() }).await.unwrap();
    // The busy event of this push, not the small one's still queued.
    let busy = event(&first, |e| topic(e, "device.changed") && e["device"]["busy"]["since"].as_i64().is_some_and(|since| since >= started)).await;
    let holder = Holder { kind: ClientKind::Gui, name: "first".into(), method: "devices.push".into() };
    assert_eq!(serde_json::from_value::<Holder>(busy["device"]["busy"]["holder"].clone()).unwrap(), holder);

    let second = client(&core.socket, "second").await;
    let error = rpc(second.call(methods::DEVICES_LAUNCH, &LaunchParams { device: "*".into(), app: "demo".into() }).await.unwrap_err());
    assert_eq!(error.kind(), Some(ErrorKind::Busy), "{error:?}");
    assert_eq!(error.data.unwrap().holder, Some(holder));

    first.cancel(held.id).await.unwrap();
    assert_eq!(kind_of(held.finish().await.unwrap_err()), ErrorKind::Cancelled);
    event(&first, |e| topic(e, "device.changed") && e["device"]["busy"].is_null()).await;

    // A third client takes the mutex and goes away: the mutex is released.
    let third = client(&core.socket, "third").await;
    let started = chrono_now();
    let _held = third.start(methods::DEVICES_PUSH, &PathParams { device: "*".into(), path: big.display().to_string() }).await.unwrap();
    let busy = event(&first, |e| topic(e, "device.changed") && e["device"]["busy"]["since"].as_i64().is_some_and(|since| since >= started)).await;
    assert_eq!(busy["device"]["busy"]["holder"]["name"], "third");
    drop(_held);
    drop(third);
    event(&first, |e| topic(e, "device.changed") && e["device"]["busy"].is_null()).await;

    signal(device.pid(), SIGCONT);
    let launched: LaunchResult = second.call_as(methods::DEVICES_LAUNCH, &LaunchParams { device: "*".into(), app: "demo".into() }).await.unwrap();
    assert_eq!(launched.output, "demo");
    // No such app: `not_found`.
    assert_eq!(kind_of(second.call(methods::DEVICES_LAUNCH, &LaunchParams { device: "*".into(), app: "missing".into() }).await.unwrap_err()), ErrorKind::NotFound);
    device.kill().await;
    core.stop().await;
}

/// A client that subscribes and then lists holds the truth: the list equals
/// the last `device.changed` of each device seen before its answer, and the
/// events after it change nothing that the list did not already show.
#[tokio::test(flavor = "multi_thread")]
async fn events_resync_with_lists() {
    let home = temp_home();
    let work = temp_home();
    let core = InProcess::start(test_options(home.path())).await;
    let client = client(&core.socket, "watcher").await;
    client.call(methods::SUBSCRIBE, &SubscribeParams { topics: None }).await.unwrap();
    let (card, report) = pair_card(&client, work.path(), "card").await;
    let mut device = RefDevice::spawn(&link_dir(&card), &core.core.store.hub_id(), core.core.link_port).await;
    event(&client, |e| topic(e, "device.changed") && !e["device"]["link"].is_null()).await;
    device.send(json!({"cmd": "apps", "count": 3})).await;
    device.send(json!({"cmd": "status"})).await;
    let actor = common::client(&core.socket, "actor").await;
    let small = package(work.path(), "p.pocket", 4096, 1);
    actor.call(methods::DEVICES_PUSH, &PathParams { device: "*".into(), path: small.display().to_string() }).await.unwrap();
    event(&client, |e| topic(e, "device.changed") && e["device"]["apps"].as_array().is_some_and(|a| a.len() == 3)).await;

    let mut accumulated: HashMap<String, Value> = HashMap::new();
    let list = client.call(methods::DEVICES_LIST, &json!({})).await.unwrap();
    while let Some(event) = client.try_event() {
        if topic(&event, "device.changed") {
            accumulated.insert(event["device"]["id"].as_str().unwrap().to_owned(), event["device"].clone());
        }
    }
    let listed: HashMap<String, Value> = list["devices"].as_array().unwrap().iter().map(|d| (d["id"].as_str().unwrap().to_owned(), d.clone())).collect();
    assert_eq!(listed.keys().collect::<Vec<_>>(), vec![&report.device]);
    assert_eq!(accumulated.get(&report.device), listed.get(&report.device), "the list equals the last device.changed before it");
    device.kill().await;
    event(&client, |e| topic(e, "device.unlinked")).await;
    let unlinked: Device = client.call_as(methods::DEVICES_GET, &DeviceParams { device: report.device.clone() }).await.unwrap();
    assert!(unlinked.link.is_none());
    core.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_core_per_home_a_taken_port_and_a_long_home() {
    let home = temp_home();
    let core = InProcess::start(test_options(home.path())).await;
    assert!(matches!(pocket_hubd::start(test_options(home.path())).await, Err(StartError::AlreadyRunning(_))));
    core.stop().await;
    assert!(!home.path().join("hubd.sock").exists());

    // A port that is taken stops the start; nothing listens elsewhere.
    let taken = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let other = temp_home();
    let mut options = test_options(other.path());
    options.link_port = Some(port);
    let error = pocket_hubd::start(options).await.err().expect("the start fails");
    assert!(matches!(error, StartError::PortInUse { port: p } if p == port), "{error}");
    assert!(error.to_string().contains(&port.to_string()));
    assert!(!other.path().join("hubd.sock").exists());

    // A program holding the port on the loopback alone is found too.
    let loopback = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = loopback.local_addr().unwrap().port();
    let mut options = test_options(other.path());
    options.link_port = Some(port);
    assert!(matches!(pocket_hubd::start(options).await, Err(StartError::PortInUse { port: p }) if p == port));
    drop(loopback);

    let long = other.path().join("x".repeat(100));
    let error = pocket_hubd::start(test_options(&long)).await.err().expect("a long home is refused");
    assert!(matches!(error, StartError::Home(_)), "{error}");
    drop(taken);
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_install_over_ftp_against_the_fake_ftpd() {
    use pocket_hub_setup::test_support::FakeFtp;
    let home = temp_home();
    let work = temp_home();
    let console = work.path().join("console");
    let ftp = FakeFtp::start(&console).await;
    let mut options = test_options(home.path());
    options.discover_targets = Some(vec![std::net::Ipv4Addr::LOCALHOST]);
    options.discover_port = Some(ftp.address.port());
    let core = InProcess::start(options).await;
    let client = client(&core.socket, "setup").await;

    let found: DiscoverResult = client.call_as(methods::SETUP_DISCOVER, &KindParams { kind: "3ds".into() }).await.unwrap();
    assert_eq!(found.candidates, vec![Candidate { address: "127.0.0.1".into(), port: ftp.address.port(), banner: "Hello!".into(), ftpd: true }]);

    let shell = program(work.path(), "pocket-shell.3dsx", 64 * 1024);
    let mut call = client
        .start(methods::SETUP_INSTALL, &SetupInstallParams { kind: "3ds".into(), host: "127.0.0.1".into(), port: Some(ftp.address.port()), program: Some(ProgramChoice::Path { path: shell.display().to_string() }) })
        .await
        .unwrap();
    let mut stages = Vec::new();
    while let Some(progress) = call.progress().await {
        let progress: SetupProgress = serde_json::from_value(progress).unwrap();
        assert!(!progress.message.is_empty());
        if stages.last() != Some(&progress.stage) {
            stages.push(progress.stage);
        }
    }
    let report: InstallReport = serde_json::from_value(call.finish().await.unwrap()).unwrap();
    assert_eq!(report.pairing, PairingResult::Key);
    assert_eq!(report.program.action, ProgramAction::Installed);
    assert!(report.confirmed);
    assert!(report.next.contains("ftpd"), "{}", report.next);
    assert_eq!(stages.first(), Some(&SetupStage::Connecting));
    assert_eq!(stages.last(), Some(&SetupStage::Done));
    for stage in [SetupStage::Reading, SetupStage::Uploading, SetupStage::Pairing] {
        assert!(stages.contains(&stage), "{stages:?}");
    }
    // The files on the console: the program, the device id and this Hub's pairing naming its link port.
    assert_eq!(std::fs::read(console.join("3ds/pocket-shell.3dsx")).unwrap(), std::fs::read(&shell).unwrap());
    let device_file = std::fs::read_to_string(console.join("pocketjs/link/device.txt")).unwrap();
    assert!(device_file.contains(&report.device));
    let companion = std::fs::read_to_string(console.join(format!("pocketjs/link/companions/{}.txt", core.core.store.hub_id()))).unwrap();
    assert!(companion.contains(&format!("port={}", core.core.link_port)), "{companion}");
    let device: Device = client.call_as(methods::DEVICES_GET, &DeviceParams { device: report.device.clone() }).await.unwrap();
    assert!(!device.setup_unconfirmed);

    // Again: the pairing and the program are in place.
    let again: InstallReport = client.call_as(methods::SETUP_INSTALL, &SetupInstallParams { kind: "3ds".into(), host: "127.0.0.1".into(), port: Some(ftp.address.port()), program: Some(ProgramChoice::Path { path: shell.display().to_string() }) }).await.unwrap();
    assert_eq!((again.pairing, again.program.action, again.device), (PairingResult::Kept, ProgramAction::Kept, report.device));

    // A console that does not answer.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    drop(closed);
    let error = rpc(client.call(methods::SETUP_INSTALL, &SetupInstallParams { kind: "3ds".into(), host: "127.0.0.1".into(), port: Some(closed_port), program: None }).await.unwrap_err());
    let data = error.data.clone().unwrap();
    assert_eq!((data.kind, data.step.as_deref(), data.reason.as_deref()), (ErrorKind::Setup, Some("connecting"), Some("unreachable")));
    let _ = Duration::ZERO;
    core.stop().await;
}

/// Device logs are Hub's to keep on (§3 `devices.logs`): on while a
/// connected client wants them, on again after a relink, off when the last
/// such client turns them off or goes away.
#[tokio::test(flavor = "multi_thread")]
async fn device_logs_stay_on_while_a_client_wants_them() {
    let home = temp_home();
    let work = temp_home();
    let core = InProcess::start(test_options(home.path())).await;
    let a = client(&core.socket, "a").await;
    let (card, report) = pair_card(&a, work.path(), "card").await;
    a.call(methods::SUBSCRIBE, &SubscribeParams { topics: Some(vec!["device.changed".into(), "device.log".into()]) }).await.unwrap();
    let on = |on: bool| LogsParams { device: report.device.clone(), on };
    // Asked while the device is not linked: accepted, and the lines start at its link.
    a.call(methods::DEVICES_LOGS, &on(true)).await.unwrap();
    let mut device = RefDevice::spawn(&link_dir(&card), &core.core.store.hub_id(), core.core.link_port).await;
    event(&a, |e| topic(e, "device.changed") && !e["device"]["link"].is_null()).await;
    let b = client(&core.socket, "b").await;

    // The device says whether a line went out: only while logs are on.
    async fn logged(device: &mut RefDevice, text: &str) -> bool {
        device.send(json!({"cmd": "log", "text": text})).await;
        device.expect("logged").await["sent"].as_bool().unwrap()
    }
    /// Asks until the device's answer is `want`, within `WAIT`: Hub turns
    /// logs on and off asynchronously to the link and the clients.
    async fn logged_becomes(device: &mut RefDevice, text: &str, want: bool) -> bool {
        let deadline = tokio::time::Instant::now() + WAIT;
        while tokio::time::Instant::now() < deadline {
            if logged(device, text).await == want {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }
    assert!(logged_becomes(&mut device, "one", true).await, "logs asked for before the link are on after it");
    b.call(methods::DEVICES_LOGS, &on(true)).await.unwrap();
    let line = event(&a, |e| topic(e, "device.log")).await;
    assert_eq!((line["id"].as_str(), line["text"].as_str()), (Some(report.device.as_str()), Some("one")));

    // a no longer wants them; b still does.
    a.call(methods::DEVICES_LOGS, &on(false)).await.unwrap();
    assert!(logged(&mut device, "two").await);

    // A relink (the device program starts again on the same card): Hub
    // turns them on again for b.
    device.kill().await;
    event(&a, |e| topic(e, "device.changed") && e["device"]["link"].is_null()).await;
    let mut device = RefDevice::spawn(&link_dir(&card), &core.core.store.hub_id(), core.core.link_port).await;
    event(&a, |e| topic(e, "device.changed") && !e["device"]["link"].is_null()).await;
    assert!(logged_becomes(&mut device, "three", true).await, "logs are on again after the relink");

    // b goes away: no client wants them, they go off.
    drop(b);
    assert!(logged_becomes(&mut device, "four", false).await, "logs go off with the last client that wanted them");
    device.kill().await;
    core.stop().await;
}

/// `program.release` takes a tag, looked up on the configured releases (a
/// GitHub-shaped server on loopback here); the report carries the release's
/// version and runtime. A tag the server lacks is `release_unavailable`; a
/// core whose release is a local file has no tags.
#[tokio::test(flavor = "multi_thread")]
async fn a_release_tag_for_setup() {
    use pocket_hub_setup::release::sha256_hex;
    use pocket_hub_setup::test_support::FakeHttp;
    let name = "pocket-shell-v9.3dsx";
    let shell = pocket_hub_setup::test_support::fake_3dsx(20_000, 3);
    let sums = format!("{}  {name}\n", sha256_hex(&shell));
    let info = format!(r#"{{"product":"pocket-shell","version":"9.0.0","source":"aaaa","runtime":"cafe","files":[{{"name":"{name}","sha256":"{}"}}]}}"#, sha256_hex(&shell));
    let release = format!(
        r#"{{"tag_name":"v9","assets":[{{"name":"build-info.json","size":{},"browser_download_url":"{{base}}/dl/build-info.json"}},{{"name":"{name}","size":{},"browser_download_url":"{{base}}/dl/{name}"}},{{"name":"SHA256SUMS","size":{},"browser_download_url":"{{base}}/dl/SHA256SUMS"}}]}}"#,
        info.len(),
        shell.len(),
        sums.len()
    );
    let http = FakeHttp::start(vec![
        ("/repos/test/shell/releases/tags/v9".into(), release.into_bytes()),
        (format!("/dl/{name}"), shell.clone()),
        ("/dl/SHA256SUMS".into(), sums.into_bytes()),
        ("/dl/build-info.json".into(), info.into_bytes()),
    ])
    .await;
    let home = temp_home();
    let work = temp_home();
    let mut options = test_options(home.path());
    options.shell_source = Some(pocket_hub_setup::ShellSource::GitHub { api: http.base.clone(), repo: "test/shell".into(), tag: None });
    let core = InProcess::start(options).await;
    let client = client(&core.socket, "release").await;
    let release_of = |tag: &str, card: &std::path::Path| SetupCardParams { kind: "3ds".into(), path: card.display().to_string(), program: Some(ProgramChoice::Release { release: tag.into() }) };

    let sd = card(work.path(), "sd");
    let report: InstallReport = client.call_as(methods::SETUP_CARD, &release_of("v9", &sd)).await.unwrap();
    assert_eq!(report.program, ProgramInfo { action: ProgramAction::Installed, version: Some("9.0.0".into()), runtime: Some("cafe".into()) });
    assert_eq!(std::fs::read(sd.join("3ds/pocket-shell.3dsx")).unwrap(), shell);

    let other = card(work.path(), "other");
    let error = rpc(client.call(methods::SETUP_CARD, &release_of("v404", &other)).await.unwrap_err());
    let data = error.data.unwrap();
    assert_eq!((data.kind, data.step.as_deref(), data.reason.as_deref()), (ErrorKind::Setup, Some("downloading"), Some("release_unavailable")));
    assert_eq!(kind_of(client.call(methods::SETUP_CARD, &release_of("../x", &other)).await.unwrap_err()), ErrorKind::Invalid);
    core.stop().await;

    // A core whose release is a local file has no tags.
    let home = temp_home();
    let mut options = test_options(home.path());
    options.shell_source = Some(pocket_hub_setup::ShellSource::File { path: program(work.path(), "local.3dsx", 1000) });
    let core = InProcess::start(options).await;
    let client = common::client(&core.socket, "release").await;
    assert_eq!(kind_of(client.call(methods::SETUP_CARD, &release_of("v9", &card(work.path(), "third"))).await.unwrap_err()), ErrorKind::Invalid);
    core.stop().await;
}
