//! The link's `catalog` and `install` (link specification §6.4) served from
//! the store client, with the reference device's `catalog()` and `install()`
//! (vendor/pocketjs/tools/pocket-link/v2/device.ts, driven by hub-core's
//! harness under Bun) on loopback, against the fake store.

#[path = "../../hub-core/tests/common/mod.rs"]
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use common::{Device, Hub, fast_config, temp_dir};
use pocket_hub_applet_store::test_support::*;
use pocket_hub_applet_store::{AppletStore, Entry, LinkCatalog, LinkConfig, StoreConfig};
use pocket_hub_core::{Endpoint, TcpCarrier};
use pocket_hub_store::Store;
use serde_json::{Value, json};

struct Setup {
    hub: Hub,
    fake: FakeStore,
    applets: Arc<AppletStore>,
    link: LinkCatalog,
    /// Milliseconds added to the snapshots' clock.
    skew: Arc<AtomicU64>,
}

async fn setup(fetch_wait: Duration) -> Setup {
    let fake = FakeStore::start().await;
    let dir = temp_dir("applets");
    let store = Arc::new(Store::open(dir.join("store")).unwrap());
    let mut config = StoreConfig::new(&fake.url, dir.join("cache"));
    config.official_key = public(&test_key(1));
    config.stall = Duration::from_secs(5);
    let records = store.clone();
    config.wanted_targets = Arc::new(move || records.devices().into_iter().map(|record| record.target).filter(|target| !target.is_empty()).collect());
    let applets = AppletStore::open(config).unwrap();
    let skew = Arc::new(AtomicU64::new(0));
    let (base, offset) = (Instant::now(), skew.clone());
    let link = LinkCatalog::new(applets.clone(), LinkConfig { fetch_wait, now: Arc::new(move || base + Duration::from_millis(offset.load(Ordering::SeqCst))) });
    let carrier = TcpCarrier::bind(Some(0)).await.unwrap();
    let endpoint = Endpoint::builder(store.clone()).config(fast_config()).listener(Box::new(carrier)).device_requests(Arc::new(link.clone())).start();
    let notifier = endpoint.notifier();
    link.set_notifier(move |targets| {
        let mut event = serde_json::Map::new();
        event.insert("t".into(), json!("catalog.changed"));
        notifier.notify("catalog", event, Some(&targets));
    });
    link.start();
    let port = endpoint.ports()[0].1.unwrap();
    let events = endpoint.subscribe();
    Setup { hub: Hub { endpoint, store, port, events, dir }, fake, applets, link, skew }
}

impl Setup {
    /// Publishes a built package for the 3DS (and the Tab5 for every third app).
    fn publish(&self, n: usize, filler: usize) -> Entry {
        let id = format!("io.example.app-{n:02}");
        let output = format!("out-{n:02}");
        let mut variants = vec![("3ds-dev", output.as_str())];
        if n.is_multiple_of(3) {
            variants.push(("tab5-home", output.as_str()));
        }
        self.fake.publish(&test_key(2), build_package(&id, "1.0.0", &format!("App {n}"), &variants, filler), icon(n as u8), "someone", &format!("Applet number {n}."))
    }

    /// The catalog fetched and every listed package verified in the cache.
    async fn ready(&self) {
        let catalog = self.applets.apps(true).await.unwrap();
        for entry in &catalog.apps {
            self.applets.package(entry, |_, _| {}).await.unwrap();
        }
    }

    async fn device(&mut self, extra: Value) -> Device {
        let (dir, id, _) = self.hub.pair();
        let mut device = Device::spawn(&self.hub, &dir, extra).await;
        self.hub.linked(&id).await;
        device.expect("linked").await;
        device
    }
}

async fn catalog(device: &mut Device, tag: &str) -> Value {
    device.send(json!({"cmd": "catalog", "tag": tag})).await;
    let tag = tag.to_owned();
    device.wait(move |v| v["ev"] == "catalog" && v["tag"] == tag.as_str()).await
}

async fn page(device: &mut Device, tag: &str, cursor: Value) -> Value {
    let mut command = json!({"cmd": "page", "tag": tag});
    command.as_object_mut().unwrap().extend(cursor.as_object().unwrap().clone());
    device.send(command).await;
    let tag = tag.to_owned();
    device.wait(move |v| v["ev"] == "page" && v["tag"] == tag.as_str()).await
}

async fn install(device: &mut Device, tag: &str, app: &str) -> Value {
    device.send(json!({"cmd": "install", "tag": tag, "app": app})).await;
    let tag = tag.to_owned();
    device.wait(move |v| v["ev"] == "install" && v["tag"] == tag.as_str()).await
}

