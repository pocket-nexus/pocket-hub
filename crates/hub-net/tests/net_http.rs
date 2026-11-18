//! `net.http` end to end (§6.5): the reference device's request through
//! Hub's real endpoint, Hub's fetch against local servers on loopback that
//! the resolve override names, and every rule of the provider.

mod support;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::*;

const APP: &str = "dev.pocket-nexus.demo";

/// A Hub whose `a.test` and `b.test` are the plain server.
async fn plain() -> (Server, Linked) {
    let server = Server::start(None, Material::default()).await;
    let linked = linked(config(&[("a.test", server.address()), ("b.test", server.address())]), json!({})).await;
    (server, linked)
}

fn header<'a>(outcome: &'a Value, name: &str) -> Option<&'a str> {
    outcome["head"]["headers"][name].as_str()
}

fn header_names(outcome: &Value) -> Vec<String> {
    outcome["head"]["headers"].as_object().unwrap().keys().cloned().collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_small_get_a_large_body_and_a_post() {
    let (mut server, mut t) = plain().await;

    let small = t.http("small", json!({"app": APP, "url": "http://a.test/small"})).await;
    assert_eq!(code(&small), "ok", "{small}");
    assert_eq!(small["text"], "hello, pocket!");
    assert_eq!(small["value"]["bytes"], 14);
    assert_eq!(small["head"]["status"], 200);
    assert_eq!(small["head"]["url"], "http://a.test/small");
    assert_eq!(header(&small, "content-type"), Some("text/plain"));
    assert!(small["head"].get("truncated").is_none());
    let Seen::Request { headers, .. } = server.request("/small").await else { unreachable!() };
    let sent = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    assert_eq!(sent("user-agent"), Some("pocket-hub"));
    assert_eq!(sent("accept-encoding"), Some("gzip, deflate, br"));
    assert_eq!(sent("host"), Some("a.test"));

    let large = t.http("large", json!({"app": APP, "url": "http://a.test/bytes/100000"})).await;
    assert_eq!(code(&large), "ok", "{large}");
    assert_eq!(large["bytes"], 100_000);
    assert_eq!(large["value"]["bytes"], 100_000);
    assert_eq!(large["sum"], fnv32(&pattern(100_000)));

    let post = t.http("post", json!({"app": APP, "url": "http://a.test/echo", "method": "POST", "body": 700, "headers": {"Content-Type": "application/octet-stream", "X-Applet": "yes"}})).await;
    assert_eq!(code(&post), "ok", "{post}");
    assert_eq!(post["bytes"], 700);
    assert_eq!(post["sum"], fnv32(&pattern(700)));
    assert_eq!(header(&post, "x-method"), Some("POST"));
    let Seen::Request { method, body, headers, .. } = server.request("/echo").await else { unreachable!() };
    assert_eq!((method.as_str(), body), ("POST", pattern(700)));
    assert!(headers.iter().any(|(n, v)| n == "x-applet" && v == "yes"));

    // A body shorter than declared ends the request with `transfer` (§5.6).
    let short = t.http("short", json!({"app": APP, "url": "http://a.test/echo", "method": "POST", "body": 700, "send": 500})).await;
    assert_eq!(code(&short), "transfer", "{short}");
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn head_and_the_head_rules() {
    let server = Server::start(None, Material { vector_headers: vector_headers() }).await;
    let mut t = linked(config(&[("a.test", server.address())]), json!({})).await;

    let head = t.http("head", json!({"app": APP, "url": "http://a.test/head", "method": "HEAD"})).await;
    assert_eq!(code(&head), "ok", "{head}");
    assert_eq!((head["bytes"].as_u64(), head["value"]["bytes"].as_u64()), (Some(0), Some(0)));
    assert_eq!(header(&head, "content-length"), Some("1234"), "HEAD keeps the length the server states");

    let cookies = t.http("cookies", json!({"app": APP, "url": "http://a.test/cookies"})).await;
    assert_eq!(code(&cookies), "ok", "{cookies}");
    assert_eq!(header_names(&cookies), ["content-type", "set-cookie", "x-one", "vary", "content-length"]);
    assert_eq!(header(&cookies, "set-cookie"), Some("session=abc; Path=/; HttpOnly\ntheme=dark, light; Max-Age=60"));
    assert_eq!(header(&cookies, "vary"), Some("accept, origin"));
    assert!(cookies["head"].get("truncated").is_none());

    // The golden head of messages.json plus one header received last: that
    // one is dropped and `truncated` is set; the rest is the vector's.
    let vector = t.http("vector", json!({"app": APP, "url": "http://a.test/vector"})).await;
    assert_eq!(code(&vector), "ok", "{vector}");
    assert_eq!(vector["head"]["truncated"], true);
    let expected: serde_json::Map<String, Value> = vector_headers().into_iter().map(|(n, v)| (n, Value::from(v))).collect();
    assert_eq!(vector["head"]["headers"], Value::Object(expected));
    t.device.kill().await;
}

fn vector_headers() -> Vec<(String, String)> {
    let vectors: Value = serde_json::from_str(include_str!("../../../vendor/pocketjs/tests/vectors/pocket-link-2/messages.json")).unwrap();
    vectors["http"]["head"]["json"]["headers"].as_object().unwrap().iter().map(|(n, v)| (n.clone(), v.as_str().unwrap().to_owned())).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn compressed_responses_are_decoded() {
    let (_server, mut t) = plain().await;
    let gzip = t.http("gzip", json!({"app": APP, "url": "http://a.test/gzip/50000"})).await;
    assert_eq!(code(&gzip), "ok", "{gzip}");
    assert_eq!((gzip["bytes"].as_u64(), gzip["sum"].as_u64()), (Some(50_000), Some(fnv32(&pattern(50_000)) as u64)));
    assert_eq!(header(&gzip, "content-encoding"), None);
    assert_eq!(header(&gzip, "content-length"), None);
    assert_eq!(header(&gzip, "x-after"), Some("kept"));
    let deflate = t.http("deflate", json!({"app": APP, "url": "http://a.test/deflate/20000"})).await;
    assert_eq!((code(&deflate).as_str(), deflate["sum"].as_u64()), ("ok", Some(fnv32(&pattern(20_000)) as u64)));
    let bad = t.http("bad", json!({"app": APP, "url": "http://a.test/badgzip"})).await;
    assert_eq!(code(&bad), "protocol", "{bad}");
    // maxBytes counts the decoded bytes.
    let over = t.http("over", json!({"app": APP, "url": "http://a.test/gzip/50000", "maxBytes": 40000})).await;
    assert_eq!((code(&over).as_str(), over["bytes"].as_u64()), ("response_too_large", Some(40_000)));
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn redirects_by_hand() {
    let (mut server, mut t) = plain().await;
    let port = server.port;

    let three = t.http("three", json!({"app": APP, "url": "http://a.test/redirect/3"})).await;
    assert_eq!((code(&three).as_str(), three["text"].as_str()), ("ok", Some("arrived")), "{three}");
    assert_eq!(three["head"]["url"], "http://a.test/redirect/0");
    let four = t.http("four", json!({"app": APP, "url": "http://a.test/redirect/4"})).await;
    assert_eq!(code(&four), "redirect", "{four}");

    // A redirect to a private address, by literal and by name, is refused before any connection.
    let literal = t.http("literal", json!({"app": APP, "url": format!("http://a.test/to/302/http://127.0.0.1:{port}/small")})).await;
    assert_eq!(code(&literal), "invalid_request", "{literal}");
    let named = t.http("named", json!({"app": APP, "url": "http://a.test/to/302/http://private192.test/small"})).await;
    assert_eq!(code(&named), "invalid_request", "{named}");
    let local = t.http("local", json!({"app": APP, "url": format!("http://a.test/to/301/http://localhost:{port}/small")})).await;
    assert_eq!(code(&local), "invalid_request", "{local}");
    assert_eq!(server.count_seen("/small"), 0, "no hop reached the private address");
    let scheme = t.http("scheme", json!({"app": APP, "url": "http://a.test/to/302/ftp://a.test/x"})).await;
    assert_eq!(code(&scheme), "redirect", "{scheme}");

    // 303 after POST: GET, no body, no content-type.
    let post = json!({"app": APP, "method": "POST", "body": 10, "headers": {"content-type": "text/plain", "authorization": "Bearer t"}});
    let mut see_other = post.clone();
    see_other["url"] = json!("http://a.test/to/303/http://a.test/method");
    let see_other = t.http("303", see_other).await;
    assert_eq!(see_other["text"], "GET 0 - auth pocket-hub", "{see_other}");
    // 302 after POST: the same.
    let mut found = post.clone();
    found["url"] = json!("http://a.test/to/302/http://a.test/method");
    assert_eq!(t.http("302", found).await["text"], "GET 0 - auth pocket-hub");
    // 307 keeps the method and the body.
    let mut temporary = post.clone();
    temporary["url"] = json!("http://a.test/to/307/http://a.test/method");
    assert_eq!(t.http("307", temporary).await["text"], "POST 10 text/plain auth pocket-hub");
    // HEAD stays HEAD after 303.
    let head = t.http("head303", json!({"app": APP, "method": "HEAD", "url": "http://a.test/to/303/http://a.test/head"})).await;
    assert_eq!((code(&head).as_str(), head["bytes"].as_u64()), ("ok", Some(0)));
    // Another origin gets no authorization or cookie.
    let mut other = post.clone();
    other["url"] = json!("http://a.test/to/307/http://b.test/method");
    assert_eq!(t.http("other", other).await["text"], "POST 10 text/plain noauth pocket-hub");
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_refused_class_by_literal_and_by_name() {
    let (mut server, mut t) = plain().await;
    let port = server.port;
    let literals = [
        format!("http://127.0.0.1:{port}/small"),
        "http://127.255.0.1/".into(),
        "http://[::1]/".into(),
        "http://10.0.0.1/".into(),
        "http://172.16.0.1/".into(),
        "http://192.168.1.1/".into(),
        "http://100.64.0.1/".into(),
        "http://169.254.169.254/latest/meta-data/".into(),
        "http://[fe80::1]/".into(),
        "http://224.0.0.251/".into(),
        "http://[ff02::1]/".into(),
        "http://[fd00::1]/".into(),
        "http://0.0.0.0/".into(),
        "http://[::]/".into(),
        "http://240.0.0.1/".into(),
        "http://255.255.255.255/".into(),
        "http://[::ffff:127.0.0.1]/".into(),
        "http://[64:ff9b::7f00:1]/".into(),
        "http://[2002:c0a8:101::1]/".into(),
        // WHATWG URL parsing turns these into 127.0.0.1 before the check.
        "http://2130706433/".into(),
        "http://0x7f.1/".into(),
        "https://127.1/".into(),
    ];
    for (i, url) in literals.iter().enumerate() {
        let outcome = t.http(&format!("l{i}"), json!({"app": APP, "url": url})).await;
        assert_eq!(code(&outcome), "invalid_request", "{url}: {outcome}");
        assert!(outcome["head"].is_null());
    }
    for (i, (name, address)) in refused_names().into_iter().enumerate() {
        let outcome = t.http(&format!("n{i}"), json!({"app": APP, "url": format!("http://{name}/")})).await;
        assert_eq!(code(&outcome), "invalid_request", "{name} ({address}): {outcome}");
        assert!(outcome["message"].as_str().unwrap().contains(&address.parse::<std::net::IpAddr>().unwrap().to_string()), "{outcome}");
    }
    let mixed = t.http("mixed", json!({"app": APP, "url": "http://mixed.test/"})).await;
    assert_eq!(code(&mixed), "invalid_request", "one private address refuses the name: {mixed}");
    let local = t.http("localhost", json!({"app": APP, "url": format!("http://localhost:{port}/small")})).await;
    assert_eq!(code(&local), "invalid_request", "{local}");
    assert_eq!(server.count_seen("/"), 0);
    t.device.kill().await;
}

/// A small GET that leaves its connection idle in Hub's pool; returns the
/// server's number for that connection.
async fn warm(t: &mut Linked, server: &mut Server, tag: &str, url: &str) -> usize {
    let outcome = t.http(tag, json!({"app": APP, "url": url})).await;
    assert_eq!((code(&outcome).as_str(), outcome["text"].as_str()), ("ok", Some("hello, pocket!")), "{outcome}");
    server.conn_of("/small").await
}

#[tokio::test(flavor = "multi_thread")]
async fn timeouts_cancels_and_a_dropped_link_end_the_fetch() {
    let (mut server, mut t) = plain().await;

    // The request's own timeout: the head and the first 1,000 bytes came,
    // the rest never does. The device keeps the prefix. It rides the
    // connection a finished request left idle, and that connection closes.
    let pooled = warm(&mut t, &mut server, "warm1", "http://a.test/small").await;
    let started = Instant::now();
    let slow = t.http("timeout", json!({"app": APP, "url": "http://a.test/slow", "timeoutMs": 600})).await;
    assert_eq!(code(&slow), "timeout", "{slow}");
    assert_eq!((slow["head"]["status"].as_u64(), slow["bytes"].as_u64()), (Some(200), Some(1000)));
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(550) && elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(server.conn_of("/slow").await, pooled, "the request rode the pooled connection");
    assert!(server.closed("/slow", Duration::from_secs(1)).await.is_some(), "the timed-out fetch's connection was dropped");

    // A device CANCEL: Hub answers `cancelled` and drops its connection to
    // the server at once (the reference Hub did not, §13.6), the pooled
    // connection the request rode included; the next request succeeds on a
    // new connection.
    let pooled = warm(&mut t, &mut server, "warm2", "http://a.test/small").await;
    let accepted = server.accepts();
    t.send("cancel", &mut json!({"app": APP, "url": "http://a.test/slow", "cancelOnHead": true})).await;
    let _ = t.device.wait(|v| v["ev"] == "http-cancelled").await;
    let cancelled_at = Instant::now();
    let closed = server.closed("/slow", Duration::from_secs(1)).await.expect("Hub dropped the server connection within a second of the CANCEL");
    assert!(closed.saturating_duration_since(cancelled_at) < Duration::from_millis(500), "{:?}", closed.saturating_duration_since(cancelled_at));
    let outcome = t.outcome("cancel").await;
    assert_eq!(code(&outcome), "cancelled", "{outcome}");
    assert_eq!(server.conn_of("/slow").await, pooled, "the cancelled request rode the pooled connection");
    let next = warm(&mut t, &mut server, "after-cancel", "http://a.test/small").await;
    assert_ne!(next, pooled);
    assert_eq!(server.accepts(), accepted + 1, "the request after the CANCEL opened a fresh connection");

    // A cancel before any head: a server that never answers. The request
    // rides the connection the last one left idle; the next one gets a new one.
    t.send("hang", &mut json!({"app": APP, "url": "http://a.test/hang", "cancelAfterMs": 300})).await;
    assert_eq!(code(&t.outcome("hang").await), "cancelled");
    assert!(server.closed("/hang", Duration::from_secs(1)).await.is_some());
    assert_eq!(server.conn_of("/hang").await, next);
    let after_hang = warm(&mut t, &mut server, "after-hang", "http://a.test/small").await;
    assert_ne!(after_hang, next);

    // The link dropped mid-download: the device process is killed.
    t.send("drop", &mut json!({"app": APP, "url": "http://a.test/slow"})).await;
    let _ = t.device.wait(|v| v["ev"] == "http-head" && v["tag"] == "drop").await;
    let Linked { device, .. } = t;
    let killed_at = Instant::now();
    device.kill().await;
    let closed = server.closed("/slow", Duration::from_secs(2)).await.expect("Hub dropped the fetch when the link ended");
    assert!(closed.saturating_duration_since(killed_at) < Duration::from_secs(1));
}

#[tokio::test(flavor = "multi_thread")]
async fn response_too_large_keeps_the_prefix() {
    let (_server, mut t) = plain().await;
    let capped = t.http("capped", json!({"app": APP, "url": "http://a.test/big/300000", "maxBytes": 100000})).await;
    assert_eq!(code(&capped), "response_too_large", "{capped}");
    assert_eq!((capped["bytes"].as_u64(), capped["sum"].as_u64()), (Some(100_000), Some(fnv32(&pattern(100_000)) as u64)));
    let default = t.http("default", json!({"app": APP, "url": "http://a.test/big/200000"})).await;
    assert_eq!((code(&default).as_str(), default["bytes"].as_u64()), ("response_too_large", Some(131_072)));
    let exact = t.http("exact", json!({"app": APP, "url": "http://a.test/big/131072"})).await;
    assert_eq!((code(&exact).as_str(), exact["bytes"].as_u64()), ("ok", Some(131_072)));
    let zero = t.http("zero", json!({"app": APP, "url": "http://a.test/small", "maxBytes": 0})).await;
    assert_eq!((code(&zero).as_str(), zero["bytes"].as_u64()), ("response_too_large", Some(0)));
    let largest = t.http("largest", json!({"app": APP, "url": "http://a.test/big/262144", "maxBytes": 262144})).await;
    assert_eq!((code(&largest).as_str(), largest["sum"].as_u64()), ("ok", Some(fnv32(&pattern(262_144)) as u64)));
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn eight_open_requests_and_the_ninth_is_busy() {
    let (mut server, mut t) = plain().await;
    for i in 1..=9 {
        t.send(&format!("h{i}"), &mut json!({"app": APP, "url": "http://a.test/hang"})).await;
    }
    let ninth = t.outcome("h9").await;
    assert_eq!(code(&ninth), "busy", "{ninth}");
    // The eight reached the server and wait there.
    let deadline = Instant::now() + WAIT;
    while server.count_seen("/hang") < 8 {
        assert!(Instant::now() < deadline, "eight requests reach the server");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    server.release();
    for i in 1..=8 {
        let outcome = t.outcome(&format!("h{i}")).await;
        assert_eq!((code(&outcome).as_str(), outcome["text"].as_str()), ("ok", Some("released")), "{outcome}");
    }
    // The places are free again.
    let after = t.http("after", json!({"app": APP, "url": "http://a.test/small"})).await;
    assert_eq!(code(&after), "ok");
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_under_12288_bytes_is_unsupported() {
    let server = Server::start(None, Material::default()).await;
    let mut t = linked(config(&[("a.test", server.address())]), json!({"maxPayload": 8192})).await;
    let outcome = t.http("small", json!({"app": APP, "url": "http://a.test/small"})).await;
    assert_eq!(code(&outcome), "unsupported", "{outcome}");
    let snapshot = t.hub.endpoint.devices().into_iter().next().unwrap();
    let link = snapshot.link.unwrap();
    assert_eq!(link.payload_limit, 8192);
    assert!(link.hub_features.contains(&"net.http".to_owned()));
    assert!(!link.features_in_use.contains(&"net.http".to_owned()), "net.http is not in use on this link");
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dns_connect_tls_failures_and_https() {
    let plain = Server::start(None, Material::default()).await;
    let (authority_config, authority) = tls(&["secure.test"], false);
    let secure = Server::start(Some(authority_config), Material::default()).await;
    let (self_signed_config, _) = tls(&["selfsigned.test"], true);
    let self_signed = Server::start(Some(self_signed_config), Material::default()).await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    drop(closed);
    let mut settings = config(&[
        ("a.test", plain.address()),
        ("secure.test", secure.address()),
        ("selfsigned.test", self_signed.address()),
        ("closed.test", std::net::SocketAddr::from(([127, 0, 0, 1], closed_port))),
    ]);
    settings.extra_roots = vec![authority];
    let mut t = linked(settings, json!({})).await;

    let dns = t.http("dns", json!({"app": APP, "url": "http://nothing-here.invalid/"})).await;
    assert_eq!(code(&dns), "dns", "{dns}");
    let unknown = t.http("unknown", json!({"app": APP, "url": "http://not-in-the-table.test/"})).await;
    assert_eq!(code(&unknown), "dns", "{unknown}");
    let connect = t.http("connect", json!({"app": APP, "url": "http://closed.test/"})).await;
    assert_eq!(code(&connect), "connect", "{connect}");
    let tls_failure = t.http("tls", json!({"app": APP, "url": "https://selfsigned.test/small"})).await;
    assert_eq!(code(&tls_failure), "tls", "{tls_failure}");
    let https = t.http("https", json!({"app": APP, "url": "https://secure.test/bytes/30000"})).await;
    assert_eq!(code(&https), "ok", "{https}");
    assert_eq!(https["sum"], fnv32(&pattern(30_000)));
    assert_eq!(https["head"]["url"], "https://secure.test/bytes/30000");
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn companion_pocket_is_not_served_and_hub_checks_the_limits() {
    let (_server, mut t) = plain().await;
    for url in ["https://companion.pocket/weather", "http://COMPANION.pocket/x"] {
        let outcome = t.http(url, json!({"app": APP, "url": url})).await;
        assert_eq!((code(&outcome).as_str(), outcome["message"].as_str()), ("unavailable", Some("this Hub runs no services")), "{outcome}");
    }
    // A redirect to it as well.
    let redirected = t.http("redirected", json!({"app": APP, "url": "http://a.test/to/302/https://companion.pocket/x"})).await;
    assert_eq!(code(&redirected), "unavailable");

    // What the device's host refuses locally, Hub refuses too.
    let long = format!("http://a.test/{}", "p".repeat(2048));
    for (tag, request) in [
        ("url", json!({"app": APP, "url": long})),
        ("scheme", json!({"app": APP, "url": "ftp://a.test/"})),
        ("method", json!({"app": APP, "url": "http://a.test/small", "method": "TRACE"})),
        ("timeout", json!({"app": APP, "url": "http://a.test/small", "timeoutMs": 120001})),
        ("max", json!({"app": APP, "url": "http://a.test/small", "maxBytes": 262145})),
        ("get-body", json!({"app": APP, "url": "http://a.test/small", "body": 5})),
        ("no-app", json!({"app": "", "url": "http://a.test/small"})),
    ] {
        let outcome = t.http(tag, request).await;
        assert_eq!(code(&outcome), "invalid_request", "{tag}: {outcome}");
    }
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_reader_slows_the_download() {
    let (_server, mut t) = plain().await;
    // The device takes 8 KiB at a time and holds each chunk 20 ms before
    // its credit returns: 200,000 bytes take at least 24 such turns.
    let started = Instant::now();
    let outcome = t.http("slow", json!({"app": APP, "url": "http://a.test/bytes/200000", "maxBytes": 262144, "slowMs": 20})).await;
    assert_eq!(code(&outcome), "ok", "{outcome}");
    assert_eq!((outcome["bytes"].as_u64(), outcome["sum"].as_u64()), (Some(200_000), Some(fnv32(&pattern(200_000)) as u64)));
    assert!(started.elapsed() >= Duration::from_millis(24 * 20), "{:?}", started.elapsed());
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn through_an_http_proxy() {
    use pocket_hub_net::{Proxies, ProxySettings};
    let mut proxy = Server::start(None, Material::default()).await;
    let mut settings = config(&[]);
    settings.proxies = Proxies::Fixed(ProxySettings { http: Some(format!("127.0.0.1:{}", proxy.port)), https: None, no_proxy: "skip.test".into() });
    let mut t = linked(settings, json!({})).await;
    // A public literal goes to the proxy, which answers it here (no network).
    let via = t.http("via", json!({"app": APP, "url": "http://93.184.215.14/x?q=1"})).await;
    assert_eq!(code(&via), "ok", "{via}");
    assert_eq!(via["text"], "proxied http://93.184.215.14/x?q=1");
    // Private destinations are refused before the proxy sees them, by literal and by name.
    for url in ["http://10.0.0.1/", "http://private10.test/", "http://loopback6.test/"] {
        let outcome = t.http(url, json!({"app": APP, "url": url})).await;
        assert_eq!(code(&outcome), "invalid_request", "{url}: {outcome}");
    }
    // Hub resolves the name itself before it hands it to the proxy.
    let unknown = t.http("unknown", json!({"app": APP, "url": "http://not-in-the-table.test/"})).await;
    assert_eq!(code(&unknown), "dns", "{unknown}");
    assert_eq!(proxy.count_seen("http://"), 1);
    t.device.kill().await;
}

// ---------------------------------------------------------------------------
// Connection reuse (clients.rs)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn sequential_https_requests_ride_one_connection() {
    let (authority_config, authority) = tls(&["secure.test"], false);
    let mut secure = Server::start(Some(authority_config), Material::default()).await;
    let mut settings = config(&[("secure.test", secure.address())]);
    settings.extra_roots = vec![authority];
    let mut t = linked(settings, json!({})).await;

    let mut times = Vec::new();
    for i in 0..10 {
        let outcome = t.http(&format!("s{i}"), json!({"app": APP, "url": "https://secure.test/small"})).await;
        assert_eq!((code(&outcome).as_str(), outcome["text"].as_str()), ("ok", Some("hello, pocket!")), "{outcome}");
        times.push(outcome["ms"].as_u64().unwrap());
        assert_eq!(secure.conn_of("/small").await, 1, "request {i} rode the first connection");
    }
    assert_eq!(secure.accepts(), 1, "ten sequential requests, one TCP connection");
    // The first request pays for the client, TCP and the TLS handshake, the
    // later ones for a request alone. The times are printed and not asserted:
    // on a busy machine their ratio says nothing, and the one accepted
    // connection above is the proof of reuse.
    eprintln!("10 HTTPS GETs on loopback, ms each as the device saw them: {times:?}");
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn each_host_name_and_each_applet_has_its_own_connections() {
    // a.test and b.test are the same server: connections still go by name.
    let (mut server, mut t) = plain().await;
    const OTHER: &str = "dev.pocket-nexus.other";
    let mut conns = Vec::new();
    for (i, (app, host)) in [(APP, "a.test"), (APP, "b.test"), (APP, "a.test"), (APP, "b.test"), (OTHER, "a.test"), (OTHER, "a.test")].into_iter().enumerate() {
        let outcome = t.http(&format!("r{i}"), json!({"app": app, "url": format!("http://{host}/small")})).await;
        assert_eq!(code(&outcome), "ok", "{outcome}");
        conns.push(server.conn_of("/small").await);
    }
    assert_eq!(conns[0], conns[2], "a.test reuses its connection");
    assert_eq!(conns[1], conns[3], "b.test reuses its connection");
    assert_ne!(conns[0], conns[1], "a.test never rides b.test's connection");
    assert_eq!(conns[4], conns[5], "the other applet reuses its own connection");
    assert!(conns[4] != conns[0] && conns[4] != conns[1], "applets never share a connection");
    assert_eq!(server.accepts(), 3);
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_response_cut_by_max_bytes_leaves_the_next_request_whole() {
    let (authority_config, authority) = tls(&["secure.test"], false);
    let mut secure = Server::start(Some(authority_config), Material::default()).await;
    let mut settings = config(&[("secure.test", secure.address())]);
    settings.extra_roots = vec![authority];
    let mut t = linked(settings, json!({})).await;

    // Cut mid-body on a pooled connection, with megabytes still to come:
    // that connection closes, and the next request reads its own response
    // whole on a new one.
    let pooled = warm(&mut t, &mut secure, "warm", "https://secure.test/small").await;
    let capped = t.http("capped", json!({"app": APP, "url": "https://secure.test/big/3000000", "maxBytes": 100000})).await;
    assert_eq!((code(&capped).as_str(), capped["bytes"].as_u64(), capped["sum"].as_u64()), ("response_too_large", Some(100_000), Some(fnv32(&pattern(100_000)) as u64)), "{capped}");
    assert_eq!(secure.conn_of("/big").await, pooled);
    assert!(secure.closed("/big", Duration::from_secs(1)).await.is_some(), "the cut response's connection was closed, not pooled");
    let next = t.http("next", json!({"app": APP, "url": "https://secure.test/bytes/5000"})).await;
    assert_eq!((code(&next).as_str(), next["bytes"].as_u64(), next["sum"].as_u64()), ("ok", Some(5000), Some(fnv32(&pattern(5000)) as u64)), "{next}");
    assert_ne!(secure.conn_of("/bytes").await, pooled);
    assert_eq!(secure.accepts(), 2);

    // Cut with the rest of the body already at Hub: the HTTP library may
    // read it to the end in the one read it makes when the response is
    // dropped, and then the connection is at a message boundary and pooled.
    // Either way the next response is its own, byte for byte.
    let short_cut = t.http("short-cut", json!({"app": APP, "url": "https://secure.test/big/300000", "maxBytes": 100000})).await;
    assert_eq!((code(&short_cut).as_str(), short_cut["bytes"].as_u64()), ("response_too_large", Some(100_000)));
    let after_cut = t.http("after-cut", json!({"app": APP, "url": "https://secure.test/bytes/9000"})).await;
    assert_eq!((code(&after_cut).as_str(), after_cut["sum"].as_u64()), ("ok", Some(fnv32(&pattern(9000)) as u64)), "{after_cut}");

    // Cut in the decoded bytes, and cut at 0 of a body that may have
    // arrived whole: whatever the connection's fate, the next response is
    // its own, byte for byte.
    let over = t.http("over", json!({"app": APP, "url": "https://secure.test/gzip/50000", "maxBytes": 40000})).await;
    assert_eq!((code(&over).as_str(), over["bytes"].as_u64()), ("response_too_large", Some(40_000)));
    let after_gzip = t.http("after-gzip", json!({"app": APP, "url": "https://secure.test/bytes/7000"})).await;
    assert_eq!((code(&after_gzip).as_str(), after_gzip["sum"].as_u64()), ("ok", Some(fnv32(&pattern(7000)) as u64)), "{after_gzip}");
    let zero = t.http("zero", json!({"app": APP, "url": "https://secure.test/small", "maxBytes": 0})).await;
    assert_eq!((code(&zero).as_str(), zero["bytes"].as_u64()), ("response_too_large", Some(0)));
    for i in 0..3 {
        let again = t.http(&format!("again{i}"), json!({"app": APP, "url": "https://secure.test/small"})).await;
        assert_eq!((code(&again).as_str(), again["text"].as_str()), ("ok", Some("hello, pocket!")), "{again}");
    }
    t.device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_override_host_never_shares_a_connection_with_a_public_lookup() {
    let mut server = Server::start(None, Material::default()).await;
    // Looked up publicly, both names are the loopback address: refused.
    let names = || Table([("a.test", "127.0.0.1"), ("a.test.", "127.0.0.1")].into_iter().map(|(n, a)| (n.to_owned(), vec![a.parse().unwrap()])).collect());
    let mut with = config(&[("a.test", server.address())]);
    with.lookup = Arc::new(names());
    let mut t = linked(with, json!({})).await;
    for i in 0..2 {
        let outcome = t.http(&format!("o{i}"), json!({"app": APP, "url": "http://a.test/small"})).await;
        assert_eq!(code(&outcome), "ok", "{outcome}");
    }
    assert_eq!(server.accepts(), 1, "the override's connection is pooled");

    // The name with a trailing dot is not the override: a public lookup,
    // refused, with the override's connection idle in the same provider.
    let dotted = t.http("dotted", json!({"app": APP, "url": "http://a.test./small"})).await;
    assert_eq!(code(&dotted), "invalid_request", "{dotted}");

    // Another provider in the same process, without the override: its
    // public lookup of a.test is refused; the first provider's pooled
    // connection to a.test is not its to ride.
    let mut without = config(&[]);
    without.lookup = Arc::new(names());
    let mut u = linked(without, json!({})).await;
    let public = u.http("public", json!({"app": APP, "url": "http://a.test/small"})).await;
    assert_eq!(code(&public), "invalid_request", "{public}");

    assert_eq!(server.accepts(), 1);
    assert_eq!(server.count_seen("/small"), 2, "only the override's own requests reached the server");
    t.device.kill().await;
    u.device.kill().await;
}

/// A resolver whose answers a test changes.
struct Switchable(Arc<Mutex<HashMap<String, Vec<IpAddr>>>>);

impl pocket_hub_net::Lookup for Switchable {
    fn lookup(&self, host: &str) -> pocket_hub_core::BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
        let found = self.0.lock().unwrap().get(host).cloned();
        Box::pin(async move { found.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "not in the test table")) })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn behind_a_proxy_every_request_is_checked_and_a_plan_change_opens_new_connections() {
    use pocket_hub_net::{Proxies, ProxySettings};
    let mut proxy = Server::start(None, Material::default()).await;
    let current = Arc::new(Mutex::new(ProxySettings { http: Some(format!("127.0.0.1:{}", proxy.port)), https: None, no_proxy: String::new() }));
    let read = current.clone();
    let public: IpAddr = "93.184.215.14".parse().unwrap();
    let names = Arc::new(Mutex::new(HashMap::from([("flip.test".to_owned(), vec![public])])));
    let mut settings = config(&[]);
    settings.proxies = Proxies::Read(Arc::new(move || read.lock().unwrap().clone()));
    settings.lookup = Arc::new(Switchable(names.clone()));
    let mut t = linked(settings, json!({})).await;

    for i in 0..2 {
        let outcome = t.http(&format!("a{i}"), json!({"app": APP, "url": "http://flip.test/x"})).await;
        assert_eq!((code(&outcome).as_str(), outcome["text"].as_str()), ("ok", Some("proxied http://flip.test/x")), "{outcome}");
    }
    assert_eq!(proxy.accepts(), 1, "one plan, one destination: one connection to the proxy");

    // The name now resolves to a private address: Hub's own lookup refuses
    // it before the proxy, though a pooled connection for it is idle.
    names.lock().unwrap().insert("flip.test".into(), vec!["10.0.0.1".parse().unwrap()]);
    let refused = t.http("refused", json!({"app": APP, "url": "http://flip.test/x"})).await;
    assert_eq!(code(&refused), "invalid_request", "{refused}");
    assert_eq!(proxy.count_seen("http://flip.test/"), 2);
    names.lock().unwrap().insert("flip.test".into(), vec![public]);

    // Another plan (the same proxy, another exception list): a new
    // connection, and the previous plan's idle one is closed.
    current.lock().unwrap().no_proxy = "skip.test".into();
    let changed = t.http("b", json!({"app": APP, "url": "http://flip.test/x"})).await;
    assert_eq!(code(&changed), "ok", "{changed}");
    assert_eq!(proxy.accepts(), 2, "a plan change opened a new connection");
    assert!(proxy.closed("http://flip.test/", Duration::from_secs(2)).await.is_some(), "the previous plan's idle connection was closed");
    let again = t.http("c", json!({"app": APP, "url": "http://flip.test/x"})).await;
    assert_eq!(code(&again), "ok", "{again}");
    assert_eq!(proxy.accepts(), 2, "the new plan's connection is reused");
    t.device.kill().await;
}


#[tokio::test(flavor = "multi_thread")]
async fn a_decoded_body_is_read_to_its_end_and_its_connection_reused() {
    // The coded stream ends 100 ms before the chunked body does. Hub reads
    // the body to its end, so the connection is pooled and the next request
    // rides it.
    let (mut server, mut t) = plain().await;
    for (i, path) in ["chunked-deflate/30000", "chunked-gzip/30000", "chunked-deflate/30000"].into_iter().enumerate() {
        let outcome = t.http(&format!("c{i}"), json!({"app": APP, "url": format!("http://a.test/{path}")})).await;
        assert_eq!((code(&outcome).as_str(), outcome["sum"].as_u64()), ("ok", Some(fnv32(&pattern(30_000)) as u64)), "{outcome}");
        assert_eq!(server.conn_of("/chunked").await, 1, "{path} rode the first connection");
    }
    assert_eq!(server.accepts(), 1);
    // Bytes after the end of the coded stream: not a body Hub can decode.
    let trailing = t.http("trailing", json!({"app": APP, "url": "http://a.test/deflate-trailing/1000"})).await;
    assert_eq!(code(&trailing), "protocol", "{trailing}");
    t.device.kill().await;
}
