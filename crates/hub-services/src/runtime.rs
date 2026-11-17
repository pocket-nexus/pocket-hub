//! The Bun every service runs on.
//!
//! Hub pins one Bun version and keeps it at `<home>/runtimes/bun-<version>/bun`.
//! When it is absent, Hub downloads the official release for the host from
//! GitHub, checks it against the release's `SHASUMS256.txt`, unpacks it,
//! checks that it runs and reports the pinned version, and only then renames
//! its folder into place: a crash at any step leaves no runtime folder, only
//! leftovers the next download removes. Callers that ask while a download
//! runs share it.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::child::InheritNothing;
use crate::util::random_hex;

/// The Bun release Hub runs services on (released 2026-10-22).
pub const BUN_VERSION: &str = "1.4.4";
const DOWNLOAD_BASE: &str = "https://github.com/oven-sh/bun/releases/download";
/// Bun's zip is about 25 MB and its binary about 60 MB; these bound a
/// response or an archive that is not what it claims.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SUMS_BYTES: usize = 256 * 1024;
const USER_AGENT: &str = "pocket-hub";

/// Bytes of the runtime's archive received so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub done: u64,
    /// The archive's size, when the server sent it.
    pub total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("Bun has no build for this computer ({0})")]
    Unsupported(String),
    #[error("the configured Bun {0} is not a file")]
    Override(String),
    #[error("downloading Bun: {0}")]
    Network(String),
    #[error("the Bun archive does not match its published SHA-256 (expected {expected}, got {actual})")]
    Checksum { expected: String, actual: String },
    #[error("unpacking Bun: {0}")]
    Unpack(String),
    #[error("the downloaded Bun does not run as {BUN_VERSION}: {0}")]
    Verify(String),
    #[error("installing Bun: {0}")]
    Io(String),
}

impl From<io::Error> for RuntimeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[derive(Debug, Clone)]
enum Download {
    Running(Progress),
    Done(Result<PathBuf, RuntimeError>),
}

struct Inner {
    home: PathBuf,
    bun_path: Option<PathBuf>,
    base_url: String,
    version: String,
    inflight: Mutex<Option<watch::Receiver<Download>>>,
}

/// Finds Hub's Bun, downloading it once when it is absent. Cheap to clone.
#[derive(Clone)]
pub struct BunRuntime {
    inner: Arc<Inner>,
}

impl BunRuntime {
    /// `home` is Hub's home folder; `bun_path`, when set, is an existing Bun
    /// used instead (nothing is downloaded then).
    pub fn new(home: impl Into<PathBuf>, bun_path: Option<PathBuf>) -> Self {
        Self::with_source(home, bun_path, DOWNLOAD_BASE, BUN_VERSION)
    }

