//! Shared by the integration tests: the system Bun, temporary homes, fixture
//! packages, and frames read and written by hand.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use pocket_hub_daemons::{ChannelOffer, Config, Supervisor, Timings};
use tokio::io::{AsyncRead, AsyncReadExt};

/// The system `bun`, or None with a note: the tests that run a daemon are
/// skipped on a computer without Bun.
pub fn system_bun() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let found = std::env::split_paths(&path).map(|dir| dir.join("bun")).find(|bun| bun.is_file());
    if found.is_none() {
        eprintln!("SKIPPED: no `bun` on PATH; this test runs a daemon on the system Bun");
    }
    found
}

/// A temporary home under /tmp: the socket path in it stays far below the
/// 104 bytes macOS allows.
pub fn temp_home() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("phd").tempdir_in("/tmp").expect("temp home")
}

pub fn fast_timings() -> Timings {
    Timings {
        open_budget: Duration::from_secs(10),
        start: Duration::from_secs(8),
        attach: Duration::from_millis(1000),
        first_frame: Duration::from_secs(2),
        stop_grace: Duration::from_millis(400),
        term_grace: Duration::from_millis(400),
        idle_override: Some(Duration::from_millis(300)),
        crash_window: Duration::from_secs(2),
        failure_backoff: Duration::from_secs(2),
        failure_backoff_max: Duration::from_secs(8),
        install: Duration::from_secs(60),
    }
}

pub fn config(home: &Path, bun: PathBuf) -> Config {
    let mut config = Config::new(home, home.join("d.sock"));
    config.bun_path = Some(bun);
    config.timings = fast_timings();
    config
}

/// A package folder with the fixture daemon and its `fixture.json`.
pub fn fixture_package(parent: &Path, id: &str, version: &str, fixture: serde_json::Value) -> PathBuf {
    let dir = parent.join(format!("src-{id}-{version}"));
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = serde_json::json!({ "daemon": 1, "id": id, "version": version, "entry": "daemon.mjs" });
    std::fs::write(dir.join("pocket.daemon.json"), manifest.to_string()).unwrap();
    std::fs::copy(fixtures().join("daemon.mjs"), dir.join("daemon.mjs")).unwrap();
    std::fs::write(dir.join("fixture.json"), fixture.to_string()).unwrap();
    dir
}

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A supervisor on a fresh home with the given fixture daemons installed.
pub async fn supervisor_with(home: &Path, bun: PathBuf, daemons: &[(&str, serde_json::Value)], tune: impl FnOnce(&mut Config)) -> Supervisor {
    let mut config = config(home, bun);
    tune(&mut config);
    let supervisor = Supervisor::new(config).await.expect("supervisor");
    for (id, fixture) in daemons {
        let source = fixture_package(home, id, "1.0.0", fixture.clone());
        supervisor.install(&source).await.expect("install");
    }
    supervisor
}

pub fn offer(app: &str) -> ChannelOffer {
    ChannelOffer {
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

pub fn frame(ty: u8, flags: u8, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![ty, flags, 0, 0];
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// One whole frame as raw bytes, header included.
pub async fn read_frame(from: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Vec<u8>> {
    let mut header = [0u8; 12];
    from.read_exact(&mut header).await?;
    let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
    let mut frame = header.to_vec();
    frame.resize(12 + length, 0);
    from.read_exact(&mut frame[12..]).await?;
    Ok(frame)
}

/// Polls `check` every 20 ms until it holds or `within` passes.
pub async fn eventually(within: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    check()
}

pub fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
