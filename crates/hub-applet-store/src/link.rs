//! The device's own store page over the link: `catalog` and `install`
//! (link specification §6.4), served from the store client's catalog so the
//! device sees the offer the computer sees.
//!
//! - **The offer of a target** is every app of the held catalog with a
//!   variant for the device's `hello.target` whose package is verified in
//!   the cache (`AppletStore::offer`), cut by the row rules (`pager::rows`).
//! - **One revision for the Hub**, raised when the rows of a target some
//!   device fetched change; `catalog.changed` then goes to the linked devices
//!   of those targets. Pages are served from a snapshot per connection
//!   (`pager::Snapshots`), keyed by the connection's id.
//! - **An install of a package Hub has not downloaded yet.** Hub downloads it
//!   before `install.head`, because the head names the file's footer hash
//!   and size, which only the verified file gives. Nothing may travel on the
//!   request meanwhile: the only progress event the device reads is
//!   `install.head` (the reference device cancels on any other), and a
//!   requester fails a request after 30 s without a frame for it (§5.5). Hub
//!   therefore waits for the download at most `fetch_wait` (20 s), inside
//!   that 30 s. Past it Hub answers `busy` (retrying later can succeed,
//!   §5.2) and the download goes on, so the device's next try finds the
//!   package in the cache; it also enters the offer then, with
//!   `catalog.changed`. A download that fails is `internal`, with the
//!   store's reason in `message`.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use pocket_hub_core::{BoxFuture, DeviceCall, DeviceRequests, ErrorReply, SinkError};
use pocket_link::consts::{APP_ID_BYTES, INSTALL_MAX_BYTES};
use pocket_link::text::encoded_bytes;
use serde_json::{Map, Value, json};

use crate::client::{AppletStore, StoreEvent};
use crate::pager::{self, Cursor, Row, Snapshots};

/// How long an install waits for a download before it answers `busy`:
/// inside the requester's 30 s stall (§5.5), with room for the reply.
pub const FETCH_WAIT: Duration = Duration::from_secs(20);

/// Bytes read from the file per piece of the stream. `CallSink::send`
/// holds at most 32 KiB at Hub; the connection cuts frames to its payload limit.
const PIECE: usize = 16 * 1024;

pub struct LinkConfig {
    pub fetch_wait: Duration,
    /// The clock of the snapshots' 30 s life.
    pub now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self { fetch_wait: FETCH_WAIT, now: Arc::new(Instant::now) }
    }
}

type Notifier = Box<dyn Fn(Vec<String>) + Send + Sync>;

struct LinkState {
    rev: u32,
    /// The rows last served or announced per target: what a change is measured against.
    served: HashMap<String, Vec<Row>>,
    snapshots: Snapshots,
}

struct Inner {
    store: Arc<AppletStore>,
    config: LinkConfig,
    state: Mutex<LinkState>,
    notifier: OnceLock<Notifier>,
}

/// The `catalog` and `install` provider. Clones share one state.
#[derive(Clone)]
pub struct LinkCatalog {
    inner: Arc<Inner>,
}

fn error(code: &str, message: impl Into<String>) -> ErrorReply {
    ErrorReply::new(code, message)
}

impl LinkCatalog {
    pub fn new(store: Arc<AppletStore>, config: LinkConfig) -> Self {
        Self { inner: Arc::new(Inner { store, config, state: Mutex::new(LinkState { rev: 1, served: HashMap::new(), snapshots: Snapshots::default() }), notifier: OnceLock::new() }) }
    }

    /// Who sends `catalog.changed`: called with the targets whose offer
    /// changed, after the revision rose. Set once (the endpoint exists after
    /// this provider).
    pub fn set_notifier(&self, notifier: impl Fn(Vec<String>) + Send + Sync + 'static) {
        let _ = self.inner.notifier.set(Box::new(notifier));
    }

