//! The store client against the fake store: the catalog's fetch, cache and
//! refresh rules; packages verified by SHA-256 and signature (accepted and
//! rejected), kept once and never downloaded again; icons; an app's details,
//! kept while the catalog is; the link's offer.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use pocket_hub_applet_store::test_support::*;
use pocket_hub_applet_store::{AppletStore, StoreConfig, StoreError, StoreEvent, package};

const KEY: u8 = 1;
const OTHER: u8 = 2;

struct Setup {
    fake: FakeStore,
    cache: tempfile::TempDir,
    clock: Arc<AtomicI64>,
}

impl Setup {
    async fn new() -> Setup {
        Setup { fake: FakeStore::start().await, cache: tempfile::tempdir().unwrap(), clock: Arc::new(AtomicI64::new(1_800_000_000_000)) }
    }

    fn config(&self, wanted: &[&str]) -> StoreConfig {
        let mut config = StoreConfig::new(&self.fake.url, self.cache.path());
        config.official_key = public(&test_key(KEY));
        config.stall = Duration::from_millis(400);
        config.request_timeout = Duration::from_secs(5);
        let clock = self.clock.clone();
        config.now = Arc::new(move || clock.load(Ordering::SeqCst));
        let wanted: Vec<String> = wanted.iter().map(|s| s.to_string()).collect();
        config.wanted_targets = Arc::new(move || wanted.clone());
        config
    }

    fn open(&self, wanted: &[&str]) -> Arc<AppletStore> {
        AppletStore::open(self.config(wanted)).unwrap()
    }
}

#[tokio::test]
async fn the_catalog_is_fetched_cached_and_refreshed() {
    let setup = Setup::new().await;
    let key = test_key(KEY);
    let hn = setup.fake.publish(&key, fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News: top stories and threads.");
    let store = setup.open(&[]);
    assert!(store.held().is_none());
    let mut events = store.subscribe();

    let catalog = store.apps(false).await.unwrap();
    assert_eq!(catalog.apps, vec![hn.clone()]);
    assert_eq!(catalog.fetched_at, setup.clock.load(Ordering::SeqCst));
    assert_eq!(setup.fake.hits("/v1/apps"), 1);
    assert!(matches!(events.recv().await.unwrap(), StoreEvent::Catalog { .. }));
    let on_disk: serde_json::Value = serde_json::from_slice(&std::fs::read(setup.cache.path().join("catalog.json")).unwrap()).unwrap();
    assert_eq!(on_disk["fetchedAt"], catalog.fetched_at);
    assert_eq!(on_disk["apps"][0]["id"], "dev.pocket-nexus.hn");
    // The icon follows, announced as a change of what `store.apps` answers.
    for _ in 0..250 {
        if store.icon_path(&hn).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(store.icon_path(&hn).is_some());
    assert!(matches!(tokio::time::timeout(Duration::from_secs(5), events.recv()).await, Ok(Ok(StoreEvent::Catalog { .. }))));
    // So do the app's details, announced once they are held.
    until(|| store.held_details(&hn).is_some()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    while events.try_recv().is_ok() {}

    // Younger than 10 minutes: answered from what is held.
    setup.clock.fetch_add(9 * 60_000, Ordering::SeqCst);
    store.apps(false).await.unwrap();
    assert_eq!(setup.fake.hits("/v1/apps"), 1);
    // A new client (Hub started again) holds the cached catalog without asking.
    let again = setup.open(&[]);
    assert_eq!(again.held().unwrap().apps, catalog.apps);
    again.apps(false).await.unwrap();
    assert_eq!(setup.fake.hits("/v1/apps"), 1);
    // Older than 10 minutes, or asked: fetched again.
    setup.clock.fetch_add(2 * 60_000, Ordering::SeqCst);
    let refetched = store.apps(false).await.unwrap();
    assert_eq!(setup.fake.hits("/v1/apps"), 2);
    assert!(refetched.fetched_at > catalog.fetched_at);
    store.apps(true).await.unwrap();
    assert_eq!(setup.fake.hits("/v1/apps"), 3);

    // The same apps again: no `Catalog` event. Another app: one.
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, StoreEvent::Catalog { .. }), "an unchanged catalog is not announced: {event:?}");
    }
    setup.fake.publish(&key, fixture("youtube.pocket"), fixture("youtube.png"), "pocket-nexus", "Videos.");
    assert_eq!(store.apps(true).await.unwrap().apps.len(), 2);
    let event = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let StoreEvent::Catalog { fetched_at } = events.recv().await.unwrap() {
                return fetched_at;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event, store.held().unwrap().fetched_at);

    // A store that fails: the error, with no older catalog in its place.
    setup.fake.catalog_status(Some(500));
    let failed = store.apps(true).await.unwrap_err();
    assert_eq!(failed.reason(), "unreachable");
    assert!(failed.to_string().contains("500"), "{failed}");
    setup.clock.fetch_add(11 * 60_000, Ordering::SeqCst);
    assert!(matches!(store.apps(false).await, Err(StoreError::Unreachable(_))));
    // Nothing answers at all.
    let mut config = setup.config(&[]);
    config.base_url = "http://127.0.0.1:9".into();
    config.cache = tempfile::tempdir().unwrap().keep();
    let nowhere = AppletStore::open(config).unwrap();
    assert!(matches!(nowhere.apps(false).await, Err(StoreError::Unreachable(_))));
}

