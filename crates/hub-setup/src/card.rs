//! The setup's storage on a 3DS SD card mounted in the computer's reader
//! (docs/POCKET-LINK.md §3.2): the same procedure as over ftpd, on files.
//!
//! On a mounted card Hub cannot tell which of its interfaces faces the
//! console, so the pairing takes every current interface address, the
//! interface of the default route first (§3.2 step 3).

use std::io::{self, ErrorKind, Write};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::storage::{FileRead, FileStat, SetupStorage, StorageError, Via, check_path};

const CHUNK: usize = 64 * 1024;

/// The IPv4 addresses of this computer's LAN interfaces, the interface of the
/// default route first. The default route's address is the local address of
/// a UDP socket connected to a TEST-NET address: `connect` on UDP sends
/// nothing and only selects the route. The list is `hub-core`'s
/// `beacon::interface_addresses` with that one reordering.
pub fn interface_addresses() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else { return Vec::new() };
    let mut addresses: Vec<Ipv4Addr> = interfaces
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .filter_map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(v4) if !v4.ip.is_link_local() => Some(v4.ip),
            _ => None,
        })
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    addresses.retain(|address| seen.insert(*address));
    if let Some(default) = default_route_address()
        && let Some(at) = addresses.iter().position(|address| *address == default) {
            let first = addresses.remove(at);
            addresses.insert(0, first);
        }
    addresses
}

fn default_route_address() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(address) if !address.is_unspecified() => Some(address),
        _ => None,
    }
}

/// A mounted SD card: `root` is the card's root folder (`/Volumes/<name>` on
/// macOS).
pub struct CardStorage {
    root: PathBuf,
    addresses: Vec<String>,
}

impl CardStorage {
    /// The card at `root`, with Hub's current interface addresses.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_addresses(root, interface_addresses().iter().map(Ipv4Addr::to_string).collect())
    }

    /// The card at `root` with the addresses the pairing takes, the one to
    /// write first.
    pub fn with_addresses(root: impl Into<PathBuf>, addresses: Vec<String>) -> Self {
        Self { root: root.into(), addresses }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether `root` looks like a 3DS's card: it holds the console's
    /// `Nintendo 3DS` folder or the homebrew `3ds` folder.
    pub fn looks_like_3ds_card(root: &Path) -> bool {
        root.join("Nintendo 3DS").is_dir() || root.join("3ds").is_dir()
    }

    fn path(&self, path: &str) -> Result<PathBuf, StorageError> {
        check_path(path)?;
        Ok(self.root.join(path))
    }
}

fn describe(path: &str, error: &io::Error) -> StorageError {
    StorageError(format!("{path}: {error}"))
}

fn write_file(target: &Path, bytes: &[u8], progress: &mut (dyn FnMut(u64) + Send)) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(target)?;
    let mut written = 0u64;
    for chunk in bytes.chunks(CHUNK) {
        file.write_all(chunk)?;
        written += chunk.len() as u64;
        progress(written);
    }
    // The owner pulls the card out after the setup: the bytes must be on it.
    file.sync_all()
}

impl SetupStorage for CardStorage {
    fn via(&self) -> Via {
        Via::Card { addresses: self.addresses.clone() }
    }

    async fn read(&mut self, path: &str) -> FileRead {
        let target = match self.path(path) {
            Ok(target) => target,
            Err(error) => return FileRead::Failed(error),
        };
        match tokio::fs::read(&target).await {
            Ok(bytes) => FileRead::File(bytes),
            // "No such file" from the card is the absent outcome; any other
            // error (permissions, I/O, a folder in the way) says nothing.
            Err(error) if error.kind() == ErrorKind::NotFound => FileRead::Absent,
            Err(error) => FileRead::Failed(describe(path, &error)),
        }
    }

    async fn list(&mut self, dir: &str) -> Result<Vec<String>, StorageError> {
        let target = self.path(dir)?;
        let mut entries = match tokio::fs::read_dir(&target).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(describe(dir, &error)),
        };
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|error| describe(dir, &error))? {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        Ok(names)
    }

    async fn stat(&mut self, path: &str) -> Result<Option<FileStat>, StorageError> {
        let target = self.path(path)?;
        match tokio::fs::metadata(&target).await {
            Ok(metadata) if metadata.is_file() => {
                let modified = metadata.modified().ok().and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map(|at| at.as_nanos().to_string());
                Ok(Some(FileStat { size: metadata.len(), modified }))
            }
            Ok(_) => Err(StorageError(format!("{path} is not a file"))),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(describe(path, &error)),
        }
    }

    async fn write(&mut self, path: &str, bytes: &[u8], progress: &mut (dyn FnMut(u64) + Send)) -> Result<(), StorageError> {
        let target = self.path(path)?;
        // `block_in_place` keeps the caller's progress callback on this task
        // without copying the bytes; it needs the multi-threaded runtime, and
        // a current-thread runtime writes inline.
        let mut run = move || write_file(&target, bytes, progress);
        let result = match tokio::runtime::Handle::try_current().map(|handle| handle.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(run),
            _ => run(),
        };
        result.map_err(|error| describe(path, &error))
    }

    async fn delete(&mut self, path: &str) -> Result<(), StorageError> {
        let target = self.path(path)?;
        match tokio::fs::remove_file(&target).await {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(describe(path, &error)),
        }
        match self.read(path).await {
            FileRead::Absent => Ok(()),
            FileRead::File(_) => Err(StorageError(format!("{path} is still there after its delete"))),
            FileRead::Failed(error) => Err(error),
        }
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<(), StorageError> {
        let (source, target) = (self.path(from)?, self.path(to)?);
        tokio::fs::rename(&source, &target).await.map_err(|error| StorageError(format!("{from} could not be renamed to {to}: {error}")))?;
        // The rename is on the card when its folder is synced; a card pulled
        // out right after must not lose it.
        if let Some(parent) = target.parent()
            && let Ok(folder) = std::fs::File::open(parent) {
                let _ = folder.sync_all();
            }
        Ok(())
    }
}
