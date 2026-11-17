//! §1: installed services, as releases.
//!
//! ```text
//! <home>/services/<id>/releases.json  the id's releases (the index)
//! <home>/services/<id>/<release>/     one release as installed: `dev` for a development install
//! <home>/service-data/<id>/           the service's own files, 0700, kept across installs
//! <home>/services-staging/            packages being installed
//! ```
//!
//! **How a release folder appears.** A package is unpacked and checked, and
//! its dependencies installed, under `services-staging/`. `services/<id>/`
//! appears only by renaming into place a staged folder that already holds
//! `releases.json` and the first release; a later release is renamed into
//! `services/<id>/` first, and `releases.json` is then replaced through a
//! temporary file and a rename. So a crash never leaves an id without its
//! index, and at the next start the staging folder is removed, as is a
//! release folder the index does not list; a listed release whose folder is
//! gone is dropped from the index.
//!
//! **What an earlier Pocket Hub left.** A folder `services/<id>/` with
//! `releases.json` is of this layout; any other (`<version>/` and
//! `current.json`, as Pocket Hub's previous protocol wrote them) is earlier.
//! Nothing under an earlier id is read, run or deleted, until the owner
//! removes it or installs the id again.
//!
//! A release is `dev` for a development install, the only kind there is
//! until releases come from the store; the index holds any release name of
//! the folder rule (`valid_release`), so the store's (16 hex) fit it as is.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::archive::{self, ArchiveError};
use crate::child::InheritNothing;
use crate::manifest::{MANIFEST_FILE, Manifest, ManifestError, valid_id};
use crate::runtime::{BunRuntime, RuntimeError};
use crate::util::{epoch_ms, inherited_env, path_with, private_dir, random_hex, write_atomic};

/// The index of an id's releases.
pub const INDEX_FILE: &str = "releases.json";
/// The release of a development install.
pub const DEV_RELEASE: &str = "dev";
/// The output of `bun install` kept in a report: its end, where errors are.
const MAX_INSTALL_OUTPUT: usize = 64 * 1024;

