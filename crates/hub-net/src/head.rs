//! The response head as the device gets it: `EVENT` `net.head` (§6.5).
//!
//! - Header names lower case (the HTTP library gives them so).
//! - Hop-by-hop headers dropped: they describe Hub's connection to the
//!   server, not the response (the reference's set, `fetch.ts`).
//! - `content-encoding` and `content-length` dropped when Hub decoded the
//!   body: neither describes the bytes the device gets.
//! - Several `set-cookie` joined with `\n` (a cookie's value may hold a
//!   comma, so the `, ` join of other repeated headers would be ambiguous);
//!   other repeated headers joined with `, ` (RFC 9110 §5.3).
//! - At most 32 headers and an encoded object of at most 8 KiB: **headers
//!   are dropped, the last received first, until both hold, and `truncated`
//!   is set**; it is absent when every header was sent.
//! - The final URL alone over 2 KiB ends the request with
//!   `response_too_large` and no `net.head`.
//!
//! "Received" order is the order in which each name first appeared on the
//! wire; repeated values of a name join at its first place.

use pocket_link::text::{encoded_bytes, json_bytes};
use reqwest::header::HeaderMap;
use serde_json::{Map, Value};

use crate::request::{HEADERS_BYTES, MAX_HEADERS, URL_BYTES};
use crate::{NetError, codes};

/// Connection-level headers, never forwarded (RFC 9110 §7.6.1).
pub const HOP_BY_HOP: [&str; 9] = ["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "proxy-connection", "te", "trailer", "transfer-encoding", "upgrade"];

/// What `net.head` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub truncated: bool,
}

impl Head {
    /// Applies the limits of §6.5 to a response's headers in received order.
    pub fn fit(status: u16, url: &str, mut headers: Vec<(String, String)>) -> Result<Head, NetError> {
        if encoded_bytes(url) > URL_BYTES {
            return Err(NetError::new(codes::RESPONSE_TOO_LARGE, format!("the final URL is over {URL_BYTES} encoded bytes")));
        }
        let mut truncated = false;
        if headers.len() > MAX_HEADERS {
            headers.truncate(MAX_HEADERS);
            truncated = true;
        }
        while json_bytes(&object(&headers)) > HEADERS_BYTES {
            headers.pop();
            truncated = true;
        }
        Ok(Head { status, url: url.to_owned(), headers, truncated })
    }

    /// The `EVENT` object.
    pub fn to_object(&self) -> Map<String, Value> {
        let mut event = Map::new();
        event.insert("t".into(), Value::from("net.head"));
        event.insert("status".into(), Value::from(self.status));
        event.insert("url".into(), Value::from(self.url.clone()));
        event.insert("headers".into(), Value::Object(object(&self.headers)));
        if self.truncated {
            event.insert("truncated".into(), Value::Bool(true));
        }
        event
    }
}

fn object(headers: &[(String, String)]) -> Map<String, Value> {
    headers.iter().map(|(name, value)| (name.clone(), Value::from(value.clone()))).collect()
}

/// A header value as text: bytes outside ASCII are read as Latin-1, as a
/// browser's `fetch` reads them (a header value is a byte string).
fn text(value: &[u8]) -> String {
    value.iter().map(|&b| b as char).collect()
}

