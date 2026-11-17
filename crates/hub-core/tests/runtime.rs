//! Hub's endpoint against the reference device (`tools/pocket-link/v2/device.ts`)
//! over loopback TCP: the handshake, the runtime operations of §6.2 and the
//! device mutex of §8.3.

mod common;

use std::time::Duration;

use common::*;
use pocket_hub_core::{HubEvent, LaunchTarget, OpError};
use serde_json::{Value, json};

fn package(len: usize, first: u8) -> bytes::Bytes {
    let mut bytes = pattern(len - 8);
    bytes[0] = first;
    let hash = pocket_hub_core::ops::fnv1a64(&bytes);
    bytes.extend_from_slice(&hash.to_le_bytes());
    bytes.into()
}

#[tokio::test(flavor = "multi_thread")]
async fn handshake_status_and_record() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    let linked = device.expect("linked").await;
    assert_eq!(linked["hub"], hub.store.hub_id());
    assert_eq!(linked["limit"], 16384);
    // ready.clock: utc now, local in Hub's zone.
    let utc = linked["clock"]["utc"].as_i64().unwrap();
    assert!((chrono::Utc::now().timestamp_millis() - utc).abs() < 10_000);
    hub.linked(&id).await;

    // The record took the hello's fields; "setup not confirmed" is settled.
    let record = hub.store.device(&id).unwrap();
    assert_eq!((record.name.as_str(), record.model.as_str(), record.target.as_str(), record.host_abi), ("Test 3DS", "New Nintendo 3DS XL", "3ds-dev", 7));
    assert_eq!(record.last_address.as_deref(), Some("127.0.0.1"));
    assert!(record.last_seen.is_some() && !record.setup_unconfirmed);

    // The status event the device sends after ready is cached.
    let handle = hub.endpoint.device(&id).unwrap();
    hub.event(|e| matches!(e, HubEvent::Status { .. })).await;
    assert_eq!(handle.cached_status().unwrap()["phase"], "booted");
    let status = handle.status().await.unwrap();
    assert_eq!(status["frame"], 1234);
    assert_eq!(status["app"]["output"], "demo");

    let snapshot = hub.endpoint.devices().into_iter().find(|d| d.id == id).unwrap();
    assert!(snapshot.linked);
    let link = snapshot.link.unwrap();
    assert_eq!(link.payload_limit, 16384);
    assert!(link.features_in_use.contains(&"runtime".to_owned()));
    assert!(!link.hub_features.contains(&"channel".to_owned()), "no service connector, no channel feature");
    // No key in anything a client sees.
    let text = serde_json::to_string(&hub.endpoint.devices()).unwrap();
    assert!(!text.contains(&record.key));
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn apps_reports_and_pages() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    device.send(json!({"cmd":"apps","count":3})).await;
    let apps = hub.event(|e| matches!(e, HubEvent::Apps { .. })).await;
    let HubEvent::Apps { apps, .. } = apps else { unreachable!() };
    assert_eq!(apps.len(), 3);
    assert_eq!(apps[2].output, "app2");
    // 64 rows at every limit do not fit 16 KiB: the report comes in two pages.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    device.send(json!({"cmd":"apps","count":64,"big":true})).await;
    let HubEvent::Apps { apps, .. } = hub.event(|e| matches!(e, HubEvent::Apps { .. })).await else { unreachable!() };
    assert_eq!(apps.len(), 64);
    assert_eq!(apps[63].id.len(), 128);
    assert_eq!(hub.endpoint.device(&id).unwrap().apps().unwrap().len(), 64);
    device.kill().await;
}

