//! The `net.http` request object and its limits (§6.5, `contracts/spec/net.ts`).
//!
//! Every limit counts the encoded bytes in the frame (§4.4): the URL's
//! escaped bytes between its quotes, and the whole encoded `headers` object,
//! braces included. The device's host checks the same limits before it
//! queues a frame (§6.5); Hub checks again because a peer that holds the key
//! can send anything (§3.9). A request that breaks one is `invalid_request`.

use std::time::Duration;

use pocket_link::consts::APP_ID_BYTES;
use pocket_link::text::{encoded_bytes, json_bytes};
use reqwest::Url;
use serde_json::{Map, Value};

use crate::{NetError, codes};

/// `NET_METHODS`: CONNECT and TRACE are not applet operations.
pub const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];
/// LINK2_HTTP_URL_BYTES: the URL's encoded bytes, in the request and in `net.head`.
pub const URL_BYTES: usize = 2048;
/// LINK2_HTTP_HEADERS_BYTES: the encoded `headers` object, braces included.
pub const HEADERS_BYTES: usize = 8192;
/// LINK2_HTTP_MAX_HEADERS.
pub const MAX_HEADERS: usize = 32;
/// LINK2_HTTP_MAX_BODY: the request body.
pub const MAX_BODY: u64 = 64 * 1024;
/// LINK2_HTTP_MAX_RESPONSE: the largest `maxBytes`.
pub const MAX_RESPONSE: u64 = 256 * 1024;
/// NET_DEFAULT_RESPONSE_BYTES.
pub const DEFAULT_RESPONSE: u64 = 128 * 1024;
/// LINK2_HTTP_MAX_TIMEOUT_MS.
pub const MAX_TIMEOUT_MS: u64 = 120_000;
/// NET_DEFAULT_TIMEOUT_MS.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
/// NET_MAX_REDIRECTS.
pub const MAX_REDIRECTS: usize = 3;

/// A request that passed every limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetRequest {
    /// The applet id: the runtime's assertion (§6.5, §7.1).
    pub app: String,
    pub url: Url,
    pub method: &'static str,
    /// Names lower case, in the order received; a name sent twice in
    /// different cases keeps the later value.
    pub headers: Vec<(String, String)>,
    pub timeout: Duration,
    pub max_bytes: u64,
    /// The request body's declared length.
    pub bytes: u64,
}

fn invalid(message: impl Into<String>) -> NetError {
    NetError::new(codes::INVALID_REQUEST, message)
}

