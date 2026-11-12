//! Hub's patient window (`conn`'s `patient`): while a state-changing request
//! of Hub is open, and after a `switching` reply until the next
//! `runtime.status` or its deadline, Hub's silence limit is
//! `Timing::state_silence` and Hub sends its own `PING` every
//! `Timing::state_ping`; everywhere else §4.6 holds unchanged.
//!
//! A raw device made with the codec plays the cases whose timing must be
//! exact (it sends nothing unless told). The reference device under Bun,
//! stopped with SIGSTOP, plays the stall of a real device; its own timers
//! are §4.6's (a PING every 3 s, silence after 10 s), so those cases use a
//! normal limit above 3 s and stalls under 10 s.

mod common;

use std::time::Duration;

use common::*;
use pocket_hub_core::{EndpointConfig, HubEvent, LaunchTarget, OpError};
use pocket_link::frame::{self, Decoded, Frame, FrameType, JsonFrame};
use pocket_link::CloseCode;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::time::Instant;

fn package(len: usize) -> bytes::Bytes {
    let mut bytes = pattern(len - 8);
    bytes[0] = 1;
    let hash = pocket_hub_core::ops::fnv1a64(&bytes);
    bytes.extend_from_slice(&hash.to_le_bytes());
    bytes.into()
}

/// Short timers for the raw device: 1 s normally, 3 s while patient, a PING
/// every 250 ms. The normal limit leaves a loaded machine time to start the
/// test's request after the raw device's last frame.
fn raw_config() -> EndpointConfig {
    let mut config = fast_config();
    config.timing.silence = Duration::from_millis(1000);
    config.timing.state_silence = Duration::from_millis(3000);
    config.timing.state_ping = Duration::from_millis(250);
    config
}

/// The raw device says it is alive, right before the test starts a request.
async fn alive(raw: &mut Raw) {
    raw.write(&frame::encode_ping(1)).await;
}

/// What Hub sent within `within`: each frame with the time since `from`, and
/// when the connection ended, if it did. With `keepalive`, the raw device
/// sends its own PING at that interval meanwhile.
async fn frames_for(raw: &mut Raw, from: Instant, within: Duration, keepalive: Option<Duration>) -> (Vec<(Duration, Frame)>, Option<Duration>) {
    let deadline = Instant::now() + within;
    let mut next_ping = keepalive.map(|every| Instant::now() + every);
    let mut frames = Vec::new();
    let mut buffer = vec![0u8; 65536];
    loop {
        while let Ok(Some(decoded)) = raw.decoder.next() {
            if let Decoded::Frame(frame) = decoded {
                frames.push((from.elapsed(), frame));
            }
        }
        let wake = next_ping.map_or(deadline, |at| at.min(deadline));
        match tokio::time::timeout_at(wake, raw.reader.read(&mut buffer)).await {
            Ok(Ok(0)) | Ok(Err(_)) => return (frames, Some(from.elapsed())),
            Ok(Ok(n)) => raw.decoder.push(&buffer[..n]),
            Err(_) if Instant::now() >= deadline => return (frames, None),
            Err(_) => {
                raw.write(&frame::encode_ping(7)).await;
                next_ping = keepalive.map(|every| Instant::now() + every);
            }
        }
    }
}

fn pings(frames: &[(Duration, Frame)]) -> Vec<Duration> {
    frames.iter().filter(|(_, f)| f.ty == FrameType::Ping).map(|(at, _)| *at).collect()
}

fn close_of(frames: &[(Duration, Frame)]) -> Option<(Duration, CloseCode)> {
    frames.iter().find(|(_, f)| f.ty == FrameType::Close).map(|(at, f)| (*at, frame::parse_close(&f.payload).0))
}

/// Hub's next REQUEST of type `t`, its id.
async fn request(raw: &mut Raw, t: &str) -> u32 {
    loop {
        let frame = raw.frame().await.expect("a request");
        if frame.ty == FrameType::Request && frame::parse_json(&frame).object.is_some_and(|o| o["t"] == t) {
            return frame.id;
        }
    }
}

