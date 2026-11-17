//! SERVICE-PROTOCOL.md §7, `store`: the service's own key-value storage, one
//! SQLite database per store, written by this process alone.
//!
//! ```text
//! <home>/service-store/<id>@dev/store.sqlite   a development release's store
//! <home>/service-store/<id>/store.sqlite       the store of the id's releases from the store (none yet)
//! ```
//!
//! **The file** is made by exactly the statements of `CREATE` (§7 "On
//! disk"), in their order, the table statements in one transaction; each
//! connection then sets `synchronous = FULL`, so a commit returns only once
//! the write-ahead log is synced. A `set` or `delete` changes `entries` and
//! the one `meta` row in one transaction, inside which the quota is checked
//! against `meta.bytes`: with one connection per store behind one lock, the
//! quota is exact whoever writes.
//!
//! **A newer format** (`user_version` above `FORMAT`) is refused before the
//! file is opened, from its header, so the file is not touched; a version
//! that only the write-ahead log holds is caught by the `PRAGMA` after the
//! open, which changes nothing either.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};

/// The database of a store, in its folder.
pub const STORE_FILE: &str = "store.sqlite";
/// The `user_version` this host writes and reads.
pub const FORMAT: i64 = 1;
/// A key: 1 to 256 bytes of UTF-8.
pub const MAX_KEY_BYTES: usize = 256;
/// A value: at most 4 MiB.
pub const MAX_VALUE_BYTES: u64 = 4 * 1024 * 1024;
/// A `list` page: at most 1,000 keys.
pub const MAX_LIST: usize = 1_000;
/// After a commit that leaves the write-ahead log over this, it is
/// checkpointed and truncated.
const WAL_CHECKPOINT_BYTES: u64 = 4 * 1024 * 1024;

/// The statements that make a store's file, as §7 writes them.
const CREATE: &str = "\
PRAGMA auto_vacuum = INCREMENTAL;
PRAGMA journal_mode = WAL;
BEGIN;
CREATE TABLE entries (
  key   BLOB PRIMARY KEY NOT NULL,
  value BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE meta (
  one   INTEGER PRIMARY KEY CHECK (one = 1),
  bytes INTEGER NOT NULL CHECK (bytes >= 0),
  keys  INTEGER NOT NULL CHECK (keys >= 0)
);
INSERT INTO meta (one, bytes, keys) VALUES (1, 0, 0);
PRAGMA user_version = 1;
COMMIT;";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A newer Pocket Hub wrote the store: it is not read or changed.
    #[error("the store was written by a newer Pocket Hub (format {0}; this one reads format {FORMAT})")]
    Format(i64),
    /// The write would raise `meta.bytes` past the limit; nothing was written.
    #[error("the write would take the store to {would} bytes, over its limit of {max}")]
    Quota { would: u64, max: u64 },
    /// The file is a database but not a store.
    #[error("{0} is not a store")]
    NotAStore(PathBuf),
    #[error("the store: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("the store's folder: {0}")]
    Io(#[from] io::Error),
}

/// `meta`: what `usage` answers and `Service.store` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Meta {
    /// The sum of every key's and value's length.
    pub bytes: u64,
    pub keys: u64,
}

/// One open store: one connection, behind one lock.
pub struct Store {
    folder: PathBuf,
    connection: Mutex<Connection>,
}

/// The `user_version` in a database file's header (offset 60, big-endian),
/// read without opening it; None for a file too short to have a header.
pub fn header_format(file: &Path) -> io::Result<Option<i64>> {
    let mut header = [0u8; 100];
    let mut opened = fs::File::open(file)?;
    let mut read = 0;
    while read < header.len() {
        match opened.read(&mut header[read..])? {
            0 => return Ok(None),
            n => read += n,
        }
    }
    if &header[..16] != b"SQLite format 3\0" {
        return Ok(None);
    }
    Ok(Some(i32::from_be_bytes([header[60], header[61], header[62], header[63]]) as i64))
}

/// Syncs a folder's entries (`fsync` of the folder).
fn sync_folder(folder: &Path) -> io::Result<()> {
    fs::File::open(folder)?.sync_all()
}

