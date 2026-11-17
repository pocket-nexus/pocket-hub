//! For tests: a fake applet store on loopback that answers the routes Hub
//! reads (`/v1/apps`, `/v1/packages/<sha256>`, `/v1/icons/<sha256>`,
//! `/v1/details/<sha256>`) with faults on demand, packages built in the
//! test, the real packages kept in `tests/fixtures` with the live store's
//! details of them, and ed25519 test keys.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use pocket_package::package_format::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::entry::{Entry, encode};

/// A test publisher key from a seed byte.
pub fn test_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// The key's public half as the store writes it.
pub fn public(key: &SigningKey) -> String {
    encode(key.verifying_key().as_bytes())
}

/// The publisher's signature of the entry's signed message.
pub fn sign(key: &SigningKey, entry: &Entry) -> String {
    encode(&key.sign(entry.signed_message().as_bytes()).to_bytes())
}

pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The folder of the real packages and icons taken from the live store once
/// (dev.pocket-nexus.hn and dev.pocket-nexus.youtube 0.3.0, with their
/// catalog entries in `apps.json`). They are built from the GPL-3.0 sources
/// of github.com/pocket-nexus/applets (`hn/`, `youtube/`).
pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(name)).unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

/// The live store's `/v1/details` answer for a fixture's package (`hn`, `youtube`).
pub fn fixture_details(name: &str) -> Value {
    serde_json::from_slice(&fixture(&format!("{name}.details.json"))).unwrap()
}

/// The store's names for the devices it knows, in its order
/// (`store/src/details.ts` `DEVICES`).
const DEVICES: [(&str, &str); 3] = [("rg-ds-plus", "Omarchy Pocket"), ("tab5-home", "M5Stack Tab5"), ("3ds-dev", "Nintendo 3DS")];

/// The details the store reads out of a package with no capabilities and no
/// surface, as a package built by `build_package` is: its builds, in the
/// store's order of devices with one it has no name for last, and the
/// applet API version its manifest names.
pub fn plain_details(package: &[u8]) -> Value {
    let parsed = pocket_package::Package::parse(package, true).expect("a package the test built or kept");
    let manifest: Value = serde_json::from_slice(parsed.manifest()).unwrap();
    let rank = |target: &str| DEVICES.iter().position(|(known, _)| *known == target).unwrap_or(DEVICES.len());
    let mut builds: Vec<(String, u32)> = (0..parsed.variant_count()).map(|index| parsed.variant(index).unwrap()).map(|v| (v.target.to_owned(), v.host_abi)).collect();
    builds.sort_by_key(|(target, _)| rank(target));
    let devices: Vec<Value> = builds
        .iter()
        .map(|(target, abi)| {
            let name = DEVICES.iter().find(|(known, _)| known == target).map_or(target.as_str(), |(_, name)| name);
            json!({"target": target, "name": name, "build": {"target": target, "hostAbi": abi, "profile": "0".repeat(64), "hash": "0".repeat(16), "sections": []}})
        })
        .collect();
    let mut details = json!({"devices": devices, "needs": [], "uses": []});
    if let Some(api) = manifest["applet"]["version"].as_u64() {
        details["api"] = json!(api);
    }
    details
}

/// A 128 × 128 PNG stand-in: the store's own check is not Hub's, so any
/// bytes do; these differ by seed.
pub fn icon(seed: u8) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\nfake-icon".to_vec();
    bytes.push(seed);
    bytes
}

fn align(n: usize) -> usize {
    (n + POCKET_ALIGN - 1) & !(POCKET_ALIGN - 1)
}

