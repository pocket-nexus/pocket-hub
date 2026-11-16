//! Against the live store (applets.pocket.nexus): the catalog, every entry's
//! signature with the official key Hub pins, every app's details, one package
//! downloaded and verified, and one icon. Ignored by default: it needs the
//! network.
//!
//! `cargo test -p pocket-hub-applet-store --test live -- --ignored --nocapture`

use std::time::{Duration, Instant};

use pocket_hub_applet_store::{AppletStore, DEFAULT_URL, OFFICIAL_KEY, StoreConfig};

#[tokio::test]
#[ignore = "reaches the live applet store"]
async fn the_live_store() {
    let cache = tempfile::tempdir().unwrap();
    let store = AppletStore::open(StoreConfig::new(DEFAULT_URL, cache.path())).unwrap();

    let started = Instant::now();
    let catalog = store.apps(true).await.expect("the live catalog");
    let catalog_ms = started.elapsed().as_millis();
    let catalog_bytes = std::fs::metadata(cache.path().join("catalog.json")).unwrap().len();
    assert!(!catalog.apps.is_empty());
    for entry in &catalog.apps {
        entry.verify(OFFICIAL_KEY).unwrap_or_else(|why| panic!("{why}"));
    }
    let three: Vec<_> = catalog.apps.iter().filter(|entry| entry.runs_on("3ds-dev")).collect();
    let total: u64 = three.iter().map(|entry| entry.bytes).sum();
    println!("catalog: {} apps in {catalog_ms} ms ({catalog_bytes} bytes kept); {} for 3ds-dev, {total} bytes of packages", catalog.apps.len(), three.len());

    // The smallest 3DS package.
    let entry = three.iter().min_by_key(|entry| entry.bytes).expect("an app for the 3DS");
    let started = Instant::now();
    let facts = store.package(entry, |_, _| {}).await.expect("the package verifies");
    let elapsed = started.elapsed();
    let variant = facts.variant("3ds-dev").expect("a 3DS variant");
    println!(
        "package: {} {} ({} bytes) in {} ms, {:.0} kB/s; footer {}, 3ds-dev output {}, variants {:?}",
        entry.id,
        entry.version,
        facts.bytes,
        elapsed.as_millis(),
        facts.bytes as f64 / 1e3 / elapsed.as_secs_f64(),
        facts.hash,
        variant.output,
        facts.variants.iter().map(|v| v.target.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(facts.bytes, entry.bytes);
    assert_eq!(std::fs::metadata(store.package_path(&entry.package)).unwrap().len(), entry.bytes);

    // The icons follow the catalog.
    for _ in 0..200 {
        if store.icon_path(entry).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let icon = store.icon_path(entry).expect("the icon arrives");
    println!("icon: {} bytes", std::fs::metadata(icon).unwrap().len());

    // Every app's details read, and name the devices its entry lists.
    for entry in &catalog.apps {
        let details = store.details(entry).await.unwrap_or_else(|error| panic!("{error}"));
        let mut targets: Vec<&str> = details.devices.iter().map(|device| device.target.as_str()).collect();
        let mut listed: Vec<&str> = entry.targets.iter().map(String::as_str).collect();
        targets.sort();
        listed.sort();
        assert_eq!(targets, listed, "{}", entry.id);
        println!("details: {} needs {:?}, uses {:?}", entry.id, details.needs, details.uses);
    }
}
