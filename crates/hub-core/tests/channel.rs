//! Applet channels (§7) relayed to a service (docs/SERVICE-PROTOCOL.md §5): the
//! reference device's `GuestChannel` on one side, a fake service speaking the
//! local dialect by hand on the other.

mod common;

use std::time::Duration;

use common::*;
use pocket_hub_core::HubEvent;
use pocket_link::ResetCode;
use pocket_link::frame::{FLAG_BODY, FLAG_END, FLAG_ERROR, FrameType};
use serde_json::{Value, json};

/// The `channel.close` reason the device is sent when its service ends
/// (link specification §7.5): the wire's name, which says daemon until the
/// specification renames it.
const CLOSED_BY_SERVICE: &str = "daemon";
const MIB: usize = 1024 * 1024;

struct Setup {
    hub: Hub,
    notices: tokio::sync::mpsc::UnboundedReceiver<HostNotice>,
    device: Device,
    id: String,
}

async fn setup(config: pocket_hub_core::EndpointConfig) -> Setup {
    let (host, notices) = FakeHost::new();
    let mut hub = Hub::start(config, Some(host)).await;
    let (dir, id, _) = hub.pair();
    let device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    Setup { hub, notices, device, id }
}

/// Opens the applet's channel: the service attaches, grants its 256 units, and
/// reads the device's 48.
async fn open(s: &mut Setup, app: &str) -> Service {
    s.device.send(json!({"cmd":"open","app":app})).await;
    let mut service = next_service(&mut s.notices).await;
    assert_eq!(service.offer.app, app);
    assert_eq!(service.offer.device, s.id);
    assert_eq!(service.offer.payload_limit, 16384);
    assert_eq!(service.message_limit, 8192);
    service.credit(service.channel, 256).await;
    let credit = service.frame().await.unwrap();
    assert_eq!((credit.ty, credit.id), (FrameType::Credit, service.channel));
    assert_eq!(pocket_link::frame::parse_credit(&credit.payload), 48);
    let opened = s.device.expect("channel-open").await;
    assert_eq!(opened["generation"].as_u64().unwrap() as u32, service.offer.generation);
    service
}

/// Reads one stream on `id` to its END, granting `window` and refilling as it
/// consumes, as a service reading a body does. Returns bytes and FNV-1a 32.
async fn read_stream(service: &mut Service, id: u32, window: u32) -> (usize, u32) {
    service.credit(id, window).await;
    let (mut bytes, mut sum, mut since) = (0usize, 0x811c_9dc5u32, 0u32);
    loop {
        let frame = service.frame().await.expect("stream bytes");
        if frame.ty != FrameType::Data || frame.id != id {
            continue;
        }
        bytes += frame.payload.len();
        sum = frame.payload.iter().fold(sum, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193));
        since += frame.payload.len() as u32;
        if frame.flags & FLAG_END != 0 {
            return (bytes, sum);
        }
        if since >= window / 2 {
            service.credit(id, since).await;
            since = 0;
        }
    }
}

/// Writes `total` pattern bytes on `id` within the device's grants, then END.
async fn write_stream(service: &mut Service, id: u32, total: usize, limit: usize) {
    let data = pattern(total);
    let (mut at, mut credit) = (0usize, 0usize);
    while at < total {
        while credit == 0 {
            let frame = service.frame().await.expect("credit");
            if frame.ty == FrameType::Credit && frame.id == id {
                credit += pocket_link::frame::parse_credit(&frame.payload) as usize;
            }
        }
        let n = credit.min(limit).min(total - at);
        service.send(FrameType::Data, 0, id, &data[at..at + n]).await;
        at += n;
        credit -= n;
    }
    service.send(FrameType::Data, FLAG_END, id, &[]).await;
}

