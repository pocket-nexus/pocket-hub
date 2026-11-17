//! The store (SERVICE-PROTOCOL.md §7) and the host connection that reaches it
//! (§6), driven with frames written here: the file as the page makes it,
//! every call, the limits, `list`'s paging, `CANCEL` before and after a
//! body's last byte, a newer format left untouched, a home with a space in
//! its path, the quota under concurrent writers, and the durability of an
//! acknowledged `set` when the host is killed.

mod common;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use pocket_link::frame::{encode_cancel, encode_credit, encode_data, encode_in, parse_json, Decoded, Dialect, FLAG_BODY, FLAG_END, FLAG_ERROR};
use pocket_link::{Frame, FrameDecoder, FrameType};
use serde_json::{json, Value};
use service_host::host::{self, HostSession, StoreBinding};
use service_host::store::{Store, StoreError, Stores};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const MIB: usize = 1024 * 1024;

fn pattern(length: usize, seed: usize) -> Vec<u8> {
    (0..length).map(|i| ((i * 31 + seed * 7 + (i >> 8)) & 0xff) as u8).collect()
}

/// A session with `store` required, on `stores`' store `name`, under `max`.
fn session(stores: Arc<Stores>, name: &str, max: u64, written: Arc<AtomicU64>) -> HostSession {
    HostSession {
        app: "dev.test.store".into(),
        requires: vec!["store".into()],
        store: Some(StoreBinding {
            stores,
            name: name.into(),
            max_bytes: Arc::new(move || max),
            written: Arc::new(move || {
                written.fetch_add(1, Ordering::Relaxed);
            }),
        }),
    }
}

/// A host connection served in this process; the client's end.
fn connect(session: HostSession) -> Client {
    let (ours, theirs) = UnixStream::pair().unwrap();
    tokio::spawn(host::serve(theirs, session));
    Client::new(ours)
}

/// One call: its `t` and arguments, and the body of a `store.set`.
struct Call {
    id: u32,
    object: Value,
    body: Option<Vec<u8>>,
}

fn call(id: u32, object: Value) -> Call {
    Call { id, object, body: None }
}

fn set(id: u32, key: &str, value: &[u8]) -> Call {
    Call { id, object: json!({"t": "store.set", "key": key, "bytes": value.len()}), body: Some(value.to_vec()) }
}

#[derive(Debug, Default, Clone)]
struct Outcome {
    reply: Option<Result<Value, Value>>,
    value: Vec<u8>,
    head: Option<Value>,
    reset: bool,
}

