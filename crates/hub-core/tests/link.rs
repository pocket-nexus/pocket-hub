//! The link itself: admission and its refusals (§3.3 to §3.8, §4.1), one link
//! per device and the duplicate id (§2.4), keepalive (§4.6), the frame rules
//! of §4.7 and §5, forgetting (§3.7) and Hub's close (§4.8). Most cases are
//! ones the reference device never produces, so a raw device made with the
//! codec plays them; the reference device covers the live replacement.

mod common;

use std::time::Duration;

use common::*;
use pocket_hub_core::HubEvent;
use pocket_link::frame::{self, FrameType, JsonFrame};
use pocket_link::preamble::{Preamble, encode as preamble};
use pocket_link::{CloseCode, ResetCode};
use serde_json::json;

fn refusal(frame: &pocket_link::Frame) -> String {
    let object = object(frame);
    assert_eq!(object["t"], "refused");
    object["code"].as_str().unwrap().to_owned()
}

async fn hello_only(port: u16, hello: serde_json::Value) -> Raw {
    let mut raw = Raw::connect(port).await;
    let mut first = preamble(Preamble { highest: 2, lowest: 2, selected: 0 }).to_vec();
    first.extend_from_slice(&pocket_link::handshake::encode(&hello).unwrap());
    raw.write(&first).await;
    assert_eq!(raw.read_exact(8).await.unwrap()[6], 2);
    raw
}

#[tokio::test(flavor = "multi_thread")]
async fn version_one_and_other_ranges_get_selected_zero() {
    let hub = Hub::start(fast_config(), None).await;
    // A version-1 console's preamble: PKLK 1 0 0 0.
    let mut raw = Raw::connect(hub.port).await;
    raw.write(&preamble(Preamble { highest: 1, lowest: 0, selected: 0 })).await;
    let answer = raw.read_exact(8).await.unwrap();
    assert_eq!(&answer[..4], b"PKLK");
    assert_eq!((answer[4], answer[5], answer[6]), (2, 2, 0));
    assert!(raw.ends_within(Duration::from_secs(2)).await);
    // A range of 3 to 4 has no common version either.
    let mut raw = Raw::connect(hub.port).await;
    raw.write(&preamble(Preamble { highest: 4, lowest: 3, selected: 0 })).await;
    assert_eq!(raw.read_exact(8).await.unwrap()[6], 0);
    // A device of range 2 to 3 waits for Hub's preamble, then sends its hello.
    let (_, id, key) = hub.pair();
    let mut raw = Raw::connect(hub.port).await;
    raw.write(&preamble(Preamble { highest: 3, lowest: 2, selected: 0 })).await;
    assert_eq!(raw.read_exact(8).await.unwrap()[6], 2);
    let nonce = pocket_link::text::new_nonce();
    raw.write(&pocket_link::handshake::encode(&Raw::hello(&id, &hub.store.hub_id(), &nonce, 16384)).unwrap()).await;
    let challenge = object(&raw.frame().await.unwrap());
    assert_eq!(challenge["t"], "challenge");
    let _ = key;
}

