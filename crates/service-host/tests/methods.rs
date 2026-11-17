//! The control API's service methods stood up from this crate alone: no
//! `pocket-hubd`, no device, no setup and no store pages, only what a
//! program serving services elsewhere would have (this crate, `hub-api` for
//! the contract's types, `hub-core` for the connector a relay calls). A temp
//! home, the supervisor started by `ServiceHost::start`, the methods answered
//! through `ServiceHost::call`, and the events seen through a `Publisher`
//! of the test's own.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use std::os::unix::fs::PermissionsExt;

use pocket_hub_api::types::{Event, Service, ServiceList, ServiceState, ServiceStore};
use pocket_hub_api::{ErrorKind, methods};
use serde_json::{Value, json};
use service_host::{HostOptions, Publisher, ServiceHost};

/// The test's own server side: it keeps every event, in order, and counts
/// the ordered sections and the resyncs it was asked for.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<Event>>,
    ordered: Mutex<usize>,
    resyncs: Mutex<usize>,
}

impl Publisher for Recorder {
    fn ordered(&self, body: &mut dyn FnMut()) {
        *self.ordered.lock().unwrap() += 1;
        body();
    }
    fn publish(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
    fn resync(&self) {
        *self.resyncs.lock().unwrap() += 1;
    }
}

impl Recorder {
    fn changed(&self, id: &str, state: ServiceState) -> bool {
        self.events.lock().unwrap().iter().any(|event| matches!(event, Event::ServiceChanged { service } if service.id == id && service.state == state))
    }
    fn any(&self, test: impl Fn(&Event) -> bool) -> bool {
        self.events.lock().unwrap().iter().any(test)
    }
}

async fn until(what: &str, condition: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_service_methods_stand_up_without_hubd() {
    let Some(bun) = system_bun() else { return };
    let home = temp_home();
    let host = ServiceHost::start(HostOptions { home: home.path().to_owned(), socket: home.path().join("s.sock"), bun: Some(bun), idle_override: None }).await.expect("the host starts");
    assert!(home.path().join("sdk/service/src/index.ts").is_file(), "the SDK is written into the home");
    let recorder = Arc::new(Recorder::default());
    let pump = host.attach(recorder.clone());
    let progress = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = progress.clone();
    let progress_of = move |members: Value| sink.lock().unwrap().push(members);

    // The methods it answers, and only those.
    for method in [methods::SERVICES_LIST, methods::SERVICES_INSTALL, methods::SERVICES_REMOVE, methods::SERVICES_START, methods::SERVICES_STOP, methods::SERVICES_RESTART, methods::SERVICES_LOGS, methods::SERVICES_RUNTIME] {
        assert!(ServiceHost::handles(method), "{method}");
    }
    assert!(!ServiceHost::handles(methods::DEVICES_LIST) && !ServiceHost::handles(methods::STORE_INSTALL) && !ServiceHost::handles(methods::HELLO));

    // services.list, empty, answered inside the server's ordering.
    let list: ServiceList = serde_json::from_value(host.call(methods::SERVICES_LIST, Value::Null, &progress_of).await.unwrap()).unwrap();
    assert!(list.services.is_empty());
    assert_eq!(*recorder.ordered.lock().unwrap(), 1);

    // services.install of a package on the host's SDK that requires its
    // store: the answer, its progress, and the event. Its store is not made
    // until the service uses it.
    let source = sdk_package(home.path(), "store-service");
    let installed: Service = serde_json::from_value(host.call(methods::SERVICES_INSTALL, json!({"path": source.display().to_string()}), &progress_of).await.unwrap()).unwrap();
    assert_eq!((installed.id.as_str(), installed.release.as_deref(), installed.state), ("dev.test.host", Some("dev"), ServiceState::Stopped));
    assert!(installed.requires == ["store"] && installed.hosts.is_empty() && installed.reason.is_none());
    assert_eq!(installed.store, Some(ServiceStore { bytes: Some(0), max_bytes: 65_536, file_bytes: 0 }));
    let store_folder = home.path().join("service-store/dev.test.host@dev");
    assert!(!store_folder.exists());
    assert_eq!(progress.lock().unwrap().last().unwrap()["stage"], "installing");
    until("service.changed for the install", || recorder.changed("dev.test.host", ServiceState::Stopped)).await;
    let list: ServiceList = serde_json::from_value(host.call(methods::SERVICES_LIST, json!({}), &progress_of).await.unwrap()).unwrap();
    assert_eq!(list.services, vec![installed.clone()]);
    assert_eq!(host.services(), list.services);

    // The contract's errors, word for word.
    let error = host.call(methods::SERVICES_START, json!({"id": "dev.none"}), &progress_of).await.unwrap_err();
    assert_eq!((error.kind(), error.message.as_str()), (Some(ErrorKind::NotFound), "No service dev.none is installed."));
    let error = host.call(methods::SERVICES_INSTALL, json!({"path": "relative/path"}), &progress_of).await.unwrap_err();
    assert_eq!((error.kind(), error.message.as_str()), (Some(ErrorKind::Invalid), "relative/path is not an absolute path."));
    let error = host.call(methods::SERVICES_LOGS, json!({}), &progress_of).await.unwrap_err();
    assert_eq!((error.code, error.message.as_str()), (-32602, "The parameters of services.logs are not usable: missing field `id`."));

    // A device's channel through the connector a relay calls: the service
    // starts on demand and attaches; then the owner's stop.
    let connector = host.connector();
    let offer = pocket_hub_core::ChannelOffer {
        device: "0123456789abcdef".into(),
        device_name: "Test 3DS".into(),
        model: "New 3DS XL".into(),
        target: "3ds".into(),
        app: "dev.test.host".into(),
        hash: "fedcba9876543210".into(),
        generation: 7,
        payload_limit: 16_384,
    };
    let stream = connector.open(offer.clone()).await.expect("the channel reaches the service");
    until("service.changed running", || recorder.changed("dev.test.host", ServiceState::Running)).await;
    // The factory counted its start in the store before the channel was taken:
    // `{"starts"}` and `1`, and `service.changed` says so within a second.
    let store = host.service("dev.test.host").unwrap().store.unwrap();
    assert_eq!((store.bytes, store.max_bytes), (Some(6 + 1), 65_536));
    assert!(store.file_bytes > 0);
    assert_eq!(std::fs::metadata(&store_folder).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(store_folder.join("store.sqlite")).unwrap().permissions().mode() & 0o777, 0o600);
    until("service.changed with the store's use", || {
        recorder.any(|event| matches!(event, Event::ServiceChanged { service } if service.id == "dev.test.host" && service.store.as_ref().is_some_and(|store| store.bytes == Some(7))))
    })
    .await;
    assert!(matches!(connector.open(pocket_hub_core::ChannelOffer { app: "dev.none".into(), ..offer }).await, Err(pocket_hub_core::ChannelOpenError::NoService)));
    let logs = host.call(methods::SERVICES_LOGS, json!({"id": "dev.test.host"}), &progress_of).await.unwrap();
    assert!(logs["lines"].as_array().is_some_and(|lines| !lines.is_empty()));
    until("service.log", || recorder.any(|event| matches!(event, Event::ServiceLog { id, release, .. } if id == "dev.test.host" && release.as_deref() == Some("dev")))).await;
    drop(stream);
    let stopped: Service = serde_json::from_value(host.call(methods::SERVICES_STOP, json!({"id": "dev.test.host"}), &progress_of).await.unwrap()).unwrap();
    assert_eq!(stopped.id, "dev.test.host");
    until("service.changed stopped", || recorder.changed("dev.test.host", ServiceState::Stopped) && host.service("dev.test.host").is_some_and(|s| s.state == ServiceState::Stopped)).await;

    // services.remove without purge keeps the store; with purge it goes.
    host.call(methods::SERVICES_REMOVE, json!({"id": "dev.test.host"}), &progress_of).await.unwrap();
    until("service.removed", || recorder.any(|event| matches!(event, Event::ServiceRemoved { id, release } if id == "dev.test.host" && release.as_deref() == Some("dev")))).await;
    assert!(host.services().is_empty());
    assert!(store_folder.join("store.sqlite").is_file(), "a removal without purge keeps the store");
    let again: Service = serde_json::from_value(host.call(methods::SERVICES_INSTALL, json!({"path": source.display().to_string()}), &progress_of).await.unwrap()).unwrap();
    assert_eq!(again.store.unwrap().bytes, Some(7), "the store outlives the install");
    host.call(methods::SERVICES_REMOVE, json!({"id": "dev.test.host", "purge": true}), &progress_of).await.unwrap();
    assert!(!store_folder.exists(), "purge deletes the store");

    // hub.status's `bun` and services.runtime, from the configured Bun.
    let bun = host.bun();
    assert!(bun.installed && !bun.version.is_empty());
    let runtime = host.call(methods::SERVICES_RUNTIME, json!({}), &progress_of).await.unwrap();
    assert_eq!((runtime["installed"].as_bool(), runtime["version"].as_str()), (Some(true), Some(bun.version.as_str())));

    assert_eq!(*recorder.resyncs.lock().unwrap(), 0);
    host.shutdown().await;
    pump.abort();
    assert!(!home.path().join("s.sock").exists(), "the service socket is gone after the shutdown");
}

/// What Pocket Hub's protocol 2 installed (`<version>/` and `current.json`,
/// the layout of #23), as the control API shows it: one `Service`, `failed`,
/// `reason: "earlier"` with its sentence, no release; not started; nothing of
/// it read or deleted; removed by the owner, with `service.removed` and a
/// null release.
#[tokio::test(flavor = "multi_thread")]
async fn an_earlier_layout_is_listed_failed_until_the_owner_removes_it() {
    let home = temp_home();
    let folder = home.path().join("services/dev.old");
    std::fs::create_dir_all(folder.join("1.0.0")).unwrap();
    std::fs::write(folder.join("1.0.0/pocket.service.json"), r#"{"service":1,"id":"dev.old","version":"1.0.0","entry":"service.mjs"}"#).unwrap();
    std::fs::write(folder.join("1.0.0/service.mjs"), "").unwrap();
    std::fs::write(folder.join("current.json"), r#"{"version":"1.0.0","installedAt":1}"#).unwrap();
    let host = ServiceHost::start(HostOptions { home: home.path().to_owned(), socket: home.path().join("s.sock"), bun: None, idle_override: None }).await.unwrap();
    let recorder = Arc::new(Recorder::default());
    let pump = host.attach(recorder.clone());
    let progress_of = |_: Value| {};

    let list: ServiceList = serde_json::from_value(host.call(methods::SERVICES_LIST, json!({}), &progress_of).await.unwrap()).unwrap();
    let earlier = serde_json::to_value(&list.services).unwrap();
    assert_eq!(
        earlier,
        json!([{"id": "dev.old", "release": null, "requires": [], "hosts": [], "state": "failed", "pid": null, "since": null, "channels": 0, "lastExit": null,
                "error": "Installed by an earlier Pocket Hub; install it again.", "reason": "earlier", "store": null}])
    );
    let error = host.call(methods::SERVICES_START, json!({"id": "dev.old"}), &progress_of).await.unwrap_err();
    assert_eq!((error.kind(), error.message.as_str()), (Some(ErrorKind::Service), "Installed by an earlier Pocket Hub; install it again."));
    let data = error.data.as_ref().unwrap();
    assert_eq!((data.reason.as_deref(), data.id.as_deref()), (Some("earlier"), Some("dev.old")));
    let error = host.call(methods::SERVICES_STOP, json!({"id": "dev.old", "release": "dev"}), &progress_of).await.unwrap_err();
    assert_eq!((error.kind(), error.message.as_str()), (Some(ErrorKind::NotFound), "No release dev of the service dev.old is installed."));
    assert!(matches!(host.connector().open(pocket_hub_core::ChannelOffer {
        device: "0123456789abcdef".into(), device_name: "Test 3DS".into(), model: "New 3DS XL".into(), target: "3ds".into(),
        app: "dev.old".into(), hash: "fedcba9876543210".into(), generation: 1, payload_limit: 16_384,
    }).await, Err(pocket_hub_core::ChannelOpenError::NoService)));
    assert!(folder.join("current.json").is_file() && folder.join("1.0.0/pocket.service.json").is_file(), "nothing of it deleted");

    host.call(methods::SERVICES_REMOVE, json!({"id": "dev.old"}), &progress_of).await.unwrap();
    until("service.removed", || recorder.any(|event| matches!(event, Event::ServiceRemoved { id, release: None } if id == "dev.old"))).await;
    assert!(!folder.exists());
    assert!(host.services().is_empty());
    host.shutdown().await;
    pump.abort();
}

/// A refused manifest is the error `service`, `reason: "manifest"`, the field
/// in `detail` (CONTROL-API.md §1, Errors), and it is refused before Bun is
/// fetched: this host has no Bun and gets none.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_manifest_is_the_error_service_with_its_field() {
    let home = temp_home();
    let host = ServiceHost::start(HostOptions { home: home.path().to_owned(), socket: home.path().join("s.sock"), bun: None, idle_override: None }).await.unwrap();
    assert!(!host.bun().installed);
    let progress = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = progress.clone();
    let progress_of = move |members: Value| sink.lock().unwrap().push(members);
    let source = home.path().join("pkg");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("service.mjs"), "").unwrap();
    let started = std::time::Instant::now();
    for (manifest, message, field) in [
        (r#"{"service":1,"id":"dev.x","entry":"service.mjs","requires":["net"]}"#, "Requires \"net\", which this Hub does not provide yet.", Some("requires")),
        (r#"{"service":1,"id":"dev.x","entry":"service.mjs","hosts":["*.workers.dev"]}"#, "Hosts entry \"*.workers.dev\" covers workers.dev, a public suffix.", Some("hosts")),
        (r#"{"service":1,"id":"dev.x","version":"1.0.0","entry":"service.mjs"}"#, "Pocket.service.json names `version`, a field this Hub does not read.", Some("version")),
        (r#"[1]"#, "Pocket.service.json is not a JSON object.", None),
    ] {
        std::fs::write(source.join("pocket.service.json"), manifest).unwrap();
        let error = host.call(methods::SERVICES_INSTALL, json!({"path": source.display().to_string()}), &progress_of).await.unwrap_err();
        let data = error.data.as_ref().unwrap();
        assert_eq!((error.kind(), error.message.as_str(), data.reason.as_deref(), data.detail.as_deref()), (Some(ErrorKind::Service), message, Some("manifest"), field), "{manifest}");
    }
    assert!(host.services().is_empty());
    // No download was started or waited for.
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    assert!(progress.lock().unwrap().iter().all(|members| members["stage"] != "runtime"));
    assert!(!host.bun().installed);
    host.shutdown().await;
}

/// Every file of `folder` with its bytes and modification time.
fn snapshot(folder: &std::path::Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut files: Vec<_> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name().to_string_lossy().into_owned(), std::fs::read(entry.path()).unwrap(), entry.metadata().unwrap().modified().unwrap())
        })
        .collect();
    files.sort();
    files
}

/// A store whose `user_version` is above this host's is refused on both
/// paths and left as it is (SERVICE-PROTOCOL.md §7): the control API lists
/// it with `reason: "store_format"` and refuses the owner's start with the
/// error `service`, `reason: "store_format"`; a process a device starts gets
/// `store_format` for its store call. No byte or time of its files changes,
/// and no file is added beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_a_newer_pocket_hub_wrote_is_refused_and_untouched() {
    let Some(bun) = system_bun() else { return };
    let home = temp_home();
    let host = ServiceHost::start(HostOptions { home: home.path().to_owned(), socket: home.path().join("s.sock"), bun: Some(bun), idle_override: None }).await.unwrap();
    let progress_of = |_: Value| {};
    let source = sdk_package(home.path(), "store-service");
    host.call(methods::SERVICES_INSTALL, json!({"path": source.display().to_string()}), &progress_of).await.unwrap();
    let folder = home.path().join("service-store/dev.test.host@dev");
    std::fs::create_dir_all(&folder).unwrap();
    {
        let newer = rusqlite::Connection::open(folder.join("store.sqlite")).unwrap();
        newer.execute_batch("CREATE TABLE entries (key BLOB PRIMARY KEY, value BLOB) WITHOUT ROWID; PRAGMA user_version = 2;").unwrap();
    }
    let before = snapshot(&folder);
    assert_eq!(before.iter().map(|(name, ..)| name.as_str()).collect::<Vec<_>>(), ["store.sqlite"]);

    let listed = host.service("dev.test.host").unwrap();
    assert_eq!(listed.reason.as_deref(), Some("store_format"));
    assert_eq!(listed.store, Some(ServiceStore { bytes: None, max_bytes: 65_536, file_bytes: before[0].1.len() as u64 }));

    let error = host.call(methods::SERVICES_START, json!({"id": "dev.test.host"}), &progress_of).await.unwrap_err();
    let data = error.data.as_ref().unwrap();
    assert_eq!((error.kind(), data.reason.as_deref(), data.id.as_deref()), (Some(ErrorKind::Service), Some("store_format"), Some("dev.test.host")));
    assert!(error.message.contains("newer Pocket Hub"), "{}", error.message);

    // A device's open starts it: its factory's store call is refused.
    let offer = pocket_hub_core::ChannelOffer {
        device: "0123456789abcdef".into(), device_name: "Test 3DS".into(), model: "New 3DS XL".into(), target: "3ds".into(),
        app: "dev.test.host".into(), hash: "fedcba9876543210".into(), generation: 1, payload_limit: 16_384,
    };
    assert!(host.connector().open(offer).await.is_err());
    let logs = host.call(methods::SERVICES_LOGS, json!({"id": "dev.test.host"}), &progress_of).await.unwrap();
    let text = logs["lines"].as_array().unwrap().iter().map(|line| line["text"].as_str().unwrap().to_owned()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("store_format"), "{text}");

    assert_eq!(snapshot(&folder), before, "the store's files are as they were");
    host.shutdown().await;
}

/// Dependencies that will not install are the error `service`, `reason:
/// "dependencies"`, with the end of `bun install`'s output in `detail`: not
/// bad input from the owner (CONTROL-API.md §1, Errors).
#[tokio::test(flavor = "multi_thread")]
async fn dependencies_that_will_not_install_have_a_reason_of_their_own() {
    let Some(bun) = system_bun() else { return };
    let home = temp_home();
    let host = ServiceHost::start(HostOptions { home: home.path().to_owned(), socket: home.path().join("s.sock"), bun: Some(bun), idle_override: None }).await.unwrap();
    let source = home.path().join("locked");
    std::fs::create_dir_all(source.join("tiny")).unwrap();
    for name in ["pocket.service.json", "bun.lock", "service.mjs", "tiny/index.js", "tiny/package.json"] {
        std::fs::copy(fixtures().join("with-lock").join(name), source.join(name)).unwrap();
    }
    // A package.json the lockfile does not match.
    std::fs::write(source.join("package.json"), r#"{"name":"with-lock","dependencies":{"tiny":"file:./tiny","other":"file:./tiny"}}"#).unwrap();
    let error = host.call(methods::SERVICES_INSTALL, json!({"path": source.display().to_string()}), &|_| {}).await.unwrap_err();
    let data = error.data.as_ref().unwrap();
    assert_eq!((error.kind(), data.reason.as_deref()), (Some(ErrorKind::Service), Some("dependencies")));
    assert!(error.message.ends_with("could not be installed: its dependencies did not install (exit status: 1)."), "{}", error.message);
    let detail = data.detail.as_deref().unwrap();
    assert!(detail.to_lowercase().contains("lockfile") && detail.len() <= 4096 + 3, "{detail}");
    assert!(host.services().is_empty());
    host.shutdown().await;
}
