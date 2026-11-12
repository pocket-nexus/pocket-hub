//! Finding ftpd on the network, with the scan range injected: loopback only.

mod support;

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use pocket_hub_setup::{DiscoverOptions, discover};
use support::*;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

fn options(port: u16) -> DiscoverOptions {
    DiscoverOptions { port, targets: Some(vec![Ipv4Addr::LOCALHOST]), ..DiscoverOptions::default() }
}

#[tokio::test]
async fn finds_the_fake_ftpd_on_loopback() {
    let fake = FakeFtp::start(&temp("discover")).await;
    let started = Instant::now();
    let found = discover(&options(fake.address.port())).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].address, Ipv4Addr::LOCALHOST);
    assert_eq!(found[0].port, fake.address.port());
    assert_eq!(found[0].banner, "Hello!");
    assert!(found[0].ftpd);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(serde_json::to_string(&found[0]).unwrap(), format!(r#"{{"address":"127.0.0.1","port":{},"banner":"Hello!","ftpd":true}}"#, fake.address.port()));
}

#[tokio::test]
async fn a_closed_port_or_another_service_is_no_candidate() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert!(discover(&options(closed)).await.is_empty());

    let other = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = other.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = other.accept().await {
            let _ = stream.write_all(b"SSH-2.0-OpenSSH\r\n").await;
        }
    });
    assert!(discover(&options(port)).await.is_empty());

    // A host that accepts and never speaks is dropped at the deadline.
    let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = silent.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = silent.accept().await {
            held.push(stream);
        }
    });
    let started = Instant::now();
    let found = discover(&DiscoverOptions { deadline: Duration::from_millis(400), ..options(port) }).await;
    assert!(found.is_empty());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn the_local_scan_is_bounded() {
    // Whatever the machine's interfaces, no subnet contributes more than a /24.
    let targets = pocket_hub_setup::discover::scan_targets();
    let mut per_24 = std::collections::BTreeMap::<[u8; 3], usize>::new();
    for target in &targets {
        let o = target.octets();
        *per_24.entry([o[0], o[1], o[2]]).or_default() += 1;
    }
    assert!(per_24.values().all(|count| *count <= 254));
    assert!(targets.iter().all(|target| !target.is_loopback()));
}
