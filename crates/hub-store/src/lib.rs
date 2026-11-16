//! What Hub keeps on disk (docs/POCKET-LINK.md §3.6): its own id and name,
//! one record per paired device, and the Hub-wide channel generation.
//!
//! The folder is mode 0700 and every file 0600, written through a temporary
//! file and a rename. No key appears in a log line, a status answer or the
//! control API.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pocket_link::text::{encoded_prefix, new_link_id};
use serde::{Deserialize, Serialize};

/// One paired device.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRecord {
    pub id: String,
    /// The device family: `3ds` for a pairing a setup wrote, the family of
    /// the device's target for a pairing by a code (`tab5`), empty for a
    /// family Hub does not know.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model: String,
    /// The link key, 64 lowercase hex digits. Never leaves this file.
    pub key: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub host_abi: u32,
    #[serde(default)]
    pub last_address: Option<String>,
    /// Epoch milliseconds of the last `ready`.
    #[serde(default)]
    pub last_seen: Option<i64>,
    /// A setup wrote this key and its final readback did not return (§3.2).
    /// A `ready` or a later setup settles it.
    #[serde(default)]
    pub setup_unconfirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Identity {
    id: String,
    name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Devices {
    #[serde(default)]
    last_generation: u32,
    #[serde(default)]
    devices: Vec<DeviceRecord>,
}

struct Inner {
    identity: Identity,
    last_generation: u32,
    devices: BTreeMap<String, DeviceRecord>,
}

pub struct Store {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

/// The computer's name as its owner set it, for Hub's display name.
fn computer_name() -> String {
    let from = |program: &str, args: &[&str]| {
        let output = std::process::Command::new(program).args(args).output().ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        let text = text.trim().trim_end_matches(".local");
        (output.status.success() && !text.is_empty()).then(|| text.to_owned())
    };
    from("scutil", &["--get", "ComputerName"]).or_else(|| from("hostname", &["-s"])).unwrap_or_else(|| "Pocket Hub".to_owned())
}

/// Writes `text` to `path` with mode 0600 through a temporary file and a rename.
pub fn write_secret(path: &Path, text: &str) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    {
        let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&temporary, path)
}

impl Store {
    /// Opens the store in `dir`, making the folder, Hub's id and its name on
    /// first use.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        Self::open_named(dir, None)
    }

    /// `open`, with the name a new Hub takes: `name` when given, else the
    /// computer's. A store that already has its identity keeps its name.
    pub fn open_named(dir: impl Into<PathBuf>, name: Option<&str>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        let identity_path = dir.join("hub.json");
        let identity = match fs::read_to_string(&identity_path) {
            Ok(text) => serde_json::from_str::<Identity>(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("hub.json: {error}")))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let identity = Identity { id: new_link_id(), name: name.map(str::to_owned).unwrap_or_else(computer_name) };
                write_secret(&identity_path, &serde_json::to_string_pretty(&identity)?)?;
                identity
            }
            Err(error) => return Err(error),
        };
        let devices = match fs::read_to_string(dir.join("devices.json")) {
            Ok(text) => serde_json::from_str::<Devices>(&text).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("devices.json: {error}")))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Devices::default(),
            Err(error) => return Err(error),
        };
        let inner = Inner {
            identity,
            last_generation: devices.last_generation,
            devices: devices.devices.into_iter().map(|record| (record.id.clone(), record)).collect(),
        };
        Ok(Self { dir, inner: Mutex::new(inner) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Writes `devices.json` as it will be after a change. Callers change
    /// their copy in memory only once this returned `Ok`, so a failed write
    /// leaves the file and the memory as they were.
    fn save(&self, last_generation: u32, devices: &BTreeMap<String, DeviceRecord>) -> io::Result<()> {
        let file = Devices { last_generation, devices: devices.values().cloned().collect() };
        write_secret(&self.dir.join("devices.json"), &serde_json::to_string_pretty(&file)?)
    }

    pub fn hub_id(&self) -> String {
        self.inner.lock().unwrap().identity.id.clone()
    }

    /// Hub's display name, at most 64 encoded bytes (§3.4).
    pub fn hub_name(&self) -> String {
        let inner = self.inner.lock().unwrap();
        encoded_prefix(&inner.identity.name, pocket_link::consts::NAME_BYTES).to_owned()
    }

    pub fn set_hub_name(&self, name: &str) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let identity = Identity { id: inner.identity.id.clone(), name: name.to_owned() };
        write_secret(&self.dir.join("hub.json"), &serde_json::to_string_pretty(&identity)?)?;
        inner.identity = identity;
        Ok(())
    }

    pub fn device(&self, id: &str) -> Option<DeviceRecord> {
        self.inner.lock().unwrap().devices.get(id).cloned()
    }

    pub fn devices(&self) -> Vec<DeviceRecord> {
        self.inner.lock().unwrap().devices.values().cloned().collect()
    }

    /// Stores a record, replacing the one of the same id.
    pub fn put_device(&self, record: DeviceRecord) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let mut devices = inner.devices.clone();
        devices.insert(record.id.clone(), record);
        self.save(inner.last_generation, &devices)?;
        inner.devices = devices;
        Ok(())
    }

    /// Changes one record. False when Hub holds no such device. The change
    /// is made on a copy, which replaces the record once it is on disk.
    pub fn update_device(&self, id: &str, change: impl FnOnce(&mut DeviceRecord)) -> io::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        let Some(record) = inner.devices.get(id) else { return Ok(false) };
        let mut record = record.clone();
        change(&mut record);
        let mut devices = inner.devices.clone();
        devices.insert(id.to_owned(), record);
        self.save(inner.last_generation, &devices)?;
        inner.devices = devices;
        Ok(true)
    }

    pub fn remove_device(&self, id: &str) -> io::Result<bool> {
        let mut inner = self.inner.lock().unwrap();
        if !inner.devices.contains_key(id) {
            return Ok(false);
        }
        let mut devices = inner.devices.clone();
        devices.remove(id);
        self.save(inner.last_generation, &devices)?;
        inner.devices = devices;
        Ok(true)
    }

    /// The next channel generation (§7.4): larger than every one given
    /// before, to any device, across restarts. It is on disk before this
    /// returns, so a crash cannot hand the same generation out twice; a
    /// failed write hands none out.
    pub fn next_generation(&self) -> io::Result<u32> {
        let mut inner = self.inner.lock().unwrap();
        let next = inner.last_generation.checked_add(1).ok_or_else(|| io::Error::other("channel generations are exhausted"))?;
        self.save(next, &inner.devices)?;
        inner.last_generation = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pocket-hub-store-{}", new_link_id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_given_name_applies_to_a_new_hub_only() {
        let dir = temp();
        let store = Store::open_named(&dir, Some("Studio Mac")).unwrap();
        assert_eq!(store.hub_name(), "Studio Mac");
        let id = store.hub_id();
        drop(store);
        let again = Store::open_named(&dir, Some("Other")).unwrap();
        assert_eq!((again.hub_name().as_str(), again.hub_id()), ("Studio Mac", id));
    }

    #[test]
    fn identity_records_and_generation_persist() {
        let dir = temp();
        let store = Store::open(&dir).unwrap();
        let id = store.hub_id();
        assert!(pocket_link::text::is_link_id(&id));
        assert_eq!(store.next_generation().unwrap(), 1);
        assert_eq!(store.next_generation().unwrap(), 2);
        store
            .put_device(DeviceRecord {
                id: "0123456789abcdef".into(),
                kind: "3ds".into(),
                name: String::new(),
                model: String::new(),
                key: "00".repeat(32),
                target: String::new(),
                host_abi: 0,
                last_address: None,
                last_seen: None,
                setup_unconfirmed: true,
            })
            .unwrap();
        drop(store);

        let store = Store::open(&dir).unwrap();
        assert_eq!(store.hub_id(), id);
        assert_eq!(store.next_generation().unwrap(), 3);
        assert!(store.device("0123456789abcdef").unwrap().setup_unconfirmed);
        let mode = fs::metadata(dir.join("devices.json")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn record(id: &str) -> DeviceRecord {
        DeviceRecord {
            id: id.into(),
            kind: "3ds".into(),
            name: String::new(),
            model: String::new(),
            key: "11".repeat(32),
            target: String::new(),
            host_abi: 0,
            last_address: None,
            last_seen: None,
            setup_unconfirmed: false,
        }
    }

    /// A write that fails changes neither the file nor what the store answers.
    #[test]
    fn a_failed_write_changes_nothing() {
        let dir = temp();
        let store = Store::open(&dir).unwrap();
        store.put_device(record("0123456789abcdef")).unwrap();
        assert_eq!(store.next_generation().unwrap(), 1);
        let before = fs::read_to_string(dir.join("devices.json")).unwrap();

        // The temporary file cannot be made in a read-only folder.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let probe = dir.join("probe");
        if fs::write(&probe, "x").is_ok() {
            // Running as root: permissions do not stop writes, so the case cannot be made.
            let _ = fs::remove_file(&probe);
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            fs::remove_dir_all(&dir).unwrap();
            return;
        }
        assert!(store.put_device(record("fedcba9876543210")).is_err());
        assert!(store.device("fedcba9876543210").is_none());
        assert!(store.update_device("0123456789abcdef", |r| r.name = "changed".into()).is_err());
        assert_eq!(store.device("0123456789abcdef").unwrap().name, "");
        assert!(store.remove_device("0123456789abcdef").is_err());
        assert!(store.device("0123456789abcdef").is_some());
        assert!(store.next_generation().is_err());
        assert!(store.set_hub_name("Other").is_err());
        assert_ne!(store.hub_name(), "Other");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(fs::read_to_string(dir.join("devices.json")).unwrap(), before);
        // The generation that failed was not handed out.
        assert_eq!(store.next_generation().unwrap(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }
}
