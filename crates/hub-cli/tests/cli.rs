//! `pocket-hub` against a real core: the main commands in both output modes,
//! the exit statuses, starting the core when none answers, `--no-start`, and
//! a core that cannot start.
//!
//! The core is the `pocket-hubd` next to `pocket-hub` in the target folder
//! (built here when it is missing). Every test uses its own home, an
//! ephemeral link port and no beacons, so nothing here touches another Hub.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const CLI: &str = env!("CARGO_BIN_EXE_pocket-hub");

fn hubd() -> PathBuf {
    let path = Path::new(CLI).with_file_name("pocket-hubd");
    if !path.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo).args(["build", "-p", "pocket-hubd", "--bin", "pocket-hubd"]).status().expect("cargo builds pocket-hubd");
        assert!(status.success());
    }
    path
}

fn bun() -> String {
    let output = Command::new("which").arg("bun").output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn home() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("phc").tempdir_in("/tmp").unwrap()
}

struct Run {
    status: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|_| panic!("not JSON: {}", self.stdout))
    }
}

fn command(home: &Path, args: &[&str]) -> Command {
    hubd();
    let mut command = Command::new(CLI);
    command
        .args(args)
        .env("POCKET_HUB_HOME", home)
        .env("POCKET_HUB_LINK_PORT", "0")
        // A fixed name on the fresh home, never this computer's.
        .env("POCKET_HUB_NAME", "Studio Mac")
        .env("POCKET_HUB_NO_BEACONS", "1")
        .env("POCKET_HUB_BUN", bun())
        // No applet store and no release source unless a test sets up its
        // fake one: a core looks Pocket Shell's release up when a device links.
        .env("POCKET_HUB_STORE_URL", "http://127.0.0.1:9")
        .env("POCKET_HUB_SHELL_API", "http://127.0.0.1:9")
        .env_remove("POCKET_HUB_SHELL_FILE")
        .env_remove("POCKET_HUB_SHELL_REPO")
        .env_remove("POCKET_HUBD")
        .current_dir(home);
    command
}

fn run(home: &Path, args: &[&str]) -> Run {
    let Output { status, stdout, stderr } = command(home, args).output().expect("pocket-hub runs");
    Run { status: status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&stdout).into_owned(), stderr: String::from_utf8_lossy(&stderr).into_owned() }
}

/// The reference device (hub-core's harness) on a card's pairing; returns
/// once it has linked.
fn device(card: &Path, hub: &str, port: u64) -> Child {
    let config = serde_json::json!({"dir": card.join("pocketjs/link"), "hub": hub, "address": "127.0.0.1", "port": port});
    let harness = concat!(env!("CARGO_MANIFEST_DIR"), "/../hub-core/tests/harness/device.ts");
    let mut child = Command::new(bun()).arg(harness).arg(config.to_string()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let deadline = Instant::now() + Duration::from_secs(10);
    for line in lines.by_ref() {
        if line.unwrap().contains("\"ev\":\"linked\"") {
            break;
        }
        assert!(Instant::now() < deadline, "the device did not link");
    }
    // The device's later lines are not read; it writes few.
    std::thread::spawn(move || for _ in lines {});
    child
}

/// Stops a core a test started even when the test fails: a started core is
/// detached and would outlive the test.
struct Stop<'a>(&'a Path);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = command(self.0, &["--no-start", "shutdown"]).output();
    }
}

fn shutdown(home: &Path) {
    let done = run(home, &["shutdown"]);
    assert_eq!(done.status, 0, "{}", done.stderr);
}

#[test]
fn starting_the_core_and_reaching_none() {
    let home = home();
    let _stop = Stop(home.path());
    let unreachable = run(home.path(), &["--no-start", "status"]);
    assert_eq!(unreachable.status, 3);
    assert!(unreachable.stderr.contains("No Pocket Hub core answers"), "{}", unreachable.stderr);
    let unreachable = run(home.path(), &["--no-start", "--json", "devices"]);
    assert_eq!(unreachable.status, 3);
    assert!(unreachable.json()["error"]["message"].as_str().unwrap().contains("No Pocket Hub core"));

    // `status` starts the core next to the command and waits for it.
    let status = run(home.path(), &["status"]);
    assert_eq!(status.status, 0, "{}", status.stderr);
    assert!(status.stdout.starts_with("Pocket Hub \"") && status.stdout.contains("Link:     tcp port"), "{}", status.stdout);
    let json = run(home.path(), &["--json", "status"]).json();
    assert_eq!(json["home"], home.path().display().to_string());
    let port = json["carriers"][0]["port"].as_u64().unwrap();
    assert!(port > 0 && port != 8628);
    // A second core for the same home exits at once with 0.
    let second = Command::new(hubd()).arg("--home").arg(home.path()).args(["--link-port", "0", "--no-beacons"]).output().unwrap();
    assert_eq!(second.status.code(), Some(0));
    assert!(run(home.path(), &["--no-start", "status"]).status == 0);

    shutdown(home.path());
    assert!(!home.path().join("hubd.sock").exists());
    assert_eq!(run(home.path(), &["shutdown"]).status, 3, "shutdown starts no core");
    assert_eq!(run(home.path(), &["--no-start", "status"]).status, 3);
}