/// `runtime.remove` (§6.2): Hub deletes an installed applet under the mutex,
/// and the device's next report no longer lists it. The device refuses its
/// launcher and the running applet with `rejected` and a `reason`, and an
/// applet it does not hold with `not_found`.
#[tokio::test(flavor = "multi_thread")]
async fn remove_an_applet_and_the_refusals() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    device.send(json!({"cmd":"apps","count":2,"home":"2026.11.2"})).await;
    hub.event(|e| matches!(e, HubEvent::Apps { apps, .. } if apps.len() == 3)).await;
    let handle = hub.endpoint.device(&id).unwrap();

    let reply = handle.remove("cli", "dev.app.0").await.unwrap();
    assert_eq!(reply["id"], "dev.app.0");
    assert_eq!(device.expect("removed").await["id"], "dev.app.0");
    let HubEvent::Apps { apps, .. } = hub.event(|e| matches!(e, HubEvent::Apps { apps, .. } if apps.len() == 2)).await else { unreachable!() };
    assert!(apps.iter().all(|app| app.id != "dev.app.0"));
    assert!(handle.snapshot().unwrap().mutex_holder.is_none(), "the mutex is released at the reply");

    let reason = |error: OpError| match error {
        OpError::Link { code, extra, .. } => (code, extra.get("reason").and_then(Value::as_str).map(str::to_owned)),
        other => panic!("not a link error: {other:?}"),
    };
    let home = handle.remove("cli", "dev.pocket-nexus.shell").await.unwrap_err();
    assert_eq!(reason(home), ("rejected".into(), Some("home".into())));
    let running = handle.remove("cli", "dev.demo").await.unwrap_err();
    assert_eq!(reason(running), ("rejected".into(), Some("running".into())));
    let missing = handle.remove("cli", "dev.app.9").await.unwrap_err();
    assert_eq!(reason(missing), ("not_found".into(), None));
    // An id over the link's 128 bytes is refused before anything is sent.
    assert!(matches!(handle.remove("cli", &"x".repeat(129)).await, Err(OpError::Invalid { .. })));
    assert!(matches!(handle.remove("cli", "").await, Err(OpError::Invalid { .. })));
    device.kill().await;
}

/// A device whose software predates `runtime.remove` does not list it: Hub
/// refuses locally, and the device is never sent the request (§6.1).
#[tokio::test(flavor = "multi_thread")]
async fn remove_needs_the_group() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let _device = Device::spawn(&hub, &dir, json!({"features":["runtime","runtime.native","apps"]})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    assert!(!handle.snapshot().unwrap().link.unwrap().features_in_use.contains(&"runtime.remove".to_owned()));
    let error = handle.remove("cli", "dev.app.0").await.unwrap_err();
    assert_eq!(error, OpError::Unsupported { feature: "runtime.remove".into() });
}