async fn reply(raw: &mut Raw, id: u32, object: Value) {
    raw.write(&frame::encode_json(FrameType::Reply, id, &object, JsonFrame::default(), 16384).unwrap()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_push_gets_the_longer_limit_and_hubs_pings() {
    let mut hub = Hub::start(raw_config(), None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit_with(hub.port, &id, &hub.store.hub_id(), &key, &["runtime", "apps"]).await;
    // `ready` is on the wire before Hub has the link in its table: a request
    // started before `Linked` would fail with `not_linked` and never go out.
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    alive(&mut raw).await;
    let push = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.push("test", package(1000), None).await })
    };
    let wire = request(&mut raw, "runtime.push").await;
    let opened = Instant::now();
    // Silent past the normal 1 s: still linked, Hub's PINGs every 250 ms.
    let (frames, ended) = frames_for(&mut raw, opened, Duration::from_millis(2200), None).await;
    assert_eq!(ended, None, "linked past the normal limit");
    assert_eq!(close_of(&frames), None);
    let during = pings(&frames);
    assert!((6..=10).contains(&during.len()), "a PING every 250 ms: {during:?}");
    assert!(during[0] >= Duration::from_millis(200), "the first one interval after the request: {during:?}");
    for pair in during.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(gap >= Duration::from_millis(200) && gap <= Duration::from_millis(600), "{during:?}");
    }

    // The REPLY ends the window: no more PINGs, and the normal limit again.
    reply(&mut raw, wire, json!({"phase": "accepted", "hash": "0000000000000000", "generation": 2, "message": "ok", "app": {"id": "dev.demo", "output": "demo", "title": "Demo"}})).await;
    let answered = Instant::now();
    assert_eq!(push.await.unwrap().unwrap()["phase"], "accepted");
    let (frames, _) = frames_for(&mut raw, answered, Duration::from_millis(2500), None).await;
    assert_eq!(pings(&frames), Vec::<Duration>::new(), "no PING of Hub's own after the window");
    let (at, code) = close_of(&frames).expect("CLOSE silence");
    assert_eq!(code, CloseCode::Silence);
    assert!(at < Duration::from_millis(2000), "the normal limit: {at:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stall_past_the_longer_limit_is_still_silence() {
    let mut hub = Hub::start(raw_config(), None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit_with(hub.port, &id, &hub.store.hub_id(), &key, &["runtime", "apps"]).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    alive(&mut raw).await;
    let push = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.push("test", package(1000), None).await })
    };
    request(&mut raw, "runtime.push").await;
    let opened = Instant::now();
    let (frames, _) = frames_for(&mut raw, opened, Duration::from_millis(5500), None).await;
    let (at, code) = close_of(&frames).expect("CLOSE silence");
    assert_eq!(code, CloseCode::Silence);
    assert!(at >= Duration::from_millis(2900) && at < Duration::from_millis(4500), "the longer limit, 3 s: {at:?}");
    assert!(matches!(push.await.unwrap(), Err(OpError::Link { code, .. }) if code == "closed"));
    let unlinked = hub.event(|e| matches!(e, HubEvent::Unlinked { .. })).await;
    let HubEvent::Unlinked { close, .. } = unlinked else { unreachable!() };
    assert_eq!((close.by, close.code.as_deref()), ("hub", Some("silence")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switching_reply_keeps_the_window_until_the_next_status() {
    let mut hub = Hub::start(raw_config(), None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit_with(hub.port, &id, &hub.store.hub_id(), &key, &["runtime", "apps"]).await;
    // `ready` is on the wire before Hub has the link in its table: a request
    // started before `Linked` would fail with `not_linked` and never go out.
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    alive(&mut raw).await;
    let launch = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.launch("test", LaunchTarget::App("demo".into())).await })
    };
    let wire = request(&mut raw, "runtime.launch").await;
    reply(&mut raw, wire, json!({"phase": "switching", "output": "demo"})).await;
    let replied = Instant::now();
    assert_eq!(launch.await.unwrap().unwrap()["phase"], "switching");
    // The request is answered: the mutex is free, and there is no launch hold.
    let snapshot = handle.snapshot().unwrap();
    assert_eq!(snapshot.mutex_holder, None);

    // The device is silent while the app's first boot creates its folders.
    let (frames, ended) = frames_for(&mut raw, replied, Duration::from_millis(1800), None).await;
    assert_eq!((ended, close_of(&frames)), (None, None), "linked past the normal limit");
    assert!(pings(&frames).len() >= 4, "{:?}", pings(&frames));

    // Its next runtime.status ends the window.
    let status = json!({"t": "runtime.status", "phase": "booted", "target": "3ds-dev", "hostAbi": 7, "generation": 1, "frame": 1});
    raw.write(&frame::encode_json(FrameType::Event, 0, &status, JsonFrame::default(), 16384).unwrap()).await;
    let reported = Instant::now();
    let (frames, _) = frames_for(&mut raw, reported, Duration::from_millis(2500), None).await;
    assert_eq!(pings(&frames), Vec::<Duration>::new());
    let (at, code) = close_of(&frames).expect("CLOSE silence");
    assert_eq!(code, CloseCode::Silence);
    assert!(at < Duration::from_millis(2000), "the normal limit again: {at:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_switching_window_without_a_status_ends_at_its_deadline() {
    let mut hub = Hub::start(raw_config(), None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit_with(hub.port, &id, &hub.store.hub_id(), &key, &["runtime", "apps"]).await;
    // `ready` is on the wire before Hub has the link in its table: a request
    // started before `Linked` would fail with `not_linked` and never go out.
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    alive(&mut raw).await;
    let launch = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.launch("test", LaunchTarget::App("demo".into())).await })
    };
    let wire = request(&mut raw, "runtime.launch").await;
    reply(&mut raw, wire, json!({"phase": "switching", "output": "demo"})).await;
    let replied = Instant::now();
    launch.await.unwrap().unwrap();
    // The device keeps the link alive itself (a PING every 250 ms): the
    // window's end shows as the end of Hub's PINGs, 3 s after the reply.
    let (frames, ended) = frames_for(&mut raw, replied, Duration::from_millis(4500), Some(Duration::from_millis(250))).await;
    assert_eq!((ended, close_of(&frames)), (None, None));
    let hubs = pings(&frames);
    assert!(hubs.len() >= 8, "{hubs:?}");
    let last = *hubs.last().unwrap();
    assert!(last >= Duration::from_millis(2400) && last < Duration::from_millis(3300), "the last PING is before the deadline: {hubs:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn other_requests_get_neither() {
    let mut hub = Hub::start(raw_config(), None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit_with(hub.port, &id, &hub.store.hub_id(), &key, &["runtime", "apps"]).await;
    // `ready` is on the wire before Hub has the link in its table: a request
    // started before `Linked` would fail with `not_linked` and never go out.
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    // A `runtime.status` request is not state-changing: the normal limit, no PING.
    alive(&mut raw).await;
    let status = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.status().await })
    };
    request(&mut raw, "runtime.status").await;
    let opened = Instant::now();
    let (frames, _) = frames_for(&mut raw, opened, Duration::from_millis(2500), None).await;
    assert_eq!(pings(&frames), Vec::<Duration>::new());
    let (at, code) = close_of(&frames).expect("CLOSE silence");
    assert_eq!(code, CloseCode::Silence);
    assert!(at < Duration::from_millis(2000), "{at:?}");
    assert!(status.await.unwrap().is_err());
}

