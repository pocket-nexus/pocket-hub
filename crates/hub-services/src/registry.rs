//! §1: installed service packages.
//!
//! ```text
//! <home>/services/<id>/<version>/   a package, as installed
//! <home>/services/<id>/current.json the version that starts next
//! <home>/service-data/<id>/         the service's own files, 0700, kept across upgrades
//! <home>/services-staging/          packages being installed
//! ```
//!
//! A package is unpacked and checked, and its dependencies installed, in the
//! staging folder; only a complete package is renamed into `services/`, and
//! `current.json` is switched to it last, through a rename. A crash at any
//! step leaves the previous version current.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::child::InheritNothing;
use crate::archive::{self, ArchiveError};
use crate::manifest::{valid_id, Manifest, ManifestError, MANIFEST_FILE};
use crate::runtime::{BunRuntime, RuntimeError};
use crate::util::{epoch_ms, inherited_env, path_with, private_dir, random_hex, write_atomic};

const CURRENT_FILE: &str = "current.json";
/// The output of `bun install` kept in a report: its end, where errors are.
const MAX_INSTALL_OUTPUT: usize = 64 * 1024;

/// An installed service: the current version of one id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledService {
    pub id: String,
    pub version: String,
    pub entry: String,
    pub idle_seconds: u64,
    /// The package folder: the process's working directory.
    pub dir: PathBuf,
    /// The service's data folder (`POCKET_SERVICE_DATA`).
    pub data: PathBuf,
    /// Epoch milliseconds.
    pub installed_at: i64,
}