/// The service's end of a host connection, written from the page: requests
/// with ids that increase, bodies within the host's credit, values granted
/// 256 KiB at a time.
struct Client {
    stream: UnixStream,
    decoder: FrameDecoder,
    buffer: Vec<u8>,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        Self { stream, decoder: FrameDecoder::local(host::PAYLOAD_LIMIT), buffer: vec![0; 64 * 1024] }
    }

    async fn send(&mut self, bytes: &[u8]) {
        self.stream.write_all(bytes).await.unwrap();
    }

    /// The next frame; None once the host closed the connection.
    async fn next(&mut self) -> Option<Frame> {
        loop {
            match self.decoder.next().expect("the host's frames are well formed") {
                Some(Decoded::Frame(frame)) => return Some(frame),
                Some(Decoded::Skipped) => continue,
                None => {}
            }
            let n = self.stream.read(&mut self.buffer).await.ok()?;
            if n == 0 {
                return None;
            }
            self.decoder.push(&self.buffer[..n]);
        }
    }

    fn request(id: u32, object: &Value, body: bool) -> Bytes {
        encode_in(Dialect::Local, FrameType::Request, if body { FLAG_BODY } else { 0 }, id, &serde_json::to_vec(object).unwrap(), host::PAYLOAD_LIMIT).unwrap()
    }

    /// Sends every call at once, then answers the host's frames until each
    /// has its REPLY. `after_body` runs once a call's last body byte is
    /// written, with the client, to cancel after it.
    async fn exchange(&mut self, calls: Vec<Call>) -> HashMap<u32, Outcome> {
        let mut bodies: HashMap<u32, (Vec<u8>, usize)> = HashMap::new();
        let mut outcomes: HashMap<u32, Outcome> = HashMap::new();
        let mut granted: HashMap<u32, u64> = HashMap::new();
        for call in &calls {
            outcomes.insert(call.id, Outcome::default());
            self.send(&Self::request(call.id, &call.object, call.body.is_some())).await;
            if let Some(body) = &call.body {
                if body.is_empty() {
                    self.send(&encode_data(call.id, &[], true)).await;
                } else {
                    bodies.insert(call.id, (body.clone(), 0));
                }
            }
        }
        while outcomes.values().any(|outcome| outcome.reply.is_none()) {
            let Some(frame) = self.next().await else { break };
            let id = frame.id;
            match frame.ty {
                FrameType::Reply => {
                    let object = Value::Object(parse_json(&frame).object.unwrap_or_default());
                    let outcome = outcomes.get_mut(&id).expect("a reply to a call made");
                    assert!(outcome.reply.is_none(), "one REPLY per request");
                    outcome.reply = Some(if frame.flags & FLAG_ERROR != 0 { Err(object) } else { Ok(object) });
                    bodies.remove(&id);
                }
                FrameType::Credit => {
                    let mut credit = pocket_link::frame::parse_credit(&frame.payload) as usize;
                    let mut frames = Vec::new();
                    if let Some((body, sent)) = bodies.get_mut(&id) {
                        while credit > 0 && *sent < body.len() {
                            let n = credit.min(body.len() - *sent).min(host::PAYLOAD_LIMIT as usize);
                            let end = *sent + n == body.len();
                            frames.push(encode_data(id, &body[*sent..*sent + n], end));
                            *sent += n;
                            credit -= n;
                        }
                    }
                    for frame in frames {
                        self.send(&frame).await;
                    }
                }
                FrameType::Event => {
                    outcomes.get_mut(&id).unwrap().head = Some(Value::Object(parse_json(&frame).object.unwrap_or_default()));
                    granted.insert(id, 256 * 1024);
                    self.send(&encode_credit(id, 256 * 1024)).await;
                }
                FrameType::Data => {
                    let outcome = outcomes.get_mut(&id).unwrap();
                    outcome.value.extend_from_slice(&frame.payload);
                    let left = granted.get_mut(&id).unwrap();
                    *left -= frame.payload.len() as u64;
                    if *left < 64 * 1024 && frame.flags & FLAG_END == 0 {
                        let more = 256 * 1024 - *left;
                        *left += more;
                        self.send(&encode_credit(id, more as u32)).await;
                    }
                }
                FrameType::Reset => outcomes.get_mut(&id).unwrap().reset = true,
                other => panic!("the host sent {other:?}"),
            }
        }
        outcomes
    }

    async fn one(&mut self, call: Call) -> Outcome {
        let id = call.id;
        self.exchange(vec![call]).await.remove(&id).unwrap()
    }
}

fn ok(outcome: &Outcome) -> &Value {
    match &outcome.reply {
        Some(Ok(value)) => value,
        other => panic!("not ok: {other:?}"),
    }
}

fn code(outcome: &Outcome) -> &str {
    match &outcome.reply {
        Some(Err(error)) => error["code"].as_str().unwrap(),
        other => panic!("no error: {other:?}"),
    }
}