/// Waits up to 5 s for `held` to hold.
async fn until(mut held: impl FnMut() -> bool) {
    for _ in 0..250 {
        if held() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(held(), "never held");
}

#[tokio::test]
async fn details_are_the_stores_read_again_after_each_catalog() {
    let setup = Setup::new().await;
    let key = test_key(KEY);
    let hn = setup.fake.publish(&key, fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News.");
    let built = setup.fake.publish(&key, build_package("io.example.plain", "1.0.0", "Plain", &[("3ds-dev", "plain"), ("x-unknown", "plain")], 100), icon(3), "someone", "Plain.");
    // The store does not answer hn's details at first.
    setup.fake.set_details(&hn.package, None);
    let store = setup.open(&[]);
    store.apps(false).await.unwrap();
    let path = format!("/v1/details/{}", hn.package);

    // Every app's details are asked for after the fetch, in the background,
    // by the hash of the package the catalog names.
    until(|| store.held_details(&built).is_some() && setup.fake.hits(&path) == 1).await;
    // A device the store has no name for keeps its target, last.
    let plain = store.details(&built).await.unwrap();
    assert_eq!(plain.devices.iter().map(|device| (device.target.as_str(), device.name.as_str())).collect::<Vec<_>>(), [("3ds-dev", "Nintendo 3DS"), ("x-unknown", "x-unknown")]);
    assert_eq!((plain.needs.len(), plain.surface.clone(), plain.api), (0, None, Some(1)));
    // Not answered, or not with details: unreachable, nothing older in their place.
    let missing = store.details(&hn).await.unwrap_err();
    assert_eq!(missing.reason(), "unreachable");
    assert!(missing.to_string().contains("404"), "{missing}");
    setup.fake.set_details(&hn.package, Some(serde_json::json!({"error": "not details"})));
    assert!(matches!(store.details(&hn).await, Err(StoreError::Unreachable(_))));
    // A failed answer is not kept: the next ask tries again, and keeps what it gets.
    setup.fake.set_details(&hn.package, Some(fixture_details("hn")));
    let details = store.details(&hn).await.unwrap();
    let asked = setup.fake.hits(&path);
    assert_eq!(store.details(&hn).await.unwrap(), details);
    assert_eq!(setup.fake.hits(&path), asked, "kept: answered without a request");
    assert_eq!(details.devices.iter().map(|device| device.name.as_str()).collect::<Vec<_>>(), ["Omarchy Pocket", "M5Stack Tab5", "Nintendo 3DS"]);
    assert_eq!(details.needs.len(), 5);
    assert_eq!(details.surface.as_ref().and_then(|surface| surface.size.as_ref()).map(|size| (size.min, size.max)), Some(([320, 240], [640, 720])));

    // The next catalog fetch reads them again; what changed is announced,
    // what did not is not.
    let mut events = store.subscribe();
    let mut grown = fixture_details("hn");
    grown["devices"][0]["name"] = serde_json::json!("Omarchy Pocket 2");
    setup.fake.set_details(&hn.package, Some(grown));
    store.apps(true).await.unwrap();
    until(|| store.held_details(&hn).is_some_and(|held| held.devices[0].name == "Omarchy Pocket 2")).await;
    let announced = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let StoreEvent::Catalog { .. } = events.recv().await.unwrap() {
                return;
            }
        }
    });
    assert!(announced.await.is_ok(), "new details are announced");
    let reads = setup.fake.hits(&path);
    store.apps(true).await.unwrap();
    until(|| setup.fake.hits(&path) == reads + 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, StoreEvent::Catalog { .. }), "the same details are not announced: {event:?}");
    }
    // A read that fails drops what was held.
    setup.fake.set_details(&hn.package, None);
    store.apps(true).await.unwrap();
    until(|| store.held_details(&hn).is_none()).await;
}

