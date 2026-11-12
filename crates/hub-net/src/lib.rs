//! Pocket Hub's `net.http` (docs/POCKET-LINK.md in PocketJS, §6.5, §12.3
//! item 6): an applet worker's HTTP request, which a device without a usable
//! TLS stack hands to Hub over the link, performed with Hub's own HTTP and
//! TLS stack and streamed back.
//!
//! - `request`: the request object and its limits (§6.5, `net.ts`).
//! - `address`: the address classes the destination policy refuses.
//! - `fetch`: the policy at every hop, redirects, proxies, the body reader.
//! - `clients`: the shared clients and connection reuse, and why reuse keeps the policy.
//! - `proxy`: `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` and the system's settings.
//! - `head`: `net.head` and its limits.
//!
//! `NetProvider` serves the `net.http` group through hub-core's
//! `DeviceRequests`. The connection gives it the request's body stream,
//! the response stream under the device's credit, the bound of 8 open
//! requests and the 12,288-byte payload floor; this crate does the rest.

mod address;
mod clients;
mod fetch;
mod head;
mod proxy;
mod request;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use pocket_hub_core::{BoxFuture, CancelCause, DeviceCall, DeviceRequests, ErrorReply, SinkError};
use reqwest::Url;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

pub use address::{AddressClass, classify};
pub use fetch::SERVICE_HOST;
pub use head::{HOP_BY_HOP, Head, response_headers};
pub use proxy::{Proxies, ProxySettings};
pub use request::{NetRequest, parse as parse_request};

/// The error codes of `NET_ERROR` (`contracts/spec/net.ts`), and the codes
/// every group uses that a `net.http` request can end with (§5.2).
pub mod codes {
    pub const UNAVAILABLE: &str = "unavailable";
    pub const INVALID_REQUEST: &str = "invalid_request";
    pub const BUSY: &str = "busy";
    pub const DNS: &str = "dns";
    pub const CONNECT: &str = "connect";
    pub const TLS: &str = "tls";
    pub const TIMEOUT: &str = "timeout";
    pub const REDIRECT: &str = "redirect";
    pub const RESPONSE_TOO_LARGE: &str = "response_too_large";
    pub const PROTOCOL: &str = "protocol";
    pub const CANCELLED: &str = "cancelled";
    pub const OTHER: &str = "other";
}

/// Why a request failed: a `NET_ERROR` code and a message for the applet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct NetError {
    pub code: String,
    pub message: String,
}

impl NetError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_owned(), message: message.into() }
    }
}

impl From<NetError> for ErrorReply {
    fn from(error: NetError) -> Self {
        ErrorReply::new(&error.code, error.message)
    }
}

/// Which destinations an applet may reach, beside the address policy.
///
/// Keyed by the request's `app`, the runtime's assertion of the applet id
/// (§6.5, never the guest's). Called for the first hop and every redirect
/// hop; a refusal ends the request with `invalid_request`. Today's policy,
/// `PublicDestinations`, allows every public destination; a store policy
/// will restrict an applet to the hosts its manifest declares.
pub trait AppPolicy: Send + Sync + 'static {
    fn allow(&self, app: &str, url: &Url) -> Result<(), String>;
}

/// Every destination the address policy allows.
#[derive(Debug, Clone, Copy, Default)]
pub struct PublicDestinations;

impl AppPolicy for PublicDestinations {
    fn allow(&self, _app: &str, _url: &Url) -> Result<(), String> {
        Ok(())
    }
}

/// For tests and development: requests for `host` go to `address`, which
/// may be private or loopback, for this host alone; the reference's
/// `--resolve <host>=<address>:<port>` (`cli.ts`). The host never goes
/// through a proxy. The URL keeps its scheme and host (TLS checks the
/// host's name), and its own port when it names one: `address`'s port
/// replaces the scheme's default port only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOverride {
    /// A DNS name, lower case.
    pub host: String,
    pub address: SocketAddr,
}

impl std::str::FromStr for ResolveOverride {
    type Err = String;

    /// `<host>=<address>:<port>`, the address an IPv4 address or a
    /// bracketed IPv6 one: `api.example.com=127.0.0.1:8080`, `x.test=[::1]:443`.
    fn from_str(spec: &str) -> Result<Self, String> {
        let wrong = || format!("{spec} is not <host>=<address>:<port>");
        let (host, target) = spec.split_once('=').ok_or_else(wrong)?;
        let host = host.trim().to_ascii_lowercase();
        let valid = !host.is_empty() && host.len() <= 253 && host.split('.').all(|label| !label.is_empty() && label.len() <= 63 && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
        if !valid || host.parse::<IpAddr>().is_ok() {
            return Err(format!("{host} is not a host name (an override names a host, not an address)"));
        }
        let address: SocketAddr = target.trim().parse().map_err(|_| wrong())?;
        if address.port() == 0 {
            return Err(wrong());
        }
        Ok(ResolveOverride { host, address })
    }
}

/// How Hub finds a name's addresses. The system's resolver by default; a
/// test gives a table (the reference's `NetOptions.resolve` seam). Every
/// address it returns goes through the destination check before Hub
/// connects to it.
pub trait Lookup: Send + Sync + 'static {
    fn lookup(&self, host: &str) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>>;
}

