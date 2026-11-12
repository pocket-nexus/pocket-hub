//! A real public HTTPS request with Hub's defaults: the system's resolver,
//! trust store and proxy settings. Needs the network, so it runs only when
//! asked: `cargo test -p pocket-hub-net --test public -- --ignored --nocapture`.

mod support;

use pocket_hub_net::NetConfig;
use serde_json::json;
use support::*;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the network"]
async fn a_public_https_url() {
    let mut t = linked(NetConfig::default(), json!({})).await;
    let outcome = t.http("public", json!({"app": "dev.pocket-nexus.demo", "url": "https://example.com/", "timeoutMs": 20000})).await;
    eprintln!("status {} url {} bytes {} in {} ms; headers {}", outcome["head"]["status"], outcome["head"]["url"], outcome["bytes"], outcome["ms"], outcome["head"]["headers"]);
    assert_eq!(code(&outcome), "ok", "{outcome}");
    assert_eq!(outcome["head"]["status"], 200);
    assert!(outcome["text"].as_str().unwrap().contains("Example Domain"), "{outcome}");
    t.device.kill().await;
}

/// Ten sequential GETs to one public HTTPS host, timed by the device: the
/// first pays for DNS, TCP and TLS; the later ones show whether Hub reused
/// the connection.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the network"]
async fn ten_sequential_gets_to_one_public_host() {
    let mut t = linked(NetConfig::default(), json!({})).await;
    let mut times = Vec::new();
    for i in 0..10 {
        let outcome = t.http(&format!("get{i}"), json!({"app": "dev.pocket-nexus.demo", "url": "https://example.com/", "timeoutMs": 20000})).await;
        assert_eq!(code(&outcome), "ok", "{outcome}");
        assert_eq!(outcome["head"]["status"], 200);
        times.push(outcome["ms"].as_u64().unwrap());
    }
    let later = &times[1..];
    let mut sorted = later.to_vec();
    sorted.sort_unstable();
    let mean = later.iter().sum::<u64>() as f64 / later.len() as f64;
    eprintln!("10 GETs of https://example.com/, ms each: {times:?}");
    eprintln!("first {} ms; later: mean {mean:.1} ms, median {} ms, min {} ms, max {} ms", times[0], sorted[sorted.len() / 2], sorted[0], sorted[sorted.len() - 1]);
    t.device.kill().await;
}