// ---------------------------------------------------------------------------
// The reference device, stopped
// ---------------------------------------------------------------------------

/// The reference device pings every 3 s: a normal limit of 4.5 s, a longer
/// one of 9 s, Hub's PING every 500 ms, and stalls of 6 s (under the
/// device's own 10 s).
fn stall_config() -> EndpointConfig {
    let mut config = fast_config();
    config.timing.silence = Duration::from_millis(4500);
    config.timing.state_silence = Duration::from_millis(9000);
    config.timing.state_ping = Duration::from_millis(500);
    config
}

const STALL: Duration = Duration::from_secs(6);

fn kill(pid: u32, signal: &str) {
    std::process::Command::new("kill").arg(format!("-{signal}")).arg(pid.to_string()).status().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_push_survives_a_stalled_device() {
    let mut hub = Hub::start(stall_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let handle = hub.endpoint.device(&id).unwrap();
    let pid = device.pid();
    kill(pid, "STOP");
    let push = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.push("test", package(2 * 1024 * 1024), None).await })
    };
    tokio::time::sleep(STALL).await;
    assert!(handle.snapshot().unwrap().linked, "linked through a stall past the normal limit");
    kill(pid, "CONT");
    assert_eq!(push.await.unwrap().unwrap()["phase"], "accepted");
    assert_eq!(device.expect("pushed").await["bytes"], 2 * 1024 * 1024);
    while let Ok(event) = hub.events.try_recv() {
        assert!(!matches!(event, HubEvent::Unlinked { .. }), "no unlink: {event:?}");
    }
    assert!(handle.snapshot().unwrap().linked);
    assert_eq!(handle.snapshot().unwrap().mutex_holder, None);
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_stall_with_nothing_open_is_silence() {
    let mut hub = Hub::start(stall_config(), None).await;
    let (dir, id, _) = hub.pair();
    let device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let pid = device.pid();
    let stopped = Instant::now();
    kill(pid, "STOP");
    let unlinked = hub.event(|e| matches!(e, HubEvent::Unlinked { .. })).await;
    let after = stopped.elapsed();
    kill(pid, "CONT");
    let HubEvent::Unlinked { close, .. } = unlinked else { unreachable!() };
    assert_eq!((close.by, close.code.as_deref()), ("hub", Some("silence")));
    assert!(after < STALL, "the normal limit: {after:?}");
    device.kill().await;
}
