//! The enclosure (SERVICE-PROTOCOL.md §2), tried the way hostile code would:
//! the probe service (tests/fixtures/probe-service) installed through the
//! host in a temp home reaches past the SDK with `node:fs`, `node:net`,
//! `bun:ffi` and child processes, and this test asserts each attempt: what
//! must be refused, what must work, and the leaks the contract accepts (so
//! a change in Bun or macOS that widens or closes one is noticed). It runs
//! under a home whose path has a space and under one reached through a
//! symbolic link.

#![cfg(target_os = "macos")]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use pocket_hub_api::methods;
use pocket_link::frame::{encode_json, parse_json, Decoded, JsonFrame};
use pocket_link::{FrameDecoder, FrameType};
use serde_json::{json, Value};
use service_host::{HostOptions, ServiceHost};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Counts the connections a listener accepts.
fn count_unix(path: &Path) -> Arc<AtomicUsize> {
    let listener = tokio::net::UnixListener::bind(path).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    count
}

/// The release folder of `id` under `home`.
fn release_folder(home: &Path, id: &str) -> PathBuf {
    std::fs::read_dir(home.join("services").join(id)).unwrap().map(|entry| entry.unwrap().path()).find(|path| path.is_dir()).unwrap()
}

fn offer(app: &str) -> pocket_hub_core::ChannelOffer {
    pocket_hub_core::ChannelOffer {
        device: "0123456789abcdef".into(),
        device_name: "Test 3DS".into(),
        model: "New 3DS XL".into(),
        target: "3ds".into(),
        app: app.into(),
        hash: "fedcba9876543210".into(),
        generation: 7,
        payload_limit: 16_384,
    }
}

