//! What the core's tests share: temporary homes, a core in this process or
//! as the real binary, clients, the reference device under Bun (hub-core's
//! harness), and the packages and cards the tests feed it.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use pocket_hub_api::types::{ClientInfo, ClientKind};
use pocket_hub_api::{Client, ClientError, ErrorKind, RpcError, Transcript};
use pocket_hubd::{Core, Options};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

pub const WAIT: Duration = Duration::from_secs(10);

/// A home under /tmp: short enough for its socket path.
pub fn temp_home() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("phd").tempdir_in("/tmp").expect("a temporary home")
}

pub fn bun() -> PathBuf {
    let output = std::process::Command::new("which").arg("bun").output().expect("which");
    let path = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    assert!(!path.is_empty(), "the tests run the reference device and the services on the system's bun");
    PathBuf::from(path)
}

/// A store URL nothing answers on (the discard port): a test that does not
/// set up a fake store never reaches the network.
pub const NO_STORE: &str = "http://127.0.0.1:9";

/// The name every test core takes on its fresh home, never the computer's:
/// a test's output, and the examples file it writes, name no real machine.
pub const HUB_NAME: &str = "Studio Mac";

/// Pocket Shell's releases at an address nothing answers on: a core looks
/// its release up when a device links, and a test that does not serve
/// releases must not reach GitHub.
pub const NO_RELEASES: &str = "http://127.0.0.1:9";

/// Options for a test core: an ephemeral link port, no beacons, the system
/// Bun, no applet store, no release source.
pub fn test_options(home: &Path) -> Options {
    let mut options = Options::new(home);
    options.store_url = NO_STORE.into();
    options.name = Some(HUB_NAME.into());
    options.shell_source = Some(pocket_hub_setup::ShellSource::GitHub { api: NO_RELEASES.into(), repo: pocket_hub_setup::release::DEFAULT_REPO.into(), tag: None });
    options.link_port = Some(0);
    options.beacons = false;
    options.bun = Some(bun());
    options.mutex_wait = Duration::from_millis(400);
    options.service_idle = Some(Duration::from_secs(1));
    options
}

/// A core running in this process.
pub struct InProcess {
    pub core: Arc<Core>,
    pub socket: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl InProcess {
    pub async fn start(options: Options) -> InProcess {
        let running = pocket_hubd::start(options).await.expect("the core starts");
        let core = running.core().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(running.run(async move {
            let _ = stopped.await;
        }));
        InProcess { socket: core.socket.clone(), core, stop: Some(stop), task }
    }

    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}

pub fn info(kind: ClientKind, name: &str) -> ClientInfo {
    ClientInfo { kind, name: name.into(), version: "0.0.0-test".into() }
}

pub async fn client(socket: &Path, name: &str) -> Client {
    Client::connect(socket, info(ClientKind::Gui, name)).await.expect("the client connects")
}

pub async fn traced(socket: &Path, name: &str, transcript: &Transcript) -> Client {
    Client::connect_traced(socket, info(ClientKind::Gui, name), Some(transcript.clone())).await.expect("the client connects")
}

pub fn rpc(error: ClientError) -> RpcError {
    match error {
        ClientError::Rpc(error) => error,
        other => panic!("expected an error answer, got {other:?}"),
    }
}

pub fn kind_of(error: ClientError) -> ErrorKind {
    rpc(error).kind().expect("an operation error")
}

/// The next event satisfying `matches`, within `WAIT`.
pub async fn event(client: &Client, mut matches: impl FnMut(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + WAIT;
    let mut skipped = VecDeque::new();
    loop {
        let Ok(event) = tokio::time::timeout_at(deadline, client.next_event()).await else {
            panic!("no matching event in time; the last ones: {skipped:?}");
        };
        let event = event.expect("the connection is open");
        if matches(&event) {
            return event;
        }
        if skipped.len() == 8 {
            skipped.pop_front();
        }
        let mut short = event.to_string();
        short.truncate(300);
        skipped.push_back(short);
    }
}

pub fn topic(event: &Value, topic: &str) -> bool {
    event["topic"] == topic
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// The byte pattern of the harness: byte i of a stream.
pub fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|i| ((i * 7 + 13) & 0xff) as u8).collect()
}

pub fn fnv32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// A `.pocket` of `len` bytes with a valid footer; `first` is its first byte
/// (0xee makes the reference device reject it).
pub fn package(dir: &Path, name: &str, len: usize, first: u8) -> PathBuf {
    let mut bytes = pattern(len - 8);
    bytes[0] = first;
    let hash = pocket_hub_core::ops::fnv1a64(&bytes);
    bytes.extend_from_slice(&hash.to_le_bytes());
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// A program with the 3DSX magic.
pub fn program(dir: &Path, name: &str, size: usize) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, pocket_hub_setup::test_support::fake_3dsx(size, 7)).unwrap();
    path
}