#[tokio::test]
async fn sample_data_is_checked_and_kept() {
    let setup = Setup::new().await;
    let key = test_key(KEY);
    let hn = setup.fake.publish(&key, fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News.");
    let none = setup.fake.publish(&key, build_package("io.example.plain", "1.0.0", "Plain", &[("3ds-dev", "plain")], 100), icon(3), "someone", "Plain.");
    let sample = serde_json::json!({"mock": 1, "routes": [{"url": "https://hacker-news.firebaseio.com/v0/topstories.json", "text": "[1,2,3]"}]});
    let hn = setup.fake.set_sample(&hn, &sample);
    let store = setup.open(&[]);
    let catalog = store.apps(false).await.unwrap();
    // The listing's hash is kept with the entry; the signature still verifies.
    let listed = catalog.app(&hn.id).unwrap();
    assert_eq!(listed.mock, hn.mock);
    listed.verify(&public(&key)).unwrap();
    assert_eq!(catalog.app(&none.id).unwrap().mock, None);

    let path = store.sample(listed).await.unwrap().expect("sample data");
    assert_eq!(path, setup.cache.path().join("mocks").join(format!("{}.json", hn.mock.as_deref().unwrap())));
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap(), sample);
    let route = format!("/v1/mocks/{}", hn.mock.as_deref().unwrap());
    assert_eq!(setup.fake.hits(&route), 1);
    // Kept: asked again, nothing is downloaded.
    store.sample(listed).await.unwrap();
    assert_eq!(setup.fake.hits(&route), 1);
    // A version with none has none.
    assert_eq!(store.sample(catalog.app(&none.id).unwrap()).await.unwrap(), None);

    // A cached file that changed is downloaded again; bytes that do not match are rejected and not kept.
    std::fs::write(&path, b"{}").unwrap();
    store.sample(listed).await.unwrap();
    assert_eq!(setup.fake.hits(&route), 2);
    let mut forged = listed.clone();
    forged.mock = Some("0".repeat(64));
    setup.fake.set_details(&forged.package, None);
    assert!(matches!(store.sample(&forged).await, Err(StoreError::Unreachable(_))), "the store has no such file");
    std::fs::remove_file(&path).unwrap();
    setup.fake.fault(hn.mock.as_deref().unwrap(), Fault::Corrupt);
    assert_eq!(store.sample(listed).await.unwrap_err().reason(), "rejected");
    assert!(!path.exists(), "rejected bytes are not kept");
}

