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
use pocket_hub_api::types::{Event, Service, ServiceList, ServiceState};
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

    // services.install of a package: the answer, its progress, and the event.
    let source = fixture_package(home.path(), "dev.test.host", "1.0.0", json!({}));
    let installed: Service = serde_json::from_value(host.call(methods::SERVICES_INSTALL, json!({"path": source.display().to_string()}), &progress_of).await.unwrap()).unwrap();
    assert_eq!((installed.id.as_str(), installed.version.as_str(), installed.state), ("dev.test.host", "1.0.0", ServiceState::Stopped));
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
    assert!(matches!(connector.open(pocket_hub_core::ChannelOffer { app: "dev.none".into(), ..offer }).await, Err(pocket_hub_core::ChannelOpenError::NoService)));
    let logs = host.call(methods::SERVICES_LOGS, json!({"id": "dev.test.host"}), &progress_of).await.unwrap();
    assert!(logs["lines"].as_array().is_some_and(|lines| !lines.is_empty()));
    until("service.log", || recorder.any(|event| matches!(event, Event::ServiceLog { id, .. } if id == "dev.test.host"))).await;
    drop(stream);
    let stopped: Service = serde_json::from_value(host.call(methods::SERVICES_STOP, json!({"id": "dev.test.host"}), &progress_of).await.unwrap()).unwrap();
    assert_eq!(stopped.id, "dev.test.host");
    until("service.changed stopped", || recorder.changed("dev.test.host", ServiceState::Stopped) && host.service("dev.test.host").is_some_and(|s| s.state == ServiceState::Stopped)).await;

    // services.remove, and its event.
    host.call(methods::SERVICES_REMOVE, json!({"id": "dev.test.host", "purge": true}), &progress_of).await.unwrap();
    until("service.removed", || recorder.any(|event| matches!(event, Event::ServiceRemoved { id } if id == "dev.test.host"))).await;
    assert!(host.services().is_empty());

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
