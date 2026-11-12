//! The storage a setup runs against: a 3DS's SD card reached over ftpd or
//! mounted in the computer's reader (docs/POCKET-LINK.md §3.2).
//!
//! Paths are relative to the card's root (`pocketjs/link/device.txt`,
//! `3ds/pocket-shell.3dsx`) and hold only letters, digits, `.`, `_`, `-` and
//! `/`, so a path can never end an FTP command or leave the card.

use std::future::Future;

pub use pocket_link::pairing::Via;

/// The link directory on the card (`sdmc:/pocketjs/link/`).
pub const LINK_DIR: &str = "pocketjs/link";

/// A storage operation that did not complete. The message names the path and
/// the server's or the system's answer, never a file's content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct StorageError(pub String);

impl StorageError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// A read's three outcomes, kept apart as §3.2 step 3 needs: a failed read
/// says nothing about the file and must never be taken for an absent one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRead {
    /// The transfer dropped, timed out, or the storage answered an error that
    /// does not say the file is missing.
    Failed(StorageError),
    /// The storage answered that the file is not there.
    Absent,
    /// The file, read whole.
    File(Vec<u8>),
}

impl FileRead {
    /// The outcome as `pocket_link::pairing` takes it. A file that is not
    /// UTF-8 is read lossily: the device's reader takes bytes, and a key line
    /// survives the replacement of other bytes.
    pub fn to_outcome(&self) -> pocket_link::pairing::ReadOutcome {
        use pocket_link::pairing::ReadOutcome;
        match self {
            Self::Failed(_) => ReadOutcome::Failed,
            Self::Absent => ReadOutcome::Absent,
            Self::File(bytes) => ReadOutcome::File(String::from_utf8_lossy(bytes).into_owned()),
        }
    }
}

/// What the storage says about a file without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub size: u64,
    /// The modification time as the storage reports it (FTP `MDTM`, or the
    /// file system's), compared only for equality; None when unavailable.
    pub modified: Option<String>,
}

/// The operations Hub's setup and the program's install need.
///
/// Every method returns a `Send` future so that an install can run on a
/// spawned task.
pub trait SetupStorage: Send {
    /// How Hub's address is known for the pairing it writes (§3.2 step 3).
    fn via(&self) -> Via;

    /// Reads a file whole.
    fn read(&mut self, path: &str) -> impl Future<Output = FileRead> + Send;

    /// The file names of a folder; an absent folder lists as empty. An error
    /// when the listing failed.
    fn list(&mut self, dir: &str) -> impl Future<Output = Result<Vec<String>, StorageError>> + Send;

    /// The size of a file, None when it is absent.
    fn stat(&mut self, path: &str) -> impl Future<Output = Result<Option<FileStat>, StorageError>> + Send;

    /// Writes a file whole, creating its parent folders. `progress` receives
    /// the bytes written so far.
    fn write(&mut self, path: &str, bytes: &[u8], progress: &mut (dyn FnMut(u64) + Send)) -> impl Future<Output = Result<(), StorageError>> + Send;

    /// Deletes a file. Done when a read afterwards answers "absent", as the
    /// reference does (`ftp-storage.ts`); deleting an absent file succeeds.
    fn delete(&mut self, path: &str) -> impl Future<Output = Result<(), StorageError>> + Send;

    /// Renames a file, replacing `to` when it exists.
    fn rename(&mut self, from: &str, to: &str) -> impl Future<Output = Result<(), StorageError>> + Send;
}

/// A path this crate sends to a storage: segments of `[A-Za-z0-9._-]`, none
/// of them `.` or `..`.
pub fn is_safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|segment| {
            !segment.is_empty() && segment != "." && segment != ".." && segment.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

pub(crate) fn check_path(path: &str) -> Result<(), StorageError> {
    if is_safe_path(path) { Ok(()) } else { Err(StorageError::new(format!("not a path the setup writes: {path:?}"))) }
}

/// A plan path of `pocket_link::pairing` (relative to the link directory) as
/// a card path.
pub(crate) fn link_path(path: &str) -> String {
    format!("{LINK_DIR}/{path}")
}

/// Either storage, so that one install runs against both. One exists at a
/// time, so the size difference of the variants does not matter.
#[allow(clippy::large_enum_variant)]
pub enum AnyStorage {
    Ftp(crate::ftp::FtpStorage),
    Card(crate::card::CardStorage),
}

impl SetupStorage for AnyStorage {
    fn via(&self) -> Via {
        match self {
            Self::Ftp(s) => s.via(),
            Self::Card(s) => s.via(),
        }
    }
    async fn read(&mut self, path: &str) -> FileRead {
        match self {
            Self::Ftp(s) => s.read(path).await,
            Self::Card(s) => s.read(path).await,
        }
    }
    async fn list(&mut self, dir: &str) -> Result<Vec<String>, StorageError> {
        match self {
            Self::Ftp(s) => s.list(dir).await,
            Self::Card(s) => s.list(dir).await,
        }
    }
    async fn stat(&mut self, path: &str) -> Result<Option<FileStat>, StorageError> {
        match self {
            Self::Ftp(s) => s.stat(path).await,
            Self::Card(s) => s.stat(path).await,
        }
    }
    async fn write(&mut self, path: &str, bytes: &[u8], progress: &mut (dyn FnMut(u64) + Send)) -> Result<(), StorageError> {
        match self {
            Self::Ftp(s) => s.write(path, bytes, progress).await,
            Self::Card(s) => s.write(path, bytes, progress).await,
        }
    }
    async fn delete(&mut self, path: &str) -> Result<(), StorageError> {
        match self {
            Self::Ftp(s) => s.delete(path).await,
            Self::Card(s) => s.delete(path).await,
        }
    }
    async fn rename(&mut self, from: &str, to: &str) -> Result<(), StorageError> {
        match self {
            Self::Ftp(s) => s.rename(from, to).await,
            Self::Card(s) => s.rename(from, to).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_paths() {
        assert!(is_safe_path("pocketjs/link/companions/0123456789abcdef.txt"));
        assert!(is_safe_path("3ds/pocket-shell.3dsx"));
        for bad in ["", "/abs", "a//b", "a/../b", ".", "a/./b", "a b", "a\r\nDELE x", "a/"] {
            assert!(!is_safe_path(bad), "{bad:?}");
        }
    }
}
