//! §3.1, §3.2, §3.6: the pairing files and Hub's setup.
//!
//! A pairing is two files on the device and one record in Hub: `device.txt`
//! and `companions/<hub id>.txt`, with a `.last` file the device keeps beside
//! the key file. Hub's setup is a plan of storage operations made from
//! completed reads; the caller does the reads and writes (over ftpd, or on the
//! mounted card) and reports their outcomes.

use std::net::Ipv4Addr;

use crate::consts::*;
use crate::text::{is_key, is_link_id};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionFile {
    pub key: String,
    pub name: String,
    pub address: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastFile {
    pub name: String,
    pub address: String,
    pub port: u16,
}

fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

pub fn format_device_file(id: &str) -> String {
    format!("{id}\n")
}

/// Hub's reading at a setup (§3.2 step 1): 16 lowercase hex digits with any
/// trailing run of newline, carriage return and space, which is what the
/// device's own reader strips. Anything else gets a new id.
pub fn parse_device_file(text: &str) -> Option<&str> {
    let id = text.trim_end_matches(['\n', '\r', ' ']);
    is_link_id(id).then_some(id)
}

pub fn format_companion_file(file: &CompanionFile) -> String {
    format!("key={}\nname={}\naddress={}\nport={}\n", file.key, one_line(&file.name), file.address, file.port)
}

pub fn format_last_file(file: &LastFile) -> String {
    format!("name={}\naddress={}\nport={}\n", one_line(&file.name), file.address, file.port)
}

/// `key=value` lines in any order; a later line of a key replaces an earlier.
fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut found = None;
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some((name, value)) = line.split_once('=')
            && name == key
        {
            found = Some(value);
        }
    }
    found
}

/// An IPv4 address in dotted decimal with no leading zeros, or "".
fn address_of(text: &str) -> &str {
    let value = field(text, "address").unwrap_or("");
    let canonical = value.parse::<Ipv4Addr>().is_ok_and(|parsed| parsed.to_string() == value);
    if canonical { value } else { "" }
}

/// A port of 1 to 65535 in at most 5 digits, or 0.
fn port_of(text: &str) -> u16 {
    let value = field(text, "port").unwrap_or("");
    if value.is_empty() || value.len() > 5 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    value.parse::<u32>().ok().filter(|port| (1..=65535).contains(port)).map_or(0, |port| port as u16)
}

/// The device's reading of a key file: lines in any order, a final newline
/// optional, unknown lines skipped. None without a `key=` of 64 lowercase hex
/// digits. A missing or bad port reads as the link's default port.
pub fn parse_companion_file(text: &str) -> Option<CompanionFile> {
    let key = field(text, "key").filter(|key| is_key(key))?;
    let port = port_of(text);
    Some(CompanionFile {
        key: key.to_owned(),
        name: field(text, "name").unwrap_or("").to_owned(),
        address: address_of(text).to_owned(),
        port: if port == 0 { PORT } else { port },
    })
}

/// A `.last` file parses when it holds an IPv4 address and a port.
pub fn parse_last_file(text: &str) -> Option<LastFile> {
    let address = address_of(text);
    let port = port_of(text);
    if address.is_empty() || port == 0 {
        return None;
    }
    Some(LastFile { name: field(text, "name").unwrap_or("").to_owned(), address: address.to_owned(), port })
}

/// The Hub id a key file's name carries: 16 lowercase hex digits and `.txt`.
fn key_file_id(name: &str) -> Option<&str> {
    let id = name.strip_suffix(KEY_SUFFIX)?;
    is_link_id(id).then_some(id)
}

/// One file of `companions/` with its text where it could be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCompanion {
    pub id: String,
    pub key: String,
    pub name: String,
    pub address: String,
    pub port: u16,
}

/// The device's load of `companions/` (§3.6): the `.txt` files in directory
/// order, at most 8, each with its `.last` file's name, address and port in
/// place of the key file's when that file parses. A ninth loadable key file is
/// ignored and reported.
pub fn load_companions(entries: &[DirectoryEntry]) -> (Vec<LoadedCompanion>, Option<&'static str>) {
    let mut companions = Vec::new();
    let mut error = None;
    for entry in entries {
        let Some(id) = key_file_id(&entry.name) else { continue };
        let Some(file) = parse_companion_file(&entry.text) else { continue };
        if companions.len() == MAX_COMPANIONS {
            error = Some("more than 8 computers");
            continue;
        }
        let last_name = format!("{id}{LAST_SUFFIX}");
        let last = entries.iter().find(|other| other.name == last_name).and_then(|other| parse_last_file(&other.text));
        companions.push(match last {
            Some(last) => LoadedCompanion { id: id.to_owned(), key: file.key, name: last.name, address: last.address, port: last.port },
            None => LoadedCompanion { id: id.to_owned(), key: file.key, name: file.name, address: file.address, port: file.port },
        });
    }
    (companions, error)
}

