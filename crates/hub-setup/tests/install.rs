//! The one-call install: the program, then the pairing, over the fake ftpd
//! and on a card folder.

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pocket_hub_setup::{Decision, InstallOptions, InstallReport, InstallTarget, PairingOutcome, ProgramAction, Progress, ShellSource, Stage, install, install_3ds};
use pocket_hub_store::Store;
use support::*;

const PROGRAM: &str = "3ds/pocket-shell.3dsx";

fn program_file(bytes: &[u8]) -> PathBuf {
    let dir = temp("program");
    let path = dir.join("pocket-shell-dev.3dsx");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn options(program: &Path) -> InstallOptions {
    let mut options = InstallOptions::new(temp("cache"));
    options.source = ShellSource::File { path: program.to_owned() };
    options.ftp_timeouts = timeouts();
    options
}

async fn run(target: &InstallTarget, store: &Store, options: &InstallOptions) -> (InstallReport, Vec<Progress>) {
    let mut events = Vec::new();
    let report = install(target, store, options, &mut |event| events.push(event)).await;
    (report, events)
}

fn ftp(fake: &FakeFtp) -> InstallTarget {
    InstallTarget::Ftp { address: fake.address }
}

#[tokio::test]
async fn a_fresh_console_gets_the_program_then_the_pairing() {
    let card = temp("console");
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let bytes = fake_3dsx(300_000, 1);
    let options = options(&program_file(&bytes));
    let (report, events) = run(&ftp(&fake), &store, &options).await;
    assert!(report.ok, "{report:?}");
    assert_eq!(report.kind, "3ds");
    assert_eq!(report.via, "ftp");
    assert_eq!(report.program.action, ProgramAction::Installed);
    assert_eq!(report.program.decision, Some(Decision::Missing));
    let release = report.program.release.as_ref().unwrap();
    assert_eq!(release.origin, "file:pocket-shell-dev.3dsx");
    assert_eq!(release.size, bytes.len() as u64);
    assert_eq!(report.pairing.as_ref().unwrap().outcome, PairingOutcome::Key);
    assert!(report.next_step.contains("exit ftpd") && report.next_step.contains("open Pocket Shell from the Homebrew Launcher"), "{}", report.next_step);
    assert!(report.next_step.contains(HUB_NAME));

    // The program is in place, uploaded under a temporary name and renamed.
    assert_eq!(std::fs::read(card.join(PROGRAM)).unwrap(), bytes);
    assert!(!card.join("3ds/pocket-shell.3dsx.tmp").exists());
    let log = writes(&fake.take_log());
    let stor = log.iter().position(|line| line == "STOR /3ds/pocket-shell.3dsx.tmp").expect("uploaded to the temporary name");
    let rnfr = log.iter().position(|line| line == "RNFR /3ds/pocket-shell.3dsx.tmp").unwrap();
    assert_eq!(log[rnfr + 1], "RNTO /3ds/pocket-shell.3dsx");
    let pairing = log.iter().position(|line| line.contains("pocketjs/link")).unwrap();
    assert!(stor < rnfr && rnfr < pairing, "the program goes before the pairing: {log:?}");

    // Progress: bytes of the upload rise to the total, then the pairing, then done.
    let uploads: Vec<&Progress> = events.iter().filter(|event| event.stage == Stage::Uploading).collect();
    assert!(uploads.len() > 3);
    assert!(uploads.windows(2).all(|pair| pair[0].done <= pair[1].done));
    assert_eq!(uploads.last().unwrap().done, bytes.len() as u64);
    assert!(uploads.iter().all(|event| event.total == Some(bytes.len() as u64)));
    let stages: Vec<Stage> = events.iter().map(|event| event.stage).collect();
    let first = |stage| stages.iter().position(|s| *s == stage).unwrap();
    assert!(first(Stage::Checking) < first(Stage::Uploading) && first(Stage::Uploading) < first(Stage::Pairing));
    assert_eq!(*stages.last().unwrap(), Stage::Done);
    let json = serde_json::to_string(&events[0]).unwrap();
    assert_eq!(json, r#"{"stage":"connecting","done":0,"total":null}"#);

    // The report is plain data and holds no key.
    let id = report.device_id.clone().unwrap();
    let key = store.device(&id).unwrap().key;
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains(&key));
    assert!(json.contains(r#""action":"installed""#));
}

#[tokio::test]
async fn a_second_install_changes_nothing() {
    let card = temp("again");
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let options = options(&program_file(&fake_3dsx(100_000, 2)));
    assert!(run(&ftp(&fake), &store, &options).await.0.ok);
    fake.take_log();
    let before = snapshot(&card);
    let (report, events) = run(&ftp(&fake), &store, &options).await;
    assert!(report.ok, "{report:?}");
    assert_eq!(report.program.action, ProgramAction::AlreadyInstalled);
    assert_eq!(report.program.decision, Some(Decision::Installed));
    assert_eq!(report.pairing.unwrap().outcome, PairingOutcome::Kept);
    assert_eq!(writes(&fake.take_log()), Vec::<String>::new());
    assert_eq!(snapshot(&card), before);
    assert!(events.iter().all(|event| event.stage != Stage::Uploading));
}

#[tokio::test]
async fn another_build_is_installed_over_the_old_one() {
    let card = temp("update");
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    assert!(run(&ftp(&fake), &store, &options(&program_file(&fake_3dsx(100_000, 3)))).await.0.ok);

    // Another size.
    let newer = fake_3dsx(120_000, 4);
    fake.take_log();
    let (report, _) = run(&ftp(&fake), &store, &options(&program_file(&newer))).await;
    assert!(report.ok, "{report:?}");
    assert_eq!(report.program.decision, Some(Decision::OtherSize));
    assert_eq!(std::fs::read(card.join(PROGRAM)).unwrap(), newer);
    // The fake, like the 3DS, does not rename over a file: the old program is
    // deleted, confirmed, and the rename runs again.
    let log = writes(&fake.take_log());
    assert!(log.contains(&"DELE /3ds/pocket-shell.3dsx".to_owned()), "{log:?}");
    assert_eq!(log.iter().filter(|line| *line == "RNTO /3ds/pocket-shell.3dsx").count(), 2);

    // The same size, other bytes: Hub's note names another build.
    let same_size = fake_3dsx(120_000, 5);
    let (report, _) = run(&ftp(&fake), &store, &options(&program_file(&same_size))).await;
    assert_eq!(report.program.decision, Some(Decision::Unrecorded));
    assert_eq!(std::fs::read(card.join(PROGRAM)).unwrap(), same_size);

    // Reinstall on request.
    let mut forced = options(&program_file(&same_size));
    forced.reinstall = true;
    let (report, _) = run(&ftp(&fake), &store, &forced).await;
    assert_eq!(report.program.decision, Some(Decision::Forced));
    assert_eq!(report.program.action, ProgramAction::Installed);
}

#[tokio::test]
async fn a_program_hub_did_not_install_is_replaced() {
    let card = temp("foreign-program");
    let bytes = fake_3dsx(50_000, 6);
    // The same bytes, put there by someone else: Hub has no note of it.
    write(&card.join(PROGRAM), &bytes);
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let (report, _) = run(&ftp(&fake), &store, &options(&program_file(&bytes))).await;
    assert!(report.ok);
    assert_eq!(report.program.decision, Some(Decision::Unrecorded));
    // Now noted: the next install skips it.
    let (report, _) = run(&ftp(&fake), &store, &options(&program_file(&bytes))).await;
    assert_eq!(report.program.action, ProgramAction::AlreadyInstalled);
}

#[tokio::test]
async fn a_dropped_upload_changes_no_pairing() {
    let card = temp("dropped");
    // A card that has held homebrew has the folder, so the first STOR is the upload.
    std::fs::create_dir_all(card.join("3ds")).unwrap();
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    fake.rule(Rule::new("STOR", ".3dsx.tmp", Action::DropData { after: 40_000, then: Then::Reply("426 Connection broken during transfer") }));
    let (report, _) = run(&ftp(&fake), &store, &options(&program_file(&fake_3dsx(200_000, 7)))).await;
    assert!(!report.ok);
    assert_eq!(report.failed_stage, Some(Stage::Uploading));
    assert_eq!(report.program.action, ProgramAction::NotInstalled);
    assert!(report.pairing.is_none());
    assert!(report.next_step.contains("Keep ftpd open"), "{}", report.next_step);
    assert!(store.devices().is_empty());
    assert!(!card.join(PROGRAM).exists());
    assert!(!card.join("pocketjs").exists());

    // The next run installs everything.
    let (report, _) = run(&ftp(&fake), &store, &options(&program_file(&fake_3dsx(200_000, 7)))).await;
    assert!(report.ok, "{report:?}");
    assert!(!card.join("3ds/pocket-shell.3dsx.tmp").exists());
}

#[tokio::test]
async fn eight_computers_stop_before_the_upload() {
    let card = temp("eight-install");
    for n in 0..8u64 {
        let text = format!("key={}\nname=Other\naddress=192.168.1.2\nport=8628\n", format!("{n:02x}").repeat(32));
        write(&companions(&card).join(format!("{:016x}.txt", 0x2000 + n)), text);
    }
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let (report, _) = run(&ftp(&fake), &store, &options(&program_file(&fake_3dsx(10_000, 8)))).await;
    assert!(!report.ok);
    assert_eq!(report.failed_stage, Some(Stage::Checking));
    assert_eq!(report.pairing.unwrap().outcome, PairingOutcome::EightComputers);
    assert!(report.next_step.contains("8 computers"));
    assert_eq!(writes(&fake.take_log()), Vec::<String>::new());
}

#[tokio::test]
async fn an_unreachable_console_is_reported() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let store = open_store();
    let target = InstallTarget::Ftp { address: closed };
    let (report, _) = run(&target, &store, &options(&program_file(&fake_3dsx(10_000, 9)))).await;
    assert!(!report.ok);
    assert_eq!(report.failed_stage, Some(Stage::Connecting));
    assert!(report.next_step.contains("open ftpd"), "{}", report.next_step);
}

#[tokio::test]
async fn a_file_that_is_not_a_program_is_refused() {
    let card = temp("not-a-program");
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let (report, _) = run(&ftp(&fake), &store, &options(&program_file(b"MZ not a 3dsx file at all, but long enough"))).await;
    assert!(!report.ok);
    assert_eq!(report.failed_stage, Some(Stage::Resolving));
    assert!(report.detail.unwrap().contains("3DSX"));
    assert_eq!(writes(&fake.take_log()), Vec::<String>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_card_install_and_its_second_run() {
    let card = temp("sd");
    std::fs::create_dir_all(card.join("Nintendo 3DS")).unwrap();
    let store = open_store();
    let bytes = fake_3dsx(80_000, 10);
    let mut options = options(&program_file(&bytes));
    options.card_addresses = Some(vec!["192.168.7.7".into(), "10.1.1.1".into()]);
    let target = InstallTarget::Card { root: card.clone() };
    let (report, events) = run(&target, &store, &options).await;
    assert!(report.ok, "{report:?}");
    assert_eq!(report.via, "card");
    assert!(report.next_step.contains("put it back in the console"));
    assert_eq!(std::fs::read(card.join(PROGRAM)).unwrap(), bytes);
    assert_eq!(events.iter().rfind(|event| event.stage == Stage::Uploading).unwrap().done, bytes.len() as u64);
    let loaded = device_load(&card);
    assert_eq!(loaded[0].address, "192.168.7.7");

    let before = snapshot(&card);
    let (report, _) = run(&target, &store, &options).await;
    assert!(report.ok);
    assert_eq!(report.program.action, ProgramAction::AlreadyInstalled);
    assert_eq!(report.pairing.unwrap().outcome, PairingOutcome::Kept);
    assert_eq!(snapshot(&card), before);

    // A folder that is not a 3DS card is refused before anything is written.
    let other = temp("not-a-card");
    let (report, _) = run(&InstallTarget::Card { root: other.clone() }, &store, &options).await;
    assert_eq!(report.failed_stage, Some(Stage::Connecting));
    assert!(snapshot(&other).is_empty());
}

/// The install's future is `Send`: the UI runs it on a spawned task.
#[tokio::test]
async fn the_install_runs_on_a_spawned_task() {
    let card = temp("spawned");
    let fake = FakeFtp::start(&card).await;
    let store = Arc::new(open_store());
    let options = options(&program_file(&fake_3dsx(10_000, 11)));
    let address: SocketAddr = fake.address;
    let task = tokio::spawn({
        let store = store.clone();
        async move {
            let mut count = 0;
            let report = install_3ds(&InstallTarget::Ftp { address }, &store, &options, &mut |_| count += 1).await;
            (report, count)
        }
    });
    let (report, count) = task.await.unwrap();
    assert!(report.ok);
    assert!(count > 0);
}
