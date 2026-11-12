//! A device that stops sending and reading while it installs (a 3DS
//! creating the folders of a new applet's slot) through the real `pocket-hubd`:
//! `store.install` waits for it within the longer limit of a state-changing
//! request instead of closing the link with `silence`.
//!
//! The reference device pings every 3 s and declares silence after 10 s, so
//! the core runs with a normal limit of 4.5 s, a longer one of 9 s and its
//! PING every 500 ms, and the device stops for 6 s.

mod common;

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use common::*;
use pocket_hub_api::types::*;
use pocket_hub_api::methods;
use pocket_hub_applet_store::test_support::{FakeStore, build_package, icon, public, test_key};
use serde_json::json;

async fn wait_for_socket(socket: &Path) {
    for _ in 0..400 {
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("pocket-hubd did not answer on {}", socket.display());
}

#[tokio::test(flavor = "multi_thread")]
async fn store_install_survives_a_stalled_device() {
    let home = temp_home();
    let work = temp_home();
    let applets = FakeStore::start().await;
    let maps = applets.publish(&test_key(1), build_package("dev.pocket-nexus.maps", "1.0.0", "Maps", &[("3ds-dev", "maps")], 1024 * 1024), icon(1), "pocket-nexus", "Maps.");
    let socket = home.path().join("hubd.sock");
    let mut core = tokio::process::Command::new(env!("CARGO_BIN_EXE_pocket-hubd"))
        .env("POCKET_HUB_NAME", HUB_NAME)
        .arg("--home")
        .arg(home.path())
        .args(["--link-port", "0", "--no-beacons", "--shell-api", NO_RELEASES])
        .args(["--link-silence-ms", "4500", "--link-state-silence-ms", "9000", "--link-state-ping-ms", "500"])
        .arg("--bun")
        .arg(bun())
        .arg("--store-url")
        .arg(&applets.url)
        .arg("--store-key")
        .arg(public(&test_key(1)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("pocket-hubd runs");
    wait_for_socket(&socket).await;
    let client = client(&socket, "Pocket Hub").await;
    client.call(methods::SUBSCRIBE, &json!({})).await.unwrap();
    let status: HubStatus = client.call_as(methods::STATUS, &json!({})).await.unwrap();
    let card = card(work.path(), "sd");
    let shell = program(work.path(), "pocket-shell.3dsx", 4096);
    let report: InstallReport = client
        .call_as(methods::SETUP_CARD, &SetupCardParams { kind: "3ds".into(), path: card.display().to_string(), program: Some(ProgramChoice::Path { path: shell.display().to_string() }) })
        .await
        .unwrap();
    let mut device = RefDevice::spawn(&link_dir(&card), &status.id, status.carriers[0].port.unwrap()).await;
    event(&client, |e| topic(e, "device.changed") && !e["device"]["link"].is_null()).await;

    // The device stops as the install's request goes out, for longer than
    // the normal limit, and resumes.
    signal(device.pid(), SIGSTOP);
    let install = client.start(methods::STORE_INSTALL, &StoreInstallParams { device: report.device.clone(), app: maps.id.clone() }).await.unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    let during: Device = client.call_as(methods::DEVICES_GET, &DeviceParams { device: report.device.clone() }).await.unwrap();
    assert!(during.link.is_some(), "linked through the stall");
    signal(device.pid(), SIGCONT);
    let installed: PushResult = serde_json::from_value(install.finish().await.unwrap()).unwrap();
    assert_eq!(installed.app.output, "maps");
    assert_eq!(device.expect("pushed").await["bytes"].as_u64(), Some(maps.bytes));
    // No unlink was published meanwhile.
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(50), client.next_event()).await {
        assert!(!topic(&event, "device.unlinked"), "{event}");
    }

    client.call(methods::SHUTDOWN, &json!({})).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), core.wait()).await.expect("the core exits").unwrap();
    device.kill().await;
}