    /// As `new`, from another release server laid out as GitHub's
    /// (`<base>/bun-v<version>/<asset>`) and for another version: a mirror,
    /// and the tests.
    pub fn with_source(home: impl Into<PathBuf>, bun_path: Option<PathBuf>, base_url: &str, version: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                home: home.into(),
                bun_path,
                base_url: base_url.trim_end_matches('/').to_owned(),
                version: version.to_owned(),
                inflight: Mutex::new(None),
            }),
        }
    }

    pub fn version(&self) -> &str {
        &self.inner.version
    }

    /// Where the downloaded runtime lives.
    pub fn install_path(&self) -> PathBuf {
        self.inner.home.join("runtimes").join(format!("bun-{}", self.inner.version)).join("bun")
    }

    /// The Bun to run now, without downloading: the configured one, or the
    /// installed one when it is there.
    pub fn available(&self) -> Option<PathBuf> {
        let path = self.inner.bun_path.clone().unwrap_or_else(|| self.install_path());
        fs::metadata(&path).is_ok_and(|meta| meta.is_file()).then_some(path)
    }

    /// The release asset for this computer: `bun-<os>-<arch>.zip`. On x86-64
    /// without AVX2 (old CPUs, and Rosetta) the `baseline` build, which needs
    /// none.
    pub fn asset_name() -> Result<String, RuntimeError> {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            "linux" => "linux",
            other => return Err(RuntimeError::Unsupported(other.into())),
        };
        let arch = match std::env::consts::ARCH {
            "aarch64" => "aarch64".to_owned(),
            "x86_64" => {
                #[cfg(target_arch = "x86_64")]
                let baseline = !std::arch::is_x86_feature_detected!("avx2");
                #[cfg(not(target_arch = "x86_64"))]
                let baseline = false;
                if baseline { "x64-baseline".to_owned() } else { "x64".to_owned() }
            }
            other => return Err(RuntimeError::Unsupported(other.into())),
        };
        Ok(format!("bun-{os}-{arch}.zip"))
    }

    pub async fn ensure(&self) -> Result<PathBuf, RuntimeError> {
        self.ensure_with(|_| {}).await
    }

    /// The Bun to run, downloading it first when needed. `progress` sees the
    /// download's bytes while this call waits for it. The download runs in
    /// its own task: a caller that stops waiting does not stop it, and a
    /// caller that comes meanwhile joins it.
    pub async fn ensure_with(&self, mut progress: impl FnMut(Progress)) -> Result<PathBuf, RuntimeError> {
        if let Some(path) = self.available() {
            return Ok(path);
        }
        if let Some(path) = &self.inner.bun_path {
            return Err(RuntimeError::Override(path.display().to_string()));
        }
        let mut rx = {
            let mut inflight = self.inner.inflight.lock().expect("runtime lock");
            match inflight.as_ref() {
                Some(rx) if matches!(*rx.borrow(), Download::Running(_)) => rx.clone(),
                _ => {
                    // Checked again under the lock: a download may have just
                    // finished.
                    if let Some(path) = self.available() {
                        return Ok(path);
                    }
                    let (tx, rx) = watch::channel(Download::Running(Progress { done: 0, total: None }));
                    *inflight = Some(rx.clone());
                    let runtime = self.clone();
                    tokio::spawn(async move {
                        let result = runtime.download(&tx).await;
                        if let Err(error) = &result {
                            tracing::warn!(%error, "the Bun download failed");
                        }
                        // Done goes out before the slot is cleared, so a waiter
                        // never misses it and a caller after a failure starts a
                        // new download.
                        let _ = tx.send(Download::Done(result));
                        *runtime.inner.inflight.lock().expect("runtime lock") = None;
                    });
                    rx
                }
            }
        };
        loop {
            let state = rx.borrow_and_update().clone();
            match state {
                Download::Running(now) => progress(now),
                Download::Done(result) => return result,
            }
            if rx.changed().await.is_err() {
                return match &*rx.borrow() {
                    Download::Done(result) => result.clone(),
                    Download::Running(_) => Err(RuntimeError::Io("the download task ended".into())),
                };
            }
        }
    }

    async fn download(&self, tx: &watch::Sender<Download>) -> Result<PathBuf, RuntimeError> {
        let asset = Self::asset_name()?;
        let runtimes = self.inner.home.join("runtimes");
        fs::create_dir_all(&runtimes)?;
        remove_leftovers(&runtimes);
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(60))
            .build()
            .map_err(|error| RuntimeError::Network(error.to_string()))?;
        let release = format!("{}/bun-v{}", self.inner.base_url, self.inner.version);
        let expected = published_sum(&client, &format!("{release}/SHASUMS256.txt"), &asset).await?;

        let archive = runtimes.join(format!(".download-{}.zip", random_hex()));
        let result = async {
            let actual = fetch_to(&client, &format!("{release}/{asset}"), &archive, tx).await?;
            if actual != expected {
                return Err(RuntimeError::Checksum { expected, actual });
            }
            let staging = runtimes.join(format!(".staging-{}", random_hex()));
            let version = self.inner.version.clone();
            let unpacked = {
                let (archive, staging) = (archive.clone(), staging.clone());
                tokio::task::spawn_blocking(move || unpack_bun(&archive, &staging))
                    .await
                    .map_err(|error| RuntimeError::Unpack(error.to_string()))?
            };
            let installed = match unpacked {
                Ok(binary) => verify(&binary, &version).await.and_then(|()| self.place(&staging)),
                Err(error) => Err(error),
            };
            let _ = fs::remove_dir_all(&staging);
            installed
        }
        .await;
        let _ = fs::remove_file(&archive);
        result
    }

    /// Renames the checked staging folder to `bun-<version>`. A folder that
    /// is already there (another process won the race) is kept.
    fn place(&self, staging: &Path) -> Result<PathBuf, RuntimeError> {
        let target = self.install_path();
        let folder = target.parent().expect("bun-<version>/bun");
        match fs::rename(staging, folder) {
            Ok(()) => Ok(target),
            Err(_) if self.available().is_some() => Ok(target),
            Err(error) => Err(error.into()),
        }
    }
}