/// What became of the package's dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DependencyInstall {
    /// No `package.json`: nothing to install.
    None,
    /// A `package.json` without `bun.lock`: Hub installs only what a lockfile
    /// pins, so the package must ship what it needs.
    NoLockfile,
    /// `bun install --frozen-lockfile` ran and succeeded.
    Installed { output: String, millis: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub service: InstalledService,
    /// The version that was current before, if any.
    pub previous: Option<String>,
    pub dependencies: DependencyInstall,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("the package source {0}: not a folder or a .tar.gz")]
    Source(String),
    #[error("the package: {0}")]
    Archive(#[from] ArchiveError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("`bun install --frozen-lockfile` failed ({status}):\n{output}")]
    Dependencies { status: String, output: String },
    #[error("`bun install --frozen-lockfile` did not finish in {seconds} s:\n{output}")]
    DependencyTimeout { seconds: u64, output: String },
    #[error("installing the package: {0}")]
    Io(#[from] io::Error),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Current {
    version: String,
    installed_at: i64,
}

pub struct Registry {
    home: PathBuf,
    runtime: BunRuntime,
    install_timeout: Duration,
    /// Hub's copy of the service SDK, linked into packages that lack one.
    sdk: Option<PathBuf>,
    /// One install or removal at a time.
    lock: tokio::sync::Mutex<()>,
}

/// Where a package's own copy of the SDK would be.
pub const SDK_LINK: &str = "node_modules/@pocketjs/service";

/// Links `sdk` at `<package>/node_modules/@pocketjs/service` when nothing is
/// there: a package that brings its own copy keeps it. A link left dangling
/// (Hub's home moved, the SDK folder was removed) is made again. True when a
/// link was made.
///
/// A link and not a copy: every installed package then runs the SDK of the
/// Hub that starts it, which is the one that speaks its protocol, and an
/// upgrade of Hub upgrades them all without a reinstall.
pub fn link_sdk(package: &Path, sdk: &Path) -> io::Result<bool> {
    let link = package.join(SDK_LINK);
    match fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() && fs::metadata(&link).is_err() => fs::remove_file(&link)?,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::create_dir_all(link.parent().expect("a scope folder"))?;
    std::os::unix::fs::symlink(sdk, &link)?;
    Ok(true)
}

impl Registry {
    pub fn new(home: impl Into<PathBuf>, runtime: BunRuntime, install_timeout: Duration) -> Self {
        let home = home.into();
        // What an install interrupted by a crash left.
        let _ = fs::remove_dir_all(home.join("services-staging"));
        Self { home, runtime, install_timeout, sdk: None, lock: tokio::sync::Mutex::new(()) }
    }

    /// Links `sdk` into every package installed from now on that has no SDK
    /// of its own (`link_sdk`).
    pub fn with_sdk(mut self, sdk: Option<PathBuf>) -> Self {
        self.sdk = sdk;
        self
    }

    fn services(&self) -> PathBuf {
        self.home.join("services")
    }

    pub fn data_dir(&self, id: &str) -> PathBuf {
        self.home.join("service-data").join(id)
    }

    /// The current version of `id`, checked again from its files.
    pub fn get(&self, id: &str) -> Option<InstalledService> {
        if !valid_id(id) {
            return None;
        }
        let folder = self.services().join(id);
        let current: Current = serde_json::from_slice(&fs::read(folder.join(CURRENT_FILE)).ok()?).ok()?;
        let dir = folder.join(&current.version);
        match Manifest::read(&dir) {
            Ok(manifest) if manifest.id == id && manifest.version == current.version => Some(InstalledService {
                id: manifest.id,
                version: manifest.version,
                entry: manifest.entry,
                idle_seconds: manifest.idle_seconds,
                dir,
                data: self.data_dir(id),
                installed_at: current.installed_at,
            }),
            Ok(_) => {
                tracing::warn!(id, "the installed manifest does not match its folder");
                None
            }
            Err(error) => {
                tracing::warn!(id, %error, "the installed package is broken");
                None
            }
        }
    }

    /// Every installed service, by id.
    pub fn list(&self) -> Vec<InstalledService> {
        let Ok(entries) = fs::read_dir(self.services()) else { return Vec::new() };
        let mut list: Vec<_> = entries.flatten().filter_map(|entry| self.get(&entry.file_name().to_string_lossy())).collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    /// Installs the package in `source`, a folder or a `.tar.gz` (`.tgz`),
    /// and makes it current. A `.tar.gz` may hold the package at its root or
    /// in one folder at its root. Versions of the same id other than the new
    /// one and those in `keep` are removed.
    pub async fn install(&self, source: &Path, keep: &[String]) -> Result<InstallReport, InstallError> {
        let _guard = self.lock.lock().await;
        let staging = self.home.join("services-staging").join(random_hex());
        private_dir(staging.parent().expect("staging parent"))?;
        fs::create_dir_all(&staging)?;
        let result = self.install_staged(source, &staging, keep).await;
        let _ = fs::remove_dir_all(&staging);
        result
    }

    async fn install_staged(&self, source: &Path, staging: &Path, keep: &[String]) -> Result<InstallReport, InstallError> {
        let unpacked = staging.join("package");
        fs::create_dir(&unpacked)?;
        let meta = fs::metadata(source).map_err(|error| InstallError::Source(format!("{}: {error}", source.display())))?;
        let name = source.file_name().map(|name| name.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        {
            let (source, unpacked) = (source.to_path_buf(), unpacked.clone());
            if meta.is_dir() {
                // The installed dependencies are made again from the lockfile;
                // a folder's own `node_modules` is taken only without one.
                let locked = has_lockfile(&source);
                tokio::task::spawn_blocking(move || {
                    let skip: &[&str] = if locked { &[".git", "node_modules"] } else { &[".git"] };
                    archive::copy_folder(&source, &unpacked, skip)
                })
                .await
                .map_err(io::Error::other)??;
            } else if meta.is_file() && (name.ends_with(".tar.gz") || name.ends_with(".tgz")) {
                tokio::task::spawn_blocking(move || archive::unpack_tar_gz(&source, &unpacked)).await.map_err(io::Error::other)??;
            } else {
                return Err(InstallError::Source(source.display().to_string()));
            }
        }
        let root = package_root(&unpacked);
        let manifest = Manifest::read(&root)?;

        let dependencies = if !root.join("package.json").is_file() {
            DependencyInstall::None
        } else if !has_lockfile(&root) {
            DependencyInstall::NoLockfile
        } else {
            self.install_dependencies(&root).await?
        };

        if let Some(sdk) = &self.sdk {
            link_sdk(&root, sdk)?;
        }

        let folder = self.services().join(&manifest.id);
        fs::create_dir_all(&folder)?;
        let dest = folder.join(&manifest.version);
        let previous = self.get(&manifest.id).map(|service| service.version);
        if fs::symlink_metadata(&dest).is_ok() {
            // The same version again: the old folder is moved aside first; a
            // process running from it keeps the files it has open.
            fs::rename(&dest, staging.join("replaced"))?;
        }
        fs::rename(&root, &dest)?;
        let installed_at = epoch_ms();
        let current = serde_json::to_vec_pretty(&Current { version: manifest.version.clone(), installed_at }).expect("current.json");
        write_atomic(&folder.join(CURRENT_FILE), &current)?;
        private_dir(&self.data_dir(&manifest.id))?;
        let mut keep = keep.to_vec();
        keep.push(manifest.version.clone());
        self.prune_locked(&manifest.id, &keep);

        let service = InstalledService {
            id: manifest.id.clone(),
            version: manifest.version,
            entry: manifest.entry,
            idle_seconds: manifest.idle_seconds,
            dir: dest,
            data: self.data_dir(&manifest.id),
            installed_at,
        };
        Ok(InstallReport { service, previous, dependencies })
    }

    /// Runs `bun install --frozen-lockfile` in `root`, bounded in time; its
    /// output (both streams) goes into the report or the error.
    async fn install_dependencies(&self, root: &Path) -> Result<DependencyInstall, InstallError> {
        let bun = self.runtime.ensure().await?;
        let mut command = tokio::process::Command::new(&bun);
        command
            .args(["install", "--frozen-lockfile"])
            .current_dir(root)
            .env_clear()
            .envs(inherited_env())
            .env("PATH", path_with(&bun))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            // Lifecycle scripts run here: no descriptor of Hub's.
            .inherit_nothing();
        let started = Instant::now();
        let child = command.spawn()?;
        let pid = child.id();
        let output = match tokio::time::timeout(self.install_timeout, child.wait_with_output()).await {
            Ok(output) => output?,
            Err(_) => {
                // The whole group: `bun install` runs lifecycle scripts.
                if let Some(pid) = pid {
                    crate::util::signal_group(pid, libc::SIGKILL);
                }
                return Err(InstallError::DependencyTimeout { seconds: self.install_timeout.as_secs(), output: String::new() });
            }
        };
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        let text = tail(text.trim(), MAX_INSTALL_OUTPUT);
        if !output.status.success() {
            return Err(InstallError::Dependencies { status: output.status.to_string(), output: text });
        }
        Ok(DependencyInstall::Installed { output: text, millis: started.elapsed().as_millis() as u64 })
    }

    /// Removes every version folder of `id` not named in `keep`, between
    /// installs.
    pub async fn prune(&self, id: &str, keep: &[String]) {
        let _guard = self.lock.lock().await;
        self.prune_locked(id, keep);
    }

    fn prune_locked(&self, id: &str, keep: &[String]) {
        if !valid_id(id) {
            return;
        }
        let Ok(entries) = fs::read_dir(self.services().join(id)) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir()
                && !keep.contains(&name)
                && let Err(error) = fs::remove_dir_all(entry.path())
            {
                tracing::warn!(id, version = name, %error, "removing an old service version");
            }
        }
    }

    /// Removes the service `id`: its packages, and with `purge_data` its data
    /// folder. The caller has stopped its process. False when it was not
    /// installed.
    pub async fn remove(&self, id: &str, purge_data: bool) -> io::Result<bool> {
        if !valid_id(id) {
            return Ok(false);
        }
        let _guard = self.lock.lock().await;
        let folder = self.services().join(id);
        let existed = folder.exists();
        if existed {
            // The pointer first: from here on nothing starts it, whatever is
            // left of the folder.
            let _ = fs::remove_file(folder.join(CURRENT_FILE));
            fs::remove_dir_all(&folder)?;
        }
        if purge_data {
            match fs::remove_dir_all(self.data_dir(id)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(existed)
    }
}

fn has_lockfile(root: &Path) -> bool {
    root.join("package.json").is_file() && (root.join("bun.lock").is_file() || root.join("bun.lockb").is_file())
}

/// The package root of an unpacked archive: the folder itself, or its only
/// entry when that is a folder and the manifest is not at the top.
fn package_root(unpacked: &Path) -> PathBuf {
    if unpacked.join(MANIFEST_FILE).exists() {
        return unpacked.to_path_buf();
    }
    let entries: Vec<_> = fs::read_dir(unpacked).map(|entries| entries.flatten().collect()).unwrap_or_default();
    match entries.as_slice() {
        [only] if only.file_type().is_ok_and(|kind| kind.is_dir()) => only.path(),
        _ => unpacked.to_path_buf(),
    }
}

/// The last `max` bytes of `text`, on a character boundary.
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}