/// The system's resolver (`getaddrinfo`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLookup;

impl Lookup for SystemLookup {
    fn lookup(&self, host: &str) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
        let host = host.to_owned();
        Box::pin(async move { Ok(tokio::net::lookup_host((host.as_str(), 0)).await?.map(|a| a.ip()).collect()) })
    }
}

/// The provider's settings.
pub struct NetConfig {
    /// Default `Proxies::from_environment()`.
    pub proxies: Proxies,
    /// Development and test overrides (`ResolveOverride`).
    pub overrides: Vec<ResolveOverride>,
    pub policy: Arc<dyn AppPolicy>,
    /// Certificates (DER) trusted beside the system's roots, for a test
    /// server's own authority.
    pub extra_roots: Vec<Vec<u8>>,
    /// Sent when the applet sends no `user-agent`.
    pub user_agent: String,
    /// Default `SystemLookup`.
    pub lookup: Arc<dyn Lookup>,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self { proxies: Proxies::from_environment(), overrides: Vec::new(), policy: Arc::new(PublicDestinations), extra_roots: Vec::new(), user_agent: "pocket-hub".into(), lookup: Arc::new(SystemLookup) }
    }
}

/// Serves `net.http` (§6.5).
#[derive(Clone)]
pub struct NetProvider {
    config: Arc<NetConfig>,
    /// The provider's own clients and idle connections, for every request it serves.
    clients: Arc<clients::Clients>,
}

impl NetProvider {
    pub fn new(config: NetConfig) -> Self {
        Self { config: Arc::new(config), clients: Arc::default() }
    }
}

impl DeviceRequests for NetProvider {
    fn features(&self) -> Vec<String> {
        vec!["net.http".into()]
    }

    fn handle(&self, call: DeviceCall) -> BoxFuture<'static, Result<Value, ErrorReply>> {
        let (config, clients) = (self.config.clone(), self.clients.clone());
        Box::pin(async move {
            let started = Instant::now();
            let mut call = call;
            let mut log = Line::of(&call.object);
            let result = serve(&config, &clients, &mut call, &mut log).await;
            log.write(&result, started);
            result.map(|bytes| json!({ "bytes": bytes })).map_err(ErrorReply::from)
        })
    }
}

/// What the log line of a request says: never headers, cookies or bodies,
/// and the URL without credentials, query values or fragment.
struct Line {
    app: String,
    method: String,
    url: String,
    status: Option<u16>,
    bytes: u64,
}

impl Line {
    fn of(object: &serde_json::Map<String, Value>) -> Line {
        let text = |k: &str| object.get(k).and_then(Value::as_str).unwrap_or("?").to_owned();
        let url = object.get("url").and_then(Value::as_str).map(loggable_url).unwrap_or_else(|| "?".into());
        Line { app: text("app"), method: text("method"), url, status: None, bytes: 0 }
    }

    fn write(&self, result: &Result<u64, NetError>, started: Instant) {
        let ms = started.elapsed().as_millis();
        let Line { app, method, url, .. } = self;
        match (result, self.status) {
            (Ok(bytes), Some(status)) => tracing::info!(target: "pocket_hub_net", "net.http: {app} {method} {url} -> {status}, {bytes} bytes in {ms} ms"),
            (Ok(bytes), None) => tracing::info!(target: "pocket_hub_net", "net.http: {app} {method} {url} -> {bytes} bytes in {ms} ms"),
            (Err(error), Some(status)) => {
                tracing::info!(target: "pocket_hub_net", "net.http: {app} {method} {url} -> {} after status {status} and {} bytes, {ms} ms", error.code, self.bytes)
            }
            (Err(error), None) => tracing::info!(target: "pocket_hub_net", "net.http: {app} {method} {url} -> {} after {ms} ms", error.code),
        }
    }
}