#[test]
fn a_core_that_cannot_start_is_reported() {
    // A link port another program holds: the core says so in one line and
    // the command exits with "core unreachable".
    let taken = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    let home = home();
    let run_taken = {
        let output = command(home.path(), &["status"]).env("POCKET_HUB_LINK_PORT", &port).output().unwrap();
        Run { status: output.status.code().unwrap(), stdout: String::from_utf8_lossy(&output.stdout).into(), stderr: String::from_utf8_lossy(&output.stderr).into() }
    };
    assert_eq!(run_taken.status, 3);
    assert!(run_taken.stderr.contains(&format!("The link port {port} is in use")), "{}", run_taken.stderr);
    let log = std::fs::read_to_string(home.path().join("logs/hubd.log")).unwrap();
    assert!(log.contains(&format!("The link port {port} is in use")), "the line is in hubd.log too");

    // The binary alone: a non-zero status and the same line on standard error.
    let direct = Command::new(hubd()).arg("--home").arg(home.path()).args(["--link-port", &port, "--no-beacons"]).output().unwrap();
    assert_eq!(direct.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&direct.stderr);
    assert!(stderr.lines().any(|l| l.starts_with("pocket-hubd: The link port") && l.contains(&port)), "{stderr}");

    // A home too long for its socket.
    let long = home.path().join("y".repeat(100));
    let refused = Command::new(hubd()).arg("--home").arg(&long).args(["--link-port", "0"]).output().unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("too long"));

    // An explicit core program that does not exist.
    let missing = command(home.path(), &["status"]).env("POCKET_HUBD", home.path().join("nowhere")).output().unwrap();
    assert_eq!(missing.status.code(), Some(3));
    drop(taken);
}

