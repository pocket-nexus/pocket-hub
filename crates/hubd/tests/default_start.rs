//! A default start: `pocket-hubd` with no `--link-port` and beacons on binds
//! TCP 8628, says so in `hub.status`, and sends beacons that decode and name
//! that port and this Hub; the second test does the same on an ephemeral
//! link port, for a computer where 8628 is taken. Ignored by default: they
//! use this computer's real beacon port (and link port) for a few seconds.
//! Run them by hand, the first only when nothing else uses 8628:
//!
//! ```text
//! cargo test -p pocket-hubd --test default_start -- --ignored --nocapture
//! ```

mod common;

use std::process::Stdio;
use std::time::Duration;

use common::*;
use pocket_hub_api::types::*;
use pocket_hub_api::{Client, methods};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the real link port 8628 and beacon port 8627"]
async fn a_default_start_binds_8628_and_sends_beacons() {
    started_with(None).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the real beacon port 8627"]
async fn beacons_name_the_link_port_in_use() {
    started_with(Some(0)).await;
}

async fn started_with(port: Option<u16>) {
    let link_port = pocket_link::consts::PORT;
    // Only a connection probes 8628: a bind, even a brief one, could take a
    // connection meant for the program that holds it.
    if port.is_none() && std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], link_port).into(), Duration::from_millis(300)).is_ok() {
        eprintln!("skipped: TCP {link_port} is in use on this computer");
        return;
    }
    let beacons = match std::net::UdpSocket::bind(("0.0.0.0", pocket_link::consts::BEACON_PORT)) {
        Ok(socket) => socket,
        Err(error) => {
            eprintln!("skipped: UDP {} cannot be bound here: {error}", pocket_link::consts::BEACON_PORT);
            return;
        }
    };
    beacons.set_read_timeout(Some(Duration::from_secs(4))).unwrap();

    let home = temp_home();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_pocket-hubd"));
    command.env("POCKET_HUB_NAME", HUB_NAME);
    command.arg("--home").arg(home.path()).arg("--bun").arg(bun()).arg("--store-url").arg(NO_STORE).arg("--shell-api").arg(NO_RELEASES);
    if let Some(port) = port {
        command.arg("--link-port").arg(port.to_string());
    }
    let mut core = command
        .env_remove("POCKET_HUB_LINK_PORT")
        .env_remove("POCKET_HUB_NO_BEACONS")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let socket = home.path().join("hubd.sock");
    let mut client = None;
    for _ in 0..200 {
        if let Ok(connected) = Client::connect(&socket, info(ClientKind::Cli, "default-start")).await {
            client = Some(connected);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let client = client.expect("the core answers");
    let status: HubStatus = client.call_as(methods::STATUS, &json!({})).await.unwrap();
    let in_use = status.carriers[0].port.unwrap();
    if port.is_none() {
        assert_eq!(status.carriers, vec![Carrier { kind: "tcp".into(), port: Some(link_port) }]);
    }
    eprintln!("hub.status carriers: {:?}", status.carriers);

    // A beacon a second, to the broadcast addresses this listener hears.
    // Another Hub's beacons may be on the network too: this core's id is the one looked for.
    let hub = status.id.clone();
    let received = tokio::task::spawn_blocking(move || {
        let mut buffer = [0u8; 256];
        for _ in 0..20 {
            let Ok((n, from)) = beacons.recv_from(&mut buffer) else { return None };
            if let Some(beacon) = pocket_link::beacon::decode(&buffer[..n])
                && beacon.companion == hub
            {
                return Some((beacon, from));
            }
        }
        None
    })
    .await
    .unwrap();
    let (beacon, from) = received.expect("a beacon arrives within 4 s");
    eprintln!("beacon from {from}: port {}, companion {}, name {:?}", beacon.port, beacon.companion, beacon.name);
    assert_eq!((beacon.port, beacon.companion.as_str()), (in_use, status.id.as_str()));

    client.call(methods::SHUTDOWN, &json!({})).await.unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(10), core.wait()).await.expect("the core exits").unwrap();
    assert!(exit.success());
    if port.is_none() {
        assert!(std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], link_port).into(), Duration::from_millis(300)).is_err(), "nothing answers on 8628 any more");
    }
}