/// Removes what a crashed download left in `runtimes`.
fn remove_leftovers(runtimes: &Path) {
    let Ok(entries) = fs::read_dir(runtimes) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".staging-") {
            let _ = fs::remove_dir_all(entry.path());
        } else if name.starts_with(".download-") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, RuntimeError> {
    client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| RuntimeError::Network(format!("{url}: {error}")))
}

/// The SHA-256 the release publishes for `asset`.
async fn published_sum(client: &reqwest::Client, url: &str, asset: &str) -> Result<String, RuntimeError> {
    let mut response = get(client, url).await?;
    let mut text = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| RuntimeError::Network(error.to_string()))? {
        text.extend_from_slice(&chunk);
        if text.len() > MAX_SUMS_BYTES {
            return Err(RuntimeError::Network(format!("{url} is too large")));
        }
    }
    let text = String::from_utf8_lossy(&text);
    text.lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == asset)
        .map(|(sum, _)| sum.to_ascii_lowercase())
        .filter(|sum| sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| RuntimeError::Network(format!("{url} lists no SHA-256 for {asset}")))
}

/// Streams `url` into the file `to`, reporting progress; the SHA-256 of what
/// was written.
async fn fetch_to(client: &reqwest::Client, url: &str, to: &Path, tx: &watch::Sender<Download>) -> Result<String, RuntimeError> {
    let mut response = get(client, url).await?;
    let total = response.content_length();
    if total.is_some_and(|total| total > MAX_DOWNLOAD_BYTES) {
        return Err(RuntimeError::Network(format!("{url} is over {MAX_DOWNLOAD_BYTES} bytes")));
    }
    let mut file = fs::File::create(to)?;
    let mut hash = Sha256::new();
    let mut done = 0u64;
    let _ = tx.send(Download::Running(Progress { done, total }));
    while let Some(chunk) = response.chunk().await.map_err(|error| RuntimeError::Network(format!("{url}: {error}")))? {
        done += chunk.len() as u64;
        if done > MAX_DOWNLOAD_BYTES {
            return Err(RuntimeError::Network(format!("{url} is over {MAX_DOWNLOAD_BYTES} bytes")));
        }
        hash.update(&chunk);
        file.write_all(&chunk)?;
        let _ = tx.send(Download::Running(Progress { done, total }));
    }
    file.sync_all()?;
    Ok(hex::encode(hash.finalize()))
}

/// Takes the `bun` binary out of the release zip into `staging/bun`, mode
/// 0755. The zip holds it as `bun-<os>-<arch>/bun`.
fn unpack_bun(archive: &Path, staging: &Path) -> Result<PathBuf, RuntimeError> {
    let unpack = |error: &dyn std::fmt::Display| RuntimeError::Unpack(error.to_string());
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).map_err(|e| unpack(&e))?;
    let index = (0..zip.len())
        .find(|&i| {
            zip.by_index(i).is_ok_and(|entry| {
                let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
                !entry.is_dir() && (name == "bun" || name.rsplit_once('/').is_some_and(|(dir, file)| file == "bun" && !dir.contains('/')))
            })
        })
        .ok_or_else(|| RuntimeError::Unpack("the archive holds no `bun`".into()))?;
    let mut entry = zip.by_index(index).map_err(|e| unpack(&e))?;
    if entry.size() > MAX_BINARY_BYTES {
        return Err(RuntimeError::Unpack("the binary is too large".into()));
    }
    fs::create_dir_all(staging)?;
    let binary = staging.join("bun");
    let mut out = fs::File::create(&binary)?;
    let copied = io::copy(&mut (&mut entry).take(MAX_BINARY_BYTES + 1), &mut out)?;
    if copied > MAX_BINARY_BYTES {
        return Err(RuntimeError::Unpack("the binary is too large".into()));
    }
    out.sync_all()?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    Ok(binary)
}

/// Runs `bun --version`: the binary must start here and be the pinned
/// version.
async fn verify(binary: &Path, version: &str) -> Result<(), RuntimeError> {
    let run = tokio::process::Command::new(binary).arg("--version").env_clear().kill_on_drop(true).inherit_nothing().output();
    let output = tokio::time::timeout(Duration::from_secs(30), run)
        .await
        .map_err(|_| RuntimeError::Verify("`bun --version` did not answer in 30 s".into()))?
        .map_err(|error| RuntimeError::Verify(error.to_string()))?;
    let printed = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || printed != version {
        return Err(RuntimeError::Verify(format!("{} printed {printed:?}", output.status)));
    }
    Ok(())
}
