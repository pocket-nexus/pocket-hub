//! One request performed: the destination policy at every hop, redirects
//! by hand, proxies, and the response body as a bounded byte reader.
//!
//! **Where the address check sits.** For a direct connection the HTTP
//! library asks `CheckedResolver` (`clients.rs`) for a name's addresses and connects to
//! exactly what it returns; the resolver refuses the hop when any address
//! is not public (the reference's rule: one private address refuses the
//! name). The addresses checked are therefore the addresses connected to,
//! with no second lookup in between. A URL whose host is a literal IP never
//! reaches a resolver, so `check_hop` classifies it before the hop is sent.
//! Every redirect hop takes the same path. Behind a proxy see `proxy.rs`.
//!
//! The clients, and the connections they keep idle between requests, are
//! shared (`clients.rs`, which also says why a reused connection cannot reach
//! an address this check would refuse). A request that ends early drops its
//! response: the HTTP library pools that connection only if the response
//! had already arrived whole, and closes it otherwise.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};

use crate::address::classify;
use crate::clients::Clients;
use crate::head::{HOP_BY_HOP, response_headers};
use crate::proxy::ProxyPlan;
use crate::request::{MAX_REDIRECTS, NetRequest};
use crate::{Lookup, NetConfig, NetError, codes};

/// The origin Pocket Hub version 1 answered itself (§11). Not served: see `check_hop`.
pub const SERVICE_HOST: &str = "companion.pocket";

/// A response whose head is in and whose body is not read yet.
pub(crate) struct Opened {
    pub status: u16,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    /// None for `HEAD`, 204 and 304: no body. Hub decodes a body's
    /// `content-encoding` of gzip, deflate or br in this reader.
    pub body: Option<Box<dyn AsyncRead + Send + Unpin>>,
}

/// Request headers Hub never forwards: hop-by-hop, and those the HTTP
/// library sets itself from the request (the reference's `REQUEST_DROP`).
fn forwarded(name: &str) -> bool {
    !HOP_BY_HOP.contains(&name) && !matches!(name, "host" | "content-length" | "expect")
}

/// Resolves a name and refuses it when any address is not public.
pub(crate) async fn resolve_checked(lookup: &dyn Lookup, host: &str) -> Result<Vec<SocketAddr>, NetError> {
    let found = lookup.lookup(host).await.map_err(|error| NetError::new(codes::DNS, format!("{host} does not resolve: {error}")))?;
    if found.is_empty() {
        return Err(NetError::new(codes::DNS, format!("{host} has no address")));
    }
    if let Some((address, class)) = found.iter().find_map(|a| classify(*a).map(|class| (*a, class))) {
        return Err(refused(host, address, class));
    }
    // Port 0: the HTTP library puts the URL's port in.
    Ok(found.into_iter().map(|a| SocketAddr::new(a, 0)).collect())
}

fn refused(host: &str, address: std::net::IpAddr, class: crate::AddressClass) -> NetError {
    let what = if host == address.to_string() { format!("{address}") } else { format!("{host} resolves to {address}, which") };
    NetError::new(codes::INVALID_REQUEST, format!("{what} is a {class} address; Hub reaches public destinations only"))
}

fn is_override(config: &NetConfig, url: &Url) -> bool {
    matches!(url.host(), Some(url::Host::Domain(d)) if config.overrides.iter().any(|o| o.host.eq_ignore_ascii_case(d)))
}

