//! The pairing setup (docs/POCKET-LINK.md §3.2): the reads, the plan of
//! `pocket_link::pairing::plan_setup`, and its operations run in order
//! against a `SetupStorage`, with Hub's record settled as the reference does
//! (`tools/pocket-link/v2/hub.ts` `setup`).
//!
//! - A failed read ends the setup with the record as it was: a failed read
//!   says nothing about the file, so the record's key is not replaced for it.
//! - Otherwise the plan's key is the record's key **before the first write**,
//!   and the record says "setup not confirmed" while the plan has operations
//!   left. A plan whose every operation succeeded (its last one is a
//!   readback) clears it; a plan cut at any step leaves it set, with the
//!   plan's key, and the report names the step.

use pocket_hub_store::{DeviceRecord, Store};
use pocket_link::consts::{COMPANIONS_DIR, DEVICE_FILE, KEY_SUFFIX, LAST_SUFFIX, MAX_COMPANIONS};
use pocket_link::pairing::{self, DirectoryEntry, ReadOutcome, SetupInput, SetupOutcome, SetupPlan, StorageOp};
use pocket_link::text::{is_link_id, new_key, new_link_id};
use serde::Serialize;

use crate::storage::{FileRead, SetupStorage, link_path};

/// How a pairing setup ended, as `SetupOutcome` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PairingOutcome {
    /// A read failed: nothing was written and Hub's record is as it was.
    FailedRead,
    /// The console already holds 8 computers: nothing was written.
    EightComputers,
    /// The pairing was in place.
    Kept,
    /// The console's last address of Hub was rewritten.
    Address,
    /// A new key was written.
    Key,
}

impl From<SetupOutcome> for PairingOutcome {
    fn from(outcome: SetupOutcome) -> Self {
        match outcome {
            SetupOutcome::FailedRead => Self::FailedRead,
            SetupOutcome::EightComputers => Self::EightComputers,
            SetupOutcome::Kept => Self::Kept,
            SetupOutcome::Address => Self::Address,
            SetupOutcome::Key => Self::Key,
        }
    }
}

/// The operation a cut setup stopped at. Paths are the card's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum FailedStep {
    Write { path: String },
    Readback { path: String },
    Delete { path: String },
    Rename { from: String, to: String },
}

impl From<&StorageOp> for FailedStep {
    fn from(op: &StorageOp) -> Self {
        match op {
            StorageOp::Write { path, .. } => Self::Write { path: link_path(path) },
            StorageOp::Readback { path, .. } => Self::Readback { path: link_path(path) },
            StorageOp::Delete { path } => Self::Delete { path: link_path(path) },
            StorageOp::Rename { from, to } => Self::Rename { from: link_path(from), to: link_path(to) },
        }
    }
}

/// What a pairing setup did. Holds no key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingReport {
    pub outcome: PairingOutcome,
    /// True when the setup has no matching final readback (or wrote nothing
    /// because a read failed or 8 computers are there): reported as failed.
    pub failed: bool,
    pub failed_at: Option<FailedStep>,
    pub device_id: Option<String>,
    /// True when Hub's record says "setup not confirmed" after this setup.
    pub unconfirmed: bool,
    /// Plain words for the owner.
    pub message: String,
    /// What went wrong, for a log or a support question.
    pub detail: Option<String>,
}

/// The reads of §3.2 steps 1 to 3, completed.
pub(crate) struct PairingReads {
    pub device: ReadOutcome,
    pub listing: Vec<DirectoryEntry>,
    pub companion: ReadOutcome,
    pub last: ReadOutcome,
    /// Why the setup cannot go on although every read the plan takes
    /// completed: the listing failed, or other computers' files could not be
    /// read and could change the count of 8.
    pub failure: Option<String>,
    /// The first failed read's reason, for the report.
    pub detail: Option<String>,
}

fn companions_path(name: &str) -> String {
    link_path(&format!("{COMPANIONS_DIR}/{name}"))
}

/// A key file the device would load (§3.6): a name of 16 lowercase hex
/// digits and `.txt`, with a `key=` line of 64 hex digits.
fn loadable(entry: &DirectoryEntry) -> bool {
    entry.name.strip_suffix(KEY_SUFFIX).is_some_and(is_link_id) && pairing::parse_companion_file(&entry.text).is_some()
}