/// The probe's lines: name → (allowed, detail).
fn probe_lines(logs: &Value) -> HashMap<String, (bool, String)> {
    logs["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|line| line["text"].as_str()?.strip_prefix("probe ").map(str::to_owned))
        .filter_map(|line| {
            let mut parts = line.splitn(3, ' ');
            let name = parts.next()?.to_owned();
            let allowed = parts.next()? == "allowed";
            Some((name, (allowed, parts.next().unwrap_or("").to_owned())))
        })
        .collect()
}

/// Installs the probe in `home`, runs it through a device's channel, and
/// asserts every line. Returns the lines.
async fn escape(home: &Path, bun: PathBuf) -> HashMap<String, (bool, String)> {
    std::fs::create_dir_all(home).unwrap();
    let canonical = std::fs::canonicalize(home).unwrap();
    let host = ServiceHost::start(HostOptions { home: home.to_owned(), socket: home.join("s.sock"), bun: Some(bun), idle_override: None }).await.unwrap();
    let progress = |_: Value| {};

    // Another service, with a store of its own, beside the probe.
    let other = sdk_package(home, "store-service");
    host.call(methods::SERVICES_INSTALL, json!({"path": other.display().to_string()}), &progress).await.unwrap();
    let other_store = canonical.join("service-store/dev.test.host@dev");
    service_host::store::Store::open(&other_store).unwrap().set(b"session", b"theirs", 1 << 20).unwrap();
    // A file of the owner's, the control socket, another socket, a TCP port,
    // and a process that is not the probe's ancestor.
    std::fs::write(home.join("secret.txt"), "the owner's").unwrap();
    let control = count_unix(&home.join("hubd.sock"));
    let outside = tempfile::Builder::new().prefix("phx").tempdir_in("/tmp").unwrap();
    let other_socket = count_unix(&outside.path().join("o.sock"));
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_port = tcp.local_addr().unwrap().port();
    let tcp_count = Arc::new(AtomicUsize::new(0));
    let seen = tcp_count.clone();
    tokio::spawn(async move {
        while tcp.accept().await.is_ok() {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    let mut stranger = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();

    let probe = sdk_package(home, "probe-service");
    let plan = json!({
        "home": canonical,
        "secret": canonical.join("secret.txt"),
        "outside": "/private/etc/hosts",
        "ownStore": canonical.join("service-store/dev.test.probe@dev/store.sqlite"),
        "otherPackage": std::fs::canonicalize(release_folder(home, "dev.test.host")).unwrap(),
        "otherStore": other_store.join("store.sqlite"),
        "controlSocket": canonical.join("hubd.sock"),
        "otherSocket": std::fs::canonicalize(outside.path()).unwrap().join("o.sock"),
        "tcpPort": tcp_port,
        "stranger": stranger.id(),
        "ancestor": std::process::id(),
    });
    std::fs::write(probe.join("probe.json"), plan.to_string()).unwrap();
    host.call(methods::SERVICES_INSTALL, json!({"path": probe.display().to_string()}), &progress).await.unwrap();

    // A device's channel: the probe runs at its start, then answers a
    // request from its store.
    let mut channel = host.connector().open(offer("dev.test.probe")).await.expect("the probe takes a channel");
    let ready = encode_json(FrameType::Handshake, 0, &json!({"t": "channel.ready", "channel": 7, "messageLimit": 8188}), JsonFrame::default(), 1024).unwrap();
    channel.writer.write_all(&ready).await.unwrap();
    let request = encode_json(FrameType::Request, 1, &json!({"t": "stored"}), JsonFrame::default(), 16_384).unwrap();
    channel.writer.write_all(&request).await.unwrap();
    let mut decoder = FrameDecoder::local(16_384);
    let mut buffer = vec![0u8; 16_384];
    let reply = 'reply: loop {
        let n = tokio::time::timeout(Duration::from_secs(10), channel.reader.read(&mut buffer)).await.expect("a reply in time").unwrap();
        assert!(n > 0, "the channel closed before the reply");
        decoder.push(&buffer[..n]);
        while let Some(Decoded::Frame(frame)) = decoder.next().unwrap() {
            if frame.ty == FrameType::Reply && frame.id == 1 {
                break 'reply Value::Object(parse_json(&frame).object.unwrap());
            }
        }
    };
    assert_eq!(reply, json!({"value": "kept"}), "a request answered from host.store, through a channel");

    let logs = host.call(methods::SERVICES_LOGS, json!({"id": "dev.test.probe", "tail": 200}), &progress).await.unwrap();
    let lines = probe_lines(&logs);
    let texts: Vec<&str> = logs["lines"].as_array().unwrap().iter().filter_map(|line| line["text"].as_str()).collect();
    assert!(texts.contains(&"probe done"), "{texts:#?}");
    for (name, (allowed, detail)) in {
        let mut sorted: Vec<_> = lines.iter().collect();
        sorted.sort();
        sorted
    } {
        eprintln!("{name:24} {} {detail}", if *allowed { "allowed" } else { "refused" });
    }

    let expect = |name: &str, allowed: bool| {
        let (seen, detail) = lines.get(name).unwrap_or_else(|| panic!("no line for {name}: {texts:#?}"));
        assert_eq!(*seen, allowed, "{name}: {detail}");
    };
    // Refused.
    for name in [
        "read-home-file", "list-home", "stat-outside", "stat-home-file", "read-own-store-file", "read-other-package", "read-other-store-file",
        "write-own-package", "write-home", "write-tmp", "connect-control-socket", "connect-other-socket", "connect-tcp", "dns-lookup", "spawn",
        "exec-file", "fork", "list-processes", "args-stranger", "args-launchd",
        // The page accepts the command lines of the host and its ancestors
        // as a leak; measured, a process in a process group of its own (as
        // Hub starts every service) reads none, its ancestors' included. A
        // change that opens it fails here.
        "args-ancestor",
    ] {
        expect(name, false);
    }
    // Works.
    for name in ["own-package", "store", "intl", "crypto", "timer"] {
        expect(name, true);
    }
    assert_eq!(lines["intl"].1, "08", "Asia/Shanghai at the epoch");
    // The accepted leaks: native calls run (and meet the profile), the
    // machine's name, the metadata of a folder above the package.
    for name in ["ffi-libsystem", "hostname", "stat-ancestor"] {
        expect(name, true);
    }
    // `sandbox-exec` execs Bun: the process Hub signals (and its group) is
    // the service's own.
    let pid = host.service("dev.test.probe").unwrap().pid.expect("running");
    assert_eq!(lines["ffi-libsystem"].1, format!("pid {pid}"));

    // Nothing reached a listener, nothing was written.
    assert_eq!((control.load(Ordering::SeqCst), other_socket.load(Ordering::SeqCst), tcp_count.load(Ordering::SeqCst)), (0, 0, 0));
    assert!(!home.join("written.txt").exists() && !release_folder(home, "dev.test.probe").join("written.txt").exists() && !Path::new("/private/tmp/ph-probe-written.txt").exists());

    drop(channel);
    let _ = stranger.kill();
    let _ = stranger.wait();
    host.shutdown().await;
    lines
}

#[tokio::test(flavor = "multi_thread")]
async fn the_probe_under_a_home_with_a_space() {
    let Some(bun) = system_bun() else { return };
    let temp = temp_home();
    escape(&temp.path().join("Pocket Hub home"), bun).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_probe_under_a_home_reached_through_a_link() {
    let Some(bun) = system_bun() else { return };
    let temp = temp_home();
    let real = temp.path().join("real home");
    std::fs::create_dir_all(&real).unwrap();
    let link = temp.path().join("h");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    escape(&link, bun).await;
}

/// A launcher that cannot start the process inside the enclosure starts
/// nothing else: here `sandbox-exec` cannot exec the Bun it is given (a copy
/// without its execute bit), says so on standard error and exits. The
/// owner's start is the error `service` with `reason: "enclosure"`, the
/// service is listed `failed` with the sentence, and a device's open is
/// refused. (A path a profile cannot name, the other way an enclosure
/// fails, cannot be made on APFS, which refuses a name that is not UTF-8.)
#[tokio::test(flavor = "multi_thread")]
async fn no_enclosure_no_start() {
    use std::os::unix::fs::PermissionsExt;
    let Some(bun) = system_bun() else { return };
    let temp = temp_home();
    let home = temp.path().join("home");
    let unrunnable = temp.path().join("bun");
    std::fs::copy(&bun, &unrunnable).unwrap();
    std::fs::set_permissions(&unrunnable, std::fs::Permissions::from_mode(0o644)).unwrap();
    let host = ServiceHost::start(HostOptions { home: home.clone(), socket: temp.path().join("s.sock"), bun: Some(unrunnable), idle_override: None }).await.unwrap();
    let progress = |_: Value| {};
    let source = sdk_package(temp.path(), "store-service");
    host.call(methods::SERVICES_INSTALL, json!({"path": source.display().to_string()}), &progress).await.unwrap();
    let error = host.call(methods::SERVICES_START, json!({"id": "dev.test.host"}), &progress).await.unwrap_err();
    let data = error.data.as_ref().unwrap();
    assert_eq!((data.reason.as_deref(), data.id.as_deref()), (Some("enclosure"), Some("dev.test.host")));
    assert!(error.message.contains("its enclosure could not be set up: sandbox-exec:"), "{}", error.message);
    let listed = host.service("dev.test.host").unwrap();
    assert_eq!(listed.state, pocket_hub_api::types::ServiceState::Failed);
    assert!(listed.error.as_deref().is_some_and(|error| error.starts_with("its enclosure could not be set up")), "{listed:?}");
    assert!(matches!(host.connector().open(offer("dev.test.host")).await, Err(pocket_hub_core::ChannelOpenError::Busy(_))));
    host.shutdown().await;
}

/// A package whose dependency `bun install` put in `node_modules` loads it
/// inside the enclosure: the fixture's entry prints the dependency's word
/// (and, having no default export, then fails its start, which is its own).
#[tokio::test(flavor = "multi_thread")]
async fn installed_dependencies_load_inside_the_enclosure() {
    let Some(bun) = system_bun() else { return };
    let home = temp_home();
    let host = ServiceHost::start(HostOptions { home: home.path().to_owned(), socket: home.path().join("s.sock"), bun: Some(bun), idle_override: None }).await.unwrap();
    let progress = |_: Value| {};
    let source = home.path().join("with-lock");
    std::fs::create_dir_all(source.join("tiny")).unwrap();
    for name in ["pocket.service.json", "package.json", "bun.lock", "service.mjs", "tiny/index.js", "tiny/package.json"] {
        std::fs::copy(fixtures().join("with-lock").join(name), source.join(name)).unwrap();
    }
    host.call(methods::SERVICES_INSTALL, json!({"path": source.display().to_string()}), &progress).await.unwrap();
    assert!(release_folder(home.path(), "com.example.locked").join("node_modules/tiny/index.js").exists());
    let error = host.call(methods::SERVICES_START, json!({"id": "com.example.locked"}), &progress).await.unwrap_err();
    assert_ne!(error.data.as_ref().and_then(|data| data.reason.as_deref()), Some("enclosure"), "{}", error.message);
    let logs = host.call(methods::SERVICES_LOGS, json!({"id": "com.example.locked"}), &progress).await.unwrap();
    let texts: Vec<&str> = logs["lines"].as_array().unwrap().iter().filter_map(|line| line["text"].as_str()).collect();
    assert!(texts.contains(&"tiny-ok"), "{texts:#?}");
    host.shutdown().await;
}

/// The time from a process's start to its `service.hello`, with the
/// enclosure and with Bun spawned by this test without it, and from the
/// host's start to its running state (`service.ready` and `host.ready`
/// answered). A measurement: run with `--ignored --nocapture`;
/// `PH_START_RUNS` sets the number of starts (40 by default).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement; run with --ignored --nocapture"]
async fn the_time_to_service_hello_with_and_without_the_enclosure() {
    let Some(bun) = system_bun() else { return };
    let runs: usize = std::env::var("PH_START_RUNS").ok().and_then(|runs| runs.parse().ok()).unwrap_or(40);
    let home = temp_home();
    let host = ServiceHost::start(HostOptions { home: home.path().to_owned(), socket: home.path().join("s.sock"), bun: Some(bun.clone()), idle_override: None }).await.unwrap();
    let progress = |_: Value| {};
    let package = sdk_package(home.path(), "store-service");
    host.call(methods::SERVICES_INSTALL, json!({"path": package.display().to_string()}), &progress).await.unwrap();
    let release = std::fs::canonicalize(release_folder(home.path(), "dev.test.host")).unwrap();
    let sdk = std::fs::canonicalize(home.path().join("sdk/service")).unwrap();

    // Through the host: start → running, then stop.
    let mut through_host = Vec::new();
    for _ in 0..runs {
        let started = Instant::now();
        host.call(methods::SERVICES_START, json!({"id": "dev.test.host"}), &progress).await.unwrap();
        through_host.push(started.elapsed().as_secs_f64() * 1000.0);
        host.call(methods::SERVICES_STOP, json!({"id": "dev.test.host"}), &progress).await.unwrap();
    }

    // Spawned here, to a socket of this test's: the time to the hello.
    let socket = std::fs::canonicalize(home.path()).unwrap().join("t.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let args: Vec<std::ffi::OsString> = vec!["--no-install".into(), "run".into(), sdk.join("src/boot.ts").into(), "./service.ts".into()];
    let mut timings: HashMap<&str, Vec<f64>> = HashMap::new();
    for _ in 0..runs {
        for enclosed in [true, false] {
            let mut command = if enclosed {
                let e = service_host::enclosure::enclose(&bun, &release, &sdk, &socket, &args).unwrap();
                let mut command = tokio::process::Command::new(&e.program);
                command.args(&e.args);
                command
            } else {
                let mut command = tokio::process::Command::new(&bun);
                command.args(&args);
                command
            };
            command.current_dir(&release).env_clear().env("POCKET_SERVICE_SOCKET", &socket).env("POCKET_SERVICE_TOKEN", "0".repeat(32)).env("POCKET_SERVICE_ID", "dev.test.host");
            command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
            let started = Instant::now();
            let mut child = command.spawn().unwrap();
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept()).await.unwrap().unwrap();
            let mut header = [0u8; 12];
            stream.read_exact(&mut header).await.unwrap();
            timings.entry(if enclosed { "enclosed" } else { "unenclosed" }).or_default().push(started.elapsed().as_secs_f64() * 1000.0);
            let _ = child.kill().await;
        }
    }
    let median = |samples: &mut Vec<f64>| {
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (samples[samples.len() / 2], samples[samples.len() * 9 / 10])
    };
    for (name, samples) in [("start → running, through the host", &mut through_host), ("spawn → hello, enclosed", timings.get_mut("enclosed").unwrap())] {
        let (m, p90) = median(samples);
        eprintln!("{name}: median {m:.1} ms, p90 {p90:.1} ms ({runs} runs)");
    }
    let (m, p90) = median(timings.get_mut("unenclosed").unwrap());
    eprintln!("spawn → hello, Bun spawned by the test without the enclosure: median {m:.1} ms, p90 {p90:.1} ms ({runs} runs)");
    host.shutdown().await;
}
