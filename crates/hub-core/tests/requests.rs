//! A device system request served through `DeviceRequests` (here `catalog`),
//! so a later crate adds the groups of §6.3 to §6.5 without touching the
//! connection code.

mod common;

use std::sync::Arc;

use common::*;
use pocket_hub_core::{BoxFuture, DeviceCall, DeviceRequests, Endpoint, ErrorReply, TcpCarrier};
use serde_json::{Value, json};

struct Catalog;

impl DeviceRequests for Catalog {
    fn features(&self) -> Vec<String> {
        vec!["catalog".into()]
    }
    fn handle(&self, call: DeviceCall) -> BoxFuture<'static, Result<Value, ErrorReply>> {
        Box::pin(async move {
            assert_eq!(call.t, "catalog");
            assert_eq!(call.target, "3ds-dev");
            Ok(json!({"apps":[{"id":"dev.one","title":"One","version":"1","summary":"","output":"one","hash":"0000000000000001","bytes":10}],"rev":"r1","more":false}))
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_handler_answers_the_device() {
    let dir = temp_dir("requests");
    let store = Arc::new(pocket_hub_store::Store::open(dir.join("store")).unwrap());
    let carrier = TcpCarrier::bind(Some(0)).await.unwrap();
    let endpoint = Endpoint::builder(store.clone()).config(fast_config()).listener(Box::new(carrier)).device_requests(Arc::new(Catalog)).start();
    assert!(endpoint.features().contains(&"catalog".to_owned()));
    let port = endpoint.ports()[0].1.unwrap();
    let mut hub = Hub { events: endpoint.subscribe(), endpoint, store, port, dir };
    let (device_dir, id, _) = hub.pair();
    let mut device = Device::spawn(&hub, &device_dir, json!({})).await;
    hub.linked(&id).await;
    // Hub's `Linked` can precede the device's reading of `ready`.
    let _ = device.expect("linked").await;
    device.send(json!({"cmd":"catalog","tag":"c"})).await;
    let reply = device.expect("catalog").await;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["rows"][0]["id"], "dev.one");
    device.kill().await;
}

/// A handler that streams 1 MiB in 4 KiB pieces through `CallSink::send`,
/// counting what Hub has taken from it.
struct Producer {
    handed: Arc<std::sync::atomic::AtomicU64>,
}

impl DeviceRequests for Producer {
    fn features(&self) -> Vec<String> {
        vec!["catalog".into()]
    }
    fn handle(&self, call: DeviceCall) -> BoxFuture<'static, Result<Value, ErrorReply>> {
        let handed = self.handed.clone();
        Box::pin(async move {
            let mut head = serde_json::Map::new();
            head.insert("t".into(), json!("catalog.head"));
            call.sink.progress(head);
            for _ in 0..256 {
                call.sink.send(bytes(&[7u8; 4096])).await.map_err(|e| ErrorReply::new("internal", e.to_string()))?;
                handed.fetch_add(4096, std::sync::atomic::Ordering::SeqCst);
            }
            call.sink.end();
            Ok(json!({"bytes": 1 << 20}))
        })
    }
}

/// A device that grants 8 KiB and then nothing holds the producer at
/// `CALL_HELD_BYTES` past what it took: Hub's memory for a response stream
/// is bounded by the device's pace, not by the producer's (§5.6, §6.5 "a
/// slow device slows the download"). The credit the device sends in the
/// same write as the request is kept until the stream opens.
#[tokio::test(flavor = "multi_thread")]
async fn a_response_stream_waits_for_the_device() {
    use pocket_link::frame::{FrameType, JsonFrame};
    let handed = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dir = temp_dir("bound");
    let store = Arc::new(pocket_hub_store::Store::open(dir.join("store")).unwrap());
    let carrier = TcpCarrier::bind(Some(0)).await.unwrap();
    let endpoint = Endpoint::builder(store.clone()).config(fast_config()).listener(Box::new(carrier)).device_requests(Arc::new(Producer { handed: handed.clone() })).start();
    let port = endpoint.ports()[0].1.unwrap();
    let hub = Hub { events: endpoint.subscribe(), endpoint, store, port, dir };
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit_with(port, &id, &hub.store.hub_id(), &key, &["runtime", "apps", "catalog"]).await;
    let mut first = pocket_link::frame::encode_json(FrameType::Request, 1, &json!({"t": "catalog"}), JsonFrame::default(), 16384).unwrap().to_vec();
    first.extend_from_slice(&pocket_link::frame::encode_credit(1, 8192));
    raw.write(&first).await;
    let head = raw.frame().await.unwrap();
    assert_eq!(head.ty, FrameType::Event);
    let mut got = 0usize;
    while got < 8192 {
        let frame = raw.frame().await.unwrap();
        if frame.ty == FrameType::Data {
            got += frame.payload.len();
        }
    }
    assert_eq!(got, 8192);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let held = handed.load(std::sync::atomic::Ordering::SeqCst);
    assert!(held <= 8192 + pocket_hub_core::CALL_HELD_BYTES + 4096, "the producer ran ahead: {held} bytes handed over");
    assert!(held >= 8192);
    // Credit for the rest: the stream completes, END, then the REPLY.
    raw.write(&pocket_link::frame::encode_credit(1, (1 << 20) - 8192)).await;
    loop {
        let frame = raw.frame().await.unwrap();
        match frame.ty {
            FrameType::Data => got += frame.payload.len(),
            FrameType::Reply => break,
            _ => {}
        }
    }
    assert_eq!(got, 1 << 20);
}

#[derive(Debug, PartialEq)]
enum Notice {
    Signal(pocket_hub_core::CancelCause),
    Dropped,
}

struct DropGuard(tokio::sync::mpsc::UnboundedSender<Notice>);
impl Drop for DropGuard {
    fn drop(&mut self) {
        let _ = self.0.send(Notice::Dropped);
    }
}

/// A handler that reports its cancel signal and then ignores it.
struct Stubborn {
    notices: tokio::sync::mpsc::UnboundedSender<Notice>,
}

impl DeviceRequests for Stubborn {
    fn features(&self) -> Vec<String> {
        vec!["catalog".into()]
    }
    fn handle(&self, call: DeviceCall) -> BoxFuture<'static, Result<Value, ErrorReply>> {
        let notices = self.notices.clone();
        Box::pin(async move {
            let _guard = DropGuard(notices.clone());
            let cause = call.cancel.cancelled().await;
            let _ = notices.send(Notice::Signal(cause));
            std::future::pending::<Result<Value, ErrorReply>>().await
        })
    }
}