/// §3.2 steps 1 to 3: `device.txt`, the listing of `companions/` with each
/// other key file's text (the plan counts the loadable ones), this Hub's key
/// file and its `.last`.
pub(crate) async fn read_pairing<S: SetupStorage>(storage: &mut S, hub_id: &str) -> PairingReads {
    let own = format!("{hub_id}{KEY_SUFFIX}");
    let mut detail = None;
    let mut note = |read: &FileRead| {
        if let (FileRead::Failed(error), None) = (read, &detail) {
            detail = Some(error.0.clone());
        }
    };
    let device = storage.read(&link_path(DEVICE_FILE)).await;
    note(&device);

    let names = match storage.list(&link_path(COMPANIONS_DIR)).await {
        Ok(names) => names,
        Err(error) => {
            return PairingReads {
                device: device.to_outcome(),
                listing: Vec::new(),
                companion: ReadOutcome::Failed,
                last: ReadOutcome::Failed,
                failure: Some("companions/ could not be listed".into()),
                detail: Some(error.0),
            };
        }
    };
    let mut listing = Vec::new();
    let mut unreadable = Vec::new();
    for name in names {
        let mut text = String::new();
        if name.ends_with(KEY_SUFFIX) && name != own {
            match storage.read(&companions_path(&name)).await {
                FileRead::File(bytes) => text = String::from_utf8_lossy(&bytes).into_owned(),
                // Gone between the listing and the read: holds no key.
                FileRead::Absent => {}
                read @ FileRead::Failed(_) => {
                    note(&read);
                    unreadable.push(name.clone());
                }
            }
        }
        listing.push(DirectoryEntry { name, text });
    }
    let companion = storage.read(&companions_path(&own)).await;
    note(&companion);
    let last = storage.read(&companions_path(&format!("{hub_id}{LAST_SUFFIX}"))).await;
    note(&last);
    let companion = companion.to_outcome();
    if let ReadOutcome::File(text) = &companion {
        for entry in listing.iter_mut().filter(|entry| entry.name == own) {
            entry.text = text.clone();
        }
    }

    // Another computer's key file that could not be read counts for nothing
    // in the plan. That is safe unless it could be the eighth: then the count
    // is unknown and the setup ends as a failed read, which writes nothing.
    let mut failure = None;
    if !unreadable.is_empty() {
        let own_loadable = listing.iter().any(|entry| entry.name == own && loadable(entry));
        let counted = listing.iter().filter(|entry| loadable(entry)).count();
        if !own_loadable && counted + unreadable.len() >= MAX_COMPANIONS {
            failure = Some(format!("companions/{} could not be read", unreadable[0]));
        }
    }
    PairingReads { device: device.to_outcome(), listing, companion, last: last.to_outcome(), failure, detail }
}

/// The plan for completed reads. `mint` false makes a plan to look at (the
/// install's check before it uploads): its minted id and key are empty and
/// never stored.
pub(crate) fn plan(reads: &PairingReads, store: &Store, hub_port: u16, via: pocket_link::pairing::Via, mint: bool) -> SetupPlan {
    let known = match &reads.device {
        ReadOutcome::File(text) => pairing::parse_device_file(text),
        _ => None,
    };
    let key = known.and_then(|id| store.device(id)).map(|record| record.key).filter(|key| !key.is_empty());
    let input = SetupInput {
        hub_id: store.hub_id(),
        key,
        hub_name: store.hub_name(),
        hub_port,
        via,
        device: reads.device.clone(),
        listing: reads.listing.clone(),
        companion: reads.companion.clone(),
        last: reads.last.clone(),
    };
    if mint { pairing::plan_setup(&input, new_link_id, new_key) } else { pairing::plan_setup(&input, String::new, String::new) }
}

pub(crate) fn owner_message(outcome: PairingOutcome, failed: bool) -> &'static str {
    match (outcome, failed) {
        (PairingOutcome::FailedRead, _) => "Hub could not read the console's files. Nothing was changed; check that the console is still connected and try again.",
        (PairingOutcome::EightComputers, _) => {
            "This console is already set up with 8 computers. Open Pocket Shell on the console and remove one of them in its settings, then try again."
        }
        (_, true) => "The setup was interrupted before Hub could check it. Try again; nothing else is needed.",
        (PairingOutcome::Kept, false) => "This console was already set up for this computer.",
        (PairingOutcome::Address, false) => "This console is set up for this computer; its address was updated.",
        (PairingOutcome::Key, false) => "This console is now set up for this computer.",
    }
}

/// Stores the record of `id` with `key`, keeping its other fields; creates it
/// with `kind` when Hub has none.
fn save(store: &Store, id: &str, kind: &str, key: &str, unconfirmed: bool) -> std::io::Result<()> {
    let record = match store.device(id) {
        Some(mut record) => {
            record.key = key.to_owned();
            record.setup_unconfirmed = unconfirmed;
            if record.kind.is_empty() {
                record.kind = kind.to_owned();
            }
            record
        }
        None => DeviceRecord {
            id: id.to_owned(),
            kind: kind.to_owned(),
            name: String::new(),
            model: String::new(),
            key: key.to_owned(),
            target: String::new(),
            host_abi: 0,
            last_address: None,
            last_seen: None,
            setup_unconfirmed: unconfirmed,
        },
    };
    store.put_device(record)
}

