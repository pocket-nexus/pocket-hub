//! Pairing by a code the device shows (§3.11). The reference device pairs
//! and links as a Tab5 does; a raw device made with the codec plays what the
//! reference never sends: a proof under another key, a `pair` Hub cannot
//! read, a request left to run out.

mod common;

use std::time::Duration;

use common::*;
use pocket_hub_core::{HubEvent, PairError, PairRequest, Timing};
use pocket_hub_store::DeviceRecord;
use pocket_link::CloseCode;
use pocket_link::frame::{self, FrameType};
use pocket_link::pair;
use pocket_link::pairing::parse_companion_file;
use pocket_link::preamble::{Preamble, encode as preamble};
use pocket_link::text::{new_key, new_link_id, new_nonce};
use serde_json::{Value, json};

const CODE: &str = "314159";
const OTHER_CODE: &str = "271828";

fn pair_message(device: &str, nonce: &str) -> Value {
    json!({"t": "pair", "device": device, "nonce": nonce, "name": "Raw Tab5", "model": "M5Stack Tab5", "target": "tab5-home"})
}

/// A raw device that sent its preamble and `message` in place of a `hello`.
async fn ask(port: u16, message: &Value) -> Raw {
    let mut raw = Raw::connect(port).await;
    let mut first = preamble(Preamble { highest: 2, lowest: 2, selected: 0 }).to_vec();
    first.extend_from_slice(&pocket_link::handshake::encode(message).unwrap());
    raw.write(&first).await;
    assert_eq!(raw.read_exact(8).await.unwrap()[6], 2);
    raw
}

/// Hub's requests once they are `count`, within `WAIT`.
async fn requests(hub: &mut Hub, count: usize) -> Vec<PairRequest> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let waiting = hub.endpoint.pair_requests();
        if waiting.len() == count {
            return waiting;
        }
        tokio::time::timeout_at(deadline, hub.event(|event| matches!(event, HubEvent::PairRequests))).await.unwrap_or_else(|_| panic!("{count} requests in time; Hub holds {waiting:?}"));
    }
}

/// The offer Hub sends after `Endpoint::pair`, as the device reads it.
async fn offer(raw: &mut Raw) -> pair::PairOffer {
    let frame = raw.frame().await.expect("pair.offer");
    assert_eq!(frame.ty, FrameType::Handshake);
    pair::parse_pair_offer(Some(&object(&frame))).expect("a pair.offer")
}

