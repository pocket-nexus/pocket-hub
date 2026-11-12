//! What the net.http tests share: a local HTTP/HTTPS server written by hand
//! (so a test controls every byte, sees when Hub drops a connection, and
//! counts the connections Hub opens: it keeps a connection alive after a
//! full response, as a real server does),
//! a certificate authority made per run, a table resolver, and a Hub with
//! the provider and the reference device linked to it.
#![allow(dead_code, unused_imports)]

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pocket_hub_core::{BoxFuture, Endpoint, TcpCarrier};
use pocket_hub_net::{Lookup, NetConfig, NetProvider, Proxies, ResolveOverride};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc};

#[path = "../../../hub-core/tests/common/mod.rs"]
pub mod common;
pub use common::{Device, Hub, WAIT, fast_config, fnv32, pattern, temp_dir};

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

/// What the server saw.
#[derive(Debug, Clone)]
pub enum Seen {
    /// `conn`: the connection it came on, numbered from 1 in accept order.
    Request { method: String, path: String, headers: Vec<(String, String)>, body: Vec<u8>, conn: usize },
    /// The client closed a connection the server held open, at `at`:
    /// during a response that never ends (`/slow`, `/hang`), or idle after
    /// a full response to `path`.
    Closed { path: String, at: Instant },
}

pub struct Server {
    pub port: u16,
    seen: mpsc::UnboundedReceiver<Seen>,
    kept: Vec<Seen>,
    release: Arc<Notify>,
    accepts: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

/// Fixed material some routes answer with.
#[derive(Default)]
pub struct Material {
    /// `/vector`: the golden `net.head` headers of messages.json, in order.
    pub vector_headers: Vec<(String, String)>,
}

enum Reply {
    Full { status: u16, headers: Vec<(String, String)>, body: Vec<u8> },
    /// A head announcing a long body, `first` bytes of it, then nothing until the client leaves.
    Slow { first: usize },
    /// Nothing until `release` (then a small 200) or until the client leaves.
    Hang,
    /// A chunked 200: `body` in one chunk, and the last chunk 100 ms later,
    /// as a server's end of a body may come in a later segment.
    Chunked { headers: Vec<(String, String)>, body: Vec<u8> },
}

fn ok(content_type: &str, body: Vec<u8>) -> Reply {
    Reply::Full { status: 200, headers: vec![("content-type".into(), content_type.into())], body }
}

fn redirect(status: u16, location: &str) -> Reply {
    Reply::Full { status, headers: vec![("location".into(), location.into())], body: Vec::new() }
}

fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn zlib(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn route(method: &str, path: &str, headers: &[(String, String)], body: &[u8], material: &Material) -> Reply {
    let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    // An absolute-form target: this server stands in for a proxy.
    if path.starts_with("http://") {
        return ok("text/plain", format!("proxied {path}").into_bytes());
    }
    let segments: Vec<&str> = path.trim_start_matches('/').splitn(3, '/').collect();
    let number = |i: usize| segments.get(i).and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
    match segments[0] {
        "small" => ok("text/plain", b"hello, pocket!".to_vec()),
        "bytes" | "big" => ok("application/octet-stream", pattern(number(1))),
        "echo" => Reply::Full {
            status: 200,
            headers: vec![("content-type".into(), "application/octet-stream".into()), ("x-method".into(), method.into())],
            body: body.to_vec(),
        },
        "method" => {
            let line = format!(
                "{method} {} {} {} {}",
                body.len(),
                header("content-type").unwrap_or_else(|| "-".into()),
                if header("authorization").is_some() { "auth" } else { "noauth" },
                header("user-agent").unwrap_or_default()
            );
            ok("text/plain", line.into_bytes())
        }
        "redirect" => {
            let n = number(1);
            if n == 0 { ok("text/plain", b"arrived".to_vec()) } else { redirect(302, &format!("/redirect/{}", n - 1)) }
        }
        // /to/<status>/<location>
        "to" => redirect(number(1) as u16, segments.get(2).copied().unwrap_or("/")),
        "head" => Reply::Full { status: 200, headers: vec![("content-type".into(), "application/octet-stream".into())], body: pattern(1234) },
        "cookies" => Reply::Full {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "text/plain".into()),
                ("Set-Cookie".into(), "session=abc; Path=/; HttpOnly".into()),
                ("Keep-Alive".into(), "timeout=5".into()),
                ("X-One".into(), "1".into()),
                ("Set-Cookie".into(), "theme=dark, light; Max-Age=60".into()),
                ("Proxy-Authenticate".into(), "Basic".into()),
                ("Vary".into(), "accept".into()),
                ("Vary".into(), "origin".into()),
            ],
            body: b"c".to_vec(),
        },
        "vector" => {
            let mut list = material.vector_headers.clone();
            list.push(("x-late".into(), "1".into()));
            Reply::Full { status: 200, headers: list, body: b"v".to_vec() }
        }
        "gzip" => Reply::Full {
            status: 200,
            headers: vec![("content-type".into(), "application/octet-stream".into()), ("content-encoding".into(), "gzip".into()), ("x-after".into(), "kept".into())],
            body: gzip(&pattern(number(1))),
        },
        "deflate" => Reply::Full { status: 200, headers: vec![("content-encoding".into(), "deflate".into())], body: zlib(&pattern(number(1))) },
        "chunked-deflate" => Reply::Chunked { headers: vec![("content-encoding".into(), "deflate".into())], body: zlib(&pattern(number(1))) },
        "chunked-gzip" => Reply::Chunked { headers: vec![("content-encoding".into(), "gzip".into())], body: gzip(&pattern(number(1))) },
        "deflate-trailing" => Reply::Full { status: 200, headers: vec![("content-encoding".into(), "deflate".into())], body: [zlib(&pattern(number(1))), b"trailing".to_vec()].concat() },
        "badgzip" => Reply::Full { status: 200, headers: vec![("content-encoding".into(), "gzip".into())], body: b"this is not gzip at all".to_vec() },
        "slow" => Reply::Slow { first: 1000 },
        "hang" => Reply::Hang,
        _ => Reply::Full { status: 404, headers: vec![], body: b"no route".to_vec() },
    }
}