#[tokio::test(flavor = "multi_thread")]
async fn each_refusal() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (_, id, key) = hub.pair();
    let hub_id = hub.store.hub_id();
    let nonce = pocket_link::text::new_nonce();

    // unknown_device: no key for the id, and a hello naming another Hub.
    let mut raw = hello_only(hub.port, Raw::hello("0123456789abcdef", &hub_id, &nonce, 16384)).await;
    assert_eq!(refusal(&raw.frame().await.unwrap()), "unknown_device");
    assert!(raw.ends_within(Duration::from_secs(2)).await);
    let mut raw = hello_only(hub.port, Raw::hello(&id, "fedcba9876543210", &nonce, 16384)).await;
    assert_eq!(refusal(&raw.frame().await.unwrap()), "unknown_device");

    // malformed: a maxPayload of 512, a missing field.
    let mut raw = hello_only(hub.port, Raw::hello(&id, &hub_id, &nonce, 512)).await;
    let frame = raw.frame().await.unwrap();
    assert_eq!(refusal(&frame), "malformed");
    assert_eq!(object(&frame)["message"], "hello: maxPayload");
    let mut hello = Raw::hello(&id, &hub_id, &nonce, 16384);
    hello.as_object_mut().unwrap().remove("target");
    let mut raw = hello_only(hub.port, hello).await;
    assert_eq!(refusal(&raw.frame().await.unwrap()), "malformed");

    // bad_proof: an auth that does not verify.
    let mut raw = hello_only(hub.port, Raw::hello(&id, &hub_id, &nonce, 16384)).await;
    let challenge = object(&raw.frame().await.unwrap());
    assert_eq!(challenge["t"], "challenge");
    let wrong = pocket_link::handshake::console_proof(&pocket_link::text::new_key(), &nonce, challenge["nonce"].as_str().unwrap(), &id, &hub_id).unwrap();
    raw.write(&pocket_link::handshake::encode(&json!({"t":"auth","proof":wrong})).unwrap()).await;
    assert_eq!(refusal(&raw.frame().await.unwrap()), "bad_proof");
    hub.event(|e| matches!(e, HubEvent::Refused { code, .. } if code == "bad_proof")).await;

    // The device's own CLOSE auth ends the handshake and is reported.
    let mut raw = hello_only(hub.port, Raw::hello(&id, &hub_id, &nonce, 16384)).await;
    let _ = raw.frame().await.unwrap();
    raw.write(&frame::encode_close(CloseCode::Auth, "")).await;
    hub.event(|e| matches!(e, HubEvent::Refused { code, .. } if code == "auth")).await;

    // A DATA frame before ready closes with protocol.
    let mut raw = Raw::connect(hub.port).await;
    let mut first = preamble(Preamble { highest: 2, lowest: 2, selected: 0 }).to_vec();
    first.extend_from_slice(&frame::encode_data(5, b"x", false));
    raw.write(&first).await;
    let _ = raw.read_exact(8).await.unwrap();
    let close = raw.frame().await.unwrap();
    assert_eq!(close.ty, FrameType::Close);
    assert_eq!(frame::parse_close(&close.payload).0, CloseCode::Protocol);

    // The whole handshake still works for the right key.
    let (_raw, ready) = Raw::admit(hub.port, &id, &hub_id, &key).await;
    assert_eq!(ready["maxPayload"], 16384);
    assert!(ready["features"].as_array().unwrap().iter().any(|f| f == "runtime"));
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_at_the_connection_limit_and_the_handshake_timeout() {
    let mut config = fast_config();
    config.max_connections = 2;
    config.timing.handshake = Duration::from_millis(400);
    let hub = Hub::start(config, None).await;
    let (_, id, _) = hub.pair();
    let hub_id = hub.store.hub_id();
    // Two connections that stay in their handshake.
    let mut first = Raw::connect(hub.port).await;
    first.write(&preamble(Preamble { highest: 2, lowest: 2, selected: 0 })).await;
    let _second = Raw::connect(hub.port).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut third = hello_only(hub.port, Raw::hello(&id, &hub_id, &pocket_link::text::new_nonce(), 16384)).await;
    assert_eq!(refusal(&third.frame().await.unwrap()), "busy");
    // 6 s (here 400 ms) without completing the handshake: CLOSE handshake_timeout.
    let _ = first.read_exact(8).await.unwrap();
    let close = first.frame().await.unwrap();
    assert_eq!(frame::parse_close(&close.payload).0, CloseCode::HandshakeTimeout);
}