/// Makes `folder` with mode 0700 when it is missing.
fn private_folder(folder: &Path) -> io::Result<()> {
    match fs::create_dir(folder) {
        Ok(()) => fs::set_permissions(folder, fs::Permissions::from_mode(0o700)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

impl Store {
    /// Opens the store in `folder`, making the folder (0700), the file (0600)
    /// and its tables when they do not exist. A store of a newer format is
    /// refused untouched.
    pub fn open(folder: &Path) -> Result<Store, StoreError> {
        let file = folder.join(STORE_FILE);
        let existed = file.exists();
        if existed && let Some(format) = header_format(&file)? && format > FORMAT {
            return Err(StoreError::Format(format));
        }
        if !existed {
            if let Some(root) = folder.parent() {
                private_folder(root)?;
            }
            private_folder(folder)?;
        }
        let connection = Connection::open_with_flags(&file, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        if !existed {
            // Before the first write: SQLite gives the write-ahead log and
            // its index the database's own mode.
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
        }
        let format: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if format > FORMAT {
            return Err(StoreError::Format(format));
        }
        if format == 0 {
            // A file whose making never committed holds no table yet: it is
            // made now. One with tables and no version is not a store.
            let tables: i64 = connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
            if tables != 0 {
                return Err(StoreError::NotAStore(file));
            }
            connection.execute_batch(CREATE)?;
            sync_folder(folder)?;
        }
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(Store { folder: folder.to_path_buf(), connection: Mutex::new(connection) })
    }

    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        let connection = self.connection();
        Ok(connection.query_row("SELECT value FROM entries WHERE key = ?1", [key], |row| row.get(0)).optional()?)
    }

    /// Replaces `key`'s value. Refused with `Quota`, writing nothing, when
    /// `meta.bytes` would pass `max` and rise: a write that lowers it, or
    /// leaves it as it is, succeeds even above the limit (§7, "A limit below
    /// the usage").
    pub fn set(&self, key: &[u8], value: &[u8], max: u64) -> Result<(), StoreError> {
        let mut connection = self.connection();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<i64> = transaction.query_row("SELECT length(value) FROM entries WHERE key = ?1", [key], |row| row.get(0)).optional()?;
        let (bytes, keys): (i64, i64) = transaction.query_row("SELECT bytes, keys FROM meta WHERE one = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let key_bytes = key.len() as i64;
        let would = bytes - old.map_or(0, |old| key_bytes + old) + key_bytes + value.len() as i64;
        if would > max as i64 && would > bytes {
            return Err(StoreError::Quota { would: would as u64, max });
        }
        transaction.execute("INSERT OR REPLACE INTO entries (key, value) VALUES (?1, ?2)", (key, value))?;
        transaction.execute("UPDATE meta SET bytes = ?1, keys = ?2 WHERE one = 1", (would, keys + i64::from(old.is_none())))?;
        transaction.commit()?;
        self.after_commit(&connection);
        Ok(())
    }

    /// Removes `key`; whether it existed.
    pub fn delete(&self, key: &[u8]) -> Result<bool, StoreError> {
        let mut connection = self.connection();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<i64> = transaction.query_row("SELECT length(value) FROM entries WHERE key = ?1", [key], |row| row.get(0)).optional()?;
        let Some(old) = old else { return Ok(false) };
        transaction.execute("DELETE FROM entries WHERE key = ?1", [key])?;
        transaction.execute("UPDATE meta SET bytes = bytes - ?1, keys = keys - 1 WHERE one = 1", [key.len() as i64 + old])?;
        transaction.commit()?;
        self.after_commit(&connection);
        Ok(true)
    }

    /// Up to `limit` keys that start with `prefix` and sort after `after`,
    /// in byte order, and whether a later one exists now.
    pub fn list(&self, prefix: &[u8], after: Option<&[u8]>, limit: usize) -> Result<(Vec<Vec<u8>>, bool), StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare_cached(
            "SELECT key FROM entries WHERE key >= ?1 AND substr(key, 1, ?2) = ?1 AND (?3 IS NULL OR key > ?3) ORDER BY key LIMIT ?4",
        )?;
        let rows = statement.query_map((prefix, prefix.len() as i64, after, limit as i64 + 1), |row| row.get::<_, Vec<u8>>(0))?;
        let mut keys = rows.collect::<Result<Vec<_>, _>>()?;
        let more = keys.len() > limit;
        keys.truncate(limit);
        Ok((keys, more))
    }

    pub fn meta(&self) -> Result<Meta, StoreError> {
        let connection = self.connection();
        let (bytes, keys): (i64, i64) = connection.query_row("SELECT bytes, keys FROM meta WHERE one = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(Meta { bytes: bytes as u64, keys: keys as u64 })
    }

    /// The database and its write-ahead log, in bytes: what the store takes
    /// on disk beside `meta.bytes`.
    pub fn file_bytes(&self) -> u64 {
        file_bytes(&self.folder)
    }

    /// §7 "Disk use": the free pages go back after a commit that leaves more
    /// than a quarter of them free, and the write-ahead log is checkpointed
    /// and truncated after a commit that leaves it over 4 MiB. A failure here
    /// leaves the commit as it is.
    fn after_commit(&self, connection: &Connection) {
        let pages = (|| -> rusqlite::Result<(i64, i64)> {
            Ok((connection.query_row("PRAGMA freelist_count", [], |row| row.get(0))?, connection.query_row("PRAGMA page_count", [], |row| row.get(0))?))
        })();
        match pages {
            Ok((free, total)) if free * 4 > total => {
                if let Err(error) = connection.execute_batch("PRAGMA incremental_vacuum;") {
                    tracing::warn!(folder = %self.folder.display(), %error, "the store's free pages were not returned");
                }
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(folder = %self.folder.display(), %error, "the store's pages could not be counted"),
        }
        let wal = fs::metadata(self.folder.join(format!("{STORE_FILE}-wal"))).map(|meta| meta.len()).unwrap_or(0);
        if wal > WAL_CHECKPOINT_BYTES
            && let Err(error) = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        {
            tracing::warn!(folder = %self.folder.display(), %error, "the store's write-ahead log was not checkpointed");
        }
    }
}

fn file_bytes(folder: &Path) -> u64 {
    [STORE_FILE.to_owned(), format!("{STORE_FILE}-wal")].iter().filter_map(|name| fs::metadata(folder.join(name)).ok()).map(|meta| meta.len()).sum()
}

/// What `Service.store` shows of one store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreUse {
    /// None for a store a newer Pocket Hub wrote.
    pub meta: Option<Meta>,
    pub file_bytes: u64,
}

/// Every store of the home, opened when first used and kept open.
pub struct Stores {
    root: PathBuf,
    open: Mutex<HashMap<String, Arc<Store>>>,
}

impl Stores {
    pub fn new(home: &Path) -> Self {
        Self { root: home.join("service-store"), open: Mutex::new(HashMap::new()) }
    }

    /// The store folder's name for a release of `id` (§1): `<id>@dev` for a
    /// development release, which has a store of its own, else `<id>`.
    pub fn name(id: &str, release: &str) -> String {
        if release == crate::registry::DEV_RELEASE { format!("{id}@dev") } else { id.to_owned() }
    }

    pub fn folder(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Store>>> {
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The store `name`, opened (and made) now if it is not open. Blocking.
    /// The table stays locked while a store opens: two first calls at once
    /// must not both make its file.
    pub fn get(&self, name: &str) -> Result<Arc<Store>, StoreError> {
        let mut table = self.table();
        if let Some(store) = table.get(name) {
            return Ok(store.clone());
        }
        let store = Arc::new(Store::open(&self.folder(name))?);
        table.insert(name.to_owned(), store.clone());
        Ok(store)
    }

    /// What `Service.store` shows of `name`, without making a store that
    /// does not exist yet. Blocking.
    pub fn usage(&self, name: &str) -> Result<StoreUse, StoreError> {
        let open = self.table().get(name).cloned();
        let store = match open {
            Some(store) => store,
            None if !self.folder(name).join(STORE_FILE).exists() => return Ok(StoreUse { meta: Some(Meta::default()), file_bytes: 0 }),
            None => match self.get(name) {
                Ok(store) => store,
                Err(StoreError::Format(_)) => return Ok(StoreUse { meta: None, file_bytes: file_bytes(&self.folder(name)) }),
                Err(error) => return Err(error),
            },
        };
        Ok(StoreUse { meta: Some(store.meta()?), file_bytes: store.file_bytes() })
    }

    /// Whether `name` is a store a newer Pocket Hub wrote, without making
    /// one. Blocking.
    pub fn is_newer(&self, name: &str) -> bool {
        matches!(self.usage(name), Ok(StoreUse { meta: None, .. }))
    }

    /// Closes and deletes every store of `id`. The caller has stopped its
    /// processes.
    pub fn purge(&self, id: &str) -> io::Result<()> {
        for name in [id.to_owned(), format!("{id}@dev")] {
            self.table().remove(&name);
            match fs::remove_dir_all(self.folder(&name)) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }
}