/// Whether `release` may name a release folder: `dev`, or 16 lower-case hex
/// digits (an applet package's footer hash, which `channel.open` carries).
pub fn valid_release(release: &str) -> bool {
    release == DEV_RELEASE || (release.len() == 16 && release.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// One installed release of a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledService {
    pub id: String,
    pub release: String,
    pub entry: String,
    pub idle_seconds: u64,
    pub requires: Vec<String>,
    pub hosts: Vec<String>,
    /// The release's folder: the process's working directory.
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
    /// A `package.json` without `bun.lock`: the host installs only what a
    /// lockfile pins, so the package must ship what it needs.
    NoLockfile,
    /// `bun install --frozen-lockfile --ignore-scripts` ran and succeeded.
    Installed { output: String, millis: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub service: InstalledService,
    /// The install replaced what an earlier Pocket Hub had left for the id.
    pub replaced_earlier: bool,
    pub dependencies: DependencyInstall,
}

/// The releases removed by `Registry::remove`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removed {
    /// The id was not installed (nor left by an earlier Pocket Hub).
    Nothing,
    /// The id's folder of an earlier Pocket Hub.
    Earlier,
    /// These releases.
    Releases(Vec<String>),
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
    #[error("`bun install --frozen-lockfile --ignore-scripts` failed ({status}):\n{output}")]
    Dependencies { status: String, output: String },
    #[error("`bun install --frozen-lockfile --ignore-scripts` did not finish in {seconds} s:\n{output}")]
    DependencyTimeout { seconds: u64, output: String },
    #[error("installing the package: {0}")]
    Io(#[from] io::Error),
}

/// Where a test stops an install as a crash would (the staging folder is
/// left as it is), to show what the next start makes of it.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPoint {
    /// The staged `<id>/` holds `releases.json` and its release; not renamed
    /// into `services/` yet.
    BeforeRename,
    /// A release renamed into an existing `services/<id>/`; `releases.json`
    /// not replaced yet.
    BeforeIndex,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    releases: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexEntry {
    release: String,
    installed_at: i64,
}

pub struct Registry {
    home: PathBuf,
    runtime: BunRuntime,
    install_timeout: Duration,
    /// The host's copy of the service SDK, linked into packages that lack one.
    sdk: Option<PathBuf>,
    /// One install or removal at a time.
    lock: tokio::sync::Mutex<()>,
    fail_point: Mutex<Option<FailPoint>>,
}

/// Where a package's own copy of the SDK would be.
pub const SDK_LINK: &str = "node_modules/@pocketjs/service";

/// Links `sdk` at `<package>/node_modules/@pocketjs/service` when nothing is
/// there: a package that brings its own copy keeps it. A link left dangling
/// (the home moved, the SDK folder was removed) is made again. True when a
/// link was made.
///
/// A link and not a copy: every installed package then runs the SDK of the
/// host that starts it, which is the one that speaks its protocol, and an
/// upgrade of the host upgrades them all without a reinstall.
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

fn read_index(folder: &Path) -> Option<Index> {
    serde_json::from_slice(&fs::read(folder.join(INDEX_FILE)).ok()?).ok()
}

fn write_index(folder: &Path, index: &Index) -> io::Result<()> {
    write_atomic(&folder.join(INDEX_FILE), &serde_json::to_vec_pretty(index).expect("releases.json"))
}

impl Registry {
    /// The registry of `home`, after the start's clean-up: what an install
    /// interrupted by a crash left in the staging folder is removed, and so
    /// is every release folder its id's index does not list (`sweep`).
    pub fn new(home: impl Into<PathBuf>, runtime: BunRuntime, install_timeout: Duration) -> Self {
        let home = home.into();
        let _ = fs::remove_dir_all(home.join("services-staging"));
        let registry = Self { home, runtime, install_timeout, sdk: None, lock: tokio::sync::Mutex::new(()), fail_point: Mutex::new(None) };
        registry.sweep();
        registry
    }

    /// Links `sdk` into every package installed from now on that has no SDK
    /// of its own (`link_sdk`).
    pub fn with_sdk(mut self, sdk: Option<PathBuf>) -> Self {
        self.sdk = sdk;
        self
    }

    #[doc(hidden)]
    pub fn set_fail_point(&self, point: Option<FailPoint>) {
        *self.fail_point.lock().unwrap() = point;
    }

    fn fails_at(&self, point: FailPoint) -> bool {
        *self.fail_point.lock().unwrap() == Some(point)
    }

    fn services(&self) -> PathBuf {
        self.home.join("services")
    }

    pub fn data_dir(&self, id: &str) -> PathBuf {
        self.home.join("service-data").join(id)
    }

    /// Each id of this layout: release folders its index does not list are
    /// removed, listed releases whose folder is gone are dropped from it. An
    /// earlier id is not touched.
    fn sweep(&self) {
        let Ok(entries) = fs::read_dir(self.services()) else { return };
        for entry in entries.flatten() {
            let folder = entry.path();
            let id = entry.file_name().to_string_lossy().into_owned();
            let Some(mut index) = read_index(&folder) else { continue };
            let Ok(children) = fs::read_dir(&folder) else { continue };
            for child in children.flatten() {
                let name = child.file_name().to_string_lossy().into_owned();
                let is_dir = child.file_type().is_ok_and(|kind| kind.is_dir());
                if is_dir && !index.releases.iter().any(|listed| listed.release == name) {
                    tracing::info!(id, release = name, "removing a release folder its index does not list");
                    let _ = fs::remove_dir_all(child.path());
                } else if !is_dir && name.starts_with('.') && name.ends_with(".tmp") {
                    // An index being replaced when the host stopped.
                    let _ = fs::remove_file(child.path());
                }
            }
            let before = index.releases.len();
            index.releases.retain(|listed| folder.join(&listed.release).is_dir());
            if index.releases.len() != before {
                tracing::warn!(id, "the index listed a release whose folder is gone");
                let _ = write_index(&folder, &index);
            }
        }
    }

    /// Whether `id` is installed in the layout of an earlier Pocket Hub: its
    /// folder has no `releases.json`. Nothing else of it is read.
    pub fn is_earlier(&self, id: &str) -> bool {
        valid_id(id) && self.services().join(id).is_dir() && !self.services().join(id).join(INDEX_FILE).exists()
    }

    /// Every id an earlier Pocket Hub left, sorted.
    pub fn earlier(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.services()) else { return Vec::new() };
        let mut ids: Vec<String> = entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|id| self.is_earlier(id)).collect();
        ids.sort();
        ids
    }

    /// One release of `id`, checked again from its files.
    pub fn release(&self, id: &str, release: &str) -> Option<InstalledService> {
        if !valid_id(id) || !valid_release(release) {
            return None;
        }
        let folder = self.services().join(id);
        let entry = read_index(&folder)?.releases.into_iter().find(|listed| listed.release == release)?;
        let dir = folder.join(release);
        match Manifest::read(&dir) {
            Ok(manifest) if manifest.id == id => Some(InstalledService {
                id: manifest.id,
                release: release.to_owned(),
                entry: manifest.entry,
                idle_seconds: manifest.idle_seconds,
                requires: manifest.requires,
                hosts: manifest.hosts,
                dir,
                data: self.data_dir(id),
                installed_at: entry.installed_at,
            }),
            Ok(_) => {
                tracing::warn!(id, release, "the installed manifest does not match its folder");
                None
            }
            Err(error) => {
                tracing::warn!(id, release, %error, "the installed release is broken");
                None
            }
        }
    }

    /// The release of `id` a channel starts: the development release when
    /// one is installed (the store's, by the device's hash, come with the
    /// store's releases).
    pub fn get(&self, id: &str) -> Option<InstalledService> {
        self.release(id, DEV_RELEASE)
    }

    /// The release each installed id starts, by id.
    pub fn list(&self) -> Vec<InstalledService> {
        let Ok(entries) = fs::read_dir(self.services()) else { return Vec::new() };
        let mut list: Vec<_> = entries.flatten().filter_map(|entry| self.get(&entry.file_name().to_string_lossy())).collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    /// Installs the package in `source`, a folder or a `.tar.gz` (`.tgz`), as
    /// the id's development release. A `.tar.gz` may hold the package at its
    /// root or in one folder at its root. An install over what an earlier
    /// Pocket Hub left replaces it: it is the owner's own action on the id.
    pub async fn install(&self, source: &Path) -> Result<InstallReport, InstallError> {
        let _guard = self.lock.lock().await;
        let staging = self.home.join("services-staging").join(random_hex());
        private_dir(staging.parent().expect("staging parent"))?;
        fs::create_dir_all(&staging)?;
        let result = self.install_staged(source, &staging).await;
        // A failure point stands for a crash: what was staged stays for the
        // next start to clean.
        if !matches!(&result, Err(InstallError::Io(error)) if error.to_string().starts_with("injected failure")) {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }

    async fn install_staged(&self, source: &Path, staging: &Path) -> Result<InstallReport, InstallError> {
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

        let release = DEV_RELEASE;
        let folder = self.services().join(&manifest.id);
        fs::create_dir_all(self.services())?;
        let installed_at = epoch_ms();
        let entry = IndexEntry { release: release.to_owned(), installed_at };
        let mut replaced_earlier = false;
        match read_index(&folder) {
            Some(mut index) => {
                // A later release: into the id's folder first, then the index.
                let dest = folder.join(release);
                if fs::symlink_metadata(&dest).is_ok() {
                    // The same release again: the old folder is moved aside
                    // first; a process running from it keeps its open files.
                    fs::rename(&dest, staging.join("replaced"))?;
                }
                fs::rename(&root, &dest)?;
                if self.fails_at(FailPoint::BeforeIndex) {
                    return Err(io::Error::other("injected failure before the index").into());
                }
                index.releases.retain(|listed| listed.release != release);
                index.releases.push(entry);
                write_index(&folder, &index)?;
            }
            None => {
                // The id's first release: the staged id folder holds its
                // index before it is renamed into place.
                let staged = staging.join("id");
                fs::create_dir(&staged)?;
                fs::rename(&root, staged.join(release))?;
                write_index(&staged, &Index { releases: vec![entry] })?;
                if self.fails_at(FailPoint::BeforeRename) {
                    return Err(io::Error::other("injected failure before the rename").into());
                }
                if folder.exists() {
                    // An earlier Pocket Hub's folder for the id, replaced by
                    // the owner's install.
                    fs::rename(&folder, staging.join("earlier"))?;
                    replaced_earlier = true;
                }
                fs::rename(&staged, &folder)?;
            }
        }
        private_dir(&self.data_dir(&manifest.id))?;

        let service = InstalledService {
            id: manifest.id.clone(),
            release: release.to_owned(),
            entry: manifest.entry,
            idle_seconds: manifest.idle_seconds,
            requires: manifest.requires,
            hosts: manifest.hosts,
            dir: folder.join(release),
            data: self.data_dir(&manifest.id),
            installed_at,
        };
        Ok(InstallReport { service, replaced_earlier, dependencies })
    }

    /// Runs `bun install --frozen-lockfile --ignore-scripts` in `root`,
    /// bounded in time; its output (both streams) goes into the report or
    /// the error. No dependency's install script runs: it would run outside
    /// any enclosure, at install time, with the owner's rights.
    async fn install_dependencies(&self, root: &Path) -> Result<DependencyInstall, InstallError> {
        let bun = self.runtime.ensure().await?;
        let mut command = tokio::process::Command::new(&bun);
        command
            .args(["install", "--frozen-lockfile", "--ignore-scripts"])
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
            .inherit_nothing();
        let started = Instant::now();
        let child = command.spawn()?;
        let pid = child.id();
        let output = match tokio::time::timeout(self.install_timeout, child.wait_with_output()).await {
            Ok(output) => output?,
            Err(_) => {
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

    /// Removes `release` of `id`, or every release with None (and the id's
    /// folder, also an earlier Pocket Hub's), and with `purge_data` the
    /// service's data folder. The caller has stopped its process.
    pub async fn remove(&self, id: &str, release: Option<&str>, purge_data: bool) -> io::Result<Removed> {
        if !valid_id(id) {
            return Ok(Removed::Nothing);
        }
        let _guard = self.lock.lock().await;
        let folder = self.services().join(id);
        let removed = match (read_index(&folder), release) {
            (None, _) if !folder.exists() => Removed::Nothing,
            (None, Some(_)) => Removed::Nothing,
            (None, None) => {
                fs::remove_dir_all(&folder)?;
                Removed::Earlier
            }
            (Some(index), None) => {
                // The index first: from here on nothing starts it, whatever
                // is left of the folder.
                let _ = fs::remove_file(folder.join(INDEX_FILE));
                fs::remove_dir_all(&folder)?;
                Removed::Releases(index.releases.into_iter().map(|listed| listed.release).collect())
            }
            (Some(mut index), Some(release)) => {
                if !index.releases.iter().any(|listed| listed.release == release) {
                    Removed::Nothing
                } else if index.releases.len() == 1 {
                    let _ = fs::remove_file(folder.join(INDEX_FILE));
                    fs::remove_dir_all(&folder)?;
                    Removed::Releases(vec![release.to_owned()])
                } else {
                    index.releases.retain(|listed| listed.release != release);
                    write_index(&folder, &index)?;
                    let _ = fs::remove_dir_all(folder.join(release));
                    Removed::Releases(vec![release.to_owned()])
                }
            }
        };
        if purge_data && (removed != Removed::Nothing || release.is_none()) {
            match fs::remove_dir_all(self.data_dir(id)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(removed)
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