/// The response's headers in received order, joined and filtered as above.
/// `decoded`: Hub decoded the body's `content-encoding`.
pub fn response_headers(map: &HeaderMap, decoded: bool) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for name in map.keys() {
        let key = name.as_str();
        if HOP_BY_HOP.contains(&key) || (decoded && (key == "content-encoding" || key == "content-length")) {
            continue;
        }
        let separator = if key == "set-cookie" { "\n" } else { ", " };
        let values: Vec<String> = map.get_all(name).iter().map(|v| text(v.as_bytes())).collect();
        out.push((key.to_owned(), values.join(separator)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderName, HeaderValue};

    fn vector() -> Value {
        let vectors: Value = serde_json::from_str(include_str!("../../../vendor/pocketjs/tests/vectors/pocket-link-2/messages.json")).unwrap();
        vectors["http"]["head"].clone()
    }

    fn pairs(object: &Value) -> Vec<(String, String)> {
        object.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect()
    }

    /// The golden `net.head` of `messages.json`: a 2 KiB URL, 32 headers
    /// whose encoded object is 8 KiB, `truncated`, 10,306 bytes in all.
    #[test]
    fn the_golden_head_at_its_limits() {
        let golden = vector();
        let headers = pairs(&golden["json"]["headers"]);
        let url = golden["json"]["url"].as_str().unwrap();
        // The vector's headers fit exactly: nothing is dropped.
        let head = Head::fit(200, url, headers.clone()).unwrap();
        assert_eq!(head.headers, headers);
        assert!(!head.truncated);
        // One more header received last is dropped, and the head says so.
        let mut more = headers.clone();
        more.push(("x-late".into(), "1".into()));
        let head = Head::fit(200, url, more).unwrap();
        assert_eq!(head.headers, headers);
        assert!(head.truncated);
        let encoded = serde_json::to_vec(&Value::Object(head.to_object())).unwrap();
        assert_eq!(encoded.len() as u64, golden["bytes"].as_u64().unwrap());
        assert_eq!(serde_json::from_slice::<Value>(&encoded).unwrap(), golden["json"]);
        for limit in golden["fits"].as_array().unwrap() {
            // The event fits one frame of each payload limit the vector names.
            assert!(encoded.len() as u64 <= limit.as_u64().unwrap());
        }
        // One byte more in the first header pushes the last one out.
        let mut wider = headers.clone();
        wider[0].1.push('v');
        let head = Head::fit(200, url, wider).unwrap();
        assert_eq!(head.headers.len(), 31);
        assert_eq!(head.headers.last().unwrap().0, "x-header-30");
        assert!(head.truncated);
        assert!(json_bytes(&object(&head.headers)) <= HEADERS_BYTES);
    }

    #[test]
    fn a_final_url_over_2_kib_is_response_too_large() {
        let url = format!("https://example.com/{}", "p".repeat(2048 - 20));
        assert!(Head::fit(200, &url, vec![]).is_ok());
        assert_eq!(Head::fit(200, &format!("{url}p"), vec![]).unwrap_err().code, "response_too_large");
        // Escapes count: a quote takes two bytes.
        let quoted = format!("https://example.com/{}\"", "p".repeat(2048 - 21));
        assert_eq!(Head::fit(200, &quoted, vec![]).unwrap_err().code, "response_too_large");
    }

    #[test]
    fn more_than_32_headers_keep_the_first_32() {
        let headers: Vec<(String, String)> = (0..40).map(|i| (format!("x-{i:02}"), "v".into())).collect();
        let head = Head::fit(204, "https://example.com/", headers).unwrap();
        assert_eq!(head.headers.len(), 32);
        assert_eq!(head.headers[31].0, "x-31");
        assert!(head.truncated);
        assert_eq!(head.to_object()["truncated"], true);
        let small = Head::fit(204, "https://example.com/", vec![("a".into(), "b".into())]).unwrap();
        assert!(!small.to_object().contains_key("truncated"));
    }

    #[test]
    fn processing_of_the_received_headers() {
        let mut map = HeaderMap::new();
        let add = |map: &mut HeaderMap, n: &str, v: &[u8]| {
            map.append(HeaderName::from_bytes(n.as_bytes()).unwrap(), HeaderValue::from_bytes(v).unwrap());
        };
        add(&mut map, "Content-Type", b"text/plain");
        add(&mut map, "Set-Cookie", b"a=1; Path=/");
        add(&mut map, "Connection", b"keep-alive");
        add(&mut map, "Transfer-Encoding", b"chunked");
        add(&mut map, "Vary", b"accept");
        add(&mut map, "Set-Cookie", b"b=2, c; HttpOnly");
        add(&mut map, "Content-Encoding", b"gzip");
        add(&mut map, "Content-Length", b"20");
        add(&mut map, "Vary", b"origin");
        add(&mut map, "X-Latin", b"caf\xe9");
        let plain = response_headers(&map, false);
        let names: Vec<&str> = plain.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["content-type", "set-cookie", "vary", "content-encoding", "content-length", "x-latin"]);
        assert_eq!(plain[1].1, "a=1; Path=/\nb=2, c; HttpOnly");
        assert_eq!(plain[2].1, "accept, origin");
        assert_eq!(plain[5].1, "café");
        let decoded = response_headers(&map, true);
        let names: Vec<&str> = decoded.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["content-type", "set-cookie", "vary", "x-latin"]);
        // A joined cookie of 3,599 characters with one newline takes 3,600 bytes encoded.
        let escaped = format!("{}\n{}", "c".repeat(1799), "c".repeat(1799));
        assert_eq!(encoded_bytes(&escaped), 3600);
    }
}