fn ids(rows: &Value) -> Vec<String> {
    rows.as_array().unwrap().iter().map(|row| row["id"].as_str().unwrap().to_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn one_page_and_an_install_with_its_footer() {
    let mut s = setup(Duration::from_secs(20)).await;
    assert!(s.hub.endpoint.features().iter().any(|f| f == "catalog") && s.hub.endpoint.features().iter().any(|f| f == "install"));
    let hn = s.fake.publish(&test_key(1), fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News: top stories and threads.");
    let tool = s.publish(1, 2000);
    let rg = s.fake.publish(&test_key(2), build_package("io.example.rg", "1.0.0", "RG", &[("rg-ds-plus", "rg")], 10), icon(9), "someone", "Elsewhere.");
    s.ready().await;
    let mut device = s.device(json!({})).await;

    let got = catalog(&mut device, "c").await;
    assert_eq!(got["ok"], true, "{got}");
    let rows = got["rows"].as_array().unwrap();
    // Ordered by id; the RG-only app is not the 3DS's.
    assert_eq!(ids(&got["rows"]), ["dev.pocket-nexus.hn", "io.example.app-01"]);
    let row = &rows[0];
    let hash = pocket_hub_core::ops::package_hash(&fixture("hn.pocket")).unwrap();
    assert_eq!(row, &json!({"id": "dev.pocket-nexus.hn", "title": "Hacker News", "version": "0.3.0", "summary": "Hacker News: top stories and threads.", "output": "hn-main", "hash": hash, "bytes": hn.bytes}));
    assert_eq!(rows[1]["output"], "out-01");
    let first = page(&mut device, "p", json!({})).await;
    assert_eq!((first["value"]["more"].as_bool(), first["value"]["apps"].as_array().map(Vec::len)), (Some(false), Some(2)), "{first}");
    let _ = (tool, rg);

    // The real package, streamed under the device's credit: the footer the
    // device computes over what it received is the head's hash.
    let done = install(&mut device, "i", "dev.pocket-nexus.hn").await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!((done["hash"].as_str(), done["stored"].as_str(), done["computed"].as_str()), (Some(hash.as_str()), Some(hash.as_str()), Some(hash.as_str())));
    assert_eq!(done["bytes"].as_u64(), Some(hn.bytes));
    // Not offered for this device's target, or not offered at all.
    let elsewhere = install(&mut device, "rg", "io.example.rg").await;
    assert_eq!((elsewhere["ok"].as_bool(), elsewhere["code"].as_str()), (Some(false), Some("not_found")));
    let nothing = install(&mut device, "none", "io.example.none").await;
    assert_eq!(nothing["code"], "not_found");
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pages_snapshots_and_catalog_changed() {
    let mut s = setup(Duration::from_secs(20)).await;
    for n in 0..12 {
        s.publish(n, 100);
    }
    s.ready().await;
    // The smallest payload limit: a page holds a few rows.
    let mut device = s.device(json!({"maxPayload": 1024})).await;
    let all = catalog(&mut device, "all").await;
    assert_eq!(all["ok"], true, "{all}");
    let expected: Vec<String> = (0..12).map(|n| format!("io.example.app-{n:02}")).collect();
    assert_eq!(ids(&all["rows"]), expected);

    let first = page(&mut device, "1", json!({})).await;
    let value = &first["value"];
    assert_eq!(value["more"], true);
    let count = value["apps"].as_array().unwrap().len();
    assert!((1..12).contains(&count), "{count}");
    assert!(value.to_string().len() <= 1024);
    let rev = value["rev"].as_u64().unwrap();
    let last = value["apps"][count - 1]["id"].as_str().unwrap().to_owned();

    // The offer changes: a new app's package arrives, the revision rises and
    // the device is told.
    let revision = s.link.rev();
    s.publish(12, 100);
    s.applets.apps(true).await.unwrap();
    device.expect("catalog.changed").await;
    assert!(s.link.rev() != revision);
    // The fetch begun before goes on from its snapshot, without the new app.
    let mut rows: Vec<String> = ids(&value["apps"]);
    let mut cursor = last.clone();
    loop {
        let next = page(&mut device, &format!("after-{cursor}"), json!({"after": cursor, "rev": rev})).await;
        assert_eq!(next["ok"], true, "{next}");
        rows.extend(ids(&next["value"]["apps"]));
        cursor = rows.last().unwrap().clone();
        if next["value"]["more"] == false {
            break;
        }
    }
    assert_eq!(rows, expected, "served from the snapshot");
    // The next fetch has the new app.
    let fresh = catalog(&mut device, "fresh").await;
    assert_eq!(fresh["rows"].as_array().unwrap().len(), 13);

    // 30 s after its last page request the snapshot is gone: `changed`.
    let first = page(&mut device, "2", json!({})).await;
    let (rev, last) = (first["value"]["rev"].clone(), first["value"]["apps"][0]["id"].clone());
    s.skew.fetch_add(29_000, Ordering::SeqCst);
    let kept = page(&mut device, "3", json!({"after": last, "rev": rev})).await;
    assert_eq!(kept["ok"], true, "{kept}");
    s.skew.fetch_add(31_000, Ordering::SeqCst);
    let gone = page(&mut device, "4", json!({"after": last, "rev": rev})).await;
    assert_eq!(gone["code"], "changed", "{gone}");
    // A cursor that names no row, or comes without its revision: `malformed`.
    let first = page(&mut device, "5", json!({})).await;
    let rev = first["value"]["rev"].clone();
    assert_eq!(page(&mut device, "6", json!({"after": "io.example.none", "rev": rev})).await["code"], "malformed");
    assert_eq!(page(&mut device, "7", json!({"after": "io.example.app-00"})).await["code"], "malformed");
    // A revision of no snapshot: `changed`.
    assert_eq!(page(&mut device, "8", json!({"after": "io.example.app-00", "rev": 999_999})).await["code"], "changed");
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn installs_of_packages_not_yet_cached_and_the_bound_of_four() {
    let mut s = setup(Duration::from_secs(2)).await;
    let early = s.publish(1, 50_000);
    s.applets.apps(true).await.unwrap();
    // Held at the store, so Hub has not downloaded them when the device asks.
    let late = s.publish(2, 50_000);
    let never = s.publish(3, 50_000);
    s.fake.fault(&early.package, Fault::Held);
    s.fake.fault(&late.package, Fault::Held);
    s.fake.fault(&never.package, Fault::Held);
    s.applets.apps(true).await.unwrap();
    let mut device = s.device(json!({})).await;
    // No row yet: the packages are not in the cache.
    assert_eq!(catalog(&mut device, "c").await["rows"], json!([]));

    // The download ends within the wait: the install goes on.
    device.send(json!({"cmd": "install", "tag": "early", "app": early.id})).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.fake.clear_fault(&early.package);
    let done = device.wait(|v| v["ev"] == "install" && v["tag"] == "early").await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["stored"], done["computed"]);
    assert_eq!(done["bytes"].as_u64(), Some(early.bytes));
    device.expect("catalog.changed").await;

    // It does not: `busy` inside the device's 30 s, and the download goes on.
    let busy = install(&mut device, "late", &late.id).await;
    assert_eq!(busy["code"], "busy", "{busy}");
    assert!(busy["ms"].as_u64().unwrap() < 5_000, "{busy}");

    // Four installs wait; a fifth catalog or install request is `busy` at once.
    for tag in ["a", "b", "c", "d"] {
        device.send(json!({"cmd": "install", "tag": tag, "app": never.id})).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let fifth = install(&mut device, "e", &never.id).await;
    assert_eq!(fifth["code"], "busy", "{fifth}");
    assert!(fifth["ms"].as_u64().unwrap() < 1_000, "{fifth}");
    let page_too = page(&mut device, "p", json!({})).await;
    assert_eq!(page_too["code"], "busy", "{page_too}");
    s.fake.release();
    for tag in ["a", "b", "c", "d"] {
        let tag = tag.to_owned();
        let done = device.wait(move |v| v["ev"] == "install" && v["tag"] == tag.as_str()).await;
        assert_eq!(done["ok"], true, "{done}");
    }
    // The one that answered `busy` finds its package now.
    let again = install(&mut device, "late-again", &late.id).await;
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(ids(&catalog(&mut device, "after").await["rows"]), ["io.example.app-01", "io.example.app-02", "io.example.app-03"]);
    device.kill().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_target_with_no_applets_hears_nothing() {
    let mut s = setup(Duration::from_secs(20)).await;
    s.publish(0, 10);
    s.ready().await;
    let mut three = s.device(json!({})).await;
    let mut other = s.device(json!({"target": "psp-dev"})).await;
    assert_eq!(catalog(&mut other, "c").await["rows"], json!([]));
    assert_eq!(ids(&catalog(&mut three, "c").await["rows"]), ["io.example.app-00"]);
    // A change of the 3DS's offer goes to the 3DS alone.
    s.publish(1, 10);
    s.applets.apps(true).await.unwrap();
    three.expect("catalog.changed").await;
    assert!(other.quiet(Duration::from_millis(500), |v| v["ev"] == "catalog.changed").await);
    let none = install(&mut other, "i", "io.example.app-00").await;
    assert_eq!(none["code"], "not_found");
    three.kill().await;
    other.kill().await;
}