async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Option<(String, String, Vec<(String, String)>, Vec<u8>)> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];
    let end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        let n = stream.read(&mut buffer).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..n]);
    };
    let head = String::from_utf8_lossy(&data[..end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    let length: usize = headers.iter().find(|(n, _)| n == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = data[end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut buffer).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&buffer[..n]);
    }
    Some((method, path, headers, body))
}

async fn wait_closed<S: AsyncRead + Unpin>(stream: &mut S) {
    let mut buffer = [0u8; 1024];
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

async fn serve_one<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, conn: usize, seen: mpsc::UnboundedSender<Seen>, release: Arc<Notify>, material: Arc<Material>) {
    let closed = |path: String| Seen::Closed { path, at: Instant::now() };
    let mut last: Option<String> = None;
    // One request after another on the connection until the client closes
    // it, asks to close it, or a reply that ends the connection.
    loop {
        let Some((method, path, headers, body)) = read_request(&mut stream).await else {
            if let Some(path) = last {
                let _ = seen.send(closed(path));
            }
            return;
        };
        let _ = seen.send(Seen::Request { method: method.clone(), path: path.clone(), headers: headers.clone(), body: body.clone(), conn });
        let close = headers.iter().any(|(n, v)| n == "connection" && v.eq_ignore_ascii_case("close"));
        match route(&method, &path, &headers, &body, &material) {
            Reply::Full { status, headers, body } => {
                let mut text = format!("HTTP/1.1 {status} X\r\n");
                for (name, value) in &headers {
                    text.push_str(&format!("{name}: {value}\r\n"));
                }
                text.push_str(&format!("Content-Length: {}\r\n", body.len()));
                if close {
                    text.push_str("Connection: close\r\n");
                }
                text.push_str("\r\n");
                let _ = stream.write_all(text.as_bytes()).await;
                if method != "HEAD" {
                    let _ = stream.write_all(&body).await;
                }
                let _ = stream.flush().await;
                if close {
                    let _ = stream.shutdown().await;
                    return;
                }
                last = Some(path);
            }
            Reply::Chunked { headers, body } => {
                let mut text = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n".to_owned();
                for (name, value) in &headers {
                    text.push_str(&format!("{name}: {value}\r\n"));
                }
                text.push_str(&format!("\r\n{:x}\r\n", body.len()));
                let _ = stream.write_all(text.as_bytes()).await;
                let _ = stream.write_all(&body).await;
                let _ = stream.write_all(b"\r\n").await;
                let _ = stream.flush().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = stream.write_all(b"0\r\n\r\n").await;
                let _ = stream.flush().await;
                last = Some(path);
            }
            Reply::Slow { first } => {
                let text = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 1000000\r\n\r\n";
                let _ = stream.write_all(text.as_bytes()).await;
                let _ = stream.write_all(&pattern(first)).await;
                let _ = stream.flush().await;
                wait_closed(&mut stream).await;
                let _ = seen.send(closed(path));
                return;
            }
            Reply::Hang => {
                tokio::select! {
                    _ = release.notified() => {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nreleased").await;
                        let _ = stream.shutdown().await;
                    }
                    _ = wait_closed(&mut stream) => {
                        let _ = seen.send(closed(path));
                    }
                }
                return;
            }
        }
    }
}