fn refusal(frame: &pocket_link::Frame) -> String {
    let object = object(frame);
    assert_eq!(object["t"], "refused");
    object["code"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_pairs_by_its_code_and_links() {
    let mut hub = Hub::start(fast_config(), None).await;
    let hub_id = hub.store.hub_id();
    // A store no Hub has set up: the device makes its id and holds no key.
    let dir = hub.dir.join("unpaired");
    std::fs::create_dir_all(&dir).unwrap();
    let mut device = Device::spawn(&hub, &dir, json!({"autoConnect": false, "name": "Desk Tab5", "model": "M5Stack Tab5", "target": "tab5-home", "features": ["apps", "runtime", "runtime.remove", "catalog", "install", "net.http"]})).await;
    assert!(hub.endpoint.pair_requests().is_empty());

    device.send(json!({"cmd": "pairOpen", "code": CODE})).await;
    assert_eq!(device.expect("pair-open").await["code"], CODE);
    device.send(json!({"cmd": "pairAsk"})).await;
    assert_eq!(device.expect("pair-ask").await["ok"], true);

    // Hub shows the request and holds no record before the code.
    let waiting = requests(&mut hub, 1).await;
    let request = &waiting[0];
    assert_eq!((request.device.as_str(), request.name.as_str(), request.model.as_str(), request.target.as_str()), (device.id.as_str(), "Desk Tab5", "M5Stack Tab5", "tab5-home"));
    assert!(request.peer.starts_with("127.0.0.1:"));
    assert_eq!(request.expires - request.since, 300_000);
    assert!(hub.store.device(&device.id).is_none());
    assert!(hub.endpoint.device(&device.id).is_none());

    // Another code: the device closes with `auth`, nothing is stored, and
    // the device asks again with the 2 tries it has left.
    assert_eq!(hub.endpoint.pair(&device.id, OTHER_CODE).await, Err(PairError::WrongCode));
    assert!(hub.store.device(&device.id).is_none());
    assert!(!dir.join("companions").join(format!("{hub_id}.txt")).exists());
    requests(&mut hub, 1).await;
    device.send(json!({"cmd": "pair", "tag": "after-wrong"})).await;
    let state = device.wait(|v| v["ev"] == "pair" && v["tag"] == "after-wrong").await;
    assert_eq!((&state["pair"]["state"], &state["pair"]["tries"]), (&json!("asking"), &json!(2)));

    // The code: the device writes the key file itself, and Hub's record holds the same key.
    assert_eq!(hub.endpoint.pair(&device.id, CODE).await, Ok(()));
    hub.event(|event| matches!(event, HubEvent::Paired { device: id } if *id == device.id)).await;
    assert!(hub.endpoint.pair_requests().is_empty());
    let record = hub.store.device(&device.id).expect("the record");
    let file = parse_companion_file(&std::fs::read_to_string(dir.join("companions").join(format!("{hub_id}.txt"))).unwrap()).expect("the key file");
    assert_eq!(file.key, record.key);
    assert_eq!((file.name.as_str(), file.address.as_str(), file.port), (hub.store.hub_name().as_str(), "127.0.0.1", hub.port));
    assert_eq!((record.kind.as_str(), record.name.as_str(), record.model.as_str(), record.target.as_str()), ("tab5", "Desk Tab5", "M5Stack Tab5", "tab5-home"));
    assert_eq!(record.last_address.as_deref(), Some("127.0.0.1"));
    assert!(!record.setup_unconfirmed);

    // It links by the handshake of §3.3, with that key.
    hub.linked(&device.id).await;
    assert_eq!(device.expect("linked").await["hub"], hub_id.as_str());
    device.send(json!({"cmd": "pair", "tag": "paired"})).await;
    let state = device.wait(|v| v["ev"] == "pair" && v["tag"] == "paired").await;
    assert_eq!(state["pair"]["state"], "paired");
    let snapshot = hub.endpoint.device(&device.id).unwrap().snapshot().unwrap();
    assert!(snapshot.linked);
    assert_eq!(snapshot.host_abi, 7, "the hello fills in what a pair does not carry");
    // No key in what Hub publishes of the device.
    assert!(!serde_json::to_string(&snapshot).unwrap().contains(&record.key));
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_whole_exchange_by_hand() {
    let mut hub = Hub::start(fast_config(), None).await;
    let hub_id = hub.store.hub_id();
    let (id, nonce) = (new_link_id(), new_nonce());
    let mut raw = ask(hub.port, &pair_message(&id, &nonce)).await;
    requests(&mut hub, 1).await;

    let endpoint = &hub.endpoint;
    let (paired, key) = tokio::join!(endpoint.pair(&id, CODE), async {
        let offer = offer(&mut raw).await;
        // The offer names this Hub, proves the code and carries the key under it.
        assert_eq!((offer.companion.as_str(), offer.name.as_str()), (hub_id.as_str(), hub.store.hub_name().as_str()));
        assert_eq!(offer.proof, pair::companion_proof(CODE, &nonce, &offer.nonce, &id, &hub_id).unwrap());
        let key = pair::mask_key(CODE, &offer.key, &nonce, &offer.nonce, &id, &hub_id).unwrap();
        // Nothing is stored before the device's proof.
        assert!(hub.store.device(&id).is_none());
        let proof = pair::console_proof(&key, &nonce, &offer.nonce, &id, &hub_id).unwrap();
        raw.write(&pocket_link::handshake::encode(&json!({"t": "pair.accept", "proof": proof})).unwrap()).await;
        let done = raw.frame().await.expect("pair.done");
        assert_eq!((done.ty, object(&done)["t"].as_str()), (FrameType::Handshake, Some("pair.done")));
        // Hub closes the connection behind it.
        assert!(raw.ends_within(Duration::from_secs(2)).await);
        key
    });
    assert_eq!(paired, Ok(()));
    let record = hub.store.device(&id).unwrap();
    assert_eq!(record.key, key);
    assert_eq!((record.kind.as_str(), record.host_abi, record.last_seen), ("tab5", 0, None));
    assert!(hub.endpoint.pair_requests().is_empty());

    // The device then links by the handshake of §3.3 with the key it unmasked.
    let (_linked, ready) = Raw::admit(hub.port, &id, &hub_id, &key).await;
    assert_eq!(ready["t"], "ready");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_code_replaces_no_key_hub_holds() {
    let mut hub = Hub::start(fast_config(), None).await;
    let hub_id = hub.store.hub_id();
    // Hub holds a pairing of this device id, as after a setup.
    let (id, held) = (new_link_id(), new_key());
    hub.store
        .put_device(DeviceRecord {
            id: id.clone(),
            kind: "3ds".into(),
            name: "Before".into(),
            model: String::new(),
            key: held.clone(),
            target: "3ds-dev".into(),
            host_abi: 7,
            last_address: None,
            last_seen: Some(1),
            setup_unconfirmed: false,
        })
        .unwrap();
    let nonce = new_nonce();
    let mut raw = ask(hub.port, &pair_message(&id, &nonce)).await;
    requests(&mut hub, 1).await;
    let endpoint = &hub.endpoint;
    let (paired, ()) = tokio::join!(endpoint.pair(&id, OTHER_CODE), async {
        let offer = offer(&mut raw).await;
        // The device's own code does not verify the proof: it closes with `auth`.
        assert_ne!(offer.proof, pair::companion_proof(CODE, &nonce, &offer.nonce, &id, &hub_id).unwrap());
        raw.write(&frame::encode_close(CloseCode::Auth, "")).await;
    });
    assert_eq!(paired, Err(PairError::WrongCode));
    let record = hub.store.device(&id).unwrap();
    assert_eq!((record.key, record.name.as_str(), record.kind.as_str()), (held.clone(), "Before", "3ds"));
    assert!(requests(&mut hub, 0).await.is_empty());

    // With the code, the record takes the new key and what the pair said;
    // what a pair does not carry stays (hostAbi, the last ready).
    let nonce = new_nonce();
    let mut raw = ask(hub.port, &pair_message(&id, &nonce)).await;
    requests(&mut hub, 1).await;
    let endpoint = &hub.endpoint;
    let (paired, key) = tokio::join!(endpoint.pair(&id, CODE), async {
        let offer = offer(&mut raw).await;
        let key = pair::mask_key(CODE, &offer.key, &nonce, &offer.nonce, &id, &hub_id).unwrap();
        let proof = pair::console_proof(&key, &nonce, &offer.nonce, &id, &hub_id).unwrap();
        raw.write(&pocket_link::handshake::encode(&json!({"t": "pair.accept", "proof": proof})).unwrap()).await;
        key
    });
    assert_eq!(paired, Ok(()));
    let record = hub.store.device(&id).unwrap();
    assert_ne!(key, held);
    assert_eq!((record.key, record.name.as_str(), record.kind.as_str(), record.target.as_str()), (key, "Raw Tab5", "tab5", "tab5-home"));
    assert_eq!((record.host_abi, record.last_seen), (7, Some(1)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proof_under_another_key_is_refused_and_stores_nothing() {
    let mut hub = Hub::start(fast_config(), None).await;
    let hub_id = hub.store.hub_id();
    let (id, nonce) = (new_link_id(), new_nonce());
    let mut raw = ask(hub.port, &pair_message(&id, &nonce)).await;
    requests(&mut hub, 1).await;
    let endpoint = &hub.endpoint;
    let (paired, ()) = tokio::join!(endpoint.pair(&id, CODE), async {
        let offer = offer(&mut raw).await;
        let proof = pair::console_proof(&new_key(), &nonce, &offer.nonce, &id, &hub_id).unwrap();
        raw.write(&pocket_link::handshake::encode(&json!({"t": "pair.accept", "proof": proof})).unwrap()).await;
        assert_eq!(refusal(&raw.frame().await.unwrap()), "bad_proof");
        assert!(raw.ends_within(Duration::from_secs(2)).await);
    });
    assert_eq!(paired, Err(PairError::BadProof));
    assert!(hub.store.device(&id).is_none());

    // A `pair.accept` to no offer is refused the same way.
    let nonce = new_nonce();
    let mut raw = ask(hub.port, &pair_message(&id, &nonce)).await;
    requests(&mut hub, 1).await;
    raw.write(&pocket_link::handshake::encode(&json!({"t": "pair.accept", "proof": "0".repeat(64)})).unwrap()).await;
    assert_eq!(refusal(&raw.frame().await.unwrap()), "bad_proof");
    assert!(requests(&mut hub, 0).await.is_empty());
    assert!(hub.store.device(&id).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dismissed_request_reads_declined() {
    let mut hub = Hub::start(fast_config(), None).await;
    let id = new_link_id();
    assert!(!hub.endpoint.decline_pair(&id), "no request yet");
    let mut raw = ask(hub.port, &pair_message(&id, &new_nonce())).await;
    requests(&mut hub, 1).await;
    assert!(hub.endpoint.decline_pair(&id));
    let frame = raw.frame().await.unwrap();
    assert_eq!(refusal(&frame), "declined");
    assert_eq!(object(&frame)["message"], "this computer's owner dismissed the request");
    assert!(raw.ends_within(Duration::from_secs(2)).await);
    assert!(requests(&mut hub, 0).await.is_empty());
    assert_eq!(hub.endpoint.pair(&id, CODE).await, Err(PairError::NoRequest));
    assert!(hub.store.device(&id).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn one_request_a_device_id_and_one_offer_a_request() {
    let mut hub = Hub::start(fast_config(), None).await;
    let hub_id = hub.store.hub_id();
    let id = new_link_id();
    let mut first = ask(hub.port, &pair_message(&id, &new_nonce())).await;
    let earlier = requests(&mut hub, 1).await.remove(0);
    // A later `pair` of the same device id replaces the one before.
    let nonce = new_nonce();
    let mut second = ask(hub.port, &pair_message(&id, &nonce)).await;
    assert!(first.ends_within(Duration::from_secs(2)).await);
    let deadline = tokio::time::Instant::now() + WAIT;
    while hub.endpoint.pair_requests().iter().any(|request| request.peer == earlier.peer) {
        assert!(tokio::time::Instant::now() < deadline, "the earlier request is still listed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let waiting = hub.endpoint.pair_requests();
    assert_eq!(waiting.len(), 1);
    assert_ne!(waiting[0].peer, earlier.peer);

    // A string that is no code is refused before anything is sent, and an
    // id that asks for nothing has no request.
    for not_a_code in ["", "31415", "3141592", "31415a", " 14159", "٣١٤١٥٩"] {
        assert_eq!(hub.endpoint.pair(&id, not_a_code).await, Err(PairError::NotACode), "{not_a_code:?}");
    }
    assert_eq!(hub.endpoint.pair(&new_link_id(), CODE).await, Err(PairError::NoRequest));
    assert_eq!(hub.endpoint.pair_requests().len(), 1);

    // While an offer is out the request waits for the device, not for a
    // code: it is not listed, takes no second code and cannot be dismissed.
    let endpoint = &hub.endpoint;
    let (paired, ()) = tokio::join!(endpoint.pair(&id, CODE), async {
        let offer = offer(&mut second).await;
        assert!(endpoint.pair_requests().is_empty());
        assert_eq!(endpoint.pair(&id, CODE).await, Err(PairError::NoRequest));
        assert!(!endpoint.decline_pair(&id));
        let key = pair::mask_key(CODE, &offer.key, &nonce, &offer.nonce, &id, &hub_id).unwrap();
        let proof = pair::console_proof(&key, &nonce, &offer.nonce, &id, &hub_id).unwrap();
        second.write(&pocket_link::handshake::encode(&json!({"t": "pair.accept", "proof": proof})).unwrap()).await;
    });
    assert_eq!(paired, Ok(()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_waits_past_the_handshake_timeout_and_runs_out() {
    // 6 s and 5 minutes, as 150 ms and 900 ms.
    let mut config = fast_config();
    config.timing = Timing { handshake: Duration::from_millis(150), pair_window: Duration::from_millis(900), ..config.timing };
    let mut hub = Hub::start(config, None).await;
    let id = new_link_id();
    let mut raw = ask(hub.port, &pair_message(&id, &new_nonce())).await;
    let request = requests(&mut hub, 1).await.remove(0);
    assert_eq!(request.expires - request.since, 900);
    // Past the time of an unfinished handshake the request is still there,
    // and Hub answers the device's PING meanwhile.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(hub.endpoint.pair_requests().len(), 1);
    raw.write(&frame::encode_ping(9)).await;
    let pong = raw.frame().await.unwrap();
    assert_eq!((pong.ty, &pong.payload[..]), (FrameType::Pong, &9u32.to_le_bytes()[..]));
    // At the window's end Hub closes it.
    let close = raw.frame().await.expect("the CLOSE");
    assert_eq!((close.ty, frame::parse_close(&close.payload).0), (FrameType::Close, CloseCode::HandshakeTimeout));
    assert!(raw.ends_within(Duration::from_secs(2)).await);
    assert!(requests(&mut hub, 0).await.is_empty());
    assert_eq!(hub.endpoint.pair(&id, CODE).await, Err(PairError::NoRequest));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pair_hub_cannot_read_is_malformed_and_a_device_that_leaves_is_gone() {
    let mut hub = Hub::start(fast_config(), None).await;
    let id = new_link_id();
    for (why, message) in [
        ("an uppercase device id", json!({"t": "pair", "device": id.to_uppercase(), "nonce": new_nonce(), "name": "x", "model": "y", "target": "tab5-home"})),
        ("a 31-digit nonce", json!({"t": "pair", "device": id, "nonce": &new_nonce()[1..], "name": "x", "model": "y", "target": "tab5-home"})),
        ("no model", json!({"t": "pair", "device": id, "nonce": new_nonce(), "name": "x", "target": "tab5-home"})),
        ("a target of 16 bytes", json!({"t": "pair", "device": id, "nonce": new_nonce(), "name": "x", "model": "y", "target": "t".repeat(16)})),
    ] {
        let mut raw = ask(hub.port, &message).await;
        assert_eq!(refusal(&raw.frame().await.unwrap()), "malformed", "{why}");
        assert!(raw.ends_within(Duration::from_secs(2)).await, "{why}");
    }
    assert!(hub.endpoint.pair_requests().is_empty());

    // A device that leaves takes its request with it, before and after an offer.
    let raw = ask(hub.port, &pair_message(&id, &new_nonce())).await;
    requests(&mut hub, 1).await;
    drop(raw);
    assert!(requests(&mut hub, 0).await.is_empty());
    let mut raw = ask(hub.port, &pair_message(&id, &new_nonce())).await;
    requests(&mut hub, 1).await;
    let endpoint = &hub.endpoint;
    let (paired, ()) = tokio::join!(endpoint.pair(&id, CODE), async {
        offer(&mut raw).await;
        drop(raw);
    });
    assert_eq!(paired, Err(PairError::Gone));
    assert!(hub.store.device(&id).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_count_among_the_connections_and_close_when_hub_quits() {
    let mut config = fast_config();
    config.max_connections = 2;
    let mut hub = Hub::start(config, None).await;
    let mut held = Vec::new();
    for _ in 0..2 {
        held.push(ask(hub.port, &pair_message(&new_link_id(), &new_nonce())).await);
    }
    requests(&mut hub, 2).await;
    // A third connection is over the limit: `busy`, as a `hello` there gets.
    let mut third = ask(hub.port, &pair_message(&new_link_id(), &new_nonce())).await;
    assert_eq!(refusal(&third.frame().await.unwrap()), "busy");
    assert_eq!(hub.endpoint.pair_requests().len(), 2);

    // Hub quits: each request's connection closes with `going_away`.
    hub.endpoint.shutdown().await;
    for raw in &mut held {
        let close = raw.frame().await.expect("the CLOSE");
        assert_eq!((close.ty, frame::parse_close(&close.payload).0), (FrameType::Close, CloseCode::GoingAway));
    }
}
