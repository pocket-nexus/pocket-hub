//! `net.http` through the real `pocket-hubd` binary: Hub lists the group,
//! the reference device's request reaches a server on loopback that
//! `--net-resolve` names, a private literal is refused, and the log line
//! carries no query value.

mod common;

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use common::*;
use pocket_hub_api::methods;
use pocket_hub_api::types::*;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn wait_for_socket(socket: &Path) {
    for _ in 0..400 {
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("pocket-hubd did not answer on {}", socket.display());
}

/// A server that answers every request with 14 bytes.
async fn server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buffer[..n]),
                    }
                }
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 14\r\nConnection: close\r\n\r\nhello, pocket!").await;
                let _ = socket.shutdown().await;
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread")]
async fn net_http_through_the_binary() {
    let home = temp_home();
    let work = temp_home();
    let socket = home.path().join("hubd.sock");
    let port = server().await;
    let mut core = tokio::process::Command::new(env!("CARGO_BIN_EXE_pocket-hubd"))
        .env("POCKET_HUB_NAME", HUB_NAME)
        .arg("--home")
        .arg(home.path())
        .args(["--link-port", "0", "--no-beacons", "--store-url", NO_STORE, "--shell-api", NO_RELEASES])
        .arg("--bun")
        .arg(bun())
        .arg("--net-resolve")
        .arg(format!("api.test=127.0.0.1:{port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("pocket-hubd runs");
    wait_for_socket(&socket).await;
    let client = client(&socket, "net test").await;
    let status: HubStatus = client.call_as(methods::STATUS, &json!({})).await.unwrap();
    let link_port = status.carriers[0].port.unwrap();

    let card = card(work.path(), "sd");
    let program = program(work.path(), "shell.3dsx", 4096);
    let report: InstallReport = client
        .call_as(methods::SETUP_CARD, &SetupCardParams { kind: "3ds".into(), path: card.display().to_string(), program: Some(ProgramChoice::Path { path: program.display().to_string() }) })
        .await
        .unwrap();
    let mut device = RefDevice::spawn(&link_dir(&card), &status.id, link_port).await;
    let _ = device.expect("linked").await;

    let linked: Value = client.call(methods::DEVICES_GET, &DeviceParams { device: report.device.clone() }).await.unwrap();
    let features: Vec<&str> = linked["link"]["features"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(features.contains(&"net.http"), "{linked}");

    device.send(json!({"cmd": "http", "tag": "get", "app": "dev.demo", "url": "http://api.test/small?token=secret-value"})).await;
    let got = device.wait(|v| v["ev"] == "http" && v["tag"] == "get").await;
    assert_eq!((got["ok"].as_bool(), got["text"].as_str(), got["value"]["bytes"].as_u64()), (Some(true), Some("hello, pocket!"), Some(14)), "{got}");

    device.send(json!({"cmd": "http", "tag": "private", "app": "dev.demo", "url": format!("http://127.0.0.1:{port}/")})).await;
    let refused = device.wait(|v| v["ev"] == "http" && v["tag"] == "private").await;
    assert_eq!(refused["code"], "invalid_request", "{refused}");

    // One line per request, the query's values left out.
    let log_file = home.path().join("logs").join("hubd.log");
    let mut log = String::new();
    for _ in 0..100 {
        log = std::fs::read_to_string(&log_file).unwrap_or_default();
        if log.contains("-> invalid_request") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(log.contains("net.http: dev.demo GET http://api.test/small?token=* -> 200, 14 bytes in "), "{log}");
    assert!(log.contains(&format!("net.http: dev.demo GET http://127.0.0.1:{port}/ -> invalid_request after ")), "{log}");
    assert!(!log.contains("secret-value"));

    device.kill().await;
    let _ = client.call(methods::SHUTDOWN, &json!({})).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), core.wait()).await;
}