// ---------------------------------------------------------------------------
// Hub: the setup (§3.2)
// ---------------------------------------------------------------------------

/// A read's three outcomes, which Hub tells apart. A failed read says nothing
/// about the file, so the record's key is not replaced for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    Failed,
    Absent,
    File(String),
}

/// How Hub reaches the device's storage, and the address it writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    /// Over ftpd: the FTP connection's local address, the one this device
    /// reaches Hub at.
    Ftp { local_address: String },
    /// On a mounted card: every current interface address of Hub, the one to
    /// write first.
    Card { addresses: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct SetupInput {
    pub hub_id: String,
    /// Hub's key for this device before the setup.
    pub key: Option<String>,
    pub hub_name: String,
    pub hub_port: u16,
    pub via: Via,
    pub device: ReadOutcome,
    /// `companions/` as listed, with each file's text where it could be read.
    pub listing: Vec<DirectoryEntry>,
    pub companion: ReadOutcome,
    pub last: ReadOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageOp {
    Write { path: String, text: String },
    /// Read `path` and compare it with `text` byte for byte.
    Readback { path: String, text: String },
    Delete { path: String },
    Rename { from: String, to: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupOutcome {
    /// A read failed: the setup ends and the record stays as it was.
    FailedRead,
    /// The device already holds 8 computers: nothing is written.
    EightComputers,
    /// The pairing is in place.
    Kept,
    /// The last address was rewritten; the key file is untouched.
    Address,
    /// A new key is written.
    Key,
}

impl SetupOutcome {
    pub fn name(self) -> &'static str {
        match self {
            Self::FailedRead => "failed-read",
            Self::EightComputers => "eight-computers",
            Self::Kept => "kept",
            Self::Address => "address",
            Self::Key => "key",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupPlan {
    pub outcome: SetupOutcome,
    pub device_id: Option<String>,
    /// In order; the caller stops at the first that fails and reports the
    /// setup as failed.
    pub ops: Vec<StorageOp>,
    /// Hub's key for the device once the plan is stored, **before any write**:
    /// the minted key when the plan writes one.
    pub key: Option<String>,
    /// The key this plan mints and its final readback confirms.
    pub minted: Option<String>,
    pub message: &'static str,
}

fn companion_path(hub_id: &str, suffix: &str) -> String {
    format!("{COMPANIONS_DIR}/{hub_id}{suffix}")
}

/// Hub's procedure against the device's storage as a plan. The record is
/// touched only after completed reads: a failed read ends the setup with the
/// key as it was. A key is minted only when the card holds no key file with
/// the record's key, so the key it replaces opens nothing; the minted key is
/// the record's key before the first write.
pub fn plan_setup(input: &SetupInput, mint_device_id: impl FnOnce() -> String, mint_key: impl FnOnce() -> String) -> SetupPlan {
    let failed = |message| SetupPlan { outcome: SetupOutcome::FailedRead, device_id: None, ops: Vec::new(), key: input.key.clone(), minted: None, message };
    if input.device == ReadOutcome::Failed {
        return failed("device.txt could not be read");
    }
    if input.companion == ReadOutcome::Failed {
        return failed("the key file could not be read");
    }
    if input.last == ReadOutcome::Failed {
        return failed("the last-address file could not be read");
    }

    let mut ops = Vec::new();
    let read_id = match &input.device {
        ReadOutcome::File(text) => parse_device_file(text).map(str::to_owned),
        _ => None,
    };
    let device_id = read_id.unwrap_or_else(|| {
        let id = mint_device_id();
        ops.push(StorageOp::Write { path: DEVICE_FILE.to_owned(), text: format_device_file(&id) });
        id
    });

    let own_key_file = format!("{}{KEY_SUFFIX}", input.hub_id);
    let loadable = |entry: &&DirectoryEntry| key_file_id(&entry.name).is_some() && parse_companion_file(&entry.text).is_some();
    let loadable_count = input.listing.iter().filter(loadable).count();
    let own_is_loadable = input.listing.iter().filter(loadable).any(|entry| entry.name == own_key_file);
    if loadable_count >= MAX_COMPANIONS && !own_is_loadable {
        return SetupPlan {
            outcome: SetupOutcome::EightComputers,
            device_id: Some(device_id),
            ops: Vec::new(),
            key: input.key.clone(),
            minted: None,
            message: "the device already holds 8 computers: remove one on the device, or delete one of its files",
        };
    }
    let leftover = format!("{}{NEW_SUFFIX}", input.hub_id);
    if input.listing.iter().any(|entry| entry.name == leftover) {
        ops.push(StorageOp::Delete { path: companion_path(&input.hub_id, NEW_SUFFIX) });
    }

    let on_card = match &input.companion {
        ReadOutcome::File(text) => parse_companion_file(text),
        _ => None,
    };
    let address = match &input.via {
        Via::Ftp { local_address } => local_address.clone(),
        Via::Card { addresses } => addresses.first().cloned().unwrap_or_default(),
    };

    if let (Some(on_card), Some(keep)) = (&on_card, &input.key)
        && &on_card.key == keep
    {
        let last = match &input.last {
            ReadOutcome::File(text) => parse_last_file(text),
            _ => None,
        };
        let (used_address, used_port) = match &last {
            Some(last) => (last.address.as_str(), last.port),
            None => (on_card.address.as_str(), on_card.port),
        };
        let own = match &input.via {
            Via::Ftp { local_address } => used_address == local_address,
            Via::Card { addresses } => addresses.iter().any(|a| a == used_address),
        };
        if own && used_port == input.hub_port {
            return SetupPlan { outcome: SetupOutcome::Kept, device_id: Some(device_id), ops, key: input.key.clone(), minted: None, message: "the pairing is in place" };
        }
        let text = format_last_file(&LastFile { name: input.hub_name.clone(), address, port: input.hub_port });
        let temporary = companion_path(&input.hub_id, &format!("{LAST_SUFFIX}.tmp"));
        let last_path = companion_path(&input.hub_id, LAST_SUFFIX);
        ops.push(StorageOp::Write { path: temporary.clone(), text: text.clone() });
        ops.push(StorageOp::Rename { from: temporary, to: last_path.clone() });
        ops.push(StorageOp::Readback { path: last_path, text });
        return SetupPlan {
            outcome: SetupOutcome::Address,
            device_id: Some(device_id),
            ops,
            key: input.key.clone(),
            minted: None,
            message: "the last address was rewritten; the key file is untouched",
        };
    }

    let minted = mint_key();
    let text = format_companion_file(&CompanionFile { key: minted.clone(), name: input.hub_name.clone(), address, port: input.hub_port });
    let fresh = companion_path(&input.hub_id, NEW_SUFFIX);
    let final_path = companion_path(&input.hub_id, KEY_SUFFIX);
    ops.push(StorageOp::Write { path: fresh.clone(), text: text.clone() });
    ops.push(StorageOp::Readback { path: fresh.clone(), text: text.clone() });
    ops.push(StorageOp::Delete { path: companion_path(&input.hub_id, LAST_SUFFIX) });
    ops.push(StorageOp::Delete { path: final_path.clone() });
    ops.push(StorageOp::Rename { from: fresh, to: final_path.clone() });
    ops.push(StorageOp::Readback { path: final_path, text });
    ops.push(StorageOp::Readback { path: DEVICE_FILE.to_owned(), text: format_device_file(&device_id) });
    SetupPlan { outcome: SetupOutcome::Key, device_id: Some(device_id), ops, key: Some(minted.clone()), minted: Some(minted), message: "a new key is written" }
}

/// How a plan ended, given how many of its operations succeeded. A plan whose
/// every operation succeeded is done; any other end is reported as failed
/// ("setup not confirmed"). The record's key is `plan.key` either way: after a
/// cut the card holds the minted key or no key Hub knows.
pub fn setup_failed_at(plan: &SetupPlan, completed: usize) -> Option<Option<&StorageOp>> {
    match plan.outcome {
        SetupOutcome::FailedRead | SetupOutcome::EightComputers => Some(None),
        _ if completed >= plan.ops.len() => None,
        _ => Some(plan.ops.get(completed)),
    }
}
