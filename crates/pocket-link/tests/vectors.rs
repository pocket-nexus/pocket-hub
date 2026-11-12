//! The codec against the specification's vectors
//! (`vendor/pocketjs/tests/vectors/pocket-link-2/`, docs/POCKET-LINK.md §10).

use std::collections::BTreeMap;
use std::path::PathBuf;

use pocket_link::frame::{self, Decoded, FrameDecoder, JsonFrame};
use pocket_link::handshake::{self, Refusal};
use pocket_link::pairing::{self, DirectoryEntry, ReadOutcome, SetupInput, StorageOp, Via};
use pocket_link::preamble::{self, Preamble, Range};
use pocket_link::{CloseCode, FrameType, LinkClose, beacon};
use serde_json::Value;

fn vector(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pocketjs/tests/vectors/pocket-link-2").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error} (run `git submodule update --init`)", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn s(value: &Value) -> &str {
    value.as_str().unwrap_or_else(|| panic!("not a string: {value}"))
}
fn bytes(value: &Value) -> Vec<u8> {
    hex::decode(s(value)).unwrap()
}
fn list(value: &Value) -> &Vec<Value> {
    value.as_array().unwrap_or_else(|| panic!("not an array: {value}"))
}

#[test]
fn beacons() {
    let v = vector("beacon.json");
    for case in list(&v["accepted"]) {
        let name = s(&case["name"]);
        let raw = bytes(&case["hex"]);
        let decoded = beacon::decode(&raw).unwrap_or_else(|| panic!("{name}: refused"));
        assert_eq!(decoded.port as u64, case["port"].as_u64().unwrap(), "{name}");
        assert_eq!(decoded.companion, s(&case["companion"]), "{name}");
        assert_eq!(decoded.name, s(&case["text"]), "{name}");
        if raw[5] == 0 {
            assert_eq!(beacon::encode(&decoded).unwrap().to_vec(), raw, "{name}: encodes back");
        }
    }
    for case in list(&v["refused"]) {
        assert!(beacon::decode(&bytes(&case["hex"])).is_none(), "{}", s(&case["name"]));
    }
    // A name longer than its field is cut on a code point.
    let cut = beacon::encode(&beacon::Beacon { port: 8628, companion: "fedcba9876543210".into(), name: format!("{}é", "a".repeat(39)) }).unwrap();
    assert_eq!(cut.to_vec(), bytes(&list(&v["accepted"])[2]["hex"]));
}

fn range(value: &Value) -> Range {
    Range { highest: value["highest"].as_u64().unwrap() as u8, lowest: value["lowest"].as_u64().unwrap() as u8 }
}

#[test]
fn preambles() {
    let v = vector("preamble.json");
    assert_eq!(preamble::encode(Preamble { highest: 2, lowest: 2, selected: 0 }).to_vec(), bytes(&v["device"]));
    for case in list(&v["selection"]) {
        let name = s(&case["name"]);
        let device = preamble::decode(&bytes(&case["deviceHex"])).unwrap();
        assert_eq!((device.highest, device.lowest), (range(&case["device"]).highest, range(&case["device"]).lowest), "{name}");
        let (answer, selected) = preamble::hub_answer(range(&case["hub"]), device);
        assert_eq!(selected as u64, case["selected"].as_u64().unwrap(), "{name}");
        assert_eq!(answer.to_vec(), bytes(&case["hubHex"]), "{name}");
        assert_eq!(selected == 0, case["hubCloses"].as_bool().unwrap(), "{name}");
    }
    for case in list(&v["deviceAccepts"]) {
        let hub = preamble::decode(&bytes(&case["hubHex"])).unwrap();
        let accepted = preamble::accept_selection(range(&case["device"]), hub);
        assert_eq!(accepted.map(u64::from), case["version"].as_u64(), "{}", s(&case["name"]));
    }
    assert!(preamble::decode(&bytes(&v["wrongMagic"])).is_none());
    // A several-version device that did not wait: its hello fails version 2's frame checks.
    let mut decoder = FrameDecoder::new();
    decoder.push(&bytes(&v["notWaiting"]["hex"]));
    assert_eq!(decoder.next().unwrap_err().code.name(), s(&v["notWaiting"]["close"]));
}

fn decode_all(raw: &[u8], limit: u32, linked: bool) -> Result<Vec<Decoded>, LinkClose> {
    let mut decoder = FrameDecoder::new();
    if linked {
        decoder.ready(limit);
    }
    // One byte at a time: a frame is the same however the carrier cuts it.
    let mut out = Vec::new();
    for byte in raw {
        decoder.push(std::slice::from_ref(byte));
        while let Some(decoded) = decoder.next()? {
            out.push(decoded);
        }
    }
    Ok(out)
}

#[test]
fn frames() {
    let v = vector("frames.json");
    for case in list(&v["valid"]) {
        let name = s(&case["name"]);
        let raw = bytes(&case["hex"]);
        let limit = case["limit"].as_u64().unwrap() as u32;
        let decoded = decode_all(&raw, limit, case["linked"].as_bool().unwrap()).unwrap_or_else(|close| panic!("{name}: {close}"));
        assert_eq!(decoded.len(), 1, "{name}");
        match &decoded[0] {
            Decoded::Skipped => assert!(name.contains("unknown type"), "{name}: skipped"),
            Decoded::Frame(f) => {
                let again = frame::encode(f.ty, f.flags, f.id, &f.payload, limit).unwrap();
                assert_eq!(again.to_vec(), raw, "{name}: encodes back");
            }
        }
    }
    for case in list(&v["refused"]) {
        let name = s(&case["name"]);
        let close = decode_all(&bytes(&case["hex"]), case["limit"].as_u64().unwrap() as u32, case["linked"].as_bool().unwrap()).expect_err(name);
        assert_eq!(close.code.name(), s(&case["close"]), "{name}: {close}");
    }
}

#[test]
fn binary_control_frames() {
    let v = vector("frames.json");
    let hex_of = |name: &str| bytes(&list(&v["valid"]).iter().find(|case| s(&case["name"]) == name).unwrap()["hex"]);
    assert_eq!(frame::encode_ping(7).to_vec(), hex_of("PING"));
    assert_eq!(frame::encode_pong(&7u32.to_le_bytes()).to_vec(), hex_of("PONG"));
    assert_eq!(frame::encode_close(CloseCode::GoingAway, "the device sleeps").to_vec(), hex_of("CLOSE going_away with a reason"));
    assert_eq!(frame::encode_cancel(5).to_vec(), hex_of("CANCEL"));
    assert_eq!(frame::encode_data(5, &[1, 2, 3], false).to_vec(), hex_of("DATA"));
    assert_eq!(frame::encode_data(5, &[4], true).to_vec(), hex_of("DATA with END and bytes"));
    assert_eq!(frame::encode_data(5, &[], true).to_vec(), hex_of("DATA with END and no bytes"));
    assert_eq!(frame::encode_credit(5, 65536).to_vec(), hex_of("CREDIT"));
    assert_eq!(frame::encode_reset(0x8000_0009, frame::ResetCode::Refused).to_vec(), hex_of("RESET refused"));
    let app = serde_json::json!({"t": "x.ping", "n": 1});
    let with = |channel, body| JsonFrame { error: false, channel, body };
    assert_eq!(frame::encode_json(FrameType::Request, 6, &app, with(Some(3), false), 16384).unwrap().to_vec(), hex_of("REQUEST with APP"));
    assert_eq!(frame::encode_json(FrameType::Request, 7, &app, with(Some(3), true), 16384).unwrap().to_vec(), hex_of("REQUEST with APP and BODY"));
    assert_eq!(frame::encode_json(FrameType::Event, 0x8000_0009, &app, with(Some(3), false), 16384).unwrap().to_vec(), hex_of("EVENT with APP that opens a stream"));

    let k = vector("keepalive.json");
    assert_eq!(frame::encode_ping(0x29).to_vec(), bytes(&k["ping"]));
    assert_eq!(frame::encode_pong(&0x29u32.to_le_bytes()).to_vec(), bytes(&k["pong"]));
}

#[test]
fn handshake_transcript() {
    let v = vector("handshake.json");
    let key = s(&v["key"]);
    let steps = list(&v["transcript"]);
    let json_of = |what: &str| &steps.iter().find(|step| s(&step["what"]) == what).unwrap()["json"];
    let hex_of = |what: &str, from: &str| bytes(&steps.iter().find(|step| s(&step["what"]) == what && s(&step["from"]) == from).unwrap()["hex"]);

    // The device's hello, as Hub reads it off the wire.
    let frames = decode_all(&hex_of("hello", "device"), 16384, false).unwrap();
    let Decoded::Frame(hello_frame) = &frames[0] else { panic!("hello skipped") };
    let hello = handshake::parse_hello(frame::parse_json(hello_frame).object.as_ref()).unwrap();
    assert_eq!(hello.device, "0123456789abcdef");
    assert_eq!(hello.host_abi, 12);
    assert_eq!(hello.max_payload, 16384);
    assert_eq!(handshake::encode(&hello).unwrap().to_vec(), hex_of("hello", "device"), "hello encodes back");

    // The HMAC inputs and both proofs.
    let nonce_c = s(&json_of("challenge")["nonce"]);
    assert_eq!(handshake::proof_message("pocket-link/2/companion", &hello.nonce, nonce_c, &hello.device, &hello.companion), s(&v["companionMessage"]));
    assert_eq!(handshake::proof_message("pocket-link/2/console", &hello.nonce, nonce_c, &hello.device, &hello.companion), s(&v["consoleMessage"]));
    let companion = handshake::companion_proof(key, &hello.nonce, nonce_c, &hello.device, &hello.companion).unwrap();
    let console = handshake::console_proof(key, &hello.nonce, nonce_c, &hello.device, &hello.companion).unwrap();
    assert_eq!(companion, s(&v["companionProof"]));
    assert_eq!(console, s(&v["consoleProof"]));

    // Hub's frames, byte for byte.
    let (answer, selected) = preamble::hub_answer(preamble::HUB_RANGE, preamble::decode(&hex_of("preamble", "device")).unwrap());
    assert_eq!((answer.to_vec(), selected), (hex_of("preamble", "hub"), 2));
    let challenge = handshake::Challenge::new(nonce_c.to_owned(), companion);
    assert_eq!(handshake::encode(&challenge).unwrap().to_vec(), hex_of("challenge", "hub"));
    let ready_json = json_of("ready");
    let features = handshake::parse_features(Some(&ready_json["features"])).unwrap();
    let clock = handshake::Clock { utc: ready_json["clock"]["utc"].as_i64().unwrap(), local: ready_json["clock"]["local"].as_i64().unwrap() };
    let ready = handshake::Ready::new(s(&ready_json["name"]), features, 16384, clock);
    assert_eq!(handshake::encode(&ready).unwrap().to_vec(), hex_of("ready", "hub"));

    // The device's auth, as Hub reads it.
    let frames = decode_all(&hex_of("auth", "device"), 16384, false).unwrap();
    let Decoded::Frame(auth_frame) = &frames[0] else { panic!("auth skipped") };
    let proof = handshake::parse_auth(frame::parse_json(auth_frame).object.as_ref()).unwrap();
    assert!(handshake::proof_equals(&proof, &console));
    assert!(!handshake::proof_equals(&proof, &challenge.proof));

    for case in list(&v["limits"]) {
        let limit = handshake::payload_limit(case["hello"].as_u64().unwrap() as u32, case["ready"].as_u64().unwrap() as u32);
        assert_eq!(limit as u64, case["limit"].as_u64().unwrap());
    }
}

#[test]
fn handshake_refusals() {
    let v = vector("handshake-refusals.json");
    for case in list(&v["refusals"]) {
        let refusal = match s(&case["code"]) {
            "unknown_device" => Refusal::UnknownDevice,
            "bad_proof" => Refusal::BadProof,
            "malformed" => Refusal::Malformed,
            "busy" => Refusal::Busy,
            other => panic!("unknown refusal {other}"),
        };
        let refused = handshake::Refused::new(refusal, s(&case["json"]["message"]));
        assert_eq!(handshake::encode(&refused).unwrap().to_vec(), bytes(&case["hex"]), "{}", s(&case["code"]));
    }
    for case in list(&v["malformedHello"]) {
        let object = case["json"].as_object();
        assert!(handshake::parse_hello(object).is_err(), "{}", s(&case["name"]));
    }
    for case in list(&v["unknownDevice"]) {
        // The hello itself is well formed: the refusal is the endpoint's, from its store.
        assert!(handshake::parse_hello(case["hello"].as_object()).is_ok(), "{}", s(&case["name"]));
    }
    let bad = &v["badProof"];
    let proof = handshake::parse_auth(bad["auth"].as_object()).unwrap();
    assert!(!handshake::proof_equals(&proof, s(&bad["expected"])));

    let mut decoder = FrameDecoder::new();
    decoder.push(&bytes(&v["dataBeforeReady"]["hex"]));
    assert_eq!(decoder.next().unwrap_err().code.name(), s(&v["dataBeforeReady"]["close"]));

    // The device's CLOSE auth, as Hub reads it.
    let frames = decode_all(&bytes(&v["deviceCloseAuth"]["close"]), 16384, false).unwrap();
    let Decoded::Frame(close) = &frames[0] else { panic!("close skipped") };
    assert_eq!(frame::parse_close(&close.payload).0, CloseCode::Auth);
}

fn read_outcome(value: &Value) -> ReadOutcome {
    match s(&value["kind"]) {
        "failed" => ReadOutcome::Failed,
        "absent" => ReadOutcome::Absent,
        "file" => ReadOutcome::File(s(&value["text"]).to_owned()),
        other => panic!("unknown read outcome {other}"),
    }
}

fn entries(value: &Value) -> Vec<DirectoryEntry> {
    list(value).iter().map(|entry| DirectoryEntry { name: s(&entry["name"]).to_owned(), text: s(&entry["text"]).to_owned() }).collect()
}

fn op_json(op: &StorageOp) -> Value {
    match op {
        StorageOp::Write { path, text } => serde_json::json!({"op": "write", "path": path, "text": text}),
        StorageOp::Readback { path, text } => serde_json::json!({"op": "readback", "path": path, "text": text}),
        StorageOp::Delete { path } => serde_json::json!({"op": "delete", "path": path}),
        StorageOp::Rename { from, to } => serde_json::json!({"op": "rename", "from": from, "to": to}),
    }
}

fn setup_input(input: &Value) -> SetupInput {
    SetupInput {
        hub_id: s(&input["hubId"]).to_owned(),
        key: input["key"].as_str().map(str::to_owned),
        hub_name: s(&input["hub"]["name"]).to_owned(),
        hub_port: input["hub"]["port"].as_u64().unwrap() as u16,
        via: match s(&input["via"]["kind"]) {
            "ftp" => Via::Ftp { local_address: s(&input["via"]["localAddress"]).to_owned() },
            _ => Via::Card { addresses: list(&input["via"]["addresses"]).iter().map(|a| s(a).to_owned()).collect() },
        },
        device: read_outcome(&input["device"]),
        listing: entries(&input["listing"]),
        companion: read_outcome(&input["companion"]),
        last: read_outcome(&input["last"]),
    }
}

#[test]
fn pairing_files() {
    let v = vector("pairing-files.json");
    assert_eq!(pairing::format_device_file(s(&v["deviceFile"]["id"])), s(&v["deviceFile"]["text"]));
    assert_eq!(pairing::format_device_file(s(&v["deviceFile"]["id"])).as_bytes(), bytes(&v["deviceFile"]["hex"]));

    let parsed = &v["companionFile"]["parsed"];
    let file = pairing::CompanionFile {
        key: s(&parsed["key"]).to_owned(),
        name: s(&parsed["name"]).to_owned(),
        address: s(&parsed["address"]).to_owned(),
        port: parsed["port"].as_u64().unwrap() as u16,
    };
    assert_eq!(pairing::format_companion_file(&file).as_bytes(), bytes(&v["companionFile"]["hex"]));
    assert_eq!(pairing::parse_companion_file(s(&v["companionFile"]["text"])), Some(file));

    let parsed = &v["lastFile"]["parsed"];
    let last = pairing::LastFile { name: s(&parsed["name"]).to_owned(), address: s(&parsed["address"]).to_owned(), port: parsed["port"].as_u64().unwrap() as u16 };
    assert_eq!(pairing::format_last_file(&last), s(&v["lastFile"]["text"]));
    assert_eq!(pairing::parse_last_file(s(&v["lastFile"]["text"])), Some(last));

    for case in list(&v["deviceFileReads"]) {
        assert_eq!(pairing::parse_device_file(s(&case["text"])), case["id"].as_str(), "{}", s(&case["name"]));
    }
    for case in list(&v["companionFileReads"]) {
        let name = s(&case["name"]);
        let read = pairing::parse_companion_file(s(&case["text"]));
        match case["parsed"].as_object() {
            None => assert!(read.is_none(), "{name}"),
            Some(expected) => {
                let read = read.unwrap_or_else(|| panic!("{name}: refused"));
                assert_eq!(read.key, s(&expected["key"]), "{name}");
                assert_eq!(read.name, s(&expected["name"]), "{name}");
                assert_eq!(read.address, s(&expected["address"]), "{name}");
                assert_eq!(read.port as u64, expected["port"].as_u64().unwrap(), "{name}");
            }
        }
    }
    for case in list(&v["load"]) {
        let name = s(&case["name"]);
        let (companions, error) = pairing::load_companions(&entries(&case["entries"]));
        assert_eq!(error, case["error"].as_str(), "{name}");
        let expected = list(&case["companions"]);
        assert_eq!(companions.len(), expected.len(), "{name}");
        for (got, want) in companions.iter().zip(expected) {
            assert_eq!(got.id, s(&want["id"]), "{name}");
            assert_eq!(got.key, s(&want["key"]), "{name}");
            assert_eq!(got.name, s(&want["name"]), "{name}");
            assert_eq!(got.address, s(&want["address"]), "{name}");
            assert_eq!(got.port as u64, want["port"].as_u64().unwrap(), "{name}");
        }
    }
}

#[test]
fn setup_plans() {
    let v = vector("pairing-files.json");
    for case in list(&v["setups"]) {
        let name = s(&case["name"]);
        let input = setup_input(&case["input"]);
        let device_ids = list(&case["minted"]["deviceIds"]);
        let keys = list(&case["minted"]["keys"]);
        let plan = pairing::plan_setup(&input, || s(&device_ids[0]).to_owned(), || s(&keys[0]).to_owned());
        let want = &case["plan"];
        assert_eq!(plan.outcome.name(), s(&want["outcome"]), "{name}");
        assert_eq!(plan.device_id.as_deref(), want["deviceId"].as_str(), "{name}: device id");
        assert_eq!(plan.key.as_deref(), want["key"].as_str(), "{name}: key");
        assert_eq!(plan.minted.as_deref(), want["minted"].as_str(), "{name}: minted");
        assert_eq!(plan.message, s(&want["message"]), "{name}");
        let ops: Vec<Value> = plan.ops.iter().map(op_json).collect();
        assert_eq!(&ops, list(&want["ops"]), "{name}: ops");
    }
}

#[test]
fn setup_power_cuts() {
    let v = vector("pairing-files.json");
    let cut = &v["powerCut"];
    let before: BTreeMap<String, String> = cut["before"].as_object().unwrap().iter().map(|(path, text)| (path.clone(), s(text).to_owned())).collect();
    // The card holds another key than the record's, so the setup mints one.
    let hub_id = "fedcba9876543210";
    let read = |path: &str| before.get(path).map_or(ReadOutcome::Absent, |text| ReadOutcome::File(text.clone()));
    let listing: Vec<DirectoryEntry> = before
        .iter()
        .filter_map(|(path, text)| path.strip_prefix("companions/").map(|name| DirectoryEntry { name: name.to_owned(), text: text.clone() }))
        .collect();
    let input = SetupInput {
        hub_id: hub_id.to_owned(),
        key: Some(s(&cut["key"]).to_owned()),
        hub_name: "Studio Mac".to_owned(),
        hub_port: 8628,
        via: Via::Ftp { local_address: "192.168.1.5".to_owned() },
        device: read("device.txt"),
        listing,
        companion: read(&format!("companions/{hub_id}.txt")),
        last: read(&format!("companions/{hub_id}.last")),
    };
    let plan = pairing::plan_setup(&input, || unreachable!("the card has an id"), || "f".repeat(64));
    assert_eq!(plan.outcome, pairing::SetupOutcome::Key);
    for case in list(&cut["cuts"]) {
        let completed = case["completed"].as_u64().unwrap() as usize;
        let mut card = before.clone();
        for op in &plan.ops[..completed] {
            match op {
                StorageOp::Write { path, text } => {
                    card.insert(path.clone(), text.clone());
                }
                StorageOp::Delete { path } => {
                    card.remove(path);
                }
                StorageOp::Rename { from, to } => {
                    if let Some(text) = card.remove(from) {
                        card.insert(to.clone(), text);
                    }
                }
                StorageOp::Readback { .. } => {}
            }
        }
        let mut holds: Vec<&str> = card.keys().map(String::as_str).collect();
        let mut want: Vec<&str> = list(&case["holds"]).iter().map(s).collect();
        holds.sort_unstable();
        want.sort_unstable();
        assert_eq!(holds, want, "cut after {completed} operations");
        let failed = pairing::setup_failed_at(&plan, completed).is_some();
        assert_eq!(failed, completed < plan.ops.len(), "cut after {completed} operations");
    }
}
