//! The pairing setup over ftpd (docs/POCKET-LINK.md §3.2) against the fake
//! ftpd of `support`.

mod support;

use pocket_hub_setup::{FailedStep, FtpStorage, PairingOutcome, PairingReport, setup_pairing};
use pocket_hub_store::Store;
use pocket_link::pairing::{CompanionFile, LastFile, format_companion_file, format_device_file, format_last_file};
use support::*;

async fn setup(fake: &FakeFtp, store: &Store) -> PairingReport {
    let mut storage = connect(fake).await;
    let report = setup_pairing(&mut storage, store, "3ds", HUB_PORT, &mut |_, _| {}).await;
    storage.close().await;
    report
}

fn key_file(key: &str, address: &str) -> String {
    format_companion_file(&CompanionFile { key: key.into(), name: HUB_NAME.into(), address: address.into(), port: HUB_PORT })
}

fn record_json(store: &Store, id: &str) -> String {
    serde_json::to_string(&store.device(id)).unwrap()
}

/// A card set up once by this Hub: (fake, store, device id, record key).
async fn paired() -> (FakeFtp, Store, String, String) {
    let card = temp("card");
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let report = setup(&fake, &store).await;
    assert!(!report.failed, "{report:?}");
    let id = report.device_id.unwrap();
    let key = store.device(&id).unwrap().key;
    fake.take_log();
    (fake, store, id, key)
}

#[tokio::test]
async fn a_fresh_card_gets_an_id_and_a_key() {
    let card = temp("fresh");
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let hub = store.hub_id();
    let report = setup(&fake, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Key);
    assert!(!report.failed && !report.unconfirmed && report.failed_at.is_none(), "{report:?}");
    let id = report.device_id.clone().unwrap();
    assert!(pocket_link::text::is_link_id(&id));

    let record = store.device(&id).unwrap();
    assert_eq!(record.kind, "3ds");
    assert!(!record.setup_unconfirmed);
    assert!(pocket_link::text::is_key(&record.key));

    // Byte-identical to what pocket_link::pairing formats; the FTP
    // connection's local address is the address written.
    assert_eq!(read_text(&card.join("pocketjs/link/device.txt")).unwrap(), format_device_file(&id));
    assert_eq!(read_text(&companions(&card).join(format!("{hub}.txt"))).unwrap(), key_file(&record.key, "127.0.0.1"));
    assert!(!companions(&card).join(format!("{hub}.last")).exists());
    assert!(!companions(&card).join(format!("{hub}.new")).exists());

    let loaded = device_load(&card);
    assert_eq!(loaded.len(), 1);
    assert_eq!((loaded[0].id.as_str(), loaded[0].key.as_str(), loaded[0].address.as_str(), loaded[0].port), (hub.as_str(), record.key.as_str(), "127.0.0.1", HUB_PORT));

    // The report holds no key.
    assert!(!serde_json::to_string(&report).unwrap().contains(&record.key));
}

#[tokio::test]
async fn a_second_run_changes_nothing() {
    let (fake, store, id, key) = paired().await;
    let before = snapshot(&fake.root);
    let report = setup(&fake, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Kept);
    assert!(!report.failed);
    assert_eq!(report.device_id.as_deref(), Some(id.as_str()));
    assert_eq!(snapshot(&fake.root), before);
    assert_eq!(writes(&fake.take_log()), Vec::<String>::new());
    assert_eq!(store.device(&id).unwrap().key, key);
}

