//! Pocket Hub's first install on a device: the program and the pairing
//! written onto the device's storage, outside the link (docs/POCKET-LINK.md
//! §1 "What does not run on the link", §3.2). A device whose storage Hub
//! does not reach is not set up here: it pairs on the link by the code it
//! shows (§3.11, `pocket_hub_core`'s endpoint).
//!
//! For a 3DS the storage is its SD card, reached over ftpd
//! (`ftp::FtpStorage`) or mounted in the computer's reader
//! (`card::CardStorage`). `install::install_3ds` is the one call for the UI
//! and the command line: it finds what the console holds, installs Pocket
//! Shell when the console lacks this build, and writes this Hub's pairing
//! with `pairing::setup_pairing`, which runs the plan of
//! `pocket_link::pairing::plan_setup`. `discover::discover` offers consoles
//! that run ftpd on the local network.
//!
//! No key appears in a log line, an error or a report.

pub mod card;
pub mod discover;
pub mod ftp;
pub mod install;
pub mod pairing;
pub mod program;
pub mod release;
pub mod storage;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use card::CardStorage;
pub use discover::{Candidate, DiscoverOptions, discover};
pub use ftp::{FTPD_PORT, FtpStorage, FtpTimeouts};
pub use install::{DeviceFamily, InstallOptions, InstallReport, InstallTarget, ProgramAction, ProgramReport, Progress, Stage, install, install_3ds};
pub use pairing::{FailedStep, PairingOutcome, PairingReport, setup_pairing};
pub use program::{Decision, InstalledProgram, InstalledPrograms, PROGRAM_NAME, PROGRAM_PATH};
pub use release::{BuildInfo, ReleaseError, ReleaseFetcher, ShellRelease, ShellSource, is_newer, version_order};
pub use storage::{FileRead, FileStat, LINK_DIR, SetupStorage, StorageError, Via};