#[tokio::test(flavor = "multi_thread")]
async fn replacement_counts_live_connections_and_refuses_a_duplicate() {
    let mut config = fast_config();
    config.timing.replaced_ping = Duration::from_millis(300);
    let mut hub = Hub::start(config, None).await;
    let (_, id, key) = hub.pair();
    let hub_id = hub.store.hub_id();

    // Three replacements of connections that answer nothing are not counted.
    let mut silent = Vec::new();
    for _ in 0..4 {
        let (raw, _) = Raw::admit(hub.port, &id, &hub_id, &key).await;
        silent.push(raw);
    }
    for raw in silent.iter_mut().take(3) {
        let ping = raw.frame().await.unwrap();
        assert_eq!(ping.ty, FrameType::Ping);
        let close = raw.frame().await.unwrap();
        assert_eq!(frame::parse_close(&close.payload).0, CloseCode::Replaced);
    }

    // Three replacements of connections that answer the PING are counted.
    let mut older = silent.pop().unwrap();
    for _ in 0..3 {
        let (newer, _) = Raw::admit(hub.port, &id, &hub_id, &key).await;
        let ping = loop {
            let frame = older.frame().await.unwrap();
            if frame.ty == FrameType::Ping {
                break frame;
            }
        };
        older.write(&frame::encode_pong(&ping.payload)).await;
        let close = older.frame().await.unwrap();
        assert_eq!(frame::parse_close(&close.payload).0, CloseCode::Replaced);
        older = newer;
    }
    let mut next = hello_only(hub.port, Raw::hello(&id, &hub_id, &pocket_link::text::new_nonce(), 16384)).await;
    assert_eq!(refusal(&next.frame().await.unwrap()), "busy");
    hub.event(|e| matches!(e, HubEvent::DuplicateId { .. })).await;
    assert!(hub.endpoint.devices()[0].duplicate_id);
    // The count starts again after the refusal: the next hello is admitted.
    let (_raw, _) = Raw::admit(hub.port, &id, &hub_id, &key).await;
    drop(older);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reference_device_is_replaced_by_its_twin() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut first = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    let twin = hub.dir.join("twin");
    copy_dir(&dir, &twin);
    let mut second = Device::spawn(&hub, &twin, json!({})).await;
    let _ = second.expect("linked").await;
    let unlinked = first.expect("unlinked").await;
    assert_eq!(unlinked["code"], "replaced");
    assert_eq!(unlinked["by"], "peer");
    // Hub still shows the device linked, through the newer connection.
    assert!(hub.endpoint.devices()[0].linked);
    first.kill().await;
    second.kill().await;
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to.join("companions")).unwrap();
    std::fs::copy(from.join("device.txt"), to.join("device.txt")).unwrap();
    for entry in std::fs::read_dir(from.join("companions")).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join("companions").join(entry.file_name())).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn silence_and_skipped_frames() {
    let mut config = fast_config();
    config.timing.silence = Duration::from_millis(400);
    let mut hub = Hub::start(config, None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit(hub.port, &id, &hub.store.hub_id(), &key).await;
    // Frames of an unknown type are skipped and count as life (§4.6, §4.7).
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        raw.write(&[0x7f, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3]).await;
    }
    // PING is answered with the same 4 bytes.
    raw.write(&frame::encode_ping(0xdead_beef)).await;
    let pong = loop {
        let frame = raw.frame().await.unwrap();
        if frame.ty == FrameType::Pong {
            break frame;
        }
    };
    assert_eq!(&pong.payload[..], &0xdead_beefu32.to_le_bytes());
    // Then nothing: CLOSE silence.
    let close = loop {
        let frame = raw.frame().await.unwrap();
        if frame.ty == FrameType::Close {
            break frame;
        }
    };
    assert_eq!(frame::parse_close(&close.payload).0, CloseCode::Silence);
    let event = hub.event(|e| matches!(e, HubEvent::Unlinked { .. })).await;
    let HubEvent::Unlinked { close, .. } = event else { unreachable!() };
    assert_eq!((close.by, close.code.as_deref()), ("hub", Some("silence")));
}

