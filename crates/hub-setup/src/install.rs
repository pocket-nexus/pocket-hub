//! One call that sets a console up: the program and the pairing, safe to run
//! again at any time.
//!
//! ```text
//! connect → read the pairing and the program's size (no write)
//!         → close; look the release up; download it only when it installs
//!         → connect → upload the program when needed → write the pairing → close
//! ```
//!
//! **The program goes before the pairing.** The pairing's writes are the only
//! step that changes Hub's record (the minted key is stored before the first
//! write, §3.2 step 4), and the upload is the long and fragile step (about
//! 28 s for 4 MB over ftpd, §13.3). Uploading first means a dropped upload
//! leaves the card's pairing and Hub's record exactly as they were: no new
//! key, no "setup not confirmed". The pairing then runs on a connection that
//! just carried the program, from reads made right before its writes, and
//! its final readback is the last thing the session does, so a success
//! report means every step was checked. The console never holds a pairing
//! written for a program it lacks. The other order would be equally
//! repairable by a second run (both steps are idempotent), but would leave a
//! fresh key and record behind a failed upload.
//!
//! **The first read is only a look**: it ends the install before any upload
//! when a read fails or the console already holds 8 computers, and gives the
//! device id under which Hub noted the program it installed. The pairing is
//! read again before it is written.
//!
//! **No session stays open across the download**: ftpd drops a session idle
//! for 60 s, so the look and the writes use two connections.

use std::net::SocketAddr;
use std::path::PathBuf;

use pocket_hub_store::Store;
use pocket_link::pairing::{ReadOutcome, SetupOutcome};
use serde::Serialize;

use crate::card::CardStorage;
use crate::ftp::{FtpStorage, FtpTimeouts};
use crate::pairing::{PairingOutcome, PairingReport, owner_message, plan, read_pairing, setup_pairing};
use crate::program::{Decision, InstalledProgram, InstalledPrograms, PROGRAM_PATH, decide, upload};
use crate::release::{ReleaseFetcher, ShellRelease, ShellSource};
use crate::storage::{AnyStorage, FileStat, SetupStorage};

/// A device family Hub sets up and whose program it installs: what the
/// setup writes and how. The 3DS is the one such family. An M5Stack Tab5 is
/// not one: it pairs on the link by the code it shows (§3.11) and Hub
/// installs no program on it, so it has no variant here; what Hub knows of
/// every family is `pocket_hub_core::family`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum DeviceFamily {
    #[serde(rename = "3ds")]
    Nintendo3ds,
}

impl DeviceFamily {
    /// The record's `kind`.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Nintendo3ds => "3ds",
        }
    }

    /// The family of a record's `kind`; None for a kind Hub does not know.
    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            "3ds" => Some(Self::Nintendo3ds),
            _ => None,
        }
    }

    /// The name of the program the device runs, which a new build always
    /// replaces: the file the setup writes at `PROGRAM_PATH`, and the name an
    /// update over the link sends (`runtime.native.install`), whatever the
    /// local file or the release asset is called. A release's asset is
    /// `pocket-shell-v<date>.3dsx`; sent under that name it would sit beside
    /// the running program as a second one.
    pub fn program_name(self) -> &'static str {
        match self {
            Self::Nintendo3ds => crate::program::PROGRAM_NAME,
        }
    }

    /// The program's path on the device's storage, as the note of what Hub
    /// installed names it (`InstalledProgram::path`).
    pub fn program_path(self) -> &'static str {
        match self {
            Self::Nintendo3ds => PROGRAM_PATH,
        }
    }
}

/// Where the console's storage is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallTarget {
    /// A 3DS running ftpd (port 5000 by default).
    Ftp { address: SocketAddr },
    /// A 3DS's SD card mounted at `root`.
    Card { root: PathBuf },
}

impl InstallTarget {
    pub fn family(&self) -> DeviceFamily {
        DeviceFamily::Nintendo3ds
    }
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// Hub's link port, written into the pairing (8628 unless taken, §2.1).
    pub hub_port: u16,
    pub source: ShellSource,
    /// The program's download cache.
    pub cache_dir: PathBuf,
    /// Install the program even when the console holds it.
    pub reinstall: bool,
    pub ftp_timeouts: FtpTimeouts,
    /// On a mounted card: the addresses the pairing takes in place of Hub's
    /// current interface addresses.
    pub card_addresses: Option<Vec<String>>,
}