#[test]
fn devices_setup_and_services_in_both_modes() {
    let home = home();
    let _stop = Stop(home.path());
    let work = home.path().join("work");
    std::fs::create_dir_all(work.join("sd/3ds")).unwrap();
    let card = work.join("sd");
    let shell = work.join("shell.3dsx");
    let mut program = b"3DSX".to_vec();
    program.extend((0..4096u32).map(|i| (i * 31) as u8));
    std::fs::write(&shell, &program).unwrap();

    let none = run(home.path(), &["devices"]);
    assert_eq!((none.status, none.stdout.trim()), (0, "No device is paired. Set a 3DS up with: pocket-hub setup 3ds\nA device that shows a code (an M5Stack Tab5) is paired with: pocket-hub pair"));
    assert_eq!(run(home.path(), &["--json", "devices"]).json(), serde_json::json!({"devices": []}));
    let no_device = run(home.path(), &["device", "status"]);
    assert_eq!(no_device.status, 1);
    assert!(no_device.stderr.contains("No device is paired") && no_device.stderr.contains("[not_found]"), "{}", no_device.stderr);

    // Setup onto the card, with a relative path for the program.
    let setup = run(home.path(), &["setup", "3ds", "--card", card.to_str().unwrap(), "--program", "work/shell.3dsx"]);
    assert_eq!(setup.status, 0, "{}", setup.stderr);
    assert!(setup.stdout.contains("a new pairing was written; the 3DS's software was installed.") && setup.stdout.contains("Next: Done."), "{}", setup.stdout);
    let again = run(home.path(), &["--json", "setup", "3ds", "--card", card.to_str().unwrap(), "--program", shell.to_str().unwrap()]).json();
    assert_eq!((again["pairing"].as_str(), again["program"]["action"].as_str(), again["confirmed"].as_bool()), (Some("kept"), Some("kept"), Some(true)));
    let id = again["device"].as_str().unwrap().to_owned();
    let devices = run(home.path(), &["devices"]);
    assert!(devices.stdout.contains(&id) && devices.stdout.contains("not linked"), "{}", devices.stdout);
    let not_linked = run(home.path(), &["--json", "device", "status"]);
    assert_eq!(not_linked.status, 1);
    assert_eq!(not_linked.json()["error"]["data"]["kind"], "not_linked");

    // The reference device links with that pairing.
    let status = run(home.path(), &["--json", "status"]).json();
    let mut device = device(&card, status["id"].as_str().unwrap(), status["carriers"][0]["port"].as_u64().unwrap());
    let fields = run(home.path(), &["device", &id, "status"]);
    assert_eq!(fields.status, 0, "{}", fields.stderr);
    assert!(fields.stdout.lines().any(|line| line == "phase: booted"), "{}", fields.stdout);
    assert_eq!(run(home.path(), &["--json", "device", "status"]).json()["phase"], "booted");

    // `device logs` follows the device's lines until it is stopped.
    let mut logs = command(home.path(), &["device", "logs"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut note = String::new();
    BufReader::new(logs.stderr.take().unwrap()).read_line(&mut note).unwrap();
    assert!(note.starts_with("Logs of Test 3DS"), "{note}");
    use std::io::Write;
    writeln!(device.stdin.as_mut().unwrap(), r#"{{"cmd":"log","text":"from the cli test","level":"info","source":"host","frame":7}}"#).unwrap();
    let mut line = String::new();
    BufReader::new(logs.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "[7] info host: from the cli test");
    let _ = logs.kill();
    let _ = logs.wait();

    let mut package = (0..4088u32).map(|i| (i * 7 + 13) as u8).collect::<Vec<u8>>();
    package[0] = 1;
    let hash = package.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3));
    package.extend_from_slice(&hash.to_le_bytes());
    std::fs::write(work.join("demo.pocket"), &package).unwrap();
    let pushed = run(home.path(), &["device", "push", "work/demo.pocket"]);
    assert_eq!(pushed.status, 0, "{}", pushed.stderr);
    assert!(pushed.stdout.starts_with("Pushed Demo (dev.demo): generation"), "{}", pushed.stdout);
    let pushed = run(home.path(), &["--json", "device", &id, "push", "work/demo.pocket"]).json();
    assert_eq!(pushed["hash"], format!("{hash:016x}"));
    let launched = run(home.path(), &["device", "launch", "demo"]);
    assert_eq!(launched.stdout.trim(), "Launched demo.");
    let shot = run(home.path(), &["device", "shot", "screen.png"]);
    assert_eq!(shot.status, 0, "{}", shot.stderr);
    assert!(std::fs::read(home.path().join("screen.png")).unwrap().starts_with(b"\x89PNG"));
    let into_cache = run(home.path(), &["--json", "device", "shot"]).json();
    assert!(Path::new(into_cache["path"].as_str().unwrap()).starts_with(home.path().join("cache/screenshots")));
    // A local build replaces the device's program whatever its name.
    std::fs::write(work.join("pocket-cli.3dsx"), &program).unwrap();
    let installed = run(home.path(), &["device", "update", "--program", "work/pocket-cli.3dsx", "--no-launch"]);
    assert_eq!(installed.stdout.trim(), "The 3DS's software was installed. It runs at the device's next start.", "{}", installed.stderr);
    let installed = run(home.path(), &["--json", "device", &id, "update", "--program", "work/pocket-cli.3dsx", "--no-launch"]).json();
    assert_eq!(installed, serde_json::json!({"phase": "installed", "name": "pocket-shell.3dsx", "version": null}));
    // No release source answers: the update fails and sends nothing.
    let unreachable = run(home.path(), &["device", "update"]);
    assert_eq!(unreachable.status, 1);
    assert!(unreachable.stderr.contains("Hub could not get the 3DS's software, so nothing was sent to the device") && unreachable.stderr.contains("[release, reason unreachable]"), "{}", unreachable.stderr);
    let unreachable = run(home.path(), &["--json", "device", "update"]).json();
    assert_eq!((unreachable["error"]["data"]["kind"].as_str(), unreachable["error"]["data"]["reason"].as_str()), (Some("release"), Some("unreachable")));
    assert_eq!(run(home.path(), &["--json", "devices"]).json()["devices"][0]["program"], serde_json::json!({"version": null, "latest": null, "update": false}), "the look failed: nothing is known");
    let apps = run(home.path(), &["device", "apps"]);
    assert_eq!(apps.status, 0);
    assert_eq!(run(home.path(), &["--json", "device", "apps"]).json(), Value::Null, "no applet report yet");
    let bad = run(home.path(), &["device", "push", "work/shell.3dsx"]);
    assert_eq!(bad.status, 1);
    assert!(bad.stderr.contains("[invalid]"), "{}", bad.stderr);

    // Services, while `watch` prints the events.
    // `watch` says on standard error once it is subscribed; the install comes after that.
    let mut watch = command(home.path(), &["watch", "--topic", "service.changed"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut note = String::new();
    BufReader::new(watch.stderr.take().unwrap()).read_line(&mut note).unwrap();
    assert!(note.starts_with("Watching the core's events"), "{note}");
    assert_eq!(run(home.path(), &["services"]).stdout.trim(), "No service is installed.");
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/service/examples/link-channel");
    let installed = run(home.path(), &["service", "install", package.to_str().unwrap()]);
    assert_eq!(installed.stdout.trim(), "Installed dev.pocket-nexus.link-channel, release dev.", "{}", installed.stderr);
    let mut first = String::new();
    BufReader::new(watch.stdout.take().unwrap()).read_line(&mut first).unwrap();
    assert!(first.starts_with("service dev.pocket-nexus.link-channel release dev: stopped"), "{first}");
    let _ = watch.kill();
    let _ = watch.wait();
    let services = run(home.path(), &["--json", "services"]).json();
    assert_eq!(services["services"][0]["state"], "stopped");
    let started = run(home.path(), &["service", "start", "dev.pocket-nexus.link-channel"]);
    assert!(started.stdout.contains("running"), "{}", started.stdout);
    let logs = run(home.path(), &["--json", "service", "logs", "dev.pocket-nexus.link-channel"]).json();
    assert!(logs["lines"].as_array().unwrap().iter().any(|line| line["text"].as_str().unwrap().contains("installed release dev")));
    assert!(run(home.path(), &["service", "stop", "dev.pocket-nexus.link-channel"]).stdout.contains("stopped"));
    assert_eq!(run(home.path(), &["service", "remove", "dev.pocket-nexus.link-channel"]).stdout.trim(), "Removed dev.pocket-nexus.link-channel; its data folder is kept.");
    assert_eq!(run(home.path(), &["service", "start", "dev.pocket-nexus.link-channel"]).status, 1);
    assert!(run(home.path(), &["runtime"]).stdout.contains("is installed at"));

    let forgotten = run(home.path(), &["device", "forget"]);
    assert_eq!(forgotten.status, 0, "{}", forgotten.stderr);
    assert_eq!(run(home.path(), &["--json", "devices"]).json()["devices"], serde_json::json!([]));
    let _ = device.kill();
    let _ = device.wait();
    shutdown(home.path());
}

/// `store`, `store details`, `store sample` and `store install` against the fake applet store, in both
/// output modes. The first command starts the core with the store's URL and
/// key; the later ones reach that core.
#[test]
fn the_store_in_both_modes() {
    use pocket_hub_applet_store::test_support::{FakeStore, build_package, fixture, fixture_details, icon, public, test_key};
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let store = runtime.block_on(FakeStore::start());
    let notes = store.publish(&test_key(1), build_package("dev.pocket-nexus.notes", "1.0.0", "Notes", &[("3ds-dev", "notes")], 30_000), icon(1), "pocket-nexus", "Notes.");
    store.publish(&test_key(2), build_package("io.example.rg", "2.0.0", "RG", &[("rg-ds-plus", "rg")], 100), icon(2), "someone", "Elsewhere.");
    let hn = store.publish(&test_key(1), fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News.");
    store.set_details(&hn.package, Some(fixture_details("hn")));
    let hn = store.set_sample(&hn, &serde_json::json!({"mock": 1, "routes": []}));
    let home = home();
    let _stop = Stop(home.path());
    let key = public(&test_key(1));
    let with_store = |args: &[&str]| {
        let Output { status, stdout, stderr } = command(home.path(), args).env("POCKET_HUB_STORE_URL", &store.url).env("POCKET_HUB_STORE_KEY", &key).output().unwrap();
        Run { status: status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&stdout).into_owned(), stderr: String::from_utf8_lossy(&stderr).into_owned() }
    };

    let table = with_store(&["store"]);
    assert_eq!(table.status, 0, "{}", table.stderr);
    let lines: Vec<&str> = table.stdout.lines().collect();
    assert!(lines[0].starts_with("TITLE") && lines[0].contains("SIZE") && !lines[0].contains("ON DEVICE"), "{}", table.stdout);
    assert!(table.stdout.contains("Notes") && table.stdout.contains("RG (by someone)") && table.stdout.contains("Catalog of "), "{}", table.stdout);
    let json = with_store(&["--json", "store"]).json();
    assert_eq!(json["apps"].as_array().unwrap().len(), 3);
    assert!(json["apps"][0].get("installed").is_none() && json["fetchedAt"].is_i64(), "{json}");

    // An applet's details, as the store reads them out of its package.
    let details = with_store(&["store", "details", &hn.id]);
    assert_eq!(details.status, 0, "{}", details.stderr);
    let lines: Vec<&str> = details.stdout.lines().collect();
    assert_eq!(lines[0], "dev.pocket-nexus.hn 0.3.0");
    assert_eq!(lines[1], "Built for:   Omarchy Pocket (rg-ds-plus, host ABI 12), M5Stack Tab5 (tab5-home, host ABI 12), Nintendo 3DS (3ds-dev, host ABI 12)");
    assert!(details.stdout.contains("Uses:        input.analog.left, system.agent\n"), "{}", details.stdout);
    assert!(details.stdout.contains("Surface:     a touch surface of its own, 320 × 240 to 640 × 720 points\n"), "{}", details.stdout);
    let plain = with_store(&["store", "details", &notes.id]);
    assert_eq!(plain.stdout, "dev.pocket-nexus.notes 1.0.0\nBuilt for:   Nintendo 3DS (3ds-dev, host ABI 12)\nNeeds:       nothing\nUses:        nothing\nApplet API:  1\n");
    let json = with_store(&["--json", "store", "details", &hn.id]).json();
    assert_eq!((json["devices"][2]["hostAbi"].as_u64(), json["surface"]["size"]["max"][1].as_u64(), json["framework"].as_str()), (Some(12), Some(720), Some("solid")), "{json}");
    // The files the store's player runs it from, downloaded into Hub's cache.
    let sample = with_store(&["store", "sample", &hn.id]);
    assert_eq!(sample.status, 0, "{}", sample.stderr);
    let mock = format!("{}.json", hn.mock.as_deref().unwrap());
    assert!(sample.stdout.starts_with("Package:      ") && sample.stdout.contains("\nSample data:  ") && sample.stdout.trim_end().ends_with(&mock), "{}", sample.stdout);
    let none = with_store(&["--json", "store", "sample", &notes.id]);
    assert_eq!((none.status, none.json()["error"]["data"]["kind"].as_str()), (1, Some("not_found")));
    let unlisted = with_store(&["--json", "store", "details", "io.example.none"]);
    assert_eq!((unlisted.status, unlisted.json()["error"]["data"]["kind"].as_str()), (1, Some("not_found")));

    // A device, paired on a card and linked.
    let work = home.path().join("work");
    std::fs::create_dir_all(work.join("sd/3ds")).unwrap();
    let card = work.join("sd");
    let shell = work.join("shell.3dsx");
    let mut program = b"3DSX".to_vec();
    program.extend((0..4096u32).map(|i| (i * 31) as u8));
    std::fs::write(&shell, &program).unwrap();
    let setup = with_store(&["--json", "setup", "3ds", "--card", card.to_str().unwrap(), "--program", shell.to_str().unwrap()]).json();
    let id = setup["device"].as_str().unwrap().to_owned();
    let status = with_store(&["--json", "status"]).json();
    let mut linked = device(&card, status["id"].as_str().unwrap(), status["carriers"][0]["port"].as_u64().unwrap());

    let for_device = with_store(&["store", "--device", &id]);
    assert_eq!(for_device.status, 0, "{}", for_device.stderr);
    assert!(for_device.stdout.lines().next().unwrap().contains("ON DEVICE"), "{}", for_device.stdout);
    assert!(!for_device.stdout.contains("RG"), "the RG-only app is not the 3DS's: {}", for_device.stdout);
    assert!(for_device.stdout.lines().any(|line| line.contains("Notes") && line.contains(" * ") && line.trim_end().ends_with('-')), "not downloaded yet: {}", for_device.stdout);
    assert!(for_device.stdout.contains("* not downloaded by Hub yet"), "{}", for_device.stdout);

    // Install it: the one paired device, without --device.
    let installed = with_store(&["store", "install", &notes.id]);
    assert_eq!(installed.status, 0, "{}", installed.stderr);
    assert!(installed.stdout.starts_with("Installed Notes (dev.pocket-nexus.notes): generation "), "{}", installed.stdout);
    let again = with_store(&["--json", "store", "install", &notes.id, "--device", &id]).json();
    assert_eq!(again["app"]["output"], "notes");
    // The device's report names it.
    let mut held = serde_json::Value::Null;
    for _ in 0..100 {
        let json = with_store(&["--json", "store", "--device", &id]).json();
        held = json["apps"].as_array().unwrap().iter().find(|app| app["id"] == notes.id.as_str()).cloned().unwrap();
        if !held["installed"].is_null() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!((held["installed"]["version"].as_str(), held["update"].as_bool()), (Some("1.0.0"), Some(false)), "{held}");
    let shown = with_store(&["store", "--device", &id]);
    assert!(shown.stdout.lines().any(|line| line.contains("Notes") && line.contains("1.0.0 installed") && !line.contains(" * ")), "{}", shown.stdout);
    assert_eq!(held["cached"], true);

    // Errors in both modes.
    let none = with_store(&["--json", "store", "install", "io.example.none"]);
    assert_eq!((none.status, none.json()["error"]["data"]["kind"].as_str()), (1, Some("not_found")));
    store.catalog_status(Some(500));
    let down = with_store(&["store", "--refresh"]);
    assert_eq!(down.status, 1);
    assert!(down.stderr.contains("[store, reason unreachable]"), "{}", down.stderr);
    let down = with_store(&["--json", "store", "--refresh"]).json();
    assert_eq!(down["error"]["data"]["reason"], "unreachable");
    let _ = linked.kill();
    let _ = linked.wait();
    drop(runtime);
}

/// The reference device with no pairing, as a Tab5 that never met a Hub: it
/// opens a pairing with the code `code` and asks the Hub. Returns once it
/// has asked; `pair` lines it writes later go to `said`.
fn asking_tab5(dir: &Path, hub: &str, port: u64, code: &str) -> (Child, std::sync::mpsc::Receiver<Value>) {
    std::fs::create_dir_all(dir).unwrap();
    let config = serde_json::json!({"dir": dir, "hub": hub, "address": "127.0.0.1", "port": port, "autoConnect": false, "name": "Desk Tab5", "model": "M5Stack Tab5", "target": "tab5-home", "features": ["apps", "runtime", "runtime.remove", "catalog", "install", "net.http"]});
    let harness = concat!(env!("CARGO_MANIFEST_DIR"), "/../hub-core/tests/harness/device.ts");
    let mut child = Command::new(bun()).arg(harness).arg(config.to_string()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", serde_json::json!({"cmd": "pairOpen", "code": code})).unwrap();
        writeln!(stdin, "{}", serde_json::json!({"cmd": "pairAsk"})).unwrap();
        stdin.flush().unwrap();
    }
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let deadline = Instant::now() + Duration::from_secs(10);
    for line in lines.by_ref() {
        if line.unwrap().contains("\"ev\":\"pair-ask\",\"ok\":true") {
            break;
        }
        assert!(Instant::now() < deadline, "the device did not ask to be paired");
    }
    let (tx, said) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in lines.map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                let _ = tx.send(value);
            }
        }
    });
    (child, said)
}