    /// Follows the store client's changes. Runs until the client is gone.
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        let mut events = self.inner.store.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(StoreEvent::Catalog { .. } | StoreEvent::Packages) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => this.recompute(),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    }

    /// The revision of the offer now.
    pub fn rev(&self) -> u32 {
        self.inner.state.lock().unwrap().rev
    }

    /// Measures the offer of every target a device fetched against what was
    /// served; when one changed, the revision rises and its devices are told.
    pub fn recompute(&self) {
        let targets: Vec<String> = self.inner.state.lock().unwrap().served.keys().cloned().collect();
        let now: Vec<(String, Vec<Row>)> = targets.into_iter().map(|target| {
            let rows = pager::rows(self.inner.store.offer(&target));
            (target, rows)
        }).collect();
        let changed: Vec<String> = {
            let mut state = self.inner.state.lock().unwrap();
            let changed: Vec<String> = now.into_iter().filter_map(|(target, rows)| (state.served.get(&target) != Some(&rows)).then(|| {
                state.served.insert(target.clone(), rows);
                target
            })).collect();
            if !changed.is_empty() {
                state.rev = state.rev.wrapping_add(1);
            }
            changed
        };
        if !changed.is_empty() {
            tracing::info!(targets = ?changed, "the link's catalog changed");
            if let Some(notify) = self.inner.notifier.get() {
                notify(changed);
            }
        }
    }

    fn catalog(&self, call: &DeviceCall) -> Result<Value, ErrorReply> {
        let cursor = pager::cursor(&call.object).map_err(|why| error("malformed", why))?;
        let now = (self.inner.config.now)();
        let page = match cursor {
            Cursor::First => {
                // The catalog the device reads is the one Hub holds; a stale
                // one is fetched again behind this answer, and the target's
                // packages are made ready, each followed by `catalog.changed`.
                self.inner.store.refresh_if_stale();
                self.inner.store.prefetch(vec![call.target.clone()]);
                let rows = pager::rows(self.inner.store.offer(&call.target));
                let mut state = self.inner.state.lock().unwrap();
                state.served.insert(call.target.clone(), rows.clone());
                let rev = state.rev;
                state.snapshots.first(call.conn, rows, rev, call.payload_limit, now)
            }
            Cursor::After { after, rev } => self.inner.state.lock().unwrap().snapshots.after(call.conn, &after, rev, call.payload_limit, now).map_err(|failure| {
                let message = match failure {
                    pager::PageError::Changed => "the catalog changed: fetch it again from the first page",
                    pager::PageError::Malformed => "after names no row of the catalog",
                };
                error(failure.code(), message)
            })?,
        };
        Ok(serde_json::to_value(page).expect("a page serializes"))
    }

    async fn install(self, call: DeviceCall) -> Result<Value, ErrorReply> {
        let app = match call.object.get("app") {
            Some(Value::String(app)) if !app.is_empty() && encoded_bytes(app) <= APP_ID_BYTES => app.clone(),
            _ => return Err(error("malformed", "app is an id of 1 to 128 bytes")),
        };
        let store = &self.inner.store;
        let entry = store
            .held()
            .and_then(|catalog| catalog.app(&app).cloned())
            .filter(|entry| entry.runs_on(&call.target))
            .ok_or_else(|| error("not_found", format!("the applet store offers no {app} for {}", call.target)))?;
        let facts = match store.ready(&entry.package) {
            Some(facts) => facts,
            None => {
                let wait = tokio::time::timeout(self.inner.config.fetch_wait, store.package(&entry, |_, _| {}));
                tokio::select! {
                    result = wait => match result {
                        Ok(Ok(facts)) => facts,
                        Ok(Err(failure)) => return Err(error("internal", failure.to_string())),
                        Err(_) => return Err(error("busy", "Hub is still downloading this package from the applet store; ask again")),
                    },
                    cause = call.cancel.cancelled() => return Err(error(cause.code(), "the install ended")),
                }
            }
        };
        if facts.variant(&call.target).is_none() {
            return Err(error("not_found", format!("the package of {app} has no variant for {}", call.target)));
        }
        if facts.bytes > INSTALL_MAX_BYTES {
            return Err(error("too_large", "the package is larger than 24 MiB"));
        }
        // Size and hash from one open descriptor (§6.4): the footer is read
        // through it and the stream reads the same file, so a file replaced
        // meanwhile streams one consistent version. The cache never rewrites
        // a name, so the footer must be the one Hub verified.
        let path = store.package_path(&entry.package);
        let opened = tokio::task::spawn_blocking(move || -> std::io::Result<(std::fs::File, u64, String)> {
            let file = std::fs::File::open(&path)?;
            let size = file.metadata()?.len();
            let mut footer = [0u8; 8];
            file.read_exact_at(&mut footer, size.saturating_sub(8))?;
            Ok((file, size, format!("{:016x}", u64::from_le_bytes(footer))))
        })
        .await
        .map_err(|_| error("internal", "the package could not be opened"))?;
        let (file, size, hash) = opened.map_err(|failure| error("internal", format!("the package could not be read: {failure}")))?;
        if size != facts.bytes || hash != facts.hash {
            store.invalidate(&entry.package);
            return Err(error("internal", "the cached package changed; Hub downloads it again"));
        }
        let mut head = Map::new();
        head.insert("t".into(), json!("install.head"));
        head.insert("app".into(), json!(entry.id));
        head.insert("hash".into(), json!(hash));
        head.insert("bytes".into(), json!(size));
        call.sink.progress(head);
        let stream = stream_file(file, size, &call);
        tokio::select! {
            result = stream => result?,
            cause = call.cancel.cancelled() => return Err(error(cause.code(), "the install ended")),
        }
        call.sink.end();
        tracing::info!(device = %call.device, app = %entry.id, version = %entry.version, bytes = size, "an applet was installed over the link");
        Ok(json!({"bytes": size}))
    }
}

/// The file's `size` bytes on the request's stream, under the device's credit.
async fn stream_file(file: std::fs::File, size: u64, call: &DeviceCall) -> Result<(), ErrorReply> {
    let file = Arc::new(Mutex::new(file));
    let mut sent = 0u64;
    while sent < size {
        let want = PIECE.min((size - sent) as usize);
        let source = file.clone();
        let piece = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
            let mut piece = vec![0u8; want];
            source.lock().unwrap().read_exact(&mut piece)?;
            Ok(piece)
        })
        .await
        .map_err(|_| error("internal", "the package could not be read"))?
        .map_err(|failure| error("internal", format!("the package could not be read: {failure}")))?;
        sent += piece.len() as u64;
        call.sink.send(Bytes::from(piece)).await.map_err(|failure| match failure {
            SinkError::Stalled => error("timeout", "30 s without credit for the package"),
            SinkError::Gone => error("cancelled", "the stream is gone"),
        })?;
    }
    Ok(())
}

impl DeviceRequests for LinkCatalog {
    fn features(&self) -> Vec<String> {
        vec!["catalog".into(), "install".into()]
    }

    fn handle(&self, call: DeviceCall) -> BoxFuture<'static, Result<Value, ErrorReply>> {
        let this = self.clone();
        Box::pin(async move {
            match call.t.as_str() {
                "catalog" => this.catalog(&call),
                "install" => this.install(call).await,
                other => Err(error("unknown_message", format!("unknown request {other}"))),
            }
        })
    }
}
