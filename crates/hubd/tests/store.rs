//! `store.apps`, `store.details` and `store.install` through the real
//! `pocket-hubd` binary, against the fake applet store, onto the reference
//! device under Bun: progress and cancel, `installed` and `update` from the
//! device's apps report, an applet's details as the store reads them, the
//! error kinds, the device mutex, and the link's own catalog of the same
//! offer.

mod common;

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use common::*;
use pocket_hub_api::types::*;
use pocket_hub_api::{Client, ErrorKind, methods};
use pocket_hub_applet_store::test_support::*;
use serde_json::{Value, json};

async fn wait_for_socket(socket: &Path) {
    for _ in 0..400 {
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("pocket-hubd did not answer on {}", socket.display());
}

/// The binary on a temporary home, with the fake store and its official key.
async fn core(home: &Path, store: &FakeStore) -> tokio::process::Child {
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_pocket-hubd"))
        .env("POCKET_HUB_NAME", HUB_NAME)
        .arg("--home")
        .arg(home)
        .args(["--link-port", "0", "--no-beacons", "--mutex-wait-ms", "400"])
        .arg("--bun")
        .arg(bun())
        .arg("--store-url")
        .arg(&store.url)
        .arg("--store-key")
        .arg(public(&test_key(1)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("pocket-hubd runs");
    wait_for_socket(&home.join("hubd.sock")).await;
    child
}

/// A device paired by `setup.card` and linked; returns it with its id.
async fn paired_device(client: &Client, work: &Path, hub: &str, port: u16) -> (RefDevice, String) {
    let shell = program(work, "pocket-shell.3dsx", 32 * 1024);
    let card = card(work, "sd");
    let report: InstallReport = client
        .call_as(methods::SETUP_CARD, &SetupCardParams { kind: "3ds".into(), path: card.display().to_string(), program: Some(ProgramChoice::Path { path: shell.display().to_string() }) })
        .await
        .unwrap();
    let device = RefDevice::spawn(&link_dir(&card), hub, port).await;
    event(client, |e| topic(e, "device.changed") && !e["device"]["link"].is_null()).await;
    (device, report.device)
}

async fn install(client: &Client, device: &str, app: &str) -> Result<Value, pocket_hub_api::ClientError> {
    client.call(methods::STORE_INSTALL, &StoreInstallParams { device: device.into(), app: app.into() }).await
}

fn app<'a>(apps: &'a StoreApps, id: &str) -> &'a StoreApp {
    apps.apps.iter().find(|app| app.id == id).unwrap_or_else(|| panic!("no {id} in {apps:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_store_through_the_binary() {
    let home = temp_home();
    let work = temp_home();
    let store = FakeStore::start().await;
    let official = test_key(1);
    let someone = test_key(2);
    let hn = store.publish(&official, fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News: top stories and threads.");
    let tool = store.publish(&someone, build_package("io.example.tool", "1.0.0", "Tool", &[("3ds-dev", "tool"), ("tab5-home", "tool")], 20_000), icon(1), "someone", "A tool.");
    let rg = store.publish(&someone, build_package("io.example.rg", "1.0.0", "RG", &[("rg-ds-plus", "rg")], 100), icon(2), "someone", "Elsewhere.");
    let mut bad = store.publish(&someone, build_package("io.example.bad", "1.0.0", "Bad", &[("3ds-dev", "bad")], 100), icon(3), "someone", "Forged.");
    bad.signature = sign(&official, &bad);
    store.set_entry(&bad);
    let corrupt = store.publish(&someone, build_package("io.example.corrupt", "1.0.0", "Corrupt", &[("3ds-dev", "corrupt")], 100), icon(4), "someone", "Broken bytes.");
    store.fault(&corrupt.package, Fault::Corrupt);
    let missing = store.publish(&someone, build_package("io.example.missing", "1.0.0", "Missing", &[("3ds-dev", "missing")], 100), icon(5), "someone", "Gone.");
    store.fault(&missing.package, Fault::Missing);
    // 600 kB in 16 kB pieces 50 ms apart: about 2 s, long enough to cancel.
    let slow = store.publish(&someone, build_package("io.example.slow", "1.0.0", "Slow", &[("3ds-dev", "slow")], 600_000), icon(6), "someone", "Slow.");
    store.fault(&slow.package, Fault::Slow { chunk: 16 * 1024, delay: Duration::from_millis(50) });

    let mut hubd = core(home.path(), &store).await;
    let socket = home.path().join("hubd.sock");
    let client = client(&socket, "Pocket Hub").await;
    client.call(methods::SUBSCRIBE, &json!({})).await.unwrap();
    let status: HubStatus = client.call_as(methods::STATUS, &json!({})).await.unwrap();
    let port = status.carriers[0].port.unwrap();

    // --- store.apps without a device: every app, no `installed` or `update`.
    let raw = client.call(methods::STORE_APPS, &StoreAppsParams::default()).await.unwrap();
    let all: StoreApps = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(all.apps.len(), 7);
    assert!(raw["apps"].as_array().unwrap().iter().all(|app| app.get("installed").is_none() && app.get("update").is_none()), "{raw}");
    let listed = app(&all, &hn.id);
    assert_eq!((listed.title.as_str(), listed.version.as_str(), listed.author.as_str(), listed.official, listed.bytes), ("Hacker News", "0.3.0", "pocket-nexus", true, 2_210_904));
    assert_eq!(listed.targets, ["3ds-dev", "rg-ds-plus", "tab5-home"]);
    assert!(!app(&all, &tool.id).official);
    // No device is paired yet: Hub has downloaded nothing.
    assert!(all.apps.iter().all(|app| !app.cached), "{raw}");
    assert_eq!(raw["apps"][0]["cached"], false);
    assert_eq!(app(&all, &tool.id).services, ["store"]);

    // --- store.details: the store's reading of the package the catalog names,
    // with no download of the package itself.
    store.set_details(&hn.package, Some(fixture_details("hn")));
    let raw = client.call(methods::STORE_DETAILS, &StoreDetailsParams { app: hn.id.clone() }).await.unwrap();
    let details: StoreDetails = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!((details.id.as_str(), details.version.as_str()), ("dev.pocket-nexus.hn", "0.3.0"));
    assert_eq!(details.devices.iter().map(|device| (device.target.as_str(), device.name.as_str(), device.host_abi)).collect::<Vec<_>>(), [("rg-ds-plus", "Omarchy Pocket", Some(12)), ("tab5-home", "M5Stack Tab5", Some(12)), ("3ds-dev", "Nintendo 3DS", Some(12))]);
    assert_eq!((details.needs.len(), details.uses.as_slice(), details.framework.as_deref(), details.api), (5, ["input.analog.left".to_owned(), "system.agent".to_owned()].as_slice(), Some("solid"), Some(1)));
    assert!(raw["devices"][0].get("build").is_none(), "a build's directory is not passed on: {raw}");
    assert_eq!(store.hits(&format!("/v1/packages/{}", hn.package)), 0);
    // A package that names no surface: null members, present.
    let plain = client.call(methods::STORE_DETAILS, &StoreDetailsParams { app: tool.id.clone() }).await.unwrap();
    assert_eq!((plain["surface"].clone(), plain["framework"].clone(), plain["needs"].clone()), (Value::Null, Value::Null, json!([])), "{plain}");
    // An applet the store does not list; a store that does not answer.
    let unlisted = rpc(client.call(methods::STORE_DETAILS, &StoreDetailsParams { app: "io.example.none".into() }).await.unwrap_err());
    assert_eq!(unlisted.data.unwrap().kind, ErrorKind::NotFound);
    store.set_details(&rg.package, None);
    let unanswered = rpc(client.call(methods::STORE_DETAILS, &StoreDetailsParams { app: rg.id.clone() }).await.unwrap_err());
    let data = unanswered.data.unwrap();
    assert_eq!((data.kind, data.reason.as_deref()), (ErrorKind::Store, Some("unreachable")));
    assert!(unanswered.message.contains("io.example.rg") && unanswered.message.contains("404"), "{}", unanswered.message);
    let changed = event(&client, |e| topic(e, "store.changed")).await;
    assert_eq!(changed["fetchedAt"], all.fetched_at);
    // The icons arrive in Hub's cache, then `store.changed` again.
    let mut icon_path = None;
    for _ in 0..100 {
        let now: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams::default()).await.unwrap();
        if let Some(path) = app(&now, &hn.id).icon.clone() {
            icon_path = Some(path);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let icon_path = icon_path.expect("the icon arrives");
    assert!(icon_path.starts_with(&home.path().join("cache/store/icons").display().to_string()), "{icon_path}");
    assert_eq!(std::fs::read(&icon_path).unwrap(), fixture("hn.png"));
    assert_eq!(store.hits("/v1/apps"), 1, "answered from the catalog Hub holds");

    // --- with a device ----------------------------------------------------------
    assert_eq!(kind_of(client.call(methods::STORE_APPS, &StoreAppsParams { device: Some("0123456789abcdef".into()), refresh: None }).await.unwrap_err()), ErrorKind::NotFound);
    let (mut device, id) = paired_device(&client, work.path(), &status.id, port).await;
    let linked: Device = client.call_as(methods::DEVICES_GET, &DeviceParams { device: id.clone() }).await.unwrap();
    let features = &linked.link.as_ref().unwrap().features;
    assert!(features.contains(&"catalog".to_owned()) && features.contains(&"install".to_owned()), "{features:?}");
    let for_device: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams { device: Some(id.clone()), refresh: None }).await.unwrap();
    let ids: Vec<&str> = for_device.apps.iter().map(|app| app.id.as_str()).collect();
    assert!(!ids.contains(&rg.id.as_str()), "the RG-only app is not the 3DS's: {ids:?}");
    assert_eq!(ids.len(), 6);
    assert_eq!((app(&for_device, &hn.id).installed.clone(), app(&for_device, &hn.id).update, app(&for_device, &hn.id).cached), (Some(None), Some(false), false));
    // A client that hears only `store.changed`: the package's arrival flips `cached`.
    let watcher = Client::connect(&socket, info(ClientKind::Gui, "watcher")).await.unwrap();
    watcher.call(methods::SUBSCRIBE, &SubscribeParams { topics: Some(vec!["store.changed".into()]) }).await.unwrap();

    // --- store.install, end to end, with progress ------------------------------
    let mut call = client.start(methods::STORE_INSTALL, &StoreInstallParams { device: id.clone(), app: hn.id.clone() }).await.unwrap();
    let mut phases: Vec<String> = Vec::new();
    let mut last = None;
    while let Some(progress) = call.progress().await {
        let progress: TransferProgress = serde_json::from_value(progress).unwrap();
        if phases.last() != Some(&progress.phase) {
            phases.push(progress.phase.clone());
        }
        last = Some(progress);
    }
    let installed: PushResult = serde_json::from_value(call.finish().await.unwrap()).unwrap();
    let footer = pocket_hub_core::ops::package_hash(&fixture("hn.pocket")).unwrap();
    assert_eq!((installed.app.id.as_str(), installed.app.output.as_str(), installed.app.title.as_str(), installed.hash.as_str()), (hn.id.as_str(), "hn-main", "Hacker News", footer.as_str()));
    assert_eq!(phases.first().map(String::as_str), Some("downloading"), "{phases:?}");
    assert!(phases.contains(&"sending".to_owned()), "{phases:?}");
    let last = last.unwrap();
    assert_eq!((last.done, last.total), (hn.bytes, hn.bytes));
    let pushed = device.expect("pushed").await;
    assert_eq!((pushed["bytes"].as_u64(), pushed["hash"].as_str()), (Some(hn.bytes), Some(footer.as_str())));
    assert!(home.path().join(format!("cache/store/packages/{}.pocket", hn.package)).is_file());
    event(&watcher, |e| topic(e, "store.changed")).await;
    let cached: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams::default()).await.unwrap();
    assert!(app(&cached, &hn.id).cached);
    // The device reports it: installed, no update.
    event(&client, |e| topic(e, "device.changed") && e["device"]["apps"].as_array().is_some_and(|apps| apps.iter().any(|a| a["id"] == hn.id.as_str()))).await;
    let after: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams { device: Some("*".into()), refresh: None }).await.unwrap();
    assert_eq!(app(&after, &hn.id).installed, Some(Some(InstalledCopy { version: "0.3.0".into(), hash: footer.clone() })));
    assert_eq!(app(&after, &hn.id).update, Some(false));
    let package_hits = store.hits(&format!("/v1/packages/{}", hn.package));

    // Installed again: from the cache.
    client.call(methods::STORE_INSTALL, &StoreInstallParams { device: id.clone(), app: hn.id.clone() }).await.unwrap();
    assert_eq!(store.hits(&format!("/v1/packages/{}", hn.package)), package_hits);

    // A newer version in the store: an update for the copy the device holds.
    client.call(methods::STORE_INSTALL, &StoreInstallParams { device: id.clone(), app: tool.id.clone() }).await.unwrap();
    event(&client, |e| topic(e, "device.changed") && e["device"]["apps"].as_array().is_some_and(|apps| apps.iter().any(|a| a["id"] == tool.id.as_str()))).await;
    store.publish(&someone, build_package("io.example.tool", "1.1.0", "Tool", &[("3ds-dev", "tool"), ("tab5-home", "tool")], 20_000), icon(1), "someone", "A better tool.");
    let refreshed: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams { device: Some(id.clone()), refresh: Some(true) }).await.unwrap();
    let newer = app(&refreshed, &tool.id);
    assert_eq!((newer.version.as_str(), newer.installed.clone().unwrap().unwrap().version.as_str(), newer.update), ("1.1.0", "1.0.0", Some(true)));
    event(&client, |e| topic(e, "store.changed") && e["fetchedAt"] == refreshed.fetched_at).await;

    // --- the link's catalog of the same offer ----------------------------------
    device.send(json!({"cmd": "catalog", "tag": "c"})).await;
    let rows = device.wait(|v| v["ev"] == "catalog" && v["tag"] == "c").await;
    let row_ids: Vec<&str> = rows["rows"].as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap()).collect();
    assert!(row_ids.contains(&hn.id.as_str()), "{rows}");
    assert!(!row_ids.contains(&bad.id.as_str()) && !row_ids.contains(&corrupt.id.as_str()) && !row_ids.contains(&rg.id.as_str()), "{rows}");

    // --- errors ----------------------------------------------------------------
    let forged = rpc(install(&client, &id, &bad.id).await.unwrap_err());
    let data = forged.data.unwrap();
    assert_eq!((data.kind, data.reason.as_deref()), (ErrorKind::Store, Some("rejected")), "{}", forged.message);
    let broken = rpc(install(&client, &id, &corrupt.id).await.unwrap_err()).data.unwrap();
    assert_eq!((broken.kind, broken.reason.as_deref()), (ErrorKind::Store, Some("rejected")));
    assert!(!home.path().join(format!("cache/store/packages/{}.pocket", corrupt.package)).exists());
    let now: StoreApps = client.call_as(methods::STORE_APPS, &StoreAppsParams::default()).await.unwrap();
    assert!(!app(&now, &corrupt.id).cached && !app(&now, &bad.id).cached);
    let gone = rpc(install(&client, &id, &missing.id).await.unwrap_err()).data.unwrap();
    assert_eq!((gone.kind, gone.reason.as_deref()), (ErrorKind::Store, Some("unreachable")));
    assert_eq!(kind_of(install(&client, &id, "io.example.nothing").await.unwrap_err()), ErrorKind::NotFound);
    assert_eq!(kind_of(install(&client, &id, &rg.id).await.unwrap_err()), ErrorKind::NotFound);

    // --- cancel while downloading, and the mutex -------------------------------
    let other = Client::connect(&socket, info(ClientKind::Cli, "pocket-hub")).await.unwrap();
    let mut held = client.start(methods::STORE_INSTALL, &StoreInstallParams { device: id.clone(), app: slow.id.clone() }).await.unwrap();
    let first = held.progress().await.expect("downloading progress");
    assert_eq!(first["phase"], "downloading");
    let busy = rpc(other.call(methods::STORE_INSTALL, &StoreInstallParams { device: id.clone(), app: hn.id.clone() }).await.unwrap_err());
    assert_eq!(busy.data.unwrap().holder, Some(Holder { kind: ClientKind::Gui, name: "Pocket Hub".into(), method: "store.install".into() }));
    assert_eq!(kind_of(other.call(methods::DEVICES_LAUNCH, &LaunchParams { device: id.clone(), app: "hn-main".into() }).await.unwrap_err()), ErrorKind::Busy);
    client.cancel(held.id).await.unwrap();
    assert_eq!(kind_of(held.finish().await.unwrap_err()), ErrorKind::Cancelled);
    event(&client, |e| topic(e, "device.changed") && e["device"]["busy"].is_null()).await;
    other.call(methods::DEVICES_LAUNCH, &LaunchParams { device: id.clone(), app: "hn-main".into() }).await.unwrap();

    // --- a store that fails, a device that is not linked -----------------------
    store.catalog_status(Some(503));
    let down = rpc(client.call(methods::STORE_APPS, &StoreAppsParams { device: None, refresh: Some(true) }).await.unwrap_err()).data.unwrap();
    assert_eq!((down.kind, down.reason.as_deref()), (ErrorKind::Store, Some("unreachable")));
    store.catalog_status(None);
    device.kill().await;
    event(&client, |e| topic(e, "device.changed") && e["device"]["link"].is_null()).await;
    assert_eq!(kind_of(install(&client, &id, &hn.id).await.unwrap_err()), ErrorKind::NotLinked);

    client.call(methods::SHUTDOWN, &json!({})).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(10), hubd.wait()).await;
}

/// The examples file's `store.apps` result decodes as the shared types say.
#[test]
fn store_apps_decodes() {
    let value: Value = json!({"apps": [{"id": "a.b.c", "title": "T", "summary": "S", "author": "x", "version": "1.0.0", "official": false, "targets": ["3ds-dev"], "bytes": 1, "icon": null, "cached": false, "services": [], "hosts": [], "installed": null, "update": false}], "fetchedAt": 1});
    let apps: StoreApps = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(apps.apps[0].installed, Some(None));
    assert_eq!(serde_json::to_value(&apps).unwrap(), value);
}