/// The sum the quota counts, read from the entries themselves, and `meta`.
fn sums(file: &Path) -> ((i64, i64), (i64, i64)) {
    let connection = rusqlite::Connection::open(file).unwrap();
    let counted = connection.query_row("SELECT coalesce(sum(length(key) + length(value)), 0), count(*) FROM entries", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    let meta = connection.query_row("SELECT bytes, keys FROM meta WHERE one = 1", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    (counted, meta)
}

#[test]
fn the_file_is_made_as_the_page_writes_it() {
    let home = common::temp_home();
    let folder = home.path().join("service-store/dev.x@dev");
    let store = Store::open(&folder).unwrap();
    drop(store);
    assert_eq!(std::fs::metadata(home.path().join("service-store")).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(folder.join("store.sqlite")).unwrap().permissions().mode() & 0o777, 0o600);
    let connection = rusqlite::Connection::open(folder.join("store.sqlite")).unwrap();
    let pragma = |name: &str| -> String { connection.query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, rusqlite::types::Value>(0)).map(|value| format!("{value:?}")).unwrap() };
    assert_eq!(pragma("auto_vacuum"), "Integer(2)", "INCREMENTAL");
    assert_eq!(pragma("journal_mode"), "Text(\"wal\")");
    assert_eq!(pragma("user_version"), "Integer(1)");
    drop(connection);
    // The sync settings of a connection the store opens (§7, "Durable").
    let store = Store::open(&folder).unwrap();
    let full = if cfg!(target_os = "macos") { 1 } else { 0 };
    assert_eq!((store.pragma("synchronous").unwrap(), store.pragma("fullfsync").unwrap(), store.pragma("checkpoint_fullfsync").unwrap()), (2, full, full));
    drop(store);
    let connection = rusqlite::Connection::open(folder.join("store.sqlite")).unwrap();
    let tables: Vec<(String, String)> = connection.prepare("SELECT name, sql FROM sqlite_schema WHERE type = 'table' ORDER BY name").unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(tables.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["entries", "meta"]);
    assert!(tables[0].1.contains("key   BLOB PRIMARY KEY NOT NULL") && tables[0].1.ends_with("WITHOUT ROWID"), "{}", tables[0].1);
    assert!(tables[1].1.contains("CHECK (one = 1)") && tables[1].1.contains("CHECK (bytes >= 0)"), "{}", tables[1].1);
    assert_eq!(sums(&folder.join("store.sqlite")), ((0, 0), (0, 0)));
}

#[test]
fn a_limit_below_the_usage_deletes_nothing_and_lets_the_store_shrink() {
    let home = common::temp_home();
    let store = Store::open(&home.path().join("s")).unwrap();
    store.set(b"a", &[1; 600], 10_000).unwrap();
    store.set(b"b", &[2; 400], 10_000).unwrap();
    assert_eq!(store.meta().unwrap().bytes, 1002);
    // A newer release's maxBytes of 500: nothing is deleted.
    assert!(matches!(store.set(b"c", &[3; 1], 500), Err(StoreError::Quota { would: 1004, max: 500 })));
    assert!(matches!(store.set(b"b", &[2; 401], 500), Err(StoreError::Quota { .. })), "a set that raises meta.bytes fails");
    store.set(b"b", &[2; 400], 500).expect("a set that leaves it as it is succeeds");
    store.set(b"a", &[1; 100], 500).expect("a set that lowers it succeeds");
    assert!(store.delete(b"b").unwrap());
    assert_eq!(store.meta().unwrap().bytes, 101);
    assert!(matches!(store.set(b"d", &[4; 500], 500), Err(StoreError::Quota { would: 602, max: 500 })), "the limit binds again");
    assert_eq!(store.get(b"a").unwrap().unwrap().len(), 100);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_call_and_its_errors() {
    let home = common::temp_home();
    let stores = Arc::new(Stores::new(home.path()));
    let written = Arc::new(AtomicU64::new(0));
    let mut client = connect(session(stores.clone(), "dev.x@dev", 4 * MIB as u64 + 1000, written.clone()));

    assert_eq!(ok(&client.one(call(1, json!({"t": "store.get", "key": "nothing"}))).await), &json!({"absent": true}));
    assert_eq!(ok(&client.one(set(2, "k", b"value")).await), &json!({}));
    let got = client.one(call(3, json!({"t": "store.get", "key": "k"}))).await;
    assert_eq!((ok(&got), got.head.as_ref(), got.value.as_slice()), (&json!({"bytes": 5}), Some(&json!({"t": "store.head", "bytes": 5})), &b"value"[..]));
    // An empty value, and the largest one (4 MiB, through several windows each way).
    assert_eq!(ok(&client.one(set(4, "empty", b"")).await), &json!({}));
    assert_eq!(client.one(call(5, json!({"t": "store.get", "key": "empty"}))).await.value.len(), 0);
    let big = pattern(4 * MIB, 1);
    assert_eq!(ok(&client.one(set(6, "big", &big)).await), &json!({}));
    let back = client.one(call(7, json!({"t": "store.get", "key": "big"}))).await;
    assert!(back.value == big, "4 MiB back as written");
    assert_eq!(ok(&client.one(call(8, json!({"t": "store.delete", "key": "k"}))).await), &json!({"existed": true}));
    assert_eq!(ok(&client.one(call(9, json!({"t": "store.delete", "key": "k"}))).await), &json!({"existed": false}));
    assert_eq!(ok(&client.one(call(10, json!({"t": "store.usage"}))).await), &json!({"bytes": 5 + 3 + 4 * MIB, "keys": 2, "maxBytes": 4 * MIB + 1000}));
    assert_eq!(written.load(Ordering::Relaxed), 4, "each committed write is told");

    // The errors, in the page's order of checks.
    for (id, object, body, expected) in [
        (11, json!({"t": "store.get"}), false, "invalid"),
        (12, json!({"t": "store.get", "key": ""}), false, "invalid"),
        (13, json!({"t": "store.get", "key": "a\u{7}b"}), false, "invalid"),
        (14, json!({"t": "store.get", "key": "x".repeat(257)}), false, "too_large"),
        (15, json!({"t": "store.set", "key": "k", "bytes": 4 * MIB + 1}), true, "too_large"),
        (16, json!({"t": "store.set", "key": "k", "bytes": 1}), false, "invalid"),
        (17, json!({"t": "store.list", "limit": 1001}), false, "invalid"),
        (18, json!({"t": "net.fetch", "url": "https://example.com/"}), false, "not_required"),
        (19, json!({"t": "media.open"}), false, "not_required"),
        (20, json!({"t": "nothing.here"}), false, "invalid"),
        (21, json!({"x": 1}), false, "invalid"),
    ] {
        let outcome = client.one(Call { id, object: object.clone(), body: body.then(Vec::new) }).await;
        assert_eq!(code(&outcome), expected, "{object}");
    }
    // A key of 256 bytes is a key; a value that passes the limit is `quota`.
    assert_eq!(ok(&client.one(set(22, &"x".repeat(256), b"1")).await), &json!({}));
    assert_eq!(code(&client.one(set(23, "more", &pattern(2000, 2))).await), "quota");
    // A service whose manifest does not require store is answered `not_required`.
    let mut bare = connect(HostSession { app: "dev.bare".into(), requires: vec![], store: None });
    assert_eq!(code(&bare.one(call(1, json!({"t": "store.usage"}))).await), "not_required");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_pages_through_keys_that_sort_awkwardly() {
    let home = common::temp_home();
    let stores = Arc::new(Stores::new(home.path()));
    let mut client = connect(session(stores.clone(), "dev.x@dev", 1 << 20, Arc::default()));
    // A key that is a prefix of others, `/`, a space, bytes above 0x7F of
    // two, three and four bytes, and the highest code points.
    let keys = ["a", "a b", "a/b", "aa", "ab", "a~", "aé", "a日", "a😀", "b", "é", "ÿ", "日本", "😀", "\u{10ffff}", "\u{fffd}"];
    for (i, key) in keys.iter().enumerate() {
        assert_eq!(ok(&client.one(set(i as u32 + 1, key, key.as_bytes())).await), &json!({}));
    }
    let mut sorted: Vec<&str> = keys.to_vec();
    sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut id = 100;
    for (prefix, limit) in [("", 1), ("", 3), ("", 1000), ("a", 2), ("a", 1000), ("é", 1), ("😀", 1), ("z", 5)] {
        let mut seen: Vec<String> = Vec::new();
        let mut after: Option<String> = None;
        loop {
            id += 1;
            let mut object = json!({"t": "store.list", "prefix": prefix, "limit": limit});
            if let Some(after) = &after {
                object["after"] = json!(after);
            }
            let page = ok(&client.one(call(id, object)).await).clone();
            let keys: Vec<String> = page["keys"].as_array().unwrap().iter().map(|key| key.as_str().unwrap().to_owned()).collect();
            assert!(keys.len() <= limit);
            let more = page["more"].as_bool().unwrap();
            seen.extend(keys.iter().cloned());
            if !more {
                break;
            }
            after = keys.last().cloned();
        }
        let expected: Vec<String> = sorted.iter().filter(|key| key.as_bytes().starts_with(prefix.as_bytes())).map(|key| key.to_string()).collect();
        assert_eq!(seen, expected, "prefix {prefix:?}, limit {limit}");
    }
}

/// §7 `CANCEL`: before the body's last byte nothing is written and the reply
/// is `cancelled`, with RESET for the body; after it the transaction
/// completes and the set's own reply is sent. A read is answered `cancelled`.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_before_and_after_the_last_byte() {
    let home = common::temp_home();
    let stores = Arc::new(Stores::new(home.path()));
    let mut client = connect(session(stores.clone(), "dev.x@dev", 1 << 24, Arc::default()));
    let value = pattern(100_000, 3);

    // Before: half the body, then CANCEL.
    client.send(&Client::request(1, &json!({"t": "store.set", "key": "before", "bytes": value.len()}), true)).await;
    let credit = client.next().await.unwrap();
    assert_eq!((credit.ty, credit.id), (FrameType::Credit, 1));
    client.send(&encode_data(1, &value[..50_000], false)).await;
    client.send(&encode_cancel(1)).await;
    let (mut reset, mut reply) = (false, None);
    while reply.is_none() {
        let frame = client.next().await.unwrap();
        match frame.ty {
            FrameType::Reset => reset = true,
            FrameType::Reply => reply = Some((frame.flags & FLAG_ERROR != 0, Value::Object(parse_json(&frame).object.unwrap()))),
            FrameType::Credit => {}
            other => panic!("{other:?}"),
        }
    }
    assert!(reset, "RESET for the body");
    assert_eq!(reply, Some((true, json!({"code": "cancelled", "message": "the service cancelled the call"}))));
    // Bytes that were in flight within the credit are dropped, not a fault.
    client.send(&encode_data(1, &value[50_000..60_000], false)).await;
    assert_eq!(ok(&client.one(call(2, json!({"t": "store.get", "key": "before"}))).await), &json!({"absent": true}));

    // After: the last byte and the CANCEL in one write.
    client.send(&Client::request(3, &json!({"t": "store.set", "key": "after", "bytes": 10}), true)).await;
    assert_eq!(client.next().await.unwrap().ty, FrameType::Credit);
    let mut both = encode_data(3, &value[..10], true).to_vec();
    both.extend_from_slice(&encode_cancel(3));
    client.send(&both).await;
    let frame = client.next().await.unwrap();
    assert_eq!((frame.ty, frame.id, frame.flags & FLAG_ERROR), (FrameType::Reply, 3, 0), "the set's own reply");
    assert_eq!(client.one(call(4, json!({"t": "store.get", "key": "after"}))).await.value, &value[..10]);

    // A read that is cancelled while its value goes out under no credit.
    client.send(&Client::request(5, &json!({"t": "store.get", "key": "after"}), false)).await;
    assert_eq!(client.next().await.unwrap().ty, FrameType::Event, "the head event");
    client.send(&encode_cancel(5)).await;
    let mut kinds = Vec::new();
    while kinds.last() != Some(&FrameType::Reply) {
        kinds.push(client.next().await.unwrap().ty);
    }
    assert_eq!(kinds, [FrameType::Reset, FrameType::Reply]);
}

/// A store whose `user_version` is above this host's: `store_format` on the
/// host connection, and the file not touched (§7).
#[tokio::test(flavor = "multi_thread")]
async fn a_newer_format_is_refused_untouched() {
    let home = common::temp_home();
    let folder = home.path().join("service-store/dev.x@dev");
    std::fs::create_dir_all(&folder).unwrap();
    {
        let newer = rusqlite::Connection::open(folder.join("store.sqlite")).unwrap();
        newer.execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE later (x); PRAGMA user_version = 2;").unwrap();
    }
    let before: Vec<_> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap()).map(|e| (e.file_name(), std::fs::read(e.path()).unwrap(), e.metadata().unwrap().modified().unwrap())).collect();
    assert!(matches!(Store::open(&folder), Err(StoreError::Format(2))));
    let stores = Arc::new(Stores::new(home.path()));
    let mut client = connect(session(stores.clone(), "dev.x@dev", 1 << 20, Arc::default()));
    for (id, object) in [(1, json!({"t": "store.get", "key": "k"})), (2, json!({"t": "store.usage"}))] {
        assert_eq!(code(&client.one(call(id, object)).await), "store_format");
    }
    assert_eq!(code(&client.one(set(3, "k", b"v")).await), "store_format");
    let after: Vec<_> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap()).map(|e| (e.file_name(), std::fs::read(e.path()).unwrap(), e.metadata().unwrap().modified().unwrap())).collect();
    assert_eq!(after, before, "no byte, time or file changed");
}