#[tokio::test(flavor = "multi_thread")]
async fn push_with_progress_and_a_rejected_push() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();

    let file = package(320_000, 1);
    let (progress_tx, mut progress) = tokio::sync::mpsc::unbounded_channel();
    let reply = handle.push("test", file.clone(), Some(progress_tx)).await.unwrap();
    assert_eq!(reply["phase"], "accepted");
    let pushed = device.expect("pushed").await;
    assert_eq!(pushed["bytes"], 320_000);
    assert_eq!(pushed["sum"].as_u64().unwrap() as u32, fnv32(&file));
    assert_eq!(pushed["hash"].as_str().unwrap(), pocket_hub_core::ops::package_hash(&file).unwrap());
    let mut phases = Vec::new();
    while let Ok(event) = progress.try_recv() {
        phases.push(event["phase"].as_str().unwrap().to_owned());
    }
    assert_eq!(phases, ["receiving", "received", "staged"]);

    let error = handle.push("test", package(4096, 0xee), None).await.unwrap_err();
    let OpError::Link { code, extra, .. } = error else { panic!("{error:?}") };
    assert_eq!(code, "rejected");
    assert_eq!(extra["phase"], "rejected");
    // A package whose footer does not match is refused before anything is sent.
    let mut broken = package(4096, 1).to_vec();
    broken[5] ^= 1;
    assert!(matches!(handle.push("test", broken.into(), None).await, Err(OpError::Invalid { .. })));
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn native_install_launching_restart_and_relink() {
    let mut config = fast_config();
    config.timing.mutex_wait = Duration::from_millis(200);
    let mut hub = Hub::start(config, None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();

    let mut file = pattern(70_000);
    file[..4].copy_from_slice(b"3DSX");
    let reply = handle.native_install("cli", "pocket.3dsx", file.clone().into(), false, None).await.unwrap();
    assert_eq!(reply["phase"], "installed");
    let native = device.expect("native").await;
    assert_eq!(native["sum"].as_u64().unwrap() as u32, fnv32(&file));

    // A bad name never reaches the device.
    assert!(matches!(handle.native_install("cli", ".x.3dsx", file.clone().into(), false, None).await, Err(OpError::Invalid { .. })));

    let reply = handle.native_install("cli", "pocket.3dsx", file.clone().into(), true, None).await.unwrap();
    assert_eq!(reply["phase"], "launching");
    // The device closes with `restart` after the REPLY.
    let unlinked = hub.event(|e| matches!(e, HubEvent::Unlinked { .. })).await;
    let HubEvent::Unlinked { close, .. } = unlinked else { unreachable!() };
    assert_eq!((close.by, close.code.as_deref()), ("device", Some("restart")));
    // The mutex is held until the device links again.
    assert!(handle.snapshot().unwrap().mutex_holder.unwrap().contains("launching"));
    assert!(matches!(handle.launch("ui", LaunchTarget::App("demo".into())).await, Err(OpError::Busy { holder }) if holder.contains("cli")));
    device.send(json!({"cmd":"connect"})).await;
    hub.linked(&id).await;
    assert_eq!(handle.snapshot().unwrap().mutex_holder, None);
    let reply = handle.launch("ui", LaunchTarget::App("demo".into())).await.unwrap();
    assert_eq!(reply["phase"], "switching");
    let error = handle.launch("ui", LaunchTarget::App("missing".into())).await.unwrap_err();
    assert_eq!(error.code(), Some("not_found"));
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn screenshot_png_and_the_unsolicited_one() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();

    let shot = handle.screenshot().await.unwrap();
    assert_eq!((shot.width, shot.height, shot.frame), (400, 480, 99));
    let decoder = png::Decoder::new(std::io::Cursor::new(shot.png.clone()));
    let mut reader = decoder.read_info().unwrap();
    let mut rgb = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut rgb).unwrap();
    assert_eq!((info.width, info.height), (400, 480));
    // Top-left pixel of the top screen: column 0, the last row of the rotated buffer.
    let source = (240 - 1) * 3;
    assert_eq!(&rgb[..3], &[((source + 2) & 0xff) as u8, ((source + 1) & 0xff) as u8, (source & 0xff) as u8]);

    device.send(json!({"cmd":"shot"})).await;
    let event = hub.event(|e| matches!(e, HubEvent::Screenshot { .. })).await;
    let HubEvent::Screenshot { width, height, .. } = event else { unreachable!() };
    assert_eq!((width, height), (400, 480));
    assert_eq!(handle.latest_screenshot().unwrap().png.len(), shot.png.len());
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn logs_on_and_off() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    // Logs are off at every new connection.
    device.send(json!({"cmd":"log","text":"early"})).await;
    assert_eq!(device.expect("logged").await["sent"], false);
    handle.logs(true).await.unwrap();
    device.send(json!({"cmd":"log","level":"warn","source":"guest","text":"hello log","frame":42})).await;
    let event = hub.event(|e| matches!(e, HubEvent::Log { .. })).await;
    let HubEvent::Log { level, source, text, frame, .. } = event else { unreachable!() };
    assert_eq!((level.as_str(), source.as_str(), text.as_str(), frame), ("warn", "guest", "hello log", 42));
    handle.logs(false).await.unwrap();
    device.send(json!({"cmd":"log","text":"late"})).await;
    assert_eq!(device.wait(|v| v["ev"] == "logged" && v["sent"] == false).await["sent"], false);
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mutex_contention_names_the_holder() {
    let mut config = fast_config();
    config.timing.mutex_wait = Duration::from_millis(300);
    let mut hub = Hub::start(config, None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();

    // A push holds the mutex; the device is stopped (SIGSTOP keeps its socket
    // open and silent), so the push stays open while a second caller waits.
    let pid = device.pid();
    unsafe_kill(pid, "STOP");
    let big = package(2 * 1024 * 1024, 1);
    let first = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.push("studio relay", big, None).await })
    };
    let mut held = false;
    for _ in 0..200 {
        if handle.snapshot().unwrap().mutex_holder.as_deref() == Some("studio relay") {
            held = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(held, "the push takes the mutex");
    let second = handle.launch("cli", LaunchTarget::App("demo".into())).await;
    assert_eq!(second.unwrap_err(), OpError::Busy { holder: "studio relay".into() });
    // A caller that waits longer gets the mutex when the push ends.
    let third = {
        let handle = handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            handle.launch("cli", LaunchTarget::App("demo".into())).await
        })
    };
    unsafe_kill(pid, "CONT");
    first.await.unwrap().unwrap();
    assert_eq!(third.await.unwrap().unwrap()["phase"], "switching");
    let _ = device.expect("pushed").await;
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_caller_cancels_its_request_and_releases_the_mutex() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    let big = package(16 * 1024 * 1024, 1);
    let task = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.push("ui", big, None).await })
    };
    tokio::time::sleep(Duration::from_millis(60)).await;
    task.abort();
    let _ = task.await;
    // The CANCEL reached the device: its push handler ends, the slot is free.
    assert_eq!(handle.snapshot().unwrap().mutex_holder, None);
    let reply = handle.push("ui", package(1000, 1), None).await.unwrap();
    assert_eq!(reply["phase"], "accepted");
    assert_eq!(device.expect("pushed").await["bytes"], 1000);
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_stall_times_out() {
    let mut config = fast_config();
    config.timing.request_stall = Duration::from_millis(300);
    config.timing.cancel_grace = Duration::from_millis(200);
    let mut hub = Hub::start(config, None).await;
    let (dir, id, _) = hub.pair();
    let device = Device::spawn(&hub, &dir, json!({"features":["runtime","apps"]})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    // The reference device grants the push's window once and only refills as
    // it writes; a device that lists no `runtime.native` answers `unsupported`
    // at once. The stall needs a request the device never answers: the
    // device's screenshot handler answers, so stop the device's process and
    // keep its socket: SIGSTOP leaves the connection open and silent.
    let pid = device_pid(&device);
    let started = std::time::Instant::now();
    unsafe_kill(pid, "STOP");
    let error = handle.status().await.unwrap_err();
    unsafe_kill(pid, "CONT");
    assert_eq!(error.code(), Some("timeout"));
    assert!(started.elapsed() < Duration::from_secs(5));
    device.kill().await;
}

fn device_pid(device: &Device) -> u32 {
    device.pid()
}

fn unsafe_kill(pid: u32, signal: &str) {
    std::process::Command::new("kill").arg(format!("-{signal}")).arg(pid.to_string()).status().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsupported_group_is_refused_locally() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let _device = Device::spawn(&hub, &dir, json!({"features":["runtime","apps"]})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    let error = handle.launch("cli", LaunchTarget::Native("pocket.3dsx".into())).await.unwrap_err();
    assert_eq!(error, OpError::Unsupported { feature: "runtime.native".into() });
    let value: Value = serde_json::to_value(&error).unwrap();
    assert_eq!(value["kind"], "unsupported");
}

/// A push counts the bytes it wrote, frame by frame, up to the package's
/// size; the mutex's holder is announced when taken and when released.
#[tokio::test(flavor = "multi_thread")]
async fn push_counts_its_bytes_and_the_holder_is_announced() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    let file = package(300 * 1024, 1);
    let (sent_tx, mut sent_rx) = tokio::sync::mpsc::unbounded_channel();
    handle.push_counted("studio", file.clone(), None, Some(sent_tx)).await.unwrap();
    let mut counts = Vec::new();
    while let Ok(count) = sent_rx.try_recv() {
        counts.push(count);
    }
    assert!(counts.len() > 1, "{counts:?}");
    assert!(counts.windows(2).all(|pair| pair[0] <= pair[1]));
    assert_eq!(*counts.last().unwrap(), file.len() as u64);

    let busy = hub.event(|e| matches!(e, HubEvent::Busy { holder: Some(_), .. })).await;
    let HubEvent::Busy { device: busy_device, holder, since } = busy else { unreachable!() };
    assert_eq!((busy_device.as_str(), holder.as_deref()), (id.as_str(), Some("studio")));
    assert!(since.unwrap() > 0);
    hub.event(|e| matches!(e, HubEvent::Busy { holder: None, .. })).await;
    assert_eq!(handle.snapshot().unwrap().mutex_since, None);
    let _ = device.expect("pushed").await;
    device.kill().await;
}