impl InstallOptions {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            hub_port: pocket_link::consts::PORT,
            source: ShellSource::default(),
            cache_dir: cache_dir.into(),
            reinstall: false,
            ftp_timeouts: FtpTimeouts::default(),
            card_addresses: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Connecting,
    /// Reading what the console holds.
    Checking,
    /// Looking the program's release up.
    Resolving,
    Downloading,
    Uploading,
    Pairing,
    Done,
}

/// One progress event, for a progress bar: bytes for the download and the
/// upload, operations for the pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: Stage,
    pub done: u64,
    pub total: Option<u64>,
}

impl Progress {
    fn at(stage: Stage) -> Self {
        Self { stage, done: 0, total: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProgramAction {
    /// Uploaded in this install.
    Installed,
    /// The console held this build.
    AlreadyInstalled,
    /// The release could not be looked up; the program on the console was
    /// kept without a check.
    KeptUnchecked,
    /// Not installed: the install ended before the upload or in it.
    NotInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramReport {
    pub action: ProgramAction,
    pub decision: Option<Decision>,
    /// Where the program is on the console.
    pub path: String,
    /// The build installed or found, with `build-info.json`'s fields for the
    /// caller to judge (a release built before Pocket Link 2 cannot link).
    pub release: Option<ShellRelease>,
    pub detail: Option<String>,
}

/// The install's result. Holds no key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub ok: bool,
    pub kind: &'static str,
    /// `ftp` or `card`.
    pub via: &'static str,
    pub device_id: Option<String>,
    pub failed_stage: Option<Stage>,
    pub program: ProgramReport,
    pub pairing: Option<PairingReport>,
    /// What the owner does next, in plain words.
    pub next_step: String,
    /// What went wrong, for a log or a support question.
    pub detail: Option<String>,
}

impl InstallReport {
    fn new(target: &InstallTarget) -> Self {
        Self {
            ok: false,
            kind: target.family().kind(),
            via: match target {
                InstallTarget::Ftp { .. } => "ftp",
                InstallTarget::Card { .. } => "card",
            },
            device_id: None,
            failed_stage: None,
            program: ProgramReport { action: ProgramAction::NotInstalled, decision: None, path: format!("sdmc:/{PROGRAM_PATH}"), release: None, detail: None },
            pairing: None,
            next_step: String::new(),
            detail: None,
        }
    }

    fn fail(mut self, stage: Stage, next_step: impl Into<String>, detail: Option<String>) -> Self {
        tracing::info!(stage = ?stage, detail = detail.as_deref().unwrap_or(""), "install failed");
        self.ok = false;
        self.failed_stage = Some(stage);
        self.next_step = next_step.into();
        self.detail = detail;
        self
    }
}

async fn open(target: &InstallTarget, options: &InstallOptions) -> Result<AnyStorage, (String, String)> {
    match target {
        InstallTarget::Ftp { address } => match FtpStorage::connect(*address, options.ftp_timeouts).await {
            Ok(storage) => Ok(AnyStorage::Ftp(storage)),
            Err(error) => Err((
                format!(
                    "Hub could not reach the console at {address}. On the console, open ftpd from the Homebrew Launcher and keep it open, check that the console and this computer are on the same Wi-Fi network, then try again."
                ),
                error.to_string(),
            )),
        },
        InstallTarget::Card { root } => {
            if !root.is_dir() {
                return Err(("Hub could not find the SD card. Put it in the card reader and choose it again.".into(), "the card's folder is not there".into()));
            }
            if !CardStorage::looks_like_3ds_card(root) {
                return Err((
                    "This does not look like a 3DS SD card (it has no \"Nintendo 3DS\" or \"3ds\" folder). Choose the card the console uses.".into(),
                    "no Nintendo 3DS or 3ds folder at the card's root".into(),
                ));
            }
            Ok(AnyStorage::Card(match &options.card_addresses {
                Some(addresses) => CardStorage::with_addresses(root, addresses.clone()),
                None => CardStorage::new(root),
            }))
        }
    }
}

async fn close(storage: AnyStorage) {
    if let AnyStorage::Ftp(storage) = storage {
        storage.close().await;
    }
}

/// Sets up a device of any family. Today that is the 3DS.
pub async fn install(target: &InstallTarget, store: &Store, options: &InstallOptions, progress: &mut (dyn FnMut(Progress) + Send)) -> InstallReport {
    match target.family() {
        DeviceFamily::Nintendo3ds => install_3ds(target, store, options, progress).await,
    }
}

/// Installs Pocket Shell (when the console does not hold it) and writes this
/// Hub's pairing onto a 3DS, over ftpd or on its mounted SD card.
pub async fn install_3ds(target: &InstallTarget, store: &Store, options: &InstallOptions, progress: &mut (dyn FnMut(Progress) + Send)) -> InstallReport {
    let kind = DeviceFamily::Nintendo3ds.kind();
    let mut report = InstallReport::new(target);
    let notes = InstalledPrograms::new(store.dir());

    // The look: what the console holds, before anything is downloaded or
    // written.
    progress(Progress::at(Stage::Connecting));
    let mut storage = match open(target, options).await {
        Ok(storage) => storage,
        Err((next, detail)) => return report.fail(Stage::Connecting, next, Some(detail)),
    };
    progress(Progress::at(Stage::Checking));
    let reads = read_pairing(&mut storage, &store.hub_id()).await;
    let look = plan(&reads, store, options.hub_port, storage.via(), false);
    let known_id = match &reads.device {
        ReadOutcome::File(text) => pocket_link::pairing::parse_device_file(text).map(str::to_owned),
        _ => None,
    };
    report.device_id = known_id.clone();
    if reads.failure.is_some() || matches!(look.outcome, SetupOutcome::FailedRead | SetupOutcome::EightComputers) {
        close(storage).await;
        let outcome = if reads.failure.is_some() { PairingOutcome::FailedRead } else { PairingOutcome::from(look.outcome) };
        let pairing = PairingReport {
            outcome,
            failed: true,
            failed_at: None,
            device_id: known_id.clone(),
            unconfirmed: known_id.as_deref().and_then(|id| store.device(id)).is_some_and(|record| record.setup_unconfirmed),
            message: owner_message(outcome, true).to_owned(),
            detail: reads.detail.clone().or_else(|| reads.failure.clone()).or_else(|| Some(look.message.to_owned())),
        };
        let next = pairing.message.clone();
        let detail = pairing.detail.clone();
        report.pairing = Some(pairing);
        return report.fail(Stage::Checking, next, detail);
    }
    let remote: Option<FileStat> = match storage.stat(PROGRAM_PATH).await {
        Ok(remote) => remote,
        Err(error) => {
            close(storage).await;
            return report.fail(Stage::Checking, owner_message(PairingOutcome::FailedRead, true), Some(error.0));
        }
    };
    close(storage).await;

    // The program: looked up, and downloaded only when it installs.
    progress(Progress::at(Stage::Resolving));
    let fetcher = match ReleaseFetcher::new(&options.cache_dir, &options.source) {
        Ok(fetcher) => fetcher,
        Err(error) => return report.fail(Stage::Resolving, "Hub could not prepare the download of the 3DS's software. Nothing was changed on the console.", Some(error.to_string())),
    };
    let mut bytes = None;
    match fetcher.resolve(&options.source).await {
        Err(error) if remote.is_some() => {
            tracing::info!(%error, "the release could not be looked up; the console's program is kept");
            report.program.action = ProgramAction::KeptUnchecked;
            report.program.detail = Some(error.to_string());
        }
        Err(error) => {
            return report.fail(
                Stage::Resolving,
                "Hub could not download the 3DS's software. Check this computer's internet connection and try again. Nothing was changed on the console.",
                Some(error.to_string()),
            );
        }
        Ok(release) => {
            let note = known_id.as_deref().and_then(|id| notes.get(id));
            let decision = decide(remote.as_ref(), &release, note.as_ref(), options.reinstall);
            tracing::info!(decision = ?decision, version = release.version.as_deref().unwrap_or(""), "program");
            report.program.decision = Some(decision);
            if decision.installs() {
                progress(Progress { stage: Stage::Downloading, done: 0, total: Some(release.size) });
                match fetcher.fetch(&release, &mut |done, total| progress(Progress { stage: Stage::Downloading, done, total })).await {
                    Ok(fetched) => bytes = Some(fetched),
                    Err(error) => {
                        report.program.release = Some(release);
                        return report.fail(
                            Stage::Downloading,
                            "Hub could not download the 3DS's software. Check this computer's internet connection and try again. Nothing was changed on the console.",
                            Some(error.to_string()),
                        );
                    }
                }
            } else {
                report.program.action = ProgramAction::AlreadyInstalled;
            }
            report.program.release = Some(release);
        }
    }

    // The writes: the program, then the pairing.
    progress(Progress::at(Stage::Connecting));
    let mut storage = match open(target, options).await {
        Ok(storage) => storage,
        Err((next, detail)) => return report.fail(Stage::Connecting, next, Some(detail)),
    };
    let mut installed = None;
    if let Some(bytes) = &bytes {
        let total = bytes.len() as u64;
        progress(Progress { stage: Stage::Uploading, done: 0, total: Some(total) });
        match upload(&mut storage, bytes, &mut |done| progress(Progress { stage: Stage::Uploading, done, total: Some(total) })).await {
            Ok(stat) => {
                report.program.action = ProgramAction::Installed;
                installed = Some(stat);
            }
            Err(error) => {
                close(storage).await;
                let next = match target {
                    InstallTarget::Ftp { .. } => {
                        "Copying the 3DS's software to the console stopped. Keep ftpd open and the console near the Wi-Fi router, then try again. Nothing else was changed."
                    }
                    InstallTarget::Card { .. } => "Copying the 3DS's software to the SD card failed. Check that the card is not locked or full, then try again.",
                };
                return report.fail(Stage::Uploading, next, Some(error.0));
            }
        }
    }

    progress(Progress::at(Stage::Pairing));
    let pairing = setup_pairing(&mut storage, store, kind, options.hub_port, &mut |done, total| progress(Progress { stage: Stage::Pairing, done, total: Some(total) })).await;
    close(storage).await;

    report.device_id = pairing.device_id.clone().or(known_id);
    if let (Some(stat), Some(release), Some(id)) = (installed, &report.program.release, &report.device_id) {
        let note = InstalledProgram { path: PROGRAM_PATH.to_owned(), sha256: release.sha256.clone(), size: release.size, version: release.version.clone(), modified: stat.modified };
        if let Err(error) = notes.put(id, note) {
            // Costs one more upload at the next install.
            tracing::warn!(%error, "the installed program could not be noted");
        }
    }
    let failed = pairing.failed;
    let next = pairing.message.clone();
    let detail = pairing.detail.clone();
    report.pairing = Some(pairing);
    if failed {
        return report.fail(Stage::Pairing, next, detail);
    }

    let name = store.hub_name();
    let mut next = match target {
        InstallTarget::Ftp { .. } => {
            format!("Done. On the console, press START to exit ftpd, then open Pocket Shell from the Homebrew Launcher. It connects to \"{name}\" by itself.")
        }
        InstallTarget::Card { .. } => {
            format!("Done. Eject the SD card, put it back in the console, then open Pocket Shell from the Homebrew Launcher. It connects to \"{name}\" by itself.")
        }
    };
    if report.program.action == ProgramAction::KeptUnchecked {
        next.push_str(" (Hub could not check for newer software for this 3DS, so the software on the console was kept.)");
    }
    report.ok = true;
    report.next_step = next;
    progress(Progress { stage: Stage::Done, done: 1, total: Some(1) });
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_family_names_the_program_on_the_device() {
        let family = DeviceFamily::from_kind("3ds").unwrap();
        assert_eq!(family, DeviceFamily::Nintendo3ds);
        assert_eq!(DeviceFamily::from_kind("tab5"), None);
        // The name an update sends is the file a setup writes: one program.
        assert_eq!(family.program_name(), "pocket-shell.3dsx");
        assert_eq!(format!("3ds/{}", family.program_name()), PROGRAM_PATH);
        assert_eq!(crate::program::PROGRAM_TEMP, format!("{PROGRAM_PATH}.tmp"));
        assert!(pocket_link::consts::NATIVE_NAME_MIN <= family.program_name().len() && family.program_name().ends_with(".3dsx"));
    }
}