#[tokio::test]
async fn a_changed_address_rewrites_only_the_last_file() {
    let (fake, store, id, key) = paired().await;
    let hub = store.hub_id();
    let txt = companions(&fake.root).join(format!("{hub}.txt"));
    let last = companions(&fake.root).join(format!("{hub}.last"));
    let key_text = read_text(&txt).unwrap();
    // The device last linked to Hub at another address.
    write(&last, format_last_file(&LastFile { name: HUB_NAME.into(), address: "10.9.8.7".into(), port: HUB_PORT }));

    let report = setup(&fake, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Address);
    assert!(!report.failed, "{report:?}");
    assert_eq!(read_text(&txt).unwrap(), key_text);
    assert_eq!(read_text(&last).unwrap(), format_last_file(&LastFile { name: HUB_NAME.into(), address: "127.0.0.1".into(), port: HUB_PORT }));
    assert!(!companions(&fake.root).join(format!("{hub}.last.tmp")).exists());
    let log = writes(&fake.take_log());
    assert!(log.iter().all(|line| line.contains(".last")), "{log:?}");
    // ftpd does not rename over a file: the old .last goes first.
    assert!(log.contains(&format!("DELE /pocketjs/link/companions/{hub}.last")), "{log:?}");
    assert_eq!(store.device(&id).unwrap().key, key);

    // The key file's own address is used when .last is gone or damaged.
    std::fs::write(&last, "garbage").unwrap();
    assert_eq!(setup(&fake, &store).await.outcome, PairingOutcome::Kept);
}

#[tokio::test]
async fn another_key_on_the_card_is_replaced_and_stored_before_the_first_write() {
    let (fake, store, id, old) = paired().await;
    let hub = store.hub_id();
    let txt = companions(&fake.root).join(format!("{hub}.txt"));
    let foreign = "ab".repeat(32);
    write(&txt, key_file(&foreign, "127.0.0.1"));

    // The connection drops at the first write: the record already holds the
    // minted key, marked "setup not confirmed".
    fake.rule(Rule::new("STOR", ".new", Action::Kill));
    let report = setup(&fake, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Key);
    assert!(report.failed && report.unconfirmed);
    assert_eq!(report.failed_at, Some(FailedStep::Write { path: format!("pocketjs/link/companions/{hub}.new") }));
    let record = store.device(&id).unwrap();
    assert!(record.key != old && record.key != foreign);
    assert!(record.setup_unconfirmed);
    assert_eq!(read_text(&txt).unwrap(), key_file(&foreign, "127.0.0.1"));

    let report = setup(&fake, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Key);
    assert!(!report.failed && !report.unconfirmed);
    let record = store.device(&id).unwrap();
    assert!(!record.setup_unconfirmed);
    assert_eq!(device_load(&fake.root)[0].key, record.key);
}

#[tokio::test]
async fn eight_other_computers_get_nothing_written() {
    let card = temp("eight");
    let fake = FakeFtp::start(&card).await;
    for n in 0..8u8 {
        let id = format!("{:016x}", 0x1000 + n as u64);
        write(&companions(&card).join(format!("{id}.txt")), key_file(&format!("{n:02x}").repeat(32), "192.168.1.2"));
    }
    let store = open_store();
    let before = snapshot(&card);
    let report = setup(&fake, &store).await;
    assert_eq!(report.outcome, PairingOutcome::EightComputers);
    assert!(report.failed);
    assert!(report.message.contains("8 computers"));
    assert_eq!(snapshot(&card), before);
    assert_eq!(writes(&fake.take_log()), Vec::<String>::new());
    assert!(store.devices().is_empty());
}

#[tokio::test]
async fn a_leftover_new_file_is_removed() {
    let (fake, store, _, _) = paired().await;
    let hub = store.hub_id();
    let leftover = companions(&fake.root).join(format!("{hub}.new"));
    write(&leftover, "key=");
    let report = setup(&fake, &store).await;
    assert_eq!(report.outcome, PairingOutcome::Kept);
    assert!(!report.failed);
    assert!(!leftover.exists());
    assert_eq!(writes(&fake.take_log()), vec![format!("DELE /pocketjs/link/companions/{hub}.new")]);
}