/// A URL for the log: no user name or password, each query parameter's
/// name kept and its value replaced by `*`, no fragment. A URL that does not
/// parse is logged as its scheme and nothing else.
pub fn loggable_url(text: &str) -> String {
    let Ok(mut url) = Url::parse(text) else {
        return text.split(':').next().filter(|s| s.len() < 16 && s.bytes().all(|b| b.is_ascii_alphabetic())).map(|s| format!("{s}:…")).unwrap_or_else(|| "(not a URL)".into());
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    if let Some(query) = url.query().map(str::to_owned) {
        let names: Vec<String> = query.split('&').filter(|p| !p.is_empty()).map(|pair| format!("{}=*", pair.split('=').next().unwrap_or(""))).collect();
        let joined = names.join("&");
        url.set_query(if names.is_empty() { None } else { Some(&joined) });
    }
    url.to_string()
}

fn cancelled(cause: CancelCause) -> NetError {
    NetError::new(cause.code(), "the request ended before Hub's fetch did")
}

fn timed_out(request: &NetRequest) -> NetError {
    NetError::new(codes::TIMEOUT, format!("no complete response within {} ms", request.timeout.as_millis()))
}

/// One request, from the object to the reply's `bytes`.
async fn serve(config: &NetConfig, clients: &clients::Clients, call: &mut DeviceCall, log: &mut Line) -> Result<u64, NetError> {
    let request = request::parse(&call.object)?;
    let cancel = call.cancel.clone();

    // The body first: Hub grants its declared length on receipt (§6.5),
    // and a stall of 30 s on it is `timeout` (§5.5).
    let body = tokio::select! {
        body = call.body.read_all(request.bytes) => body.map_err(|e| NetError::new(&e.code, e.message))?,
        cause = cancel.cancelled() => return Err(cancelled(cause)),
    };

    // The request's own timeout runs from here to the body's END: the
    // fetch, the redirects and the download under the device's credit.
    let deadline = tokio::time::Instant::now() + request.timeout;
    let opened = tokio::select! {
        opened = fetch::open(config, clients, &request, Bytes::from(body)) => opened?,
        _ = tokio::time::sleep_until(deadline) => return Err(timed_out(&request)),
        cause = cancel.cancelled() => return Err(cancelled(cause)),
    };
    log.status = Some(opened.status);
    let head = Head::fit(opened.status, opened.url.as_str(), opened.headers)?;
    call.sink.progress(head.to_object());

    let Some(mut reader) = opened.body else {
        call.sink.end();
        return Ok(0);
    };
    // Every early return below drops `reader`, and with it a response not
    // read to its end: the HTTP library then closes that connection instead
    // of returning it to the pool (`clients.rs`).
    let mut buffer = vec![0u8; 16 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = tokio::select! {
            read = reader.read(&mut buffer) => read.map_err(|e| fetch::map_read_error(&e))?,
            _ = tokio::time::sleep_until(deadline) => return Err(timed_out(&request)),
            cause = cancel.cancelled() => return Err(cancelled(cause)),
        };
        if n == 0 {
            break;
        }
        // Past `maxBytes` the device keeps the prefix (§6.5): Hub sends the
        // bytes up to the limit, then the error.
        let room = request.max_bytes - total;
        let over = n as u64 > room;
        let take = if over { room as usize } else { n };
        if take > 0 {
            let chunk = Bytes::copy_from_slice(&buffer[..take]);
            tokio::select! {
                sent = call.sink.send(chunk) => sent.map_err(|e| match e {
                    SinkError::Stalled => NetError::new(codes::TIMEOUT, "30 s without credit for the response"),
                    SinkError::Gone => NetError::new(codes::CANCELLED, "the response stream is gone"),
                })?,
                _ = tokio::time::sleep_until(deadline) => return Err(timed_out(&request)),
                cause = cancel.cancelled() => return Err(cancelled(cause)),
            }
            total += take as u64;
            log.bytes = total;
        }
        if over {
            return Err(NetError::new(codes::RESPONSE_TOO_LARGE, format!("the response is over {} bytes", request.max_bytes)));
        }
    }
    call.sink.end();
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_shows_no_credentials_and_no_query_values() {
        assert_eq!(loggable_url("https://user:secret@api.example.com/v1/items?token=abc&q=cats&flag#frag"), "https://api.example.com/v1/items?token=*&q=*&flag=*");
        assert_eq!(loggable_url("http://example.com/plain"), "http://example.com/plain");
        assert_eq!(loggable_url("ftp:/x y"), "ftp:…");
        assert_eq!(loggable_url("not a url at all"), "(not a URL)");
    }

    #[test]
    fn overrides_parse() {
        let o: ResolveOverride = "API.Example.com=127.0.0.1:8080".parse().unwrap();
        assert_eq!((o.host.as_str(), o.address), ("api.example.com", "127.0.0.1:8080".parse().unwrap()));
        let v6: ResolveOverride = "x.test=[::1]:443".parse().unwrap();
        assert_eq!(v6.address, "[::1]:443".parse().unwrap());
        for bad in ["x.test", "x.test=127.0.0.1", "1.2.3.4=127.0.0.1:80", "=127.0.0.1:80", "x.test=localhost:80", "x..test=127.0.0.1:80", "x.test=127.0.0.1:0"] {
            assert!(bad.parse::<ResolveOverride>().is_err(), "{bad}");
        }
    }
}