#[tokio::test]
async fn packages_are_verified_kept_and_never_downloaded_twice() {
    let setup = Setup::new().await;
    let key = test_key(KEY);
    let entry = setup.fake.publish(&key, fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News.");
    let store = setup.open(&[]);
    store.apps(false).await.unwrap();
    let path = format!("/v1/packages/{}", entry.package);

    let mut seen = Vec::new();
    let facts = store.package(&entry, |done, total| seen.push((done, total))).await.unwrap();
    assert_eq!(seen.last(), Some(&(entry.bytes, entry.bytes)));
    assert_eq!((facts.id.as_str(), facts.version.as_str(), facts.bytes), ("dev.pocket-nexus.hn", "0.3.0", 2_210_904));
    let targets: Vec<&str> = facts.variants.iter().map(|v| v.target.as_str()).collect();
    assert_eq!(targets, ["3ds-dev", "rg-ds-plus", "tab5-home"]);
    assert_eq!(facts.variant("3ds-dev").unwrap().output, "hn-main");
    let cached = store.package_path(&entry.package);
    assert_eq!(cached.file_name().unwrap().to_str().unwrap(), format!("{}.pocket", entry.package));
    assert_eq!(std::fs::read(&cached).unwrap(), fixture("hn.pocket"));
    assert_eq!(setup.fake.hits(&path), 1);

    // Again: from the cache.
    store.package(&entry, |_, _| {}).await.unwrap();
    assert_eq!(setup.fake.hits(&path), 1);
    // Hub started again: the cached file is verified, not downloaded.
    let again = setup.open(&[]);
    let mut events = again.subscribe();
    let wait = tokio::time::timeout(Duration::from_secs(10), events.recv()).await;
    assert!(matches!(wait, Ok(Ok(StoreEvent::Packages))), "{wait:?}");
    assert!(again.ready(&entry.package).is_some());
    again.package(&entry, |_, _| {}).await.unwrap();
    assert_eq!(setup.fake.hits(&path), 1);

    // A cached file that no longer holds its bytes is removed at the start
    // and downloaded again when it is asked for.
    std::fs::write(&cached, b"garbage").unwrap();
    let third = setup.open(&[]);
    for _ in 0..100 {
        if !cached.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!cached.exists());
    third.package(&entry, |_, _| {}).await.unwrap();
    assert_eq!(setup.fake.hits(&path), 2);
}

#[tokio::test]
async fn a_package_that_does_not_verify_is_rejected_and_not_kept() {
    let setup = Setup::new().await;
    let key = test_key(KEY);
    let other = test_key(OTHER);
    let hn = setup.fake.publish(&key, fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News.");
    let built = build_package("io.example.tool", "1.0.0", "Tool", &[("3ds-dev", "tool"), ("rg-ds-plus", "tool")], 4000);
    let tool = setup.fake.publish(&other, built, icon(3), "someone", "A tool.");
    let store = setup.open(&[]);
    store.apps(false).await.unwrap();
    let no_files = |sha: &str| !store.package_path(sha).exists() && std::fs::read_dir(setup.cache.path().join("packages")).map(|d| d.count()).unwrap_or(0) == 0;

    // Bytes that do not match the SHA-256.
    setup.fake.fault(&hn.package, Fault::Corrupt);
    let corrupt = store.package(&hn, |_, _| {}).await.unwrap_err();
    assert_eq!(corrupt.reason(), "rejected");
    assert!(corrupt.to_string().contains("SHA-256"), "{corrupt}");
    assert!(no_files(&hn.package));

    // A signature by another key: nothing is downloaded.
    let mut forged = tool.clone();
    forged.signature = sign(&key, &tool);
    setup.fake.set_entry(&forged);
    store.apps(true).await.unwrap();
    let before = setup.fake.hits(&format!("/v1/packages/{}", tool.package));
    let unsigned = store.package(&forged, |_, _| {}).await.unwrap_err();
    assert_eq!(unsigned.reason(), "rejected");
    assert!(unsigned.to_string().contains("signature"), "{unsigned}");
    assert_eq!(setup.fake.hits(&format!("/v1/packages/{}", tool.package)), before);
    // The official publisher's id from another key.
    let mut squatted = forged.clone();
    squatted.id = "dev.pocket-nexus.tool".into();
    squatted.signature = sign(&other, &squatted);
    assert!(store.package(&squatted, |_, _| {}).await.unwrap_err().to_string().contains("official publisher"));

    // A package whose identity is another app's, correctly signed.
    let liar = build_package("io.example.liar", "1.0.0", "Liar", &[("3ds-dev", "liar")], 100);
    let mut lying = setup.fake.publish(&other, liar, icon(4), "someone", "Lies.");
    lying.id = "io.example.honest".into();
    lying.signature = sign(&other, &lying);
    setup.fake.set_entry(&lying);
    let wrong = store.package(&lying, |_, _| {}).await.unwrap_err();
    assert_eq!(wrong.reason(), "rejected");
    assert!(wrong.to_string().contains("says it is io.example.liar"), "{wrong}");

    // 404, and a download that stalls: unreachable.
    setup.fake.clear_fault(&hn.package);
    setup.fake.fault(&hn.package, Fault::Missing);
    let missing = store.package(&hn, |_, _| {}).await.unwrap_err();
    assert_eq!(missing.reason(), "unreachable");
    assert!(missing.to_string().contains("404"), "{missing}");
    setup.fake.fault(&hn.package, Fault::Stall);
    let mut got = 0;
    let stalled = store.package(&hn, |done, _| got = done).await.unwrap_err();
    assert_eq!(stalled.reason(), "unreachable");
    assert!(stalled.to_string().contains("nothing arrived"), "{stalled}");
    assert!(got > 0 && got < hn.bytes, "{got}");
    assert!(no_files(&hn.package));

    // Fixed: verified and kept.
    setup.fake.clear_fault(&hn.package);
    store.package(&hn, |_, _| {}).await.unwrap();
    assert!(store.package_path(&hn.package).is_file());
    // The real entry of the test key's package verifies.
    let honest = store.package(&tool, |_, _| {}).await.unwrap();
    assert_eq!(honest.variant("rg-ds-plus").unwrap().output, "tool");
}

#[tokio::test]
async fn icons_offer_and_prefetch() {
    let setup = Setup::new().await;
    let key = test_key(KEY);
    let other = test_key(OTHER);
    let hn = setup.fake.publish(&key, fixture("hn.pocket"), fixture("hn.png"), "pocket-nexus", "Hacker News:\n top   stories.");
    let only_rg = setup.fake.publish(&other, build_package("io.example.rg", "2.0.0", "RG only", &[("rg-ds-plus", "rg")], 10), icon(5), "someone", "RG.");
    let mut bad = setup.fake.publish(&other, build_package("io.example.bad", "1.0.0", "Bad", &[("3ds-dev", "bad")], 10), icon(6), "someone", "Bad.");
    bad.signature = sign(&key, &bad);
    setup.fake.set_entry(&bad);
    // The 3DS is a paired device's target: its packages are fetched after the catalog.
    let store = setup.open(&["3ds-dev"]);
    let mut events = store.subscribe();
    assert!(store.offer("3ds-dev").is_empty());
    store.apps(false).await.unwrap();

    let mut icons_arrived = false;
    let mut packages = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !(icons_arrived && packages) {
            match events.recv().await.unwrap() {
                StoreEvent::Catalog { .. } if store.icon_path(&hn).is_some() => icons_arrived = true,
                StoreEvent::Packages => packages = true,
                _ => {}
            }
        }
    })
    .await
    .expect("the icons and the 3DS's packages arrive");
    assert_eq!(std::fs::read(store.icon_path(&hn).unwrap()).unwrap(), fixture("hn.png"));
    assert!(store.icon_path(&only_rg).is_some());
    assert!(store.icon_path(&bad).is_none(), "no icon for an entry whose signature does not verify");
    // Only the wanted target's packages, and not the one that does not verify.
    assert_eq!(setup.fake.hits(&format!("/v1/packages/{}", hn.package)), 1);
    assert_eq!(setup.fake.hits(&format!("/v1/packages/{}", only_rg.package)), 0);
    assert_eq!(setup.fake.hits(&format!("/v1/packages/{}", bad.package)), 0);

    let rows = store.offer("3ds-dev");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!((row.id.as_str(), row.output.as_str(), row.version.as_str(), row.title.as_str()), ("dev.pocket-nexus.hn", "hn-main", "0.3.0", "Hacker News"));
    assert_eq!(row.summary, "Hacker News: top stories.");
    assert_eq!(row.hash, pocket_hub_core::ops::package_hash(&fixture("hn.pocket")).unwrap());
    assert_eq!(row.bytes, hn.bytes);
    // hn has a variant for the Tab5 too; a target with no applets; a target
    // whose package is not fetched yet.
    assert_eq!(store.offer("tab5-home").iter().map(|row| row.output.as_str()).collect::<Vec<_>>(), ["hn-main"]);
    assert!(store.offer("psp").is_empty());
    assert!(store.offer("").is_empty());
    assert!(store.offer("rg-ds-plus").iter().all(|row| row.id != "io.example.rg"));
    store.package(&only_rg, |_, _| {}).await.unwrap();
    assert!(store.offer("rg-ds-plus").iter().any(|row| row.id == "io.example.rg" && row.output == "rg"));

    // An app the store no longer lists leaves the offer and its files go.
    setup.fake.unlist("dev.pocket-nexus.hn");
    store.apps(true).await.unwrap();
    assert!(store.offer("3ds-dev").is_empty());
    assert!(!store.package_path(&hn.package).exists());
    assert!(store.icon_path(&hn).is_none());
}

#[test]
fn built_packages_read_back() {
    let bytes = build_package("io.example.multi", "1.2.3", "Multi", &[("tab5-home", "m-tab"), ("3ds-dev", "m-3ds")], 777);
    let facts = package::read(&bytes).unwrap();
    assert_eq!((facts.id.as_str(), facts.version.as_str(), facts.title.as_str()), ("io.example.multi", "1.2.3", "Multi"));
    assert_eq!(facts.variants.iter().map(|v| (v.target.as_str(), v.output.as_str())).collect::<Vec<_>>(), [("3ds-dev", "m-3ds"), ("tab5-home", "m-tab")]);
    assert_eq!(facts.hash, pocket_hub_core::ops::package_hash(&bytes).unwrap());
}