/// RFC 9110 `token`: the characters of a header name.
fn is_token(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A header value HTTP/1.1 can carry: no control character but tab.
fn is_field_value(value: &str) -> bool {
    value.bytes().all(|b| b == b'\t' || (b >= 0x20 && b != 0x7f))
}

fn integer(object: &Map<String, Value>, key: &str, default: Option<u64>, max: u64, min: u64) -> Result<u64, NetError> {
    match (object.get(key), default) {
        (None, Some(default)) => Ok(default),
        (None, None) => Err(invalid(format!("{key} is missing"))),
        (Some(value), _) => match value.as_u64() {
            Some(n) if (min..=max).contains(&n) => Ok(n),
            _ => Err(invalid(format!("{key} must be an integer from {min} to {max}"))),
        },
    }
}

/// Reads a `net.http` request object.
pub fn parse(object: &Map<String, Value>) -> Result<NetRequest, NetError> {
    let app = match object.get("app").and_then(Value::as_str) {
        Some(app) if !app.is_empty() && encoded_bytes(app) <= APP_ID_BYTES => app.to_owned(),
        _ => return Err(invalid("app must be an applet id of 1 to 128 bytes")),
    };
    let Some(text) = object.get("url").and_then(Value::as_str) else { return Err(invalid("url must be a string")) };
    if encoded_bytes(text) > URL_BYTES {
        return Err(invalid(format!("the url is over {URL_BYTES} encoded bytes")));
    }
    let url = Url::parse(text).map_err(|error| invalid(format!("the url does not parse: {error}")))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(invalid("only http: and https: URLs"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid("the url names no host"));
    }
    let method = match object.get("method").and_then(Value::as_str) {
        Some(method) => METHODS.iter().copied().find(|m| *m == method).ok_or_else(|| invalid(format!("the method {method} is not one net.http carries")))?,
        None => return Err(invalid("method must be a string")),
    };
    let empty = Map::new();
    let headers = match object.get("headers") {
        None => &empty,
        Some(Value::Object(headers)) => headers,
        Some(_) => return Err(invalid("headers must be an object of strings")),
    };
    if headers.len() > MAX_HEADERS {
        return Err(invalid(format!("more than {MAX_HEADERS} headers")));
    }
    if json_bytes(headers) > HEADERS_BYTES {
        return Err(invalid(format!("the headers object is over {HEADERS_BYTES} encoded bytes")));
    }
    let mut list: Vec<(String, String)> = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let Some(value) = value.as_str() else { return Err(invalid(format!("the header {name} is not a string"))) };
        if !is_token(name) || !is_field_value(value) {
            return Err(invalid(format!("the header {name} is not one HTTP can carry")));
        }
        let name = name.to_ascii_lowercase();
        list.retain(|(n, _)| *n != name);
        list.push((name, value.to_owned()));
    }
    let timeout_ms = integer(object, "timeoutMs", Some(DEFAULT_TIMEOUT_MS), MAX_TIMEOUT_MS, 1)?;
    let max_bytes = integer(object, "maxBytes", Some(DEFAULT_RESPONSE), MAX_RESPONSE, 0)?;
    let bytes = integer(object, "bytes", None, MAX_BODY, 0)?;
    if bytes > 0 && (method == "GET" || method == "HEAD") {
        return Err(invalid(format!("{method} carries no body")));
    }
    Ok(NetRequest { app, url, method, headers: list, timeout: Duration::from_millis(timeout_ms), max_bytes, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn base() -> Value {
        json!({"t":"net.http","app":"dev.demo","url":"https://example.com/a","method":"GET","headers":{},"bytes":0})
    }

    fn with(key: &str, value: Value) -> Map<String, Value> {
        let mut o = object(base());
        o.insert(key.into(), value);
        o
    }

    fn code(o: &Map<String, Value>) -> Option<String> {
        parse(o).err().map(|e| e.code)
    }

    #[test]
    fn defaults_and_limits() {
        let r = parse(&object(base())).unwrap();
        assert_eq!((r.timeout.as_millis(), r.max_bytes, r.bytes), (30_000, 131_072, 0));
        assert!(parse(&with("timeoutMs", json!(120_000))).is_ok());
        assert_eq!(code(&with("timeoutMs", json!(120_001))).as_deref(), Some("invalid_request"));
        assert_eq!(code(&with("timeoutMs", json!(0))).as_deref(), Some("invalid_request"));
        assert_eq!(code(&with("timeoutMs", json!(1.5))).as_deref(), Some("invalid_request"));
        assert!(parse(&with("maxBytes", json!(262_144))).is_ok());
        assert!(parse(&with("maxBytes", json!(0))).is_ok());
        assert_eq!(code(&with("maxBytes", json!(262_145))).as_deref(), Some("invalid_request"));
        let mut post = with("method", json!("POST"));
        post.insert("bytes".into(), json!(65_536));
        assert!(parse(&post).is_ok());
        post.insert("bytes".into(), json!(65_537));
        assert_eq!(code(&post).as_deref(), Some("invalid_request"));
        assert_eq!(code(&with("bytes", json!(1))).as_deref(), Some("invalid_request"), "GET carries no body");
        let mut missing = object(base());
        missing.remove("bytes");
        assert_eq!(code(&missing).as_deref(), Some("invalid_request"));
    }

    #[test]
    fn methods_and_schemes() {
        for method in METHODS {
            assert!(parse(&with("method", json!(method))).is_ok(), "{method}");
        }
        for method in ["get", "CONNECT", "TRACE", "PROPFIND"] {
            assert_eq!(code(&with("method", json!(method))).as_deref(), Some("invalid_request"), "{method}");
        }
        for url in ["ftp://example.com/", "file:///etc/passwd", "data:text/plain,x", "not a url", "http://"] {
            assert_eq!(code(&with("url", json!(url))).as_deref(), Some("invalid_request"), "{url}");
        }
        assert!(parse(&with("url", json!("http://example.com:8080/x?y=1"))).is_ok());
    }

    #[test]
    fn headers_rules() {
        assert_eq!(code(&with("headers", json!({"bad name": "x"}))).as_deref(), Some("invalid_request"));
        assert_eq!(code(&with("headers", json!({"x-a": "line\r\nbreak"}))).as_deref(), Some("invalid_request"));
        assert_eq!(code(&with("headers", json!({"x-a": 1}))).as_deref(), Some("invalid_request"));
        assert_eq!(code(&with("headers", json!(["x"]))).as_deref(), Some("invalid_request"));
        let many: Map<String, Value> = (0..33).map(|i| (format!("x-{i}"), json!("v"))).collect();
        assert_eq!(code(&with("headers", Value::Object(many))).as_deref(), Some("invalid_request"));
        let r = parse(&with("headers", json!({"X-Token": "a", "x-token": "b", "Accept": "text/plain"}))).unwrap();
        assert_eq!(r.headers, vec![("x-token".to_owned(), "b".to_owned()), ("accept".to_owned(), "text/plain".to_owned())]);
    }

    /// The golden request of `messages.json`: every field at its limit
    /// passes; one byte more of URL or headers does not.
    #[test]
    fn the_golden_request_at_its_limits() {
        let vectors: Value = serde_json::from_str(include_str!("../../../vendor/pocketjs/tests/vectors/pocket-link-2/messages.json")).unwrap();
        let http = &vectors["http"];
        let request = object(http["request"]["json"].clone());
        assert_eq!(serde_json::to_vec(&request).unwrap().len() as u64, http["request"]["bytes"].as_u64().unwrap());
        assert_eq!(encoded_bytes(request["url"].as_str().unwrap()) as u64, http["request"]["urlBytes"].as_u64().unwrap());
        assert_eq!(json_bytes(&request["headers"]) as u64, http["request"]["headersBytes"].as_u64().unwrap());
        assert_eq!(http["minPayload"], 12_288);
        let parsed = parse(&request).unwrap();
        assert_eq!((parsed.method, parsed.bytes, parsed.max_bytes, parsed.timeout.as_millis()), ("OPTIONS", 65_536, 262_144, 120_000));
        assert_eq!(parsed.headers.len(), 32);

        let mut long_url = request.clone();
        long_url.insert("url".into(), json!(format!("{}p", request["url"].as_str().unwrap())));
        assert_eq!(code(&long_url).as_deref(), Some("invalid_request"));
        let mut long_headers = request.clone();
        let mut headers = request["headers"].as_object().unwrap().clone();
        let first = headers.get("x-header-00").unwrap().as_str().unwrap().to_owned();
        headers.insert("x-header-00".into(), json!(format!("{first}v")));
        long_headers.insert("headers".into(), Value::Object(headers));
        assert_eq!(code(&long_headers).as_deref(), Some("invalid_request"));
        // A newline costs two bytes encoded (§4.4): 3,599 characters take 3,600.
        let escaped = &http["escaped"];
        assert_eq!(encoded_bytes(escaped["text"].as_str().unwrap()) as u64, escaped["encodedBytes"].as_u64().unwrap());
    }
}