fn read_failed(device_id: Option<String>, detail: Option<String>) -> PairingReport {
    PairingReport {
        outcome: PairingOutcome::FailedRead,
        failed: true,
        failed_at: None,
        device_id,
        unconfirmed: false,
        message: owner_message(PairingOutcome::FailedRead, true).to_owned(),
        detail,
    }
}

/// Runs one operation of the plan against the card's link directory.
async fn run_op<S: SetupStorage>(storage: &mut S, op: &StorageOp) -> Result<(), String> {
    match op {
        StorageOp::Write { path, text } => storage.write(&link_path(path), text.as_bytes(), &mut |_| {}).await.map_err(|error| error.0),
        StorageOp::Readback { path, text } => match storage.read(&link_path(path)).await {
            FileRead::File(bytes) if bytes == text.as_bytes() => Ok(()),
            FileRead::File(_) => Err(format!("{path} read back with other content")),
            FileRead::Absent => Err(format!("{path} is not there on its readback")),
            FileRead::Failed(error) => Err(error.0),
        },
        StorageOp::Delete { path } => storage.delete(&link_path(path)).await.map_err(|error| error.0),
        StorageOp::Rename { from, to } => storage.rename(&link_path(from), &link_path(to)).await.map_err(|error| error.0),
    }
}

/// The pairing setup of §3.2 against `storage`, for a device of `kind`
/// (`"3ds"`). `progress` receives (operations done, operations planned).
pub async fn setup_pairing<S: SetupStorage>(
    storage: &mut S,
    store: &Store,
    kind: &str,
    hub_port: u16,
    progress: &mut (dyn FnMut(u64, u64) + Send),
) -> PairingReport {
    let reads = read_pairing(storage, &store.hub_id()).await;
    if let Some(failure) = &reads.failure {
        tracing::info!(failure, "pairing setup: a read failed; nothing written");
        return read_failed(None, reads.detail.clone().or_else(|| Some(failure.clone())));
    }
    let plan = plan(&reads, store, hub_port, storage.via(), true);
    let outcome = PairingOutcome::from(plan.outcome);
    if matches!(plan.outcome, SetupOutcome::FailedRead | SetupOutcome::EightComputers) {
        tracing::info!(outcome = plan.outcome.name(), "pairing setup: nothing written");
        if plan.outcome == SetupOutcome::FailedRead {
            return read_failed(None, reads.detail.clone().or_else(|| Some(plan.message.to_owned())));
        }
        return PairingReport {
            outcome,
            failed: true,
            failed_at: None,
            device_id: plan.device_id.clone(),
            unconfirmed: false,
            message: owner_message(outcome, true).to_owned(),
            detail: Some(plan.message.to_owned()),
        };
    }
    let (Some(id), Some(key)) = (plan.device_id.clone(), plan.key.clone()) else {
        // plan_setup gives every other outcome an id and a key.
        return read_failed(None, Some("the plan has no device id".into()));
    };

    // The plan's key is the record's key before the first write (§3.2 step 4);
    // a record that cannot be stored stops the setup before the card changes.
    let total = plan.ops.len() as u64;
    if let Err(error) = save(store, &id, kind, &key, total > 0) {
        tracing::warn!(device = id, %error, "pairing setup: the record could not be stored; nothing written");
        return PairingReport {
            outcome,
            failed: true,
            failed_at: plan.ops.first().map(FailedStep::from),
            unconfirmed: store.device(&id).is_some_and(|record| record.setup_unconfirmed),
            device_id: Some(id),
            message: "Hub could not save its own settings. Nothing was written to the console.".to_owned(),
            detail: Some(error.to_string()),
        };
    }

    progress(0, total);
    let mut completed = 0usize;
    let mut detail = None;
    for op in &plan.ops {
        if let Err(error) = run_op(storage, op).await {
            detail = Some(error);
            break;
        }
        completed += 1;
        progress(completed as u64, total);
    }
    let failed_at = pairing::setup_failed_at(&plan, completed);
    let failed = failed_at.is_some();
    let settled = save(store, &id, kind, &key, failed);
    if let Err(error) = &settled {
        tracing::warn!(device = id, %error, "pairing setup: the record could not be settled");
    }
    tracing::info!(device = id, outcome = plan.outcome.name(), completed, planned = total, failed, "pairing setup");
    PairingReport {
        outcome,
        failed,
        failed_at: failed_at.flatten().map(FailedStep::from),
        device_id: Some(id.clone()),
        unconfirmed: store.device(&id).is_some_and(|record| record.setup_unconfirmed),
        message: owner_message(outcome, failed).to_owned(),
        detail: detail.or_else(|| settled.err().map(|error| format!("Hub's record: {error}"))),
    }
}