/// The checks of one hop before it is sent (§6.5).
async fn check_hop(config: &NetConfig, app: &str, url: &Url, plan: &ProxyPlan) -> Result<(), NetError> {
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(NetError::new(codes::REDIRECT, format!("a redirect to {}:", url.scheme())));
    }
    // **No `https://companion.pocket/<service>`** (§11): the owner decided on
    // no compatibility with the origin version 1 of Hub answered itself, and
    // this Hub runs no services behind it. The answer is the reference Hub's
    // for a Hub without services (`cli.ts` `netFetch`): `unavailable`.
    if url.host_str().is_some_and(|h| h.eq_ignore_ascii_case(SERVICE_HOST)) {
        return Err(NetError::new(codes::UNAVAILABLE, "this Hub runs no services"));
    }
    config.policy.allow(app, url).map_err(|why| NetError::new(codes::INVALID_REQUEST, why))?;
    match url.host() {
        Some(url::Host::Ipv4(address)) => {
            if let Some(class) = classify(address.into()) {
                return Err(refused(&address.to_string(), address.into(), class));
            }
        }
        Some(url::Host::Ipv6(address)) => {
            if let Some(class) = classify(address.into()) {
                return Err(refused(&address.to_string(), address.into(), class));
            }
        }
        Some(url::Host::Domain(name)) => {
            // Behind a proxy Hub's own lookup is the check (proxy.rs); on the
            // direct path the resolver checks at connect time.
            if !is_override(config, url) && plan.for_url(url).is_some() {
                resolve_checked(&*config.lookup, name).await?;
            }
        }
        None => return Err(NetError::new(codes::INVALID_REQUEST, "the url names no host")),
    }
    Ok(())
}

/// A proxy named by host name is resolved once and used by address, so the
/// direct-path resolver only ever sees destinations.
async fn pin_proxies(lookup: &dyn Lookup, plan: &mut ProxyPlan) -> Result<(), NetError> {
    for proxy in plan.proxies_mut() {
        if let Some(url::Host::Domain(name)) = proxy.host() {
            let name = name.to_owned();
            let first = *lookup
                .lookup(&name)
                .await
                .map_err(|error| NetError::new(codes::DNS, format!("the proxy {name} does not resolve: {error}")))?
                .first()
                .ok_or_else(|| NetError::new(codes::DNS, format!("the proxy {name} has no address")))?;
            proxy.set_ip_host(first).map_err(|_| NetError::new(codes::OTHER, format!("the proxy {name} cannot be addressed")))?;
        }
    }
    Ok(())
}

/// The codings Hub decodes: the body the device gets is the representation.
#[derive(Debug, Clone, Copy)]
enum Coding {
    Gzip,
    Deflate,
    Brotli,
}

fn coding(headers: &HeaderMap) -> Option<Coding> {
    let mut values = headers.get_all(reqwest::header::CONTENT_ENCODING).iter();
    let value = values.next()?.to_str().ok()?.trim().to_ascii_lowercase();
    if values.next().is_some() {
        return None;
    }
    match value.as_str() {
        "gzip" | "x-gzip" => Some(Coding::Gzip),
        "deflate" => Some(Coding::Deflate),
        "br" => Some(Coding::Brotli),
        _ => None,
    }
}