#[tokio::test]
async fn a_failed_read_leaves_the_record_untouched() {
    type Make = fn(&str) -> Rule;
    let cases: Vec<(&str, Make, bool)> = vec![
        ("data dropped, 426", |own| Rule::new("RETR", own, Action::DropData { after: 5, then: Then::Reply("426 Connection broken during transfer") }), false),
        ("data dropped, then a 226 anyway", |own| Rule::new("RETR", own, Action::DropData { after: 5, then: Then::Reply("226 OK") }), false),
        ("data dropped, connection killed", |own| Rule::new("RETR", own, Action::DropData { after: 5, then: Then::Kill }), false),
        ("an I/O error is not a missing file", |own| Rule::new("RETR", own, Action::Reply("450 Input/output error")), false),
        ("SIZE refused with 421", |_| Rule::new("SIZE", "device.txt", Action::Reply("421 Service not available")), false),
        ("device.txt stalls", |_| Rule::new("SIZE", "device.txt", Action::Stall), true),
        ("the key file's data stalls", |own| Rule::new("RETR", own, Action::Stall), true),
        ("the listing fails", |_| Rule::new("NLST", "companions", Action::DropData { after: 0, then: Then::Kill }), false),
    ];
    for (name, make, short) in cases {
        let (fake, store, id, _) = paired().await;
        let hub = store.hub_id();
        let own = format!("{hub}.txt");
        // A setup that read the file would mint a key for it.
        write(&companions(&fake.root).join(&own), key_file(&"ab".repeat(32), "127.0.0.1"));
        let record = record_json(&store, &id);
        let before = snapshot(&fake.root);
        fake.rule(make(&own));
        let mut storage = FtpStorage::connect(fake.address, if short { short_timeouts() } else { timeouts() }).await.unwrap();
        let report = setup_pairing(&mut storage, &store, "3ds", HUB_PORT, &mut |_, _| {}).await;
        assert_eq!(report.outcome, PairingOutcome::FailedRead, "{name}: {report:?}");
        assert!(report.failed, "{name}");
        assert_eq!(fake.rules_left(), 0, "{name}: the fault fired");
        assert_eq!(record_json(&store, &id), record, "{name}: the record is untouched");
        assert_eq!(snapshot(&fake.root), before, "{name}: the card is untouched");
        assert_eq!(writes(&fake.take_log()), Vec::<String>::new(), "{name}");
    }
}

/// The spec's power-cut table (§3.2): a cut at each step of the key-writing
/// plan, the card it leaves, the record, and the next setup's repair.
#[tokio::test]
async fn a_cut_at_each_step_of_the_key_plan() {
    #[derive(Debug)]
    enum Row {
        DuringNew,
        AfterLastDeleted,
        BetweenDeleteAndRename,
        AfterRename,
    }
    for row in [Row::DuringNew, Row::AfterLastDeleted, Row::BetweenDeleteAndRename, Row::AfterRename] {
        let (fake, store, id, old) = paired().await;
        let hub = store.hub_id();
        let dir = companions(&fake.root);
        let (txt, last, new) = (dir.join(format!("{hub}.txt")), dir.join(format!("{hub}.last")), dir.join(format!("{hub}.new")));
        let foreign = key_file(&"ab".repeat(32), "127.0.0.1");
        write(&txt, &foreign);
        write(&last, format_last_file(&LastFile { name: HUB_NAME.into(), address: "127.0.0.1".into(), port: HUB_PORT }));

        let (rule, step) = match row {
            Row::DuringNew => (Rule::new("STOR", ".new", Action::DropData { after: 10, then: Then::Kill }), FailedStep::Write { path: format!("pocketjs/link/companions/{hub}.new") }),
            Row::AfterLastDeleted => (Rule::new("DELE", format!("{hub}.txt"), Action::Kill), FailedStep::Delete { path: format!("pocketjs/link/companions/{hub}.txt") }),
            Row::BetweenDeleteAndRename => (
                Rule::new("RNFR", ".new", Action::Kill),
                FailedStep::Rename { from: format!("pocketjs/link/companions/{hub}.new"), to: format!("pocketjs/link/companions/{hub}.txt") },
            ),
            // The first RETR of the key file is the setup's read; the second is
            // the final readback.
            Row::AfterRename => (Rule::new("RETR", format!("{hub}.txt"), Action::Kill).skip(1), FailedStep::Readback { path: format!("pocketjs/link/companions/{hub}.txt") }),
        };
        fake.rule(rule);
        let report = setup(&fake, &store).await;
        assert_eq!(fake.rules_left(), 0, "{row:?}");
        assert_eq!(report.outcome, PairingOutcome::Key, "{row:?}");
        assert!(report.failed && report.unconfirmed, "{row:?}: {report:?}");
        assert_eq!(report.failed_at, Some(step), "{row:?}");
        let record = store.device(&id).unwrap();
        assert!(record.setup_unconfirmed, "{row:?}");
        assert_ne!(record.key, old, "{row:?}");
        let written = key_file(&record.key, "127.0.0.1");

        match row {
            Row::DuringNew => {
                assert_eq!(read_text(&txt).unwrap(), foreign);
                assert!(last.exists());
                assert_eq!(std::fs::read(&new).unwrap(), written.as_bytes()[..10].to_vec());
            }
            Row::AfterLastDeleted => {
                assert_eq!(read_text(&txt).unwrap(), foreign);
                assert!(!last.exists());
                assert_eq!(read_text(&new).unwrap(), written);
            }
            Row::BetweenDeleteAndRename => {
                assert!(!txt.exists() && !last.exists());
                assert_eq!(read_text(&new).unwrap(), written);
                assert!(device_load(&fake.root).is_empty());
            }
            Row::AfterRename => {
                assert_eq!(read_text(&txt).unwrap(), written);
                assert!(!last.exists() && !new.exists());
                // The pairing is in place: the device holds the record's key.
                assert_eq!(device_load(&fake.root)[0].key, record.key);
            }
        }

        // The next setup repairs it with no memory of the failed one.
        let report = setup(&fake, &store).await;
        assert!(!report.failed && !report.unconfirmed, "{row:?}: {report:?}");
        let expected = if matches!(row, Row::AfterRename) { PairingOutcome::Kept } else { PairingOutcome::Key };
        assert_eq!(report.outcome, expected, "{row:?}");
        let repaired = store.device(&id).unwrap();
        assert!(!repaired.setup_unconfirmed);
        assert!(!new.exists() && !dir.join(format!("{hub}.last.tmp")).exists(), "{row:?}");
        assert_eq!(device_load(&fake.root)[0].key, repaired.key, "{row:?}");
    }
}