/// §5.4: Hub answers a `CANCEL` itself at once, the handler gets the signal
/// at once, and a handler that ignores it is dropped after the wind-down.
/// The link's end fires the same signal with `Closed`.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_reaches_the_handler_and_nothing_outlives_the_wind_down() {
    use pocket_hub_core::CancelCause;
    use pocket_link::frame::{FLAG_ERROR, FrameType, JsonFrame};
    let (tx, mut notices) = tokio::sync::mpsc::unbounded_channel();
    let dir = temp_dir("cancel");
    let store = Arc::new(pocket_hub_store::Store::open(dir.join("store")).unwrap());
    let carrier = TcpCarrier::bind(Some(0)).await.unwrap();
    let mut config = fast_config();
    config.timing.handler_wind_down = std::time::Duration::from_millis(300);
    let endpoint = Endpoint::builder(store.clone()).config(config).listener(Box::new(carrier)).device_requests(Arc::new(Stubborn { notices: tx })).start();
    let port = endpoint.ports()[0].1.unwrap();
    let hub = Hub { events: endpoint.subscribe(), endpoint, store, port, dir };
    let (_, id, key) = hub.pair();
    let (mut raw, _) = Raw::admit_with(port, &id, &hub.store.hub_id(), &key, &["runtime", "apps", "catalog"]).await;
    raw.write(&pocket_link::frame::encode_json(FrameType::Request, 1, &json!({"t": "catalog"}), JsonFrame::default(), 16384).unwrap()).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let cancelled_at = std::time::Instant::now();
    raw.write(&pocket_link::frame::encode_cancel(1)).await;
    let reply = loop {
        let frame = raw.frame().await.unwrap();
        if frame.ty == FrameType::Reply {
            break frame;
        }
    };
    assert_ne!(reply.flags & FLAG_ERROR, 0);
    assert_eq!(object(&reply)["code"], "cancelled");
    assert_eq!(next(&mut notices).await, Notice::Signal(CancelCause::Cancelled));
    assert!(cancelled_at.elapsed() < std::time::Duration::from_millis(250));
    assert_eq!(next(&mut notices).await, Notice::Dropped);
    let dropped = cancelled_at.elapsed();
    assert!(dropped >= std::time::Duration::from_millis(250) && dropped < std::time::Duration::from_secs(2), "{dropped:?}");

    // The link ends under an open request.
    raw.write(&pocket_link::frame::encode_json(FrameType::Request, 3, &json!({"t": "catalog"}), JsonFrame::default(), 16384).unwrap()).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    drop(raw);
    assert_eq!(next(&mut notices).await, Notice::Signal(CancelCause::Closed));
    assert_eq!(next(&mut notices).await, Notice::Dropped);
}

async fn next(notices: &mut tokio::sync::mpsc::UnboundedReceiver<Notice>) -> Notice {
    tokio::time::timeout(WAIT, notices.recv()).await.expect("a notice in time").expect("notices")
}