/// `pair`: a device that shows a code is listed, dismissed, and paired by
/// its code, in both output modes.
#[test]
fn pairing_a_device_by_its_code() {
    let home = home();
    let _stop = Stop(home.path());
    let none = run(home.path(), &["pair"]);
    assert_eq!((none.status, none.stdout.trim()), (0, "No device asks to be paired. On the device, open its pairing page and choose this computer; then run this again."));
    assert_eq!(run(home.path(), &["--json", "pair"]).json(), serde_json::json!({"requests": []}));
    let nobody = run(home.path(), &["pair", "--code", "314159"]);
    assert_eq!(nobody.status, 1);
    assert!(nobody.stderr.contains("No device asks to be paired.") && nobody.stderr.contains("[not_found]"), "{}", nobody.stderr);

    let status = run(home.path(), &["--json", "status"]).json();
    let (hub, port) = (status["id"].as_str().unwrap().to_owned(), status["carriers"][0]["port"].as_u64().unwrap());
    let (mut tab5, _said) = asking_tab5(&home.path().join("tab5"), &hub, port, "314159");
    // The request is Hub's a moment after the device has sent it.
    let deadline = Instant::now() + Duration::from_secs(10);
    let listed = loop {
        let listed = run(home.path(), &["--json", "pair"]).json();
        if listed["requests"].as_array().is_some_and(|requests| requests.len() == 1) {
            break listed;
        }
        assert!(Instant::now() < deadline, "the request did not reach Hub");
        std::thread::sleep(Duration::from_millis(50));
    };
    let request = &listed["requests"][0];
    let id = request["device"].as_str().unwrap().to_owned();
    assert_eq!((request["kind"].as_str(), request["name"].as_str(), request["model"].as_str(), request["address"].as_str()), (Some("tab5"), Some("Desk Tab5"), Some("M5Stack Tab5"), Some("127.0.0.1")));
    let table = run(home.path(), &["pair"]);
    assert!(table.stdout.contains(&id) && table.stdout.contains("Desk Tab5") && table.stdout.contains("pocket-hub pair --code <digits>"), "{}", table.stdout);

    // What is no code, and a code that is not the device's.
    let not_a_code = run(home.path(), &["pair", "--code", "31415"]);
    assert_eq!(not_a_code.status, 1);
    assert!(not_a_code.stderr.contains("[invalid]"), "{}", not_a_code.stderr);
    assert_eq!(run(home.path(), &["pair", "--code", "314159", "--decline"]).status, 2, "one or the other");
    let wrong = run(home.path(), &["pair", "--code", "271828"]);
    assert_eq!(wrong.status, 1);
    assert!(wrong.stderr.contains("The code is not the one the device shows.") && wrong.stderr.contains("[pairing, reason wrong_code]"), "{}", wrong.stderr);
    assert_eq!(run(home.path(), &["--json", "devices"]).json(), serde_json::json!({"devices": []}), "a wrong code pairs nothing");

    // The device asks again within a second; this time by its id.
    let deadline = Instant::now() + Duration::from_secs(10);
    while run(home.path(), &["--json", "pair"]).json()["requests"].as_array().is_none_or(|requests| requests.is_empty()) {
        assert!(Instant::now() < deadline, "the device did not ask again");
        std::thread::sleep(Duration::from_millis(50));
    }
    let paired = run(home.path(), &["pair", "--code", "314159", "--device", &id]);
    assert_eq!(paired.status, 0, "{}", paired.stderr);
    assert_eq!(paired.stdout.trim(), format!("Paired Desk Tab5 ({id}). It connects by itself, now and whenever it is on."));
    let devices = run(home.path(), &["--json", "devices"]).json();
    assert_eq!((devices["devices"][0]["id"].as_str(), devices["devices"][0]["kind"].as_str()), (Some(id.as_str()), Some("tab5")));
    assert_eq!(devices["devices"][0]["supports"], serde_json::json!([]));
    // It links by itself; what its family does not take says so.
    let deadline = Instant::now() + Duration::from_secs(10);
    while run(home.path(), &["--json", "devices"]).json()["devices"][0]["link"].is_null() {
        assert!(Instant::now() < deadline, "the paired device did not link");
        std::thread::sleep(Duration::from_millis(50));
    }
    let shot = run(home.path(), &["device", "shot"]);
    assert_eq!(shot.status, 1);
    assert!(shot.stderr.contains("This device takes no screenshot.") && shot.stderr.contains("[link, code unsupported]"), "{}", shot.stderr);

    // A request that is dismissed: the device reads why.
    let _ = tab5.kill();
    let _ = tab5.wait();
    let (mut other, said) = asking_tab5(&home.path().join("other"), &hub, port, "161803");
    let deadline = Instant::now() + Duration::from_secs(10);
    while run(home.path(), &["--json", "pair"]).json()["requests"].as_array().is_none_or(|requests| requests.is_empty()) {
        assert!(Instant::now() < deadline, "the second device's request did not reach Hub");
        std::thread::sleep(Duration::from_millis(50));
    }
    let dismissed = run(home.path(), &["pair", "--decline"]);
    assert_eq!((dismissed.status, dismissed.stdout.trim()), (0, "The request was dismissed; the device says so."));
    assert_eq!(run(home.path(), &["--json", "pair"]).json(), serde_json::json!({"requests": []}));
    {
        use std::io::Write;
        writeln!(other.stdin.as_mut().unwrap(), "{}", serde_json::json!({"cmd": "pair", "tag": "after"})).unwrap();
    }
    let state = said.iter().find(|line| line["ev"] == "pair" && line["tag"] == "after").expect("the device answers");
    assert_eq!(state["pair"]["error"], "that computer dismissed the request");
    let _ = other.kill();
    let _ = other.wait();
    shutdown(home.path());
}

