//! Installing Pocket Shell on a 3DS's card, only when it is not there yet.
//!
//! **The "already installed" rule.** Hub never downloads the program back
//! from the console to compare it (4 MB at SD speed). It keeps, per device
//! id, what it installed: the release's SHA-256 and size and the file's
//! modification time as the storage reported it right after the upload. The
//! program counts as installed when all of these hold:
//!
//! 1. `3ds/pocket-shell.3dsx` is on the card and its size is the release's;
//! 2. Hub's note for this device id names the same SHA-256 and size;
//! 3. the modification time the storage reports now is the one noted (when
//!    both are known), so a file of the same size written by someone else
//!    since is not taken for Hub's.
//!
//! Anything else installs: an absent file, another size, no note (another
//! computer or the owner put it there; a size can match between two builds),
//! or a note for another build. An install costs about 28 s for 4 MB over
//! ftpd (§13.3) and is correct in every case; skipping is the optimisation.
//!
//! The note lives in `installed-programs.json` in Hub's store folder
//! (mode 0600, written through a temporary file and a rename), because the
//! device record has no field for it yet.
//!
//! **The upload** goes to `3ds/pocket-shell.3dsx.tmp`, which the Homebrew
//! Launcher does not list, and is renamed into place; the size is checked
//! afterwards. A cut leaves either the old program whole or no program and a
//! `.tmp`, never a partial `.3dsx`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::release::ShellRelease;
use crate::storage::{FileStat, SetupStorage, StorageError};

/// The program's file name, written once: the card paths below and the name
/// an install over the link gives the device (`runtime.native.install`
/// replaces `sdmc:/3ds/<name>`, §6.2) are made from it, so a setup and an
/// update always write the same file.
macro_rules! program_name {
    () => {
        "pocket-shell.3dsx"
    };
}

/// The program's file name in `sdmc:/3ds/` (`DeviceFamily::program_name`).
pub const PROGRAM_NAME: &str = program_name!();
/// Where the program goes on a 3DS card (`sdmc:/3ds/pocket-shell.3dsx`).
pub const PROGRAM_PATH: &str = concat!("3ds/", program_name!());
pub const PROGRAM_TEMP: &str = concat!("3ds/", program_name!(), ".tmp");

/// What Hub installed on one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledProgram {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub version: Option<String>,
    pub modified: Option<String>,
}

/// The notes of `installed-programs.json`, by device id.
pub struct InstalledPrograms {
    path: PathBuf,
}

impl InstalledPrograms {
    pub const FILE: &'static str = "installed-programs.json";

    /// The notes in `dir` (Hub's store folder).
    pub fn new(dir: &Path) -> Self {
        Self { path: dir.join(Self::FILE) }
    }

    fn load(&self) -> BTreeMap<String, InstalledProgram> {
        std::fs::read_to_string(&self.path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn get(&self, device: &str) -> Option<InstalledProgram> {
        self.load().remove(device)
    }

    pub fn put(&self, device: &str, program: InstalledProgram) -> std::io::Result<()> {
        let mut all = self.load();
        all.insert(device.to_owned(), program);
        pocket_hub_store::write_secret(&self.path, &serde_json::to_string_pretty(&all)?)
    }

    pub fn remove(&self, device: &str) -> std::io::Result<()> {
        let mut all = self.load();
        if all.remove(device).is_some() {
            pocket_hub_store::write_secret(&self.path, &serde_json::to_string_pretty(&all)?)?;
        }
        Ok(())
    }
}

/// Why the program is or is not installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    /// The console holds this build, as Hub noted it.
    Installed,
    /// No program on the card.
    Missing,
    /// A program of another size.
    OtherSize,
    /// The same size, but Hub has no note that it installed this build there.
    Unrecorded,
    /// The same size and a note, but the file changed since.
    Changed,
    /// The caller asked for an install.
    Forced,
}

impl Decision {
    pub fn installs(self) -> bool {
        self != Self::Installed
    }
}

/// The rule of this module's comment.
pub fn decide(remote: Option<&FileStat>, release: &ShellRelease, note: Option<&InstalledProgram>, force: bool) -> Decision {
    if force {
        return Decision::Forced;
    }
    let Some(remote) = remote else { return Decision::Missing };
    if remote.size != release.size {
        return Decision::OtherSize;
    }
    let Some(note) = note.filter(|note| note.sha256 == release.sha256 && note.size == release.size && note.path == PROGRAM_PATH) else {
        return Decision::Unrecorded;
    };
    match (&note.modified, &remote.modified) {
        (Some(noted), Some(now)) if noted != now => Decision::Changed,
        _ => Decision::Installed,
    }
}

/// Uploads the program to the temporary name, renames it into place and
/// checks its size. Returns what the storage reports of the installed file.
pub async fn upload<S: SetupStorage>(storage: &mut S, bytes: &[u8], progress: &mut (dyn FnMut(u64) + Send)) -> Result<FileStat, StorageError> {
    storage.write(PROGRAM_TEMP, bytes, progress).await?;
    storage.rename(PROGRAM_TEMP, PROGRAM_PATH).await?;
    match storage.stat(PROGRAM_PATH).await? {
        Some(stat) if stat.size == bytes.len() as u64 => Ok(stat),
        Some(stat) => Err(StorageError(format!("{PROGRAM_PATH} is {} bytes after the upload, not {}", stat.size, bytes.len()))),
        None => Err(StorageError(format!("{PROGRAM_PATH} is not there after the upload"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(size: u64, sha: &str) -> ShellRelease {
        serde_json::from_value(serde_json::json!({
            "version": "1", "tag": "v1", "fileName": "p.3dsx", "size": size, "sha256": sha, "buildInfo": null, "origin": "test"
        }))
        .unwrap()
    }

    #[test]
    fn the_rule() {
        let release = release(10, "aa");
        let stat = |size, modified: Option<&str>| FileStat { size, modified: modified.map(str::to_owned) };
        let note = |sha: &str, modified: Option<&str>| InstalledProgram { path: PROGRAM_PATH.into(), sha256: sha.into(), size: 10, version: None, modified: modified.map(str::to_owned) };
        assert_eq!(decide(None, &release, None, false), Decision::Missing);
        assert_eq!(decide(Some(&stat(9, None)), &release, Some(&note("aa", None)), false), Decision::OtherSize);
        assert_eq!(decide(Some(&stat(10, None)), &release, None, false), Decision::Unrecorded);
        assert_eq!(decide(Some(&stat(10, None)), &release, Some(&note("bb", None)), false), Decision::Unrecorded);
        assert_eq!(decide(Some(&stat(10, Some("2"))), &release, Some(&note("aa", Some("1"))), false), Decision::Changed);
        assert_eq!(decide(Some(&stat(10, Some("1"))), &release, Some(&note("aa", Some("1"))), false), Decision::Installed);
        assert_eq!(decide(Some(&stat(10, None)), &release, Some(&note("aa", Some("1"))), false), Decision::Installed);
        assert_eq!(decide(Some(&stat(10, Some("1"))), &release, Some(&note("aa", Some("1"))), true), Decision::Forced);
    }
}