fn put32(bytes: &mut [u8], at: usize, value: usize) {
    bytes[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

/// A valid `.pocket` (format 2) with a manifest and one variant per
/// `(target, output)`, each holding an identity and a script of `filler`
/// bytes: what `pocket_package::Package::parse` accepts with its hashes.
pub fn build_package(id: &str, version: &str, title: &str, variants: &[(&str, &str)], filler: usize) -> Vec<u8> {
    let manifest = serde_json::to_vec(&json!({"pocket": 2, "id": id, "title": title, "version": version, "applet": {"version": 1, "summary": "A test applet."}})).unwrap();
    let mut variants: Vec<(&str, &str)> = variants.to_vec();
    variants.sort();
    let sections: Vec<Vec<(u32, Vec<u8>)>> = variants
        .iter()
        .map(|(_, output)| {
            let identity = pocket_package::embedded::identity(output, id, title).unwrap();
            let mut script = format!("/* {id} {version} */").into_bytes();
            script.resize(filler.max(script.len()), b' ');
            script.push(0);
            vec![(SECTION_IDENTITY, identity), (SECTION_JS, script)]
        })
        .collect();
    let table = align(POCKET_HEADER_SIZE + manifest.len());
    let strings = table + variants.len() * POCKET_VARIANT_SIZE;
    let strings_size: usize = variants.iter().map(|(target, _)| target.len()).sum();
    let mut directory = align(strings + strings_size);
    let mut payload = align(directory + sections.iter().map(|s| s.len() * POCKET_SECTION_SIZE).sum::<usize>());
    let mut placed: Vec<Vec<usize>> = Vec::new();
    for list in &sections {
        let mut offsets = Vec::new();
        for (_, bytes) in list {
            offsets.push(payload);
            payload = align(payload + bytes.len());
        }
        placed.push(offsets);
    }
    let mut out = vec![0u8; payload + POCKET_FOOTER_SIZE];
    put32(&mut out, 0, POCKET_MAGIC as usize);
    put32(&mut out, 4, POCKET_VERSION as usize);
    put32(&mut out, OFFSET_HEADER_MANIFEST_SIZE, manifest.len());
    put32(&mut out, OFFSET_HEADER_VARIANT_COUNT, variants.len());
    put32(&mut out, OFFSET_HEADER_STRING_TABLE_OFFSET, strings);
    put32(&mut out, OFFSET_HEADER_STRING_TABLE_SIZE, strings_size);
    out[POCKET_HEADER_SIZE..POCKET_HEADER_SIZE + manifest.len()].copy_from_slice(&manifest);
    let mut string_at = strings;
    for (index, (target, _)) in variants.iter().enumerate() {
        let entry = table + index * POCKET_VARIANT_SIZE;
        put32(&mut out, entry + OFFSET_VARIANT_TARGET_OFFSET, string_at);
        put32(&mut out, entry + OFFSET_VARIANT_TARGET_SIZE, target.len());
        out[string_at..string_at + target.len()].copy_from_slice(target.as_bytes());
        string_at += target.len();
        put32(&mut out, entry + OFFSET_VARIANT_HOST_ABI, 12);
        put32(&mut out, entry + OFFSET_VARIANT_SECTION_COUNT, sections[index].len());
        put32(&mut out, entry + OFFSET_VARIANT_SECTIONS_OFFSET, directory);
        let payloads: Vec<&[u8]> = sections[index].iter().map(|(_, bytes)| bytes.as_slice()).collect();
        let hash = pocket_package::package::fnv1a64(&payloads);
        out[entry + OFFSET_VARIANT_HASH..entry + OFFSET_VARIANT_HASH + 8].copy_from_slice(&hash.to_le_bytes());
        out[entry + OFFSET_VARIANT_PROFILE_HASH] = index as u8 + 1;
        for (s, (kind, bytes)) in sections[index].iter().enumerate() {
            let at = directory + s * POCKET_SECTION_SIZE;
            put32(&mut out, at + OFFSET_SECTION_KIND, *kind as usize);
            put32(&mut out, at + OFFSET_SECTION_OFFSET, placed[index][s]);
            put32(&mut out, at + OFFSET_SECTION_SIZE, bytes.len());
            out[placed[index][s]..placed[index][s] + bytes.len()].copy_from_slice(bytes);
        }
        directory += sections[index].len() * POCKET_SECTION_SIZE;
    }
    let hash = pocket_package::package::fnv1a64(&[&out[..payload]]);
    out[payload..].copy_from_slice(&hash.to_le_bytes());
    out
}

/// What the fake store does instead of serving a file.
#[derive(Debug, Clone)]
pub enum Fault {
    /// The file with one byte changed: its SHA-256 does not match.
    Corrupt,
    /// `404`.
    Missing,
    /// The head and half the bytes, then nothing.
    Stall,
    /// Nothing until `release` (a download Hub has not finished yet).
    Held,
    /// The bytes in pieces of `chunk` with `delay` between them.
    Slow { chunk: usize, delay: Duration },
}

#[derive(Default)]
struct State {
    apps: Vec<Value>,
    files: HashMap<String, Vec<u8>>,
    faults: HashMap<String, Fault>,
    hits: HashMap<String, usize>,
    catalog_status: Option<u16>,
}

/// The fake store. Dropping it stops its server.
pub struct FakeStore {
    pub url: String,
    state: Arc<Mutex<State>>,
    released: watch::Sender<u64>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeStore {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeStore {
    pub async fn start() -> FakeStore {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let (released, _) = watch::channel(0u64);
        let (shared, gate) = (state.clone(), released.clone());
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, shared.clone(), gate.subscribe()));
            }
        });
        FakeStore { url, state, released, task }
    }

    /// Lists a package: its catalog entry is made from the package's facts
    /// and signed by `key`, and its details are `plain_details` (a test
    /// gives others with `set_details`). Returns the entry.
    pub fn publish(&self, key: &SigningKey, package: Vec<u8>, icon: Vec<u8>, author: &str, summary: &str) -> Entry {
        let facts = crate::package::read(&package).expect("a package the test built or kept");
        let mut entry = Entry {
            id: facts.id.clone(),
            version: facts.version.clone(),
            title: facts.title.clone(),
            summary: summary.into(),
            author: author.into(),
            key: public(key),
            services: vec!["store".into()],
            hosts: vec!["example.com".into()],
            package: sha256(&package),
            icon: sha256(&icon),
            bytes: package.len() as u64,
            signature: String::new(),
            targets: facts.variants.iter().map(|v| v.target.clone()).collect(),
        };
        entry.signature = sign(key, &entry);
        let details = serde_json::to_vec(&plain_details(&package)).unwrap();
        let mut state = self.state.lock().unwrap();
        state.files.insert(format!("/v1/packages/{}", entry.package), package);
        state.files.insert(format!("/v1/icons/{}", entry.icon), icon);
        state.files.insert(format!("/v1/details/{}", entry.package), details);
        state.apps.retain(|app| app["id"] != entry.id.as_str());
        state.apps.push(listed(&entry, key));
        entry
    }

    /// Replaces an app's entry as listed, signature included (to list one
    /// whose signature does not verify).
    pub fn set_entry(&self, entry: &Entry) {
        let mut state = self.state.lock().unwrap();
        let mut value = serde_json::to_value(entry).unwrap();
        value["official"] = json!(false);
        value["created"] = json!(1);
        match state.apps.iter_mut().find(|app| app["id"] == entry.id.as_str()) {
            Some(app) => *app = value,
            None => state.apps.push(value),
        }
    }

    /// What `/v1/details/<sha256>` answers for a package: these details, or
    /// with None a `404` as for a package the store does not list.
    pub fn set_details(&self, sha256: &str, details: Option<Value>) {
        let path = format!("/v1/details/{sha256}");
        let mut state = self.state.lock().unwrap();
        match details {
            Some(details) => state.files.insert(path, serde_json::to_vec(&details).unwrap()),
            None => state.files.remove(&path),
        };
    }

    pub fn unlist(&self, id: &str) {
        self.state.lock().unwrap().apps.retain(|app| app["id"] != id);
    }

    pub fn fault(&self, sha256: &str, fault: Fault) {
        self.state.lock().unwrap().faults.insert(sha256.to_owned(), fault);
    }

    pub fn clear_fault(&self, sha256: &str) {
        self.state.lock().unwrap().faults.remove(sha256);
        self.released.send_modify(|n| *n += 1);
    }

    /// Lets every `Held` answer go.
    pub fn release(&self) {
        self.state.lock().unwrap().faults.retain(|_, fault| !matches!(fault, Fault::Held));
        self.released.send_modify(|n| *n += 1);
    }

    /// `/v1/apps` answers this status with an error body instead of the catalog.
    pub fn catalog_status(&self, status: Option<u16>) {
        self.state.lock().unwrap().catalog_status = status;
    }

    /// How many requests reached a path (`/v1/apps`, `/v1/packages/<sha>`).
    pub fn hits(&self, path: &str) -> usize {
        self.state.lock().unwrap().hits.get(path).copied().unwrap_or(0)
    }
}