#[test]
fn usage_errors_and_help() {
    let home = home();
    let help = run(home.path(), &["--help"]);
    assert_eq!(help.status, 0);
    assert!(help.stdout.contains("Exit status: 0 success, 1 the operation failed, 2 a usage error, 3 no Pocket Hub core could be reached."));
    let device_help = run(home.path(), &["device", "--help"]);
    assert!(device_help.stdout.contains("update [--program <file.3dsx> | --release <tag>] [--no-launch]"), "{}", device_help.stdout);
    assert_eq!(run(home.path(), &["device", "update", "--program", "a.3dsx", "--release", "v1"]).status, 2);
    assert_eq!(run(home.path(), &["device", "install-program", "a.3dsx"]).status, 2, "replaced by update");
    assert_eq!(run(home.path(), &["frobnicate"]).status, 2);
    assert_eq!(run(home.path(), &["device", "0123456789abcdef", "explode"]).status, 2);
    // Usage errors need no core: none was started.
    assert!(!home.path().join("hubd.sock").exists());
}

/// `device update` from a release on a GitHub-shaped server on loopback, and
/// the program's version and update in `devices` and `device status`, in
/// both output modes.
#[test]
fn device_update_in_both_modes() {
    use pocket_hub_setup::test_support::{FakeHttp, fake_3dsx, github_release};
    use std::io::Write;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let release = fake_3dsx(40_000, 9);
    let releases = runtime.block_on(FakeHttp::start(github_release("test/shell", "v2026.11.11", "2026.11.11", &release, true)));
    let home = home();
    let _stop = Stop(home.path());
    let hub = |args: &[&str]| {
        let Output { status, stdout, stderr } = command(home.path(), args).env("POCKET_HUB_SHELL_API", &releases.base).env("POCKET_HUB_SHELL_REPO", "test/shell").output().unwrap();
        Run { status: status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&stdout).into_owned(), stderr: String::from_utf8_lossy(&stderr).into_owned() }
    };

    let work = home.path().join("work");
    std::fs::create_dir_all(work.join("sd/3ds")).unwrap();
    let card = work.join("sd");
    std::fs::write(work.join("shell.3dsx"), fake_3dsx(4096, 1)).unwrap();
    let setup = hub(&["--json", "setup", "3ds", "--card", card.to_str().unwrap(), "--program", "work/shell.3dsx"]).json();
    let id = setup["device"].as_str().unwrap().to_owned();
    let unknown = hub(&["devices"]);
    assert!(unknown.stdout.lines().next().unwrap().contains("PROGRAM"), "{}", unknown.stdout);
    assert!(unknown.stdout.lines().nth(1).unwrap().contains(" -  "), "no version before a report: {}", unknown.stdout);

    // The device links (Hub looks the release up) and reports its program.
    let status = hub(&["--json", "status"]).json();
    let mut linked = device(&card, status["id"].as_str().unwrap(), status["carriers"][0]["port"].as_u64().unwrap());
    writeln!(linked.stdin.as_mut().unwrap(), r#"{{"cmd":"apps","count":0,"home":"2026.11.2"}}"#).unwrap();
    let mut program = Value::Null;
    for _ in 0..200 {
        program = hub(&["--json", "devices"]).json()["devices"][0]["program"].clone();
        if !program["version"].is_null() && !program["latest"].is_null() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(program, serde_json::json!({"version": "2026.11.2", "latest": "2026.11.11", "update": true}));
    let listed = hub(&["devices"]);
    assert!(listed.stdout.contains("2026.11.2 (update available: 2026.11.11)"), "{}", listed.stdout);
    let status = hub(&["device", "status"]);
    assert!(status.stdout.lines().any(|line| line == "program: 2026.11.2 (update available: 2026.11.11)"), "{}", status.stdout);
    assert_eq!(hub(&["--json", "device", "status"]).json()["phase"], "booted", "--json prints the runtime status as it came");

    // The latest release, then a tag, not launched; then launched.
    let updated = hub(&["device", "update", "--no-launch"]);
    assert_eq!(updated.status, 0, "{}", updated.stderr);
    assert_eq!(updated.stdout.trim(), "Version 2026.11.11 of the 3DS's software was installed. It runs at the device's next start.");
    let tagged = hub(&["--json", "device", &id, "update", "--release", "v2026.11.11", "--no-launch"]).json();
    assert_eq!(tagged, serde_json::json!({"phase": "installed", "name": "pocket-shell.3dsx", "version": "2026.11.11"}));
    let missing = hub(&["device", "update", "--release", "v1999.01.01"]);
    assert_eq!(missing.status, 1);
    assert!(missing.stderr.contains("There is no release v1999.01.01 of the 3DS's software. [release, reason not_found]"), "{}", missing.stderr);
    let missing = hub(&["--json", "device", "update", "--release", "v1999.01.01"]).json();
    assert_eq!(missing["error"]["data"], serde_json::json!({"kind": "release", "reason": "not_found"}));
    let launched = hub(&["device", "update"]);
    assert_eq!(launched.status, 0, "{}", launched.stderr);
    assert_eq!(launched.stdout.trim(), "Version 2026.11.11 of the 3DS's software was installed. The device restarts and connects again by itself.");
    assert_eq!(releases.hits("/dl/v2026.11.11/pocket-shell-v2026.11.11.3dsx"), 1, "downloaded once, then from the cache");
    let restarting = hub(&["devices"]);
    assert!(restarting.stdout.contains("restarting into a program"), "{}", restarting.stdout);

    let _ = linked.kill();
    let _ = linked.wait();
    drop(runtime);
}

/// A core started by the command line holds none of the descriptors the
/// command line had: this test gives `pocket-hub` a file at descriptor 150
/// without close-on-exec, and the core it starts must not hold that file.
#[test]
fn a_started_core_holds_no_descriptor_of_its_client() {
    use std::os::fd::AsRawFd;
    let home = home();
    let _stop = Stop(home.path());
    let marker = home.path().join(format!("leak-marker-{}.txt", std::process::id()));
    let file = std::fs::File::create(&marker).unwrap();
    // SAFETY: dup2 onto a number this test owns; the copy has no close-on-exec,
    // so `pocket-hub` inherits it, and so would the core it starts.
    assert_eq!(unsafe { libc::dup2(file.as_raw_fd(), 150) }, 150);
    let status = run(home.path(), &["status"]);
    // SAFETY: this test's own copy.
    unsafe { libc::close(150) };
    assert_eq!(status.status, 0, "{}", status.stderr);
    // The core holds its home's lock file: that names its pid.
    let pid = Command::new("lsof").arg("-t").arg(home.path().join("hubd.lock")).output().unwrap();
    let pid = String::from_utf8_lossy(&pid.stdout).lines().next().expect("the core holds its lock").trim().to_owned();
    let open = Command::new("lsof").args(["-n", "-P", "-Fn", "-p", &pid]).output().unwrap();
    let open = String::from_utf8_lossy(&open.stdout);
    assert!(open.contains("hubd.lock"), "lsof lists the core's files: {open}");
    assert!(!open.contains("leak-marker"), "the core holds its client's descriptor: {open}");
    drop(file);
    shutdown(home.path());
}

/// A resident core of Pocket Hub 0.2.2, answering as that release does
/// (measured against the binaries built from `v0.2.1` and `v0.2.2`):
/// protocol 1, and any other protocol refused with `-32602`, its sentence
/// and no `data`. It answers `hub.hello` and `hub.shutdown` alone, records
/// every call, and removes its socket after answering `hub.shutdown`, as a
/// core does.
struct EarlierCore {
    socket: PathBuf,
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl EarlierCore {
    fn start(home: &Path) -> Self {
        use std::io::Write;
        let socket = home.join("hubd.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (recorded, path) = (calls.clone(), socket.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut writer = stream.try_clone().unwrap();
                let mut hello = false;
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let (id, method) = (request["id"].clone(), request["method"].as_str().unwrap_or("").to_owned());
                    let answer = if method == "hub.hello" {
                        let protocol = request["params"]["protocol"].as_u64().unwrap_or(0);
                        recorded.lock().unwrap().push(format!("hub.hello {protocol}"));
                        if protocol == 1 {
                            hello = true;
                            serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"protocol": 1, "hub": {"id": "0011223344556677", "name": "Studio Mac", "version": "0.2.2"}}})
                        } else {
                            serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("This Hub speaks protocol 1, not {protocol}.")}})
                        }
                    } else {
                        recorded.lock().unwrap().push(method.clone());
                        if hello {
                            serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}})
                        } else {
                            serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32600, "message": "The first request must be hub.hello."}})
                        }
                    };
                    writeln!(writer, "{answer}").unwrap();
                    if method == "hub.shutdown" && hello {
                        let _ = std::fs::remove_file(&path);
                        return;
                    }
                }
            }
        });
        Self { socket, calls }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