/// Sends the request and follows redirects by hand (§6.5, the reference's
/// rules): at most 3, each hop checked; 303, and 301/302 after a `POST`,
/// become `GET` without body or `content-type` (`HEAD` stays `HEAD`);
/// `authorization` and `cookie` are not carried to another origin.
pub(crate) async fn open(config: &NetConfig, clients: &Clients, request: &NetRequest, body: Bytes) -> Result<Opened, NetError> {
    let mut plan = config.proxies.current()?.plan()?;
    pin_proxies(&*config.lookup, &mut plan).await?;

    let mut url = request.url.clone();
    let mut method = request.method;
    let mut body = if method == "GET" || method == "HEAD" { None } else { Some(body) };
    let mut headers: Vec<(String, String)> = request.headers.iter().filter(|(n, _)| forwarded(n)).cloned().collect();
    if !headers.iter().any(|(n, _)| n == "user-agent") {
        headers.push(("user-agent".into(), config.user_agent.clone()));
    }
    if method != "HEAD" && !headers.iter().any(|(n, _)| n == "accept-encoding") {
        headers.push(("accept-encoding".into(), "gzip, deflate, br".into()));
    }

    for hop in 0.. {
        check_hop(config, &request.app, &url, &plan).await?;
        // Each hop takes the client of its host's kind: an override host's
        // connections never share a pool with any other host's (`clients.rs`).
        let client = if is_override(config, &url) { clients.overrides(config, &request.app)? } else { clients.public(config, &request.app, &plan)? };
        let mut map = HeaderMap::new();
        for (name, value) in &headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| NetError::new(codes::INVALID_REQUEST, format!("the header {name} is not one HTTP can carry")))?;
            let value = HeaderValue::from_str(value).map_err(|_| NetError::new(codes::INVALID_REQUEST, format!("the header {name} is not one HTTP can carry")))?;
            map.append(name, value);
        }
        let method_value = reqwest::Method::from_bytes(method.as_bytes()).expect("one of NET_METHODS");
        let mut builder = client.request(method_value, url.clone()).headers(map);
        if let Some(body) = &body {
            builder = builder.body(body.clone());
        }
        let response = builder.send().await.map_err(|error| map_error(&error))?;
        let status = response.status().as_u16();
        let location = response.headers().get(reqwest::header::LOCATION).cloned();
        if (300..400).contains(&status) && status != 304 && let Some(location) = location {
            drop(response);
            if hop >= MAX_REDIRECTS {
                return Err(NetError::new(codes::REDIRECT, format!("more than {MAX_REDIRECTS} redirects")));
            }
            let target = location.to_str().map_err(|_| NetError::new(codes::REDIRECT, "a redirect location that is not text"))?;
            let next = url.join(target).map_err(|error| NetError::new(codes::REDIRECT, format!("a bad redirect location: {error}")))?;
            if next.scheme() != "http" && next.scheme() != "https" {
                return Err(NetError::new(codes::REDIRECT, format!("a redirect to {}:", next.scheme())));
            }
            if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                if method != "HEAD" {
                    method = "GET";
                }
                body = None;
                headers.retain(|(n, _)| n != "content-type");
            }
            if next.origin() != url.origin() {
                headers.retain(|(n, _)| n != "authorization" && n != "cookie");
            }
            url = next;
            continue;
        }
        let no_body = method == "HEAD" || status == 204 || status == 304 || status < 200;
        let coding = if no_body { None } else { coding(response.headers()) };
        let headers = response_headers(response.headers(), coding.is_some());
        let body = if no_body { None } else { Some(reader(response, coding)) };
        return Ok(Opened { status, url, headers, body });
    }
    unreachable!("the loop returns")
}

/// A response body as an `AsyncBufRead`: the HTTP library is asked for the
/// next frame only when the reader wants bytes, so the download moves at
/// the pace the device grants (§5.6) and Hub holds one frame at a time.
struct BodyRead {
    body: reqwest::Body,
    chunk: Bytes,
    done: bool,
}

impl AsyncBufRead for BodyRead {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        use http_body::Body;
        let this = self.get_mut();
        while this.chunk.is_empty() && !this.done {
            match Pin::new(&mut this.body).poll_frame(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => this.done = true,
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(io::Error::other(error))),
                Poll::Ready(Some(Ok(frame))) => {
                    if let Ok(data) = frame.into_data() {
                        this.chunk = data;
                    }
                }
            }
        }
        Poll::Ready(Ok(&this.chunk[..]))
    }

    fn consume(self: Pin<&mut Self>, amount: usize) {
        let this = self.get_mut();
        let _ = this.chunk.split_to(amount.min(this.chunk.len()));
    }
}

impl AsyncRead for BodyRead {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let available = match self.as_mut().poll_fill_buf(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(bytes)) => {
                let n = bytes.len().min(buf.remaining());
                buf.put_slice(&bytes[..n]);
                n
            }
        };
        self.consume(available);
        Poll::Ready(Ok(()))
    }
}

/// A decoder that, once its coded stream ended, reads the raw body to its
/// end. zlib and brotli stop at the end of their stream, which can come
/// before the end of the body (a chunked body's last chunk often arrives in
/// a later segment); a response dropped there is not read whole, and the
/// HTTP library closes its connection instead of pooling it (`clients.rs`).
/// Bytes after the coded stream are not a body Hub can decode: `protocol`,
/// as the gzip decoder already makes them.
struct ToTheEnd<D> {
    decoder: D,
    raw: fn(&mut D) -> &mut BodyRead,
    ended: bool,
}