/// A home whose path has a space in it: the store and the host connection
/// work there as anywhere.
#[tokio::test(flavor = "multi_thread")]
async fn a_home_with_a_space_in_its_path() {
    let temp = common::temp_home();
    let home = temp.path().join("Pocket Hub home");
    std::fs::create_dir_all(&home).unwrap();
    let stores = Arc::new(Stores::new(&home));
    let mut client = connect(session(stores.clone(), "dev.x@dev", 1 << 20, Arc::default()));
    assert_eq!(ok(&client.one(set(1, "k", b"spaced")).await), &json!({}));
    assert_eq!(client.one(call(2, json!({"t": "store.get", "key": "k"}))).await.value, b"spaced");
    assert!(home.join("service-store/dev.x@dev/store.sqlite").is_file());
}

/// The quota is exact under concurrency: four connections (four processes
/// of one id) with eight sets in flight each, at the limit's edge. Sampled
/// all along, `meta.bytes` never passes `maxBytes`; afterwards it is the sum
/// of the entries, and the writes that fit are the ones that succeeded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_quota_is_exact_under_concurrent_writers() {
    let home = common::temp_home();
    let stores = Arc::new(Stores::new(home.path()));
    const MAX: u64 = 100_000;
    let store = stores.get("dev.x@dev").unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let highest = Arc::new(AtomicU64::new(0));
    let sampler = {
        let (store, running, highest) = (store.clone(), running.clone(), highest.clone());
        std::thread::spawn(move || {
            let mut samples = 0u64;
            while running.load(Ordering::Relaxed) {
                highest.fetch_max(store.meta().unwrap().bytes, Ordering::Relaxed);
                samples += 1;
            }
            samples
        })
    };
    let mut tasks = Vec::new();
    for connection in 0..4u32 {
        let stores = stores.clone();
        tasks.push(tokio::spawn(async move {
            let mut client = connect(session(stores, "dev.x@dev", MAX, Arc::default()));
            let (mut ok_sets, mut quota) = (0u32, 0u32);
            let mut id = 0;
            for round in 0..20u32 {
                let calls: Vec<Call> = (0..8u32)
                    .map(|n| {
                        id += 1;
                        // New keys, and replacements of keys of earlier rounds that grow or shrink them.
                        let key = format!("c{connection}-{}", (round * 8 + n) % 60);
                        let size = 700 + ((round * 131 + n * 17 + connection * 7) % 900) as usize;
                        set(id, &key, &pattern(size, id as usize))
                    })
                    .collect();
                for outcome in client.exchange(calls).await.values() {
                    match &outcome.reply {
                        Some(Ok(_)) => ok_sets += 1,
                        Some(Err(error)) if error["code"] == "quota" => quota += 1,
                        other => panic!("{other:?}"),
                    }
                }
            }
            (ok_sets, quota)
        }));
    }
    let (mut ok_sets, mut quota) = (0, 0);
    for task in tasks {
        let (a, b) = task.await.unwrap();
        ok_sets += a;
        quota += b;
    }
    running.store(false, Ordering::Relaxed);
    let samples = sampler.join().unwrap();
    let file = home.path().join("service-store/dev.x@dev/store.sqlite");
    let ((sum, count), (bytes, keys)) = sums(&file);
    eprintln!("quota: {ok_sets} sets succeeded, {quota} refused `quota`; meta.bytes {bytes} of {MAX}, highest sampled {} in {samples} samples", highest.load(Ordering::Relaxed));
    assert!(quota > 0 && ok_sets > 0, "the limit's edge was reached");
    assert!(highest.load(Ordering::Relaxed) <= MAX);
    assert_eq!((sum, count), (bytes, keys), "meta agrees with the entries");
    assert!(bytes as u64 <= MAX && bytes as u64 > MAX - 2000, "{bytes}");
}