/// An empty "SD card": a folder with the `3ds` folder a card has.
pub fn card(dir: &Path, name: &str) -> PathBuf {
    let card = dir.join(name);
    std::fs::create_dir_all(card.join("3ds")).unwrap();
    card
}

/// The folder of a card the reference device reads its pairing from.
pub fn link_dir(card: &Path) -> PathBuf {
    card.join("pocketjs/link")
}

/// The link-channel example service package of the SDK.
pub fn link_channel_package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/service/examples/link-channel").canonicalize().unwrap()
}

pub fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 checks that the process exists; it sends nothing.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Waits up to `WAIT` for the process to be gone: an ended process can be
/// seen a moment after the event that announced its end.
pub async fn gone(pid: u32) -> bool {
    let deadline = tokio::time::Instant::now() + WAIT;
    while alive(pid) {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

pub const SIGSTOP: i32 = libc::SIGSTOP;
pub const SIGCONT: i32 = libc::SIGCONT;

/// A signal to a process this test started.
pub fn signal(pid: u32, signal: i32) {
    // SAFETY: no memory is passed.
    unsafe {
        libc::kill(pid as i32, signal);
    }
}

// ---------------------------------------------------------------------------
// The reference device under Bun (crates/hub-core/tests/harness/device.ts)
// ---------------------------------------------------------------------------

pub struct RefDevice {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::UnboundedReceiver<Value>,
    seen: VecDeque<Value>,
    pub id: String,
}

impl RefDevice {
    /// Runs the reference device on the pairing in `dir` (a card's
    /// `pocketjs/link`), connecting to the hub `hub` on `port`.
    pub async fn spawn(dir: &Path, hub: &str, port: u16) -> RefDevice {
        RefDevice::spawn_with(dir, hub, port, json!({})).await
    }

    /// `spawn`, with members of the harness's config of the test's choosing:
    /// the device's name, model, target and features, and `autoConnect`.
    pub async fn spawn_with(dir: &Path, hub: &str, port: u16, extra: Value) -> RefDevice {
        let mut config = json!({"dir": dir, "hub": hub, "address": "127.0.0.1", "port": port});
        if let (Value::Object(config), Value::Object(extra)) = (&mut config, extra) {
            config.extend(extra);
        }
        let harness = concat!(env!("CARGO_MANIFEST_DIR"), "/../hub-core/tests/harness/device.ts");
        let mut child = Command::new(bun())
            .arg(harness)
            .arg(config.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("bun runs the reference device");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        let _ = tx.send(value);
                    }
                    Err(_) => eprintln!("device: {line}"),
                }
            }
        });
        let mut device = RefDevice { child, stdin, lines, seen: VecDeque::new(), id: String::new() };
        let ready = device.expect("ready").await;
        device.id = ready["device"].as_str().unwrap().to_owned();
        device
    }

    pub async fn send(&mut self, command: Value) {
        let mut line = command.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
        self.stdin.flush().await.unwrap();
    }

    pub async fn wait(&mut self, mut matches: impl FnMut(&Value) -> bool) -> Value {
        if let Some(index) = self.seen.iter().position(&mut matches) {
            return self.seen.remove(index).unwrap();
        }
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let value = tokio::time::timeout_at(deadline, self.lines.recv())
                .await
                .unwrap_or_else(|_| {
                    let recent: Vec<&Value> = self.seen.iter().filter(|v| v["t"] != "tick").rev().take(10).collect();
                    panic!("no matching device event in time; recent: {recent:?}")
                })
                .expect("the device runs");
            if matches(&value) {
                return value;
            }
            self.seen.push_back(value);
            // Ticks come ten times a second; only the last few matter.
            if self.seen.len() > 2000 {
                self.seen.pop_front();
            }
        }
    }

    pub async fn expect(&mut self, ev: &str) -> Value {
        let ev = ev.to_owned();
        self.wait(move |v| v["ev"] == ev.as_str()).await
    }

    pub async fn reply(&mut self, tag: &str) -> Value {
        let tag = tag.to_owned();
        self.wait(move |v| v["ev"] == "reply" && v["tag"] == tag.as_str()).await
    }

    pub fn pid(&self) -> u32 {
        self.child.id().expect("the device runs")
    }

    pub async fn kill(mut self) {
        let _ = self.child.kill().await;
    }
}
