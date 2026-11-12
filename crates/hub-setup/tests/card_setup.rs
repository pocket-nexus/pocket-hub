//! The pairing setup on a mounted card (docs/POCKET-LINK.md §3.2) in a
//! temporary folder.

mod support;

use std::os::unix::fs::PermissionsExt;

use pocket_hub_setup::{CardStorage, PairingOutcome, PairingReport, SetupStorage, Via, setup_pairing};
use pocket_hub_store::Store;
use pocket_link::pairing::{CompanionFile, LastFile, format_companion_file, format_device_file, format_last_file};
use support::*;

const ADDRESSES: [&str; 2] = ["192.168.1.20", "10.0.0.5"];

async fn setup(card: &std::path::Path, store: &Store) -> PairingReport {
    let mut storage = CardStorage::with_addresses(card, ADDRESSES.iter().map(|a| a.to_string()).collect());
    setup_pairing(&mut storage, store, "3ds", HUB_PORT, &mut |_, _| {}).await
}

fn last_file(address: &str, port: u16) -> String {
    format_last_file(&LastFile { name: HUB_NAME.into(), address: address.into(), port })
}

#[tokio::test]
async fn a_fresh_card_then_a_second_run() {
    let card = temp("card-fresh");
    let store = open_store();
    let hub = store.hub_id();
    let report = setup(&card, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Key);
    assert!(!report.failed);
    let id = report.device_id.unwrap();
    let record = store.device(&id).unwrap();
    assert_eq!(record.kind, "3ds");
    assert!(!record.setup_unconfirmed);
    assert_eq!(read_text(&card.join("pocketjs/link/device.txt")).unwrap(), format_device_file(&id));
    // The first address, the default route's interface, is the one written.
    let expected = format_companion_file(&CompanionFile { key: record.key.clone(), name: HUB_NAME.into(), address: ADDRESSES[0].into(), port: HUB_PORT });
    assert_eq!(read_text(&companions(&card).join(format!("{hub}.txt"))).unwrap(), expected);
    assert_eq!(device_load(&card)[0].key, record.key);

    let before = snapshot(&card);
    let report = setup(&card, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Kept);
    assert_eq!(snapshot(&card), before);
}

#[tokio::test]
async fn the_last_address_is_kept_when_it_is_any_of_hubs() {
    let card = temp("card-last");
    let store = open_store();
    let hub = store.hub_id();
    assert!(!setup(&card, &store).await.failed);
    let last = companions(&card).join(format!("{hub}.last"));

    // Hub's second interface: a `ready` may have proved it.
    write(&last, last_file(ADDRESSES[1], HUB_PORT));
    assert_eq!(setup(&card, &store).await.outcome, PairingOutcome::Kept);
    assert_eq!(read_text(&last).unwrap(), last_file(ADDRESSES[1], HUB_PORT));

    // None of Hub's addresses: rewritten with the first.
    write(&last, last_file("172.16.0.9", HUB_PORT));
    let report = setup(&card, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Address);
    assert!(!report.failed);
    assert_eq!(read_text(&last).unwrap(), last_file(ADDRESSES[0], HUB_PORT));

    // Another port.
    write(&last, last_file(ADDRESSES[0], 9000));
    assert_eq!(setup(&card, &store).await.outcome, PairingOutcome::Address);
    assert_eq!(read_text(&last).unwrap(), last_file(ADDRESSES[0], HUB_PORT));
    assert!(!companions(&card).join(format!("{hub}.last.tmp")).exists());
}