impl Server {
    pub async fn start(tls: Option<Arc<tokio_rustls::rustls::ServerConfig>>, material: Material) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, seen) = mpsc::unbounded_channel();
        let release = Arc::new(Notify::new());
        let material = Arc::new(material);
        let notify = release.clone();
        let accepts = Arc::new(AtomicUsize::new(0));
        let counter = accepts.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { return };
                let conn = counter.fetch_add(1, Ordering::SeqCst) + 1;
                let (tx, notify, material, tls) = (tx.clone(), notify.clone(), material.clone(), tls.clone());
                tokio::spawn(async move {
                    match tls {
                        None => serve_one(socket, conn, tx, notify, material).await,
                        Some(config) => {
                            if let Ok(stream) = tokio_rustls::TlsAcceptor::from(config).accept(socket).await {
                                serve_one(stream, conn, tx, notify, material).await;
                            }
                        }
                    }
                });
            }
        });
        Server { port, seen, kept: Vec::new(), release, accepts, task }
    }

    /// The TCP connections accepted so far.
    pub fn accepts(&self) -> usize {
        self.accepts.load(Ordering::SeqCst)
    }

    pub fn address(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    /// Answers every request waiting on `/hang`.
    pub fn release(&self) {
        self.release.notify_waiters();
    }

    async fn next(&mut self, within: Duration, mut matches: impl FnMut(&Seen) -> bool) -> Option<Seen> {
        if let Some(i) = self.kept.iter().position(&mut matches) {
            return Some(self.kept.remove(i));
        }
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let seen = tokio::time::timeout_at(deadline, self.seen.recv()).await.ok()??;
            if matches(&seen) {
                return Some(seen);
            }
            self.kept.push(seen);
        }
    }

    /// The next request whose path starts with `prefix`.
    pub async fn request(&mut self, prefix: &str) -> Seen {
        self.next(WAIT, |s| matches!(s, Seen::Request { path, .. } if path.starts_with(prefix))).await.unwrap_or_else(|| panic!("no request for {prefix}"))
    }

    /// When the client closed a held connection for a path starting with `prefix`.
    pub async fn closed(&mut self, prefix: &str, within: Duration) -> Option<Instant> {
        match self.next(within, |s| matches!(s, Seen::Closed { path, .. } if path.starts_with(prefix))).await {
            Some(Seen::Closed { at, .. }) => Some(at),
            _ => None,
        }
    }

    /// The connection the next request whose path starts with `prefix` came on.
    pub async fn conn_of(&mut self, prefix: &str) -> usize {
        match self.request(prefix).await {
            Seen::Request { conn, .. } => conn,
            Seen::Closed { .. } => unreachable!(),
        }
    }

    /// Requests seen so far whose path starts with `prefix`.
    pub fn count_seen(&mut self, prefix: &str) -> usize {
        while let Ok(seen) = self.seen.try_recv() {
            self.kept.push(seen);
        }
        self.kept.iter().filter(|s| matches!(s, Seen::Request { path, .. } if path.starts_with(prefix))).count()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ---------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as i32;
    (y, m, d)
}

/// Today plus `offset` days. Apple's TLS policy refuses server
/// certificates valid for more than 825 days, so the test's are short-lived.
fn ymd(offset: i64) -> (i32, u8, u8) {
    let today = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() / 86_400) as i64;
    civil(today + offset)
}

macro_rules! day {
    ($offset:expr) => {{
        let (y, m, d) = ymd($offset);
        rcgen::date_time_ymd(y, m, d)
    }};
}

/// A server configuration for `names`, and the DER of the authority that
/// signed it. With `self_signed` the leaf signs itself and the authority is
/// unrelated (a client that trusts the authority still refuses the server).
pub fn tls(names: &[&str], self_signed: bool) -> (Arc<tokio_rustls::rustls::ServerConfig>, Vec<u8>) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.distinguished_name.push(DnType::CommonName, "Pocket Hub test authority");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    ca.not_before = day!(-1);
    ca.not_after = day!(30);
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca, ca_key);

    let leaf_key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    leaf.distinguished_name.push(DnType::CommonName, names[0]);
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.not_before = day!(-1);
    leaf.not_after = day!(30);
    let leaf_cert = if self_signed { leaf.self_signed(&leaf_key).unwrap() } else { leaf.signed_by(&leaf_key, &issuer).unwrap() };

    use tokio_rustls::rustls;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![leaf_cert.der().clone()], key)
        .unwrap();
    (Arc::new(config), ca_cert.der().to_vec())
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// A table of names; `localhost` and `.invalid` names go to the system's
/// resolver, any other name does not resolve.
pub struct Table(pub HashMap<String, Vec<IpAddr>>);