/// The connection dies at every command in turn. Whatever the step, a failed
/// read leaves the record as it was, any later cut leaves it "not
/// confirmed", and the next setup ends with the card and the record agreeing.
#[tokio::test]
async fn a_dropped_connection_at_any_command_is_repaired() {
    let mut n = 1;
    loop {
        let (fake, store, id, _) = paired().await;
        let hub = store.hub_id();
        write(&companions(&fake.root).join(format!("{hub}.txt")), key_file(&"ab".repeat(32), "127.0.0.1"));
        let before = record_json(&store, &id);
        // USER and TYPE come first; the setup's commands follow.
        fake.kill_after(n + 2);
        let report = setup(&fake, &store).await;
        let fired = report.failed;
        if report.outcome == PairingOutcome::FailedRead {
            assert_eq!(record_json(&store, &id), before, "command {n}");
        } else if report.failed {
            assert_eq!(report.outcome, PairingOutcome::Key, "command {n}");
            assert!(store.device(&id).unwrap().setup_unconfirmed, "command {n}");
        }
        fake.clear_faults();
        let repair = setup(&fake, &store).await;
        assert!(!repair.failed, "command {n}: {repair:?}");
        let record = store.device(&id).unwrap();
        assert!(!record.setup_unconfirmed);
        assert_eq!(device_load(&fake.root)[0].key, record.key, "command {n}");
        if !fired {
            assert!(n > 10, "the fault fired at most {n} commands in");
            break;
        }
        n += 1;
    }
}

#[tokio::test]
async fn a_damaged_device_file_gets_a_new_id() {
    let card = temp("damaged");
    write(&card.join("pocketjs/link/device.txt"), "not an id\n");
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let report = setup(&fake, &store).await;
    assert!(!report.failed);
    let id = report.device_id.unwrap();
    assert_eq!(read_text(&card.join("pocketjs/link/device.txt")).unwrap(), format_device_file(&id));

    // An id followed by the run the device's reader strips is kept.
    let card = temp("kept-id");
    write(&card.join("pocketjs/link/device.txt"), "0123456789abcdef \r\n\n");
    let fake = FakeFtp::start(&card).await;
    let report = setup(&fake, &store).await;
    assert_eq!(report.device_id.as_deref(), Some("0123456789abcdef"));
    assert_eq!(read_text(&card.join("pocketjs/link/device.txt")).unwrap(), "0123456789abcdef \r\n\n");
}