/// One case of the frame rules on a fresh link: the frame sent and the close code it earns.
async fn closes_with(frame_bytes: Vec<u8>, code: CloseCode) {
    let hub = Hub::start(fast_config(), None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit(hub.port, &id, &hub.store.hub_id(), &key).await;
    raw.write(&frame_bytes).await;
    let close = loop {
        let frame = raw.frame().await.expect("a CLOSE before the end");
        if frame.ty == FrameType::Close {
            break frame;
        }
    };
    assert_eq!(frame::parse_close(&close.payload).0, code, "{}", frame::parse_close(&close.payload).1);
}

#[tokio::test(flavor = "multi_thread")]
async fn frame_rules_close_the_link() {
    let json = |t: FrameType, id: u32, v: serde_json::Value| frame::encode_json(t, id, &v, JsonFrame::default(), 16384).unwrap().to_vec();
    // A HANDSHAKE after ready.
    closes_with(pocket_link::handshake::encode(&json!({"t":"hello"})).unwrap().to_vec(), CloseCode::Protocol).await;
    // A length over the negotiated limit.
    closes_with(vec![0x12, 0, 0, 0, 0x01, 0x40, 0, 0, 0, 0, 0, 0], CloseCode::TooLarge).await;
    // A system event that requires id 0 with another id.
    closes_with(json(FrameType::Event, 9, json!({"t":"runtime.status"})), CloseCode::Protocol).await;
    // A progress event naming no open request of Hub.
    closes_with(json(FrameType::Event, 0, json!({"t":"runtime.install"})), CloseCode::Protocol).await;
    // A REQUEST whose id is Hub's range.
    closes_with(json(FrameType::Request, 0x8000_0005, json!({"t":"catalog"})), CloseCode::Protocol).await;
    // A features event of 17 entries.
    let list: Vec<String> = (0..17).map(|i| format!("f{i}")).collect();
    closes_with(json(FrameType::Event, 0, json!({"t":"features","features":list})), CloseCode::Protocol).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_and_events_hub_does_not_serve() {
    let hub = Hub::start(fast_config(), None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit(hub.port, &id, &hub.store.hub_id(), &key).await;
    let send = |t: FrameType, id: u32, v: serde_json::Value| frame::encode_json(t, id, &v, JsonFrame::default(), 16384).unwrap();
    raw.write(&send(FrameType::Request, 1, json!({"t":"catalog"}))).await;
    raw.write(&send(FrameType::Request, 3, json!({"t":"nope"}))).await;
    raw.write(&frame::encode(FrameType::Request, 0, 5, b"[1]", 16384).unwrap()).await;
    raw.write(&send(FrameType::Request, 7, json!({"t":"channel.open","app":"x","hash":"0000000000000000"}))).await;
    // An event with an unknown t on a fresh id may open a stream: RESET refused.
    raw.write(&send(FrameType::Event, 9, json!({"t":"mystery"}))).await;
    // An event on a late id of Hub's range is ignored, whatever its t.
    raw.write(&send(FrameType::Event, 0x8000_0042, json!({"t":"features"}))).await;
    // An APP request for a channel that is not open.
    raw.write(&frame::encode_json(FrameType::Request, 11, &json!({"t":"x"}), JsonFrame { channel: Some(77), ..Default::default() }, 16384).unwrap()).await;
    let mut seen = std::collections::HashMap::new();
    while seen.len() < 6 {
        let frame = raw.frame().await.unwrap();
        match frame.ty {
            FrameType::Reply => {
                assert_eq!(frame.flags, frame::FLAG_ERROR);
                seen.insert(frame.id, object(&frame)["code"].as_str().unwrap().to_owned());
            }
            FrameType::Reset => {
                assert_eq!(frame::parse_reset(&frame.payload), Some(ResetCode::Refused));
                seen.insert(frame.id, "reset".into());
            }
            _ => {}
        }
    }
    assert_eq!(seen[&1], "unsupported");
    assert_eq!(seen[&3], "unknown_message");
    assert_eq!(seen[&5], "malformed");
    // No channel host: Hub does not list `channel`, so the group is not in use.
    assert_eq!(seen[&7], "unsupported");
    assert_eq!(seen[&9], "reset");
    assert_eq!(seen[&11], "no_channel");
}

#[tokio::test(flavor = "multi_thread")]
async fn data_past_credit_closes_with_flow() {
    let hub = Hub::start(fast_config(), None).await;
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit(hub.port, &id, &hub.store.hub_id(), &key).await;
    let handle = hub.endpoint.device(&id).unwrap();
    let shot = tokio::spawn(async move { handle.screenshot().await });
    let request = loop {
        let frame = raw.frame().await.unwrap();
        if frame.ty == FrameType::Request {
            break frame;
        }
    };
    let head = json!({"t":"runtime.screenshot","frame":1,"format":"bgr8-rotated","surfaces":[{"name":"top","width":400,"height":240,"bytes":288000}]});
    raw.write(&frame::encode_json(FrameType::Event, request.id, &head, JsonFrame::default(), 16384).unwrap()).await;
    let credit = loop {
        let frame = raw.frame().await.unwrap();
        if frame.ty == FrameType::Credit {
            break frame;
        }
    };
    assert_eq!(credit.id, request.id);
    assert_eq!(frame::parse_credit(&credit.payload), 256 * 1024);
    // Hub refills its window up to the declared 288,000 bytes and no further:
    // one byte past them is past credit.
    let mut left = 288_000usize;
    while left > 0 {
        let n = left.min(16384);
        raw.write(&frame::encode_data(request.id, &vec![0u8; n], false)).await;
        left -= n;
        // Let the grants come back as a real sender would wait for them.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    raw.write(&frame::encode_data(request.id, &[0; 1], false)).await;
    let close = loop {
        let frame = raw.frame().await.unwrap();
        if frame.ty == FrameType::Close {
            break frame;
        }
    };
    assert_eq!(frame::parse_close(&close.payload).0, CloseCode::Flow);
    assert_eq!(shot.await.unwrap().unwrap_err().code(), Some("closed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn forget_and_shutdown() {
    let mut hub = Hub::start(fast_config(), None).await;
    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    assert!(hub.endpoint.forget(&id).await.unwrap());
    let unlinked = device.expect("unlinked").await;
    assert_eq!(unlinked["code"], "forgotten");
    assert!(hub.store.device(&id).is_none());
    assert!(hub.endpoint.devices().is_empty());
    assert!(hub.endpoint.device(&id).is_none());
    device.kill().await;

    let (dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &dir, json!({})).await;
    hub.linked(&id).await;
    hub.endpoint.shutdown().await;
    let unlinked = device.expect("unlinked").await;
    assert_eq!(unlinked["code"], "going_away");
    device.kill().await;
}