/// CONTROL-API.md §1: the command line never replaces a core. Against one of
/// another protocol it says so in one line and starts nothing; `shutdown`
/// stops it through the two frozen methods, in its own protocol, and calls
/// nothing else in that dialect.
#[test]
fn a_core_of_another_protocol_is_named_and_stopped_never_replaced() {
    let home = home();
    let earlier = EarlierCore::start(home.path());
    let status = run(home.path(), &["status"]);
    assert_eq!(status.status, 3, "{}", status.stderr);
    assert_eq!(
        status.stderr.trim(),
        format!(
            "pocket-hub: The Pocket Hub core on {} speaks protocol 1 of the control API and this pocket-hub speaks 3: use the pocket-hub of its version, or stop it with `pocket-hub shutdown`.",
            earlier.socket.display()
        )
    );
    assert!(!home.path().join("logs").exists(), "no core of this version was started beside it");
    assert_eq!(earlier.calls(), ["hub.hello 3"]);

    let stopped = run(home.path(), &["shutdown"]);
    assert_eq!((stopped.status, stopped.stdout.trim()), (0, "The Pocket Hub core stopped."), "{}", stopped.stderr);
    assert_eq!(earlier.calls(), ["hub.hello 3", "hub.hello 3", "hub.hello 1", "hub.shutdown"]);
    assert!(!earlier.socket.exists());
    let none = run(home.path(), &["shutdown"]);
    assert_eq!(none.status, 3);
    assert!(none.stderr.contains("there is nothing to stop"), "{}", none.stderr);
}