/// An entry as `/v1/apps` lists it, with the members Hub does not keep.
fn listed(entry: &Entry, key: &SigningKey) -> Value {
    let mut value = serde_json::to_value(entry).unwrap();
    value["official"] = json!(entry.key == public(key));
    value["created"] = json!(1_793_668_752_374u64);
    value["target"] = json!(entry.targets.first());
    value["profile"] = json!("0".repeat(64));
    value["mock"] = Value::Null;
    value
}

async fn serve(mut socket: tokio::net::TcpStream, state: Arc<Mutex<State>>, mut released: watch::Receiver<u64>) {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match socket.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
        if head.len() > 64 * 1024 {
            return;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let (body, status, fault) = {
        let mut state = state.lock().unwrap();
        *state.hits.entry(path.clone()).or_default() += 1;
        if path == "/v1/apps" {
            match state.catalog_status {
                Some(status) => (br#"{"error":"the store failed"}"#.to_vec(), status, None),
                None => (serde_json::to_vec(&json!({"apps": state.apps})).unwrap(), 200, None),
            }
        } else {
            let sha = path.rsplit('/').next().unwrap_or("").to_owned();
            // A fault names a file by its hash: a package's details, named by
            // the package's hash, are not that file.
            let fault = if path.starts_with("/v1/details/") { None } else { state.faults.get(&sha).cloned() };
            match state.files.get(&path) {
                Some(bytes) => (bytes.clone(), 200, fault),
                None => (br#"{"error":"no listed version has that file"}"#.to_vec(), 404, None),
            }
        }
    };
    let respond = |status: u16, length: usize| format!("HTTP/1.1 {status} {}\r\ncontent-length: {length}\r\ncontent-type: application/octet-stream\r\nconnection: close\r\n\r\n", if status == 200 { "OK" } else { "Error" });
    match fault {
        None => {
            let _ = socket.write_all(respond(status, body.len()).as_bytes()).await;
            let _ = socket.write_all(&body).await;
        }
        Some(Fault::Missing) => {
            let body = br#"{"error":"the file is missing"}"#;
            let _ = socket.write_all(respond(404, body.len()).as_bytes()).await;
            let _ = socket.write_all(body).await;
        }
        Some(Fault::Corrupt) => {
            let mut body = body;
            let middle = body.len() / 2;
            body[middle] ^= 0x55;
            let _ = socket.write_all(respond(200, body.len()).as_bytes()).await;
            let _ = socket.write_all(&body).await;
        }
        Some(Fault::Stall) => {
            let _ = socket.write_all(respond(200, body.len()).as_bytes()).await;
            let _ = socket.write_all(&body[..body.len() / 2]).await;
            let _ = socket.flush().await;
            // Holds the connection open with nothing more to say.
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
        Some(Fault::Held) => {
            let sha = path.rsplit('/').next().unwrap_or("").to_owned();
            loop {
                if !matches!(state.lock().unwrap().faults.get(&sha), Some(Fault::Held)) {
                    break;
                }
                if released.changed().await.is_err() {
                    return;
                }
            }
            let _ = socket.write_all(respond(200, body.len()).as_bytes()).await;
            let _ = socket.write_all(&body).await;
        }
        Some(Fault::Slow { chunk, delay }) => {
            let _ = socket.write_all(respond(200, body.len()).as_bytes()).await;
            for piece in body.chunks(chunk.max(1)) {
                if socket.write_all(piece).await.is_err() {
                    return;
                }
                let _ = socket.flush().await;
                tokio::time::sleep(delay).await;
            }
        }
    }
    let _ = socket.flush().await;
    let _ = socket.shutdown().await;
}