#[tokio::test]
async fn an_unreadable_key_file_is_a_failed_read() {
    if unsafe_root() {
        return;
    }
    let card = temp("card-unreadable");
    let store = open_store();
    let hub = store.hub_id();
    let id = setup(&card, &store).await.device_id.unwrap();
    let txt = companions(&card).join(format!("{hub}.txt"));
    write(&txt, format_companion_file(&CompanionFile { key: "cd".repeat(32), name: "x".into(), address: "1.2.3.4".into(), port: 1 }));
    std::fs::set_permissions(&txt, std::fs::Permissions::from_mode(0o000)).unwrap();
    let record = serde_json::to_string(&store.device(&id)).unwrap();

    let report = setup(&card, &store).await;
    std::fs::set_permissions(&txt, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(report.outcome, PairingOutcome::FailedRead);
    assert!(report.failed);
    assert_eq!(serde_json::to_string(&store.device(&id)).unwrap(), record);
    assert!(report.detail.unwrap().contains("ermission"));
}

#[tokio::test]
async fn an_unreadable_file_of_another_computer_matters_only_at_eight() {
    if unsafe_root() {
        return;
    }
    let card = temp("card-others");
    let store = open_store();
    let other = |n: u64| companions(&card).join(format!("{:016x}.txt", 0x3000 + n));
    let text = |n: u64| format_companion_file(&CompanionFile { key: format!("{n:02x}").repeat(32), name: "Other".into(), address: "192.168.1.2".into(), port: HUB_PORT });
    write(&other(0), text(0));
    std::fs::set_permissions(other(0), std::fs::Permissions::from_mode(0o000)).unwrap();
    // One unreadable file among few cannot make eight: the setup goes on.
    let report = setup(&card, &store).await;
    assert!(!report.failed, "{report:?}");
    std::fs::remove_file(companions(&card).join(format!("{}.txt", store.hub_id()))).unwrap();

    // Seven readable and one unreadable: the count is unknown, nothing is written.
    for n in 1..8 {
        write(&other(n), text(n));
    }
    let before = snapshot_without(&card, &other(0));
    let report = setup(&card, &store).await;
    std::fs::set_permissions(other(0), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(report.outcome, PairingOutcome::FailedRead, "{report:?}");
    assert_eq!(snapshot_without(&card, &other(0)), before);
}

#[tokio::test]
async fn card_storage_operations() {
    let card = temp("card-ops");
    let mut storage = CardStorage::with_addresses(&card, vec!["10.0.0.1".into()]);
    assert_eq!(storage.via(), Via::Card { addresses: vec!["10.0.0.1".into()] });
    assert_eq!(storage.read("a/b.txt").await, pocket_hub_setup::FileRead::Absent);
    assert_eq!(storage.list("a").await.unwrap(), Vec::<String>::new());
    assert_eq!(storage.stat("a/b.txt").await.unwrap(), None);
    let mut seen = Vec::new();
    storage.write("a/b.txt", &vec![7u8; 200_000], &mut |done| seen.push(done)).await.unwrap();
    assert_eq!(*seen.last().unwrap(), 200_000);
    assert_eq!(storage.stat("a/b.txt").await.unwrap().unwrap().size, 200_000);
    storage.write("a/c.txt", b"c", &mut |_| {}).await.unwrap();
    storage.rename("a/c.txt", "a/b.txt").await.unwrap();
    assert_eq!(storage.read("a/b.txt").await, pocket_hub_setup::FileRead::File(b"c".to_vec()));
    storage.delete("a/b.txt").await.unwrap();
    storage.delete("a/b.txt").await.unwrap();
    assert_eq!(storage.list("a").await.unwrap(), Vec::<String>::new());
    assert!(matches!(storage.read("../escape").await, pocket_hub_setup::FileRead::Failed(_)));
    assert!(storage.write("/abs", b"x", &mut |_| {}).await.is_err());
}

fn unsafe_root() -> bool {
    // Permissions do not stop root; the unreadable-file cases need them.
    std::process::Command::new("id").arg("-u").output().map(|out| String::from_utf8_lossy(&out.stdout).trim() == "0").unwrap_or(false)
}

fn snapshot_without(root: &std::path::Path, skip: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let skip = skip.strip_prefix(root).unwrap().to_string_lossy().into_owned();
    let mut files = std::collections::BTreeMap::new();
    for (name, _) in std::fs::read_dir(companions(root)).unwrap().map(|e| (e.unwrap().path(), ())) {
        let relative = name.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        if relative != skip {
            files.insert(relative, std::fs::read(&name).unwrap());
        }
    }
    if let Ok(device) = std::fs::read(root.join("pocketjs/link/device.txt")) {
        files.insert("device.txt".into(), device);
    }
    files
}