/// The child half of `an_acknowledged_set_survives_sigkill`: serves host
/// connections on the socket it is given until it is killed. Run alone it
/// does nothing.
#[test]
fn durability_child() {
    let (Some(socket), Some(home)) = (std::env::var_os("PH_STORE_CHILD_SOCKET"), std::env::var_os("PH_STORE_CHILD_HOME")) else { return };
    tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let stores = Arc::new(Stores::new(Path::new(&home)));
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(host::serve(stream, session(stores.clone(), "dev.x@dev", 1 << 28, Arc::default())));
        }
    });
}

/// Durability, measured: a host process killed with SIGKILL in the middle of
/// a burst of `set`s, eight in flight. Reopened, every acknowledged `set`
/// is there with its value; one in flight is whole or absent; `meta`
/// agrees with the entries. `PH_DURABILITY_RUNS` sets the number of kills
/// (20 by default).
#[tokio::test(flavor = "multi_thread")]
async fn an_acknowledged_set_survives_sigkill() {
    let runs: u32 = std::env::var("PH_DURABILITY_RUNS").ok().and_then(|runs| runs.parse().ok()).unwrap_or(20);
    let exe = std::env::current_exe().unwrap();
    let (mut acked_total, mut in_flight_whole, mut in_flight_absent) = (0u64, 0u64, 0u64);
    for run in 0..runs {
        let home = common::temp_home();
        let socket = home.path().join("h.sock");
        let mut child = std::process::Command::new(&exe)
            .args(["--exact", "durability_child", "--nocapture", "--test-threads=1"])
            .env("PH_STORE_CHILD_SOCKET", &socket)
            .env("PH_STORE_CHILD_HOME", home.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stream = loop {
            match UnixStream::connect(&socket).await {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        };
        let mut client = Client::new(stream);
        // Kill after a number of acknowledgements that differs per run.
        let kill_after = 1 + (run as u64 * 37) % 150;
        let value_of = |i: u32| pattern(1 + (i as usize * 997) % 30_000, i as usize);
        let (mut next, mut acked) = (1u32, Vec::<u32>::new());
        let mut bodies: HashMap<u32, (Vec<u8>, usize)> = HashMap::new();
        let mut in_flight: Vec<u32> = Vec::new();
        'burst: loop {
            while in_flight.len() < 8 {
                let value = value_of(next);
                client.send(&Client::request(next, &json!({"t": "store.set", "key": format!("k{next}"), "bytes": value.len()}), true)).await;
                bodies.insert(next, (value, 0));
                in_flight.push(next);
                next += 1;
            }
            let frame = client.next().await.expect("the host is alive until it is killed");
            match frame.ty {
                FrameType::Credit => {
                    let mut credit = pocket_link::frame::parse_credit(&frame.payload) as usize;
                    let mut frames = Vec::new();
                    if let Some((body, sent)) = bodies.get_mut(&frame.id) {
                        while credit > 0 && *sent < body.len() {
                            let n = credit.min(body.len() - *sent).min(host::PAYLOAD_LIMIT as usize);
                            frames.push(encode_data(frame.id, &body[*sent..*sent + n], *sent + n == body.len()));
                            *sent += n;
                            credit -= n;
                        }
                    }
                    for frame in frames {
                        client.send(&frame).await;
                    }
                }
                FrameType::Reply => {
                    assert_eq!(frame.flags & FLAG_ERROR, 0, "{:?}", parse_json(&frame).object);
                    acked.push(frame.id);
                    in_flight.retain(|id| *id != frame.id);
                    bodies.remove(&frame.id);
                    if acked.len() as u64 >= kill_after {
                        // SAFETY: a process this test started, by its pid.
                        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGKILL) };
                        break 'burst;
                    }
                }
                other => panic!("{other:?}"),
            }
        }
        child.wait().unwrap();
        let store = Store::open(&home.path().join("service-store/dev.x@dev")).unwrap();
        for id in &acked {
            assert_eq!(store.get(format!("k{id}").as_bytes()).unwrap(), Some(value_of(*id)), "run {run}: acknowledged set k{id}");
        }
        for id in &in_flight {
            match store.get(format!("k{id}").as_bytes()).unwrap() {
                Some(value) => {
                    assert_eq!(value, value_of(*id), "run {run}: k{id} is whole");
                    in_flight_whole += 1;
                }
                None => in_flight_absent += 1,
            }
        }
        drop(store);
        let ((sum, count), (bytes, keys)) = sums(&home.path().join("service-store/dev.x@dev/store.sqlite"));
        assert_eq!((sum, count), (bytes, keys), "run {run}: meta agrees with the entries");
        acked_total += acked.len() as u64;
    }
    eprintln!("durability: {runs} kills, {acked_total} acknowledged sets all present; unacknowledged in flight: {in_flight_whole} whole, {in_flight_absent} absent, none partial");
}