impl Lookup for Table {
    fn lookup(&self, host: &str) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
        if host == "localhost" || host.ends_with(".invalid") {
            return pocket_hub_net::SystemLookup.lookup(host);
        }
        let found = self.0.get(host).cloned();
        let host = host.to_owned();
        Box::pin(async move { found.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{host} is not in the test table"))) })
    }
}

/// Names that resolve to each refused class, in both families.
pub fn refused_names() -> Vec<(&'static str, &'static str)> {
    vec![
        ("loopback4.test", "127.0.0.1"),
        ("loopback6.test", "::1"),
        ("private10.test", "10.1.2.3"),
        ("private172.test", "172.16.5.5"),
        ("private192.test", "192.168.1.1"),
        ("cgnat.test", "100.64.0.1"),
        ("linklocal4.test", "169.254.169.254"),
        ("linklocal6.test", "fe80::1"),
        ("multicast4.test", "224.0.0.251"),
        ("multicast6.test", "ff02::1"),
        ("ula.test", "fd00::1"),
        ("unspecified4.test", "0.0.0.0"),
        ("unspecified6.test", "::"),
        ("reserved.test", "240.0.0.1"),
        ("broadcast.test", "255.255.255.255"),
        ("mapped.test", "::ffff:192.168.0.1"),
        ("nat64.test", "64:ff9b::a00:1"),
        ("sixtofour.test", "2002:7f00:1::1"),
        ("documentation.test", "2001:db8::1"),
    ]
}

pub fn table() -> Table {
    let mut map: HashMap<String, Vec<IpAddr>> = refused_names().into_iter().map(|(n, a)| (n.to_owned(), vec![a.parse().unwrap()])).collect();
    // One public address and one private: the name is refused whole.
    map.insert("mixed.test".into(), vec!["93.184.215.14".parse().unwrap(), "10.0.0.1".parse().unwrap()]);
    Table(map)
}

/// A config for tests: no proxy, the table resolver, and `overrides`.
pub fn config(overrides: &[(&str, SocketAddr)]) -> NetConfig {
    NetConfig {
        proxies: Proxies::Direct,
        overrides: overrides.iter().map(|(h, a)| ResolveOverride { host: h.to_string(), address: *a }).collect(),
        lookup: Arc::new(table()),
        ..NetConfig::default()
    }
}

// ---------------------------------------------------------------------------
// Hub and device
// ---------------------------------------------------------------------------

pub struct Linked {
    pub hub: Hub,
    pub device: Device,
}

/// A Hub serving net.http with `config`, and the reference device linked to it.
pub async fn linked(config: NetConfig, device: Value) -> Linked {
    let dir = temp_dir("net");
    let store = Arc::new(pocket_hub_store::Store::open(dir.join("store")).unwrap());
    let carrier = TcpCarrier::bind(Some(0)).await.unwrap();
    let endpoint = Endpoint::builder(store.clone()).config(fast_config()).listener(Box::new(carrier)).device_requests(Arc::new(NetProvider::new(config))).start();
    assert!(endpoint.features().contains(&"net.http".to_owned()));
    let port = endpoint.ports()[0].1.unwrap();
    let mut hub = Hub { events: endpoint.subscribe(), endpoint, store, port, dir };
    let (device_dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &device_dir, device).await;
    hub.linked(&id).await;
    let _ = device.expect("linked").await;
    Linked { hub, device }
}

impl Linked {
    /// Sends one request and waits for its outcome.
    pub async fn http(&mut self, tag: &str, mut command: Value) -> Value {
        self.send(tag, &mut command).await;
        self.outcome(tag).await
    }

    pub async fn send(&mut self, tag: &str, command: &mut Value) {
        command["cmd"] = json!("http");
        command["tag"] = json!(tag);
        self.device.send(command.clone()).await;
    }

    pub async fn outcome(&mut self, tag: &str) -> Value {
        let tag = tag.to_owned();
        self.device.wait(move |v| v["ev"] == "http" && v["tag"] == tag.as_str()).await
    }
}

/// The error code of an outcome, or "ok".
pub fn code(outcome: &Value) -> String {
    if outcome["ok"] == true { "ok".into() } else { outcome["code"].as_str().unwrap_or("?").to_owned() }
}