/// The guest channel's close reason, once the device has read the close.
async fn closed_reason(device: &mut Device) -> Value {
    for _ in 0..100 {
        device.send(json!({"cmd":"state"})).await;
        let state = device.expect("state").await;
        if state["state"]["state"] == "closed" && !state["state"]["reason"].is_null() {
            return state["state"]["reason"].clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the guest channel did not close");
}

fn reset_code(frame: &pocket_link::Frame) -> Option<ResetCode> {
    pocket_link::frame::parse_reset(&frame.payload)
}

#[tokio::test(flavor = "multi_thread")]
async fn open_refusals_and_a_cancelled_open() {
    let mut config = fast_config();
    config.timing.channel_open = Duration::from_millis(300);
    let mut s = setup(config).await;
    // `no_daemon` is the wire's code for an applet with no service (§7.1).
    for (app, code) in [("no.service", "no_daemon"), ("busy.service", "busy"), ("rejected.app", "rejected")] {
        s.device.send(json!({"cmd":"open","app":app})).await;
        let event = s.hub.event(|e| matches!(e, HubEvent::ChannelRefused { .. })).await;
        let HubEvent::ChannelRefused { app: refused, code: got, .. } = event else { unreachable!() };
        assert_eq!((refused.as_str(), got.as_str()), (app, code));
    }
    // A service that does not attach in time: `busy` at LINK_CHANNEL_OPEN_MS, and the open is dropped.
    s.device.send(json!({"cmd":"open","app":"slow.service"})).await;
    loop {
        match tokio::time::timeout(WAIT, s.notices.recv()).await.unwrap().unwrap() {
            HostNotice::Dropped(app) => break assert_eq!(app, "slow.service"),
            HostNotice::Opening(_) => continue,
            HostNotice::Attached(_) => panic!("slow.service attached"),
        }
    }
    let mut reason = Value::Null;
    for _ in 0..50 {
        s.device.send(json!({"cmd":"state"})).await;
        reason = s.device.expect("state").await["state"]["reason"].clone();
        if reason == "busy" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(reason, "busy");
    // The guest exits (its 2 s retry would open again); notices of that settle first.
    s.device.send(json!({"cmd":"exit"})).await;
    while tokio::time::timeout(Duration::from_millis(400), s.notices.recv()).await.is_ok() {}
    // A new guest exits while its open is pending: CANCEL, answered `cancelled`,
    // and the open dropped well before its 300 ms deadline.
    s.device.send(json!({"cmd":"open","app":"slow.service"})).await;
    match tokio::time::timeout(WAIT, s.notices.recv()).await.unwrap().unwrap() {
        HostNotice::Opening(app) => assert_eq!(app, "slow.service"),
        _ => panic!("expected the open to reach the host"),
    }
    let started = std::time::Instant::now();
    s.device.send(json!({"cmd":"exit"})).await;
    match tokio::time::timeout(WAIT, s.notices.recv()).await.unwrap().unwrap() {
        HostNotice::Dropped(app) => assert_eq!(app, "slow.service"),
        _ => panic!("expected the open to be dropped"),
    }
    assert!(started.elapsed() < Duration::from_millis(250), "dropped by the CANCEL, not the deadline");
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_events_and_units_both_ways() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    let channel = service.channel;

    // The applet asks; the service answers. Applet ids pass through unchanged.
    s.device.send(json!({"cmd":"request","tag":"a","t":"ping","args":{"n":1}})).await;
    let request = service.next_non_credit().await;
    assert_eq!(request.ty, FrameType::Request);
    assert!(request.id < 0x8000_0000);
    assert_eq!(object(&request)["t"], "ping");
    service.send_json(FrameType::Reply, 0, request.id, &json!({"pong":2})).await;
    let reply = s.device.reply("a").await;
    assert_eq!(reply["value"]["pong"], 2);

    // The service asks with an id of its own range; Hub maps it onto the link.
    let id = service.take_id();
    service.send_json(FrameType::Request, 0, id, &json!({"t":"echo","x":"y"})).await;
    let reply = service.next_non_credit().await;
    assert_eq!((reply.ty, reply.id, reply.flags), (FrameType::Reply, id, 0));
    assert_eq!(object(&reply)["echo"]["x"], "y");

    // An applet error code passes through unread.
    s.device.send(json!({"cmd":"request","tag":"e","t":"fails"})).await;
    let request = service.next_non_credit().await;
    service.send_json(FrameType::Reply, FLAG_ERROR, request.id, &json!({"code":"app.denied","message":"no"})).await;
    assert_eq!(s.device.reply("e").await["code"], "app.denied");

    // Events: the service's reaches the applet; the applet's reaches the service.
    service.send_json(FrameType::Event, 0, 0, &json!({"t":"note","hello":1})).await;
    assert_eq!(s.device.expect("ch-event").await["args"]["hello"], 1);
    // The guest took it: the device regrants its unit to the service.
    let credit = service.frame().await.unwrap();
    assert_eq!((credit.ty, credit.id), (FrameType::Credit, channel));
    assert_eq!(pocket_link::frame::parse_credit(&credit.payload), 1);

    // An event of 600 bytes costs 2 units of the 256 the service granted.
    s.device.send(json!({"cmd":"emit","t":"big","args":{"pad":"p".repeat(580)}})).await;
    assert_eq!(s.device.expect("emitted").await["ok"], true);
    let event = service.next_non_credit().await;
    assert_eq!((event.ty, event.id), (FrameType::Event, 0));
    assert!(event.payload.len() > 512);
    s.device.send(json!({"cmd":"state"})).await;
    assert_eq!(s.device.expect("state").await["units"], 254);
    service.credit(channel, 2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    s.device.send(json!({"cmd":"state"})).await;
    assert_eq!(s.device.expect("state").await["units"], 256);

    // Progress named like system events is the applet's (§5.3 step 1).
    let id = service.take_id();
    service.send_json(FrameType::Request, 0, id, &json!({"t":"named"})).await;
    let first = service.next_non_credit().await;
    let second = service.next_non_credit().await;
    let reply = service.next_non_credit().await;
    assert_eq!((first.ty, first.id), (FrameType::Event, id));
    assert_eq!(object(&first)["t"], "log");
    assert_eq!(object(&second)["t"], "features");
    assert_eq!((reply.ty, reply.id), (FrameType::Reply, id));
}

#[tokio::test(flavor = "multi_thread")]
async fn one_mib_each_way_under_credit() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    let expected = fnv32(&pattern(MIB));

    // (a) The applet's request body toward the service.
    s.device.send(json!({"cmd":"request","tag":"up","t":"upload","body":MIB})).await;
    let request = service.next_non_credit().await;
    assert_eq!(request.flags, FLAG_BODY);
    let (bytes, sum) = read_stream(&mut service, request.id, 256 * 1024).await;
    assert_eq!((bytes, sum), (MIB, expected));
    service.send_json(FrameType::Reply, 0, request.id, &json!({"bytes":bytes})).await;
    assert_eq!(s.device.reply("up").await["value"]["bytes"], MIB);

    // (b) The service's response stream toward the applet: head event, bytes, END, REPLY.
    s.device.send(json!({"cmd":"request","tag":"down","t":"download"})).await;
    let request = service.next_non_credit().await;
    service.send_json(FrameType::Event, 0, request.id, &json!({"t":"head","bytes":MIB})).await;
    write_stream(&mut service, request.id, MIB, 16384).await;
    service.send_json(FrameType::Reply, 0, request.id, &json!({"done":true})).await;
    let reply = s.device.reply("down").await;
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["bytes"], MIB);
    assert_eq!(reply["sum"].as_u64().unwrap() as u32, expected);

    // (c) The service asks the applet for bytes: the response comes on a mapped id.
    let id = service.take_id();
    service.send_json(FrameType::Request, 0, id, &json!({"t":"download","bytes":MIB})).await;
    let head = service.next_non_credit().await;
    assert_eq!((head.ty, head.id), (FrameType::Event, id));
    let (bytes, sum) = read_stream(&mut service, id, 256 * 1024).await;
    assert_eq!((bytes, sum), (MIB, expected));
    let reply = service.next_non_credit().await;
    assert_eq!((reply.ty, reply.id), (FrameType::Reply, id));

    // (d) A service event that opens a stream toward the applet.
    let id = service.take_id();
    service.send_json(FrameType::Event, 0, id, &json!({"t":"blob","n":1})).await;
    write_stream(&mut service, id, MIB, 16384).await;
    let got = s.device.expect("ch-stream").await;
    assert_eq!(got["bytes"], MIB);
    assert_eq!(got["sum"].as_u64().unwrap() as u32, expected);

    // (e) The service's request with a body toward the applet.
    let id = service.take_id();
    service.send_json(FrameType::Request, FLAG_BODY, id, &json!({"t":"upload"})).await;
    write_stream(&mut service, id, 300_000, 16384).await;
    let reply = service.next_non_credit().await;
    assert_eq!((reply.ty, reply.id), (FrameType::Reply, id));
    assert_eq!(object(&reply)["bytes"], 300_000);
    assert_eq!(object(&reply)["sum"].as_u64().unwrap() as u32, fnv32(&pattern(300_000)));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_head_step_cancel_and_bounds() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;

    // Bytes before any progress event are refused before they reach the wire.
    s.device.send(json!({"cmd":"request","tag":"h","t":"download"})).await;
    let request = service.next_non_credit().await;
    service.send(FrameType::Data, 0, request.id, b"early").await;
    let reset = service.next_non_credit().await;
    assert_eq!((reset.ty, reset.id, reset_code(&reset)), (FrameType::Reset, request.id, Some(ResetCode::Refused)));
    service.send_json(FrameType::Reply, 0, request.id, &json!({})).await;
    assert_eq!(s.device.reply("h").await["bytes"], 0);

    // The applet cancels: Hub answers `cancelled` at once and tells the service.
    s.device.send(json!({"cmd":"request","tag":"c","t":"slow"})).await;
    let request = service.next_non_credit().await;
    s.device.send(json!({"cmd":"cancel","tag":"c"})).await;
    assert_eq!(s.device.reply("c").await["code"], "cancelled");
    let cancel = service.next_non_credit().await;
    assert_eq!((cancel.ty, cancel.id), (FrameType::Cancel, request.id));
    // What the service still sends for it is dropped; a progress event it spent
    // a unit on comes back to it as CREDIT on the channel id.
    service.send_json(FrameType::Event, 0, request.id, &json!({"t":"late"})).await;
    let back = service.frame().await.unwrap();
    assert_eq!((back.ty, back.id, pocket_link::frame::parse_credit(&back.payload)), (FrameType::Credit, service.channel, 1));
    service.send_json(FrameType::Reply, 0, request.id, &json!({"late":true})).await;
    // An event over messageLimit is dropped and its 17 units returned.
    service.send_json(FrameType::Event, 0, 0, &json!({"t":"note","pad":"x".repeat(8300)})).await;
    let back = service.frame().await.unwrap();
    assert_eq!((back.ty, back.id, pocket_link::frame::parse_credit(&back.payload)), (FrameType::Credit, service.channel, 17));

    // Eight open requests of the applet; the ninth gets `busy` from Hub.
    for n in 0..9 {
        s.device.send(json!({"cmd":"request","tag":format!("r{n}"),"t":"wait"})).await;
    }
    assert_eq!(s.device.reply("r8").await["code"], "busy");
    let mut ids = Vec::new();
    while ids.len() < 8 {
        let frame = service.next_non_credit().await;
        assert_eq!(frame.ty, FrameType::Request);
        ids.push(frame.id);
    }
    assert!(service.frame_within(Duration::from_millis(200)).await.is_none_or(|f| f.ty == FrameType::Credit));
    for id in ids {
        service.send_json(FrameType::Reply, 0, id, &json!({})).await;
    }

    // Eight open requests of the service; the ninth gets `busy` from Hub.
    let mut ours = Vec::new();
    for _ in 0..9 {
        let id = service.take_id();
        ours.push(id);
        service.send_json(FrameType::Request, 0, id, &json!({"t":"hang"})).await;
    }
    let busy = service.next_non_credit().await;
    assert_eq!((busy.ty, busy.id, busy.flags), (FrameType::Reply, ours[8], FLAG_ERROR));
    assert_eq!(object(&busy)["code"], "busy");
    // The service cancels one once the applet's handler runs (a CANCEL read in
    // the same turn as its REQUEST withdraws it before any handler sees it):
    // the CANCEL reaches the applet, whose host answers `cancelled`.
    tokio::time::sleep(Duration::from_millis(100)).await;
    service.send(FrameType::Cancel, 0, ours[0], &[]).await;
    let answer = service.next_non_credit().await;
    assert_eq!((answer.ty, answer.id), (FrameType::Reply, ours[0]));
    assert_eq!(object(&answer)["code"], "cancelled");
    let _ = s.device.expect("applet-cancelled").await;

    // Objects over messageLimit (8,192 here): Hub fails the service's request
    // with `too_large`, and gives the applet `too_large` for a service reply over it.
    let id = service.take_id();
    service.send_json(FrameType::Request, 0, id, &json!({"t":"echo","pad":"x".repeat(8200)})).await;
    let refused = service.next_non_credit().await;
    assert_eq!((refused.id, object(&refused)["code"].as_str()), (id, Some("too_large")));
    s.device.send(json!({"cmd":"request","tag":"big","t":"bigreply"})).await;
    let request = service.next_non_credit().await;
    service.send_json(FrameType::Reply, 0, request.id, &json!({"pad":"x".repeat(8200)})).await;
    assert_eq!(s.device.reply("big").await["code"], "too_large");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_fifth_stream_is_refused() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    // Five streams from the applet that the service does not take yet.
    for n in 0..5 {
        s.device.send(json!({"cmd":"emitStream","tag":format!("s{n}"),"t":"blob","bytes":1000})).await;
    }
    let mut ids = Vec::new();
    while ids.len() < 5 {
        let frame = service.next_non_credit().await;
        assert_eq!(frame.ty, FrameType::Event);
        ids.push(frame.id);
    }
    assert!(ids[..4].iter().all(|&id| id != 0));
    // The fifth arrives as a standalone event: its stream was refused on the link.
    assert_eq!(ids[4], 0);
    let failed = s.device.expect("stream-failed").await;
    assert_eq!(failed["tag"], "s4");
    // The four others complete once the service reads them.
    for id in &ids[..4] {
        let (bytes, _) = read_stream(&mut service, *id, 4096).await;
        assert_eq!(bytes, 1000);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_past_the_applets_units_loses_its_channel() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    let channel = service.channel;
    // 48 units granted: three events of 16 units, and a fourth in the same write.
    let pad = "z".repeat(7900);
    let event = json!({"t":"note","pad":pad}).to_string().into_bytes();
    let frames: Vec<_> = (0..4).map(|_| (FrameType::Event, 0u8, 0u32, event.clone())).collect();
    service.send_all(&frames).await;
    let close = loop {
        let frame = service.next_non_credit().await;
        if frame.ty == FrameType::Event && frame.id == channel {
            break frame;
        }
    };
    assert_eq!(object(&close)["reason"], "overflow");
    assert!(service.frame().await.is_none_or(|f| f.ty == FrameType::Credit), "nothing after the close notice");
    assert_eq!(closed_reason(&mut s.device).await, CLOSED_BY_SERVICE);
}

#[tokio::test(flavor = "multi_thread")]
async fn close_reasons_and_generations() {
    let mut s = setup(fast_config()).await;
    let service = open(&mut s, "dev.demo").await;
    let first = service.offer.generation;
    let mut service = service;

    // The realm resets: `reset` to the service, then a new open with a larger generation.
    s.device.send(json!({"cmd":"reset"})).await;
    let close = service.next_non_credit().await;
    assert_eq!((close.id, object(&close)["reason"].as_str()), (service.channel, Some("reset")));
    assert!(service.frame().await.is_none(), "the service's socket closes after its notice");
    let mut service = next_service(&mut s.notices).await;
    assert!(service.offer.generation > first);
    service.credit(service.channel, 256).await;

    // Hub's policy ends it: `policy` both ways.
    let handle = s.hub.endpoint.device(&s.id).unwrap();
    let info = handle.snapshot().unwrap().link.unwrap().channels;
    assert_eq!(info.len(), 1);
    assert_eq!(info[0].generation, service.offer.generation);
    assert!(handle.close_channel(info[0].channel));
    let close = service.next_non_credit().await;
    assert_eq!(object(&close)["reason"], "policy");
    assert_eq!(closed_reason(&mut s.device).await, "policy");

    // The applet uses the channel again: a new open; then the service hangs up.
    s.device.send(json!({"cmd":"request","tag":"again","t":"x"})).await;
    let service = next_service(&mut s.notices).await;
    service.hang_up().await;
    assert_eq!(closed_reason(&mut s.device).await, CLOSED_BY_SERVICE);

    // `channel` leaves use on Hub's side: `feature` both ways; it comes back and the applet reopens.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    s.device.send(json!({"cmd":"request","tag":"again2","t":"x"})).await;
    let mut service = next_service(&mut s.notices).await;
    service.credit(service.channel, 256).await;
    let (host, notices) = FakeHost::new();
    s.hub.endpoint.set_channel_host(None);
    let close = service.next_non_credit().await;
    assert_eq!(object(&close)["reason"], "feature");
    assert_eq!(closed_reason(&mut s.device).await, "feature");
    s.device.send(json!({"cmd":"state"})).await;
    let state = s.device.expect("state").await;
    assert!(!state["inUse"].as_array().unwrap().contains(&json!("channel")));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    s.notices = notices;
    s.hub.endpoint.set_channel_host(Some(host));
    let service = next_service(&mut s.notices).await;
    assert!(service.offer.generation > first + 2);

    // The device exits the applet: `exit`.
    let mut service = service;
    s.device.send(json!({"cmd":"exit"})).await;
    let close = service.next_non_credit().await;
    assert_eq!(object(&close)["reason"], "exit");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_link_gives_the_service_one_close_and_the_reopen_a_larger_generation() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    let first = service.offer.generation;
    // A request of the service is open when the link goes.
    let id = service.take_id();
    service.send_json(FrameType::Request, 0, id, &json!({"t":"hang"})).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let pid = s.device.pid();
    std::process::Command::new("kill").arg("-9").arg(pid.to_string()).status().unwrap();
    let close = service.next_non_credit().await;
    assert_eq!((close.ty, close.id), (FrameType::Event, service.channel));
    assert_eq!(object(&close)["reason"], "link");
    // Exactly one notice: the socket ends after it, with no REPLY for the open request.
    assert!(service.frame().await.is_none());

    // A new run of the device reopens with a larger generation.
    let dir = s.hub.dir.join("again");
    let (fresh_dir, fresh_id, _) = s.hub.pair();
    let _ = (dir, fresh_id);
    let mut device = Device::spawn(&s.hub, &fresh_dir, json!({})).await;
    device.send(json!({"cmd":"open","app":"dev.demo"})).await;
    let service = next_service(&mut s.notices).await;
    assert!(service.offer.generation > first);
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_connection_closes_its_channels_with_link() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    // A second connection of the same device id reaches ready.
    let record = s.hub.store.device(&s.id).unwrap();
    let (_raw, _) = Raw::admit(s.hub.port, &s.id, &s.hub.store.hub_id(), &record.key).await;
    let close = service.next_non_credit().await;
    assert_eq!(object(&close)["reason"], "link");
    assert!(service.frame().await.is_none());
    let unlinked = s.device.expect("unlinked").await;
    assert_eq!(unlinked["code"], "replaced");
    let _: Value = unlinked;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_service_gets_stop_and_the_device_service() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    // A HANDSHAKE after channel.ready is ignored, not a broken rule.
    service.send_json(FrameType::Handshake, 0, 0, &json!({"t":"channel.attach","ticket":"x"})).await;
    let _ = service.stop.take().unwrap().send(());
    let close = service.next_non_credit().await;
    assert_eq!((close.ty, close.id), (FrameType::Event, service.channel));
    assert_eq!(object(&close)["reason"], "stop");
    assert!(service.frame().await.is_none());
    let mut reason = Value::Null;
    for _ in 0..50 {
        s.device.send(json!({"cmd":"state"})).await;
        reason = s.device.expect("state").await["state"]["reason"].clone();
        if reason == CLOSED_BY_SERVICE {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(reason, CLOSED_BY_SERVICE);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_services_requests_end_with_one_reply_each() {
    let mut config = fast_config();
    config.timing.request_stall = Duration::from_millis(400);
    config.timing.cancel_grace = Duration::from_millis(300);
    let mut s = setup(config).await;
    let mut service = open(&mut s, "dev.demo").await;
    // The applet's host never answers `hang`: Hub's own `timeout` after the stall.
    let id = service.take_id();
    service.send_json(FrameType::Request, 0, id, &json!({"t":"hang"})).await;
    let reply = service.next_non_credit().await;
    assert_eq!((reply.ty, reply.id, reply.flags), (FrameType::Reply, id, FLAG_ERROR));
    assert_eq!(object(&reply)["code"], "timeout");
    // The applet's host answers the CANCEL Hub sent; that REPLY is not a second one.
    assert!(service.frame_within(Duration::from_millis(600)).await.is_none_or(|f| f.ty == FrameType::Credit));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_fifth_stream_toward_the_applet_is_refused() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    // Four event streams toward the applet stay open: no bytes, no END.
    let mut open_ids = Vec::new();
    for _ in 0..4 {
        let id = service.take_id();
        open_ids.push(id);
        service.send_json(FrameType::Event, 0, id, &json!({"t":"blob"})).await;
    }
    // A fifth stream-opening event: RESET refused to the service; the applet gets the event alone.
    let fifth = service.take_id();
    service.send_json(FrameType::Event, 0, fifth, &json!({"t":"blob","fifth":true})).await;
    let reset = loop {
        let frame = service.next_non_credit().await;
        if frame.ty == FrameType::Reset {
            break frame;
        }
    };
    assert_eq!((reset.id, reset_code(&reset)), (fifth, Some(ResetCode::Refused)));
    let alone = s.device.wait(|v| v["ev"] == "ch-stream" && v["args"]["fifth"] == true).await;
    assert_eq!(alone["bytes"], -1);
    // A fifth REQUEST with BODY is forwarded, and its body refused.
    let id = service.take_id();
    service.send_json(FrameType::Request, FLAG_BODY, id, &json!({"t":"upload"})).await;
    let mut got_reset = false;
    let mut got_reply = false;
    while !(got_reset && got_reply) {
        let frame = service.next_non_credit().await;
        if frame.id != id {
            continue;
        }
        match frame.ty {
            FrameType::Reset => got_reset = reset_code(&frame) == Some(ResetCode::Refused),
            FrameType::Reply => got_reply = true,
            _ => {}
        }
    }
    // Ending one stream frees a place.
    service.send(FrameType::Data, FLAG_END, open_ids[0], &[]).await;
    let _ = s.device.wait(|v| v["ev"] == "ch-stream" && v["bytes"] == 0).await;
    let sixth = service.take_id();
    service.send_json(FrameType::Event, 0, sixth, &json!({"t":"blob","sixth":true})).await;
    assert!(service.frame_within(Duration::from_millis(300)).await.is_none_or(|f| f.ty != FrameType::Reset));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_device_dropping_channel_closes_it_with_feature() {
    let mut s = setup(fast_config()).await;
    let mut service = open(&mut s, "dev.demo").await;
    s.device.send(json!({"cmd":"features","list":["runtime","runtime.native","apps"]})).await;
    let close = service.next_non_credit().await;
    assert_eq!(object(&close)["reason"], "feature");
    let event = s.hub.event(|e| matches!(e, pocket_hub_core::HubEvent::Features { .. })).await;
    let pocket_hub_core::HubEvent::Features { features_in_use, .. } = event else { unreachable!() };
    assert!(!features_in_use.contains(&"channel".to_owned()));
    // Back in use (a second later, by the one-a-second rule): the applet opens again.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    s.device.send(json!({"cmd":"features","list":["runtime","runtime.native","apps","channel"]})).await;
    let again = next_service(&mut s.notices).await;
    assert!(again.offer.generation > service.offer.generation);
}