impl<D: AsyncRead + Unpin> AsyncRead for ToTheEnd<D> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.ended {
            let before = buf.filled().len();
            std::task::ready!(Pin::new(&mut this.decoder).poll_read(cx, buf))?;
            if buf.filled().len() > before || buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            this.ended = true;
        }
        let raw = (this.raw)(&mut this.decoder);
        match std::task::ready!(Pin::new(raw).poll_fill_buf(cx)) {
            Ok([]) => Poll::Ready(Ok(())),
            Ok(_) => Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, "bytes after the end of the coded body"))),
            Err(error) => Poll::Ready(Err(error)),
        }
    }
}

fn reader(response: reqwest::Response, coding: Option<Coding>) -> Box<dyn AsyncRead + Send + Unpin> {
    use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder};
    let raw = BodyRead { body: reqwest::Body::from(response), chunk: Bytes::new(), done: false };
    match coding {
        None => Box::new(raw),
        Some(Coding::Gzip) => {
            let mut decoder = GzipDecoder::new(raw);
            decoder.multiple_members(true);
            Box::new(ToTheEnd { decoder, raw: GzipDecoder::get_mut, ended: false })
        }
        Some(Coding::Deflate) => Box::new(ToTheEnd { decoder: ZlibDecoder::new(raw), raw: ZlibDecoder::get_mut, ended: false }),
        Some(Coding::Brotli) => Box::new(ToTheEnd { decoder: BrotliDecoder::new(raw), raw: BrotliDecoder::get_mut, ended: false }),
    }
}

/// Every error of a chain, the one inside an `io::Error` included (its
/// `source()` skips it).
fn chain<'a>(error: &'a (dyn std::error::Error + 'static)) -> Vec<&'a (dyn std::error::Error + 'static)> {
    let mut out = Vec::new();
    let mut current = Some(error);
    while let Some(e) = current {
        out.push(e);
        // An `io::Error` may wrap another (TLS failures come so).
        current = match e.downcast_ref::<io::Error>().and_then(|io| io.get_ref()) {
            Some(inner) => Some(inner as &(dyn std::error::Error + 'static)),
            None => e.source(),
        };
    }
    out
}

pub(crate) fn chain_text(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = String::new();
    for e in chain(error) {
        let part = e.to_string();
        if !text.contains(&part) {
            if !text.is_empty() {
                text.push_str(": ");
            }
            text.push_str(&part);
        }
    }
    text
}

/// A failure of the HTTP library as a `NET_ERROR` code (§6.5).
pub(crate) fn map_error(error: &reqwest::Error) -> NetError {
    let links = chain(error);
    // The policy's own refusal or lookup failure, raised in the resolver.
    if let Some(own) = links.iter().find_map(|e| e.downcast_ref::<NetError>()) {
        return own.clone();
    }
    let text = chain_text(error);
    if links.iter().any(|e| e.downcast_ref::<rustls::Error>().is_some()) {
        return NetError::new(codes::TLS, text);
    }
    if error.is_timeout() {
        return NetError::new(codes::TIMEOUT, text);
    }
    if error.is_connect() {
        return NetError::new(codes::CONNECT, text);
    }
    if error.is_redirect() {
        return NetError::new(codes::REDIRECT, text);
    }
    if error.is_body() || error.is_decode() || error.is_request() {
        return NetError::new(codes::PROTOCOL, text);
    }
    NetError::new(codes::OTHER, text)
}

/// A failure while reading the body: the server's stream or Hub's decoding of it.
pub(crate) fn map_read_error(error: &io::Error) -> NetError {
    let links = chain(error);
    if links.iter().any(|e| e.downcast_ref::<rustls::Error>().is_some()) {
        return NetError::new(codes::TLS, chain_text(error));
    }
    NetError::new(codes::PROTOCOL, format!("the response body broke off: {}", chain_text(error)))
}
