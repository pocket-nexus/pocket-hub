//! Hub as the applet store's client (docs/CONTROL-API.md §3 "Store").
//!
//! - **The catalog** is `GET /v1/apps`: the newest listed version of every
//!   app. Hub keeps it with its fetch time in `<cache>/catalog.json` and
//!   fetches it again when it is older than `max_age` (10 minutes) or a
//!   caller asks.
//! - **Files are content-addressed**: a package is `<cache>/packages/<sha256>.pocket`,
//!   an icon `<cache>/icons/<sha256>.png`, named by the SHA-256 the catalog
//!   gives. A download is written under a temporary name and renamed once it
//!   is verified, so a name in the cache always holds the bytes it names; a
//!   verified file is never downloaded again.
//! - **Every package is verified before it is kept**: its publisher's
//!   ed25519 signature over the catalog entry (`Entry::verify`), its size and
//!   SHA-256 against the entry, and its layout, footer and section hashes
//!   (`package::read`); its manifest and every variant's identity must name
//!   the entry's id and version, and every target the entry lists must have
//!   a variant. A file that fails is not kept: the error is `Rejected`.
//! - **The store does not honour range requests** (`/v1/packages/<sha256>`
//!   answers `200` with the whole file, measured against the live store), and
//!   its manifest route gives the variants' section hashes, not the footer
//!   hash a device checks: a row of the link's catalog therefore needs the
//!   whole package. Hub downloads the packages of the targets of its paired
//!   devices after each catalog fetch (`prefetch`), one at a time, and a
//!   row is offered once its package is verified in the cache.
//! - **An app's details** are `GET /v1/details/<sha256>` of the package the
//!   held catalog names: the store reads them out of the package, and Hub
//!   reads no manifest of its own. They are kept in memory and read again
//!   after each catalog fetch, one app at a time (`fetch_details`): the
//!   store's reading of a package grows (a new device gets its name), so
//!   they are not kept for good as a package is. A read that fails drops
//!   what was held, so nothing older stands in for them; a caller asks for
//!   those Hub does not hold.
//! - **An app's sample data** (`/v1/mocks/<sha256>`, the JSON the store's
//!   player answers an applet's requests from) is content-addressed like an
//!   icon: `<cache>/mocks/<sha256>.json`, downloaded when a caller wants it
//!   and checked against the SHA-256 the catalog names.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pocket_link::consts::INSTALL_MAX_BYTES;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, watch};

use crate::details::{self, DETAILS_MAX_BYTES, Details};
use crate::entry::{Entry, OFFICIAL_KEY};
use crate::package::{self, PackageFacts};
use crate::pager::Row;

/// The live store.
pub const DEFAULT_URL: &str = "https://applets.pocket.nexus";
/// The catalog is fetched again when it is older than this (docs/CONTROL-API.md §3).
pub const MAX_AGE: Duration = Duration::from_secs(10 * 60);
/// A catalog answer over this is not the store's: ten apps take 8 KB today.
pub const CATALOG_MAX_BYTES: usize = 8 * 1024 * 1024;
/// The store takes icons of at most 128 KiB (`ICON_MAX`).
pub const ICON_MAX_BYTES: usize = 128 * 1024;
/// Sample data over this is not the store's: the largest today is 3 MB.
pub const SAMPLE_MAX_BYTES: usize = 32 * 1024 * 1024;

/// How the client is set up.
#[derive(Clone)]
pub struct StoreConfig {
    /// The store's base URL, without a trailing slash.
    pub base_url: String,
    /// `<home>/cache/store`.
    pub cache: PathBuf,
    /// The key whose apps are `official`, base64url.
    pub official_key: String,
    pub max_age: Duration,
    /// A download that receives nothing for this long fails (`Unreachable`).
    pub stall: Duration,
    /// The longest wait for the catalog, an icon or an app's details, and for a package's answer head.
    pub request_timeout: Duration,
    /// Epoch milliseconds now: the catalog's age is measured with it.
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// The targets whose packages Hub keeps ready: the targets of its
    /// paired devices.
    pub wanted_targets: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
}

impl StoreConfig {
    pub fn new(base_url: &str, cache: impl Into<PathBuf>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            cache: cache.into(),
            official_key: OFFICIAL_KEY.to_owned(),
            max_age: MAX_AGE,
            stall: Duration::from_secs(30),
            request_timeout: Duration::from_secs(30),
            now: Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)),
            wanted_targets: Arc::new(Vec::new),
        }
    }
}

/// Why the store could not be used: docs/CONTROL-API.md's error `store`
/// with its `reason`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No answer, or not the store's answer (a status other than 200, a body
    /// that does not parse, a download that stalls or ends short).
    #[error("{0}")]
    Unreachable(String),
    /// A file does not match its hash, its publisher's signature does not
    /// verify, or the package is not what its entry says.
    #[error("{0}")]
    Rejected(String),
    /// Hub's own side failed (its cache could not be written): not the store's fault.
    #[error("{0}")]
    Local(String),
}

impl StoreError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Unreachable(_) => "unreachable",
            Self::Rejected(_) => "rejected",
            Self::Local(_) => "local",
        }
    }
}

/// The catalog Hub holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    /// Epoch milliseconds of the fetch.
    pub fetched_at: i64,
    /// In the store's order (by title).
    pub apps: Vec<Entry>,
}

impl Catalog {
    pub fn app(&self, id: &str) -> Option<&Entry> {
        self.apps.iter().find(|app| app.id == id)
    }
}

/// What changed in what the client holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreEvent {
    /// Another catalog, or icons that arrived (`store.changed`).
    Catalog { fetched_at: i64 },
    /// A package was verified into the cache, or found wanting: the link's
    /// offer may have changed.
    Packages,
}

#[derive(Clone)]
struct Fetch {
    done: u64,
    total: u64,
    result: Option<Result<Arc<PackageFacts>, StoreError>>,
}

#[derive(Default)]
struct State {
    catalog: Option<Arc<Catalog>>,
    /// Verified packages by SHA-256.
    ready: HashMap<String, Arc<PackageFacts>>,
    /// Packages this process found wanting, by SHA-256: the prefetch does
    /// not download them again; an install does.
    rejected: HashMap<String, String>,
    inflight: HashMap<String, watch::Receiver<Fetch>>,
    /// Apps' details by their package's SHA-256, read again after each
    /// catalog fetch (`fetch_details`).
    details: HashMap<String, Arc<Details>>,
    prefetching: bool,
    prefetch_again: bool,
    fetching_icons: bool,
    fetching_details: bool,
    refreshing: bool,
}

/// The store's client. Cheap to share: `Arc<AppletStore>`.
pub struct AppletStore {
    config: StoreConfig,
    http: reqwest::Client,
    state: Mutex<State>,
    /// One catalog fetch at a time.
    fetch_lock: tokio::sync::Mutex<()>,
    /// One request for details at a time: a caller that waited finds what
    /// the background's request kept (`fetch_details`) instead of asking again.
    details_lock: tokio::sync::Mutex<()>,
    /// Catalog fetches completed: a caller that waited for the lock knows
    /// whether one ended meanwhile.
    fetches: std::sync::atomic::AtomicU64,
    events: broadcast::Sender<StoreEvent>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Writes `bytes` at `path` through a temporary name in the same folder,
/// flushed before the rename: a reader sees the old file, the new one, or
/// nothing.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().expect("a cache file has a folder");
    std::fs::create_dir_all(dir)?;
    let mut temp = tempfile_in(dir)?;
    temp.1.write_all(bytes)?;
    temp.1.sync_all()?;
    std::fs::rename(&temp.0, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp.0);
    })
}

/// A new file `.part-<pid>-<n>` in `dir`.
fn tempfile_in(dir: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = dir.join(format!(".part-{}-{n}", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// The store on this computer (a mirror, a test store) is reached directly,
/// whatever proxy the environment names.
fn loopback(url: &str) -> bool {
    use reqwest::Url;
    match Url::parse(url).ok().as_ref().and_then(Url::host) {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name == "localhost",
        None => false,
    }
}

/// Checks a package's facts against the entry that names it.
fn consistent(entry: &Entry, facts: &PackageFacts) -> Result<(), String> {
    if facts.id != entry.id || facts.version != entry.version {
        return Err(format!("the package of {} {} says it is {} {}", entry.id, entry.version, facts.id, facts.version));
    }
    if let Some(variant) = facts.variants.iter().find(|variant| variant.id != entry.id) {
        return Err(format!("the {} variant of {} names the app {}", variant.target, entry.id, variant.id));
    }
    if let Some(target) = entry.targets.iter().find(|target| facts.variant(target).is_none()) {
        return Err(format!("the package of {} has no variant for {target}, which the catalog lists", entry.id));
    }
    Ok(())
}

impl AppletStore {
    /// The client, with the catalog of the last run when the cache holds
    /// one. Packages already in the cache are verified again in the
    /// background (their SHA-256, signature and layout; nothing is
    /// downloaded), and become part of the offer as they pass.
    pub fn open(config: StoreConfig) -> Result<Arc<Self>, String> {
        let mut builder = reqwest::Client::builder().user_agent(concat!("pocket-hub/", env!("CARGO_PKG_VERSION"))).connect_timeout(Duration::from_secs(10));
        if loopback(&config.base_url) {
            builder = builder.no_proxy();
        }
        let http = builder.build().map_err(|error| format!("the HTTP client cannot start: {error}"))?;
        let held = std::fs::read(config.cache.join("catalog.json")).ok().and_then(|bytes| serde_json::from_slice::<Catalog>(&bytes).ok());
        let store = Arc::new(Self {
            config,
            http,
            state: Mutex::new(State { catalog: held.map(Arc::new), ..State::default() }),
            fetch_lock: tokio::sync::Mutex::new(()),
            details_lock: tokio::sync::Mutex::new(()),
            fetches: std::sync::atomic::AtomicU64::new(0),
            events: broadcast::channel(64).0,
        });
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let this = store.clone();
            handle.spawn(async move { this.verify_cached().await });
        }
        Ok(store)
    }

    pub fn config(&self) -> &StoreConfig {
        &self.config
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StoreEvent> {
        self.events.subscribe()
    }

    fn emit(&self, event: StoreEvent) {
        let _ = self.events.send(event);
    }

    /// The catalog held now, whatever its age.
    pub fn held(&self) -> Option<Arc<Catalog>> {
        self.state.lock().unwrap().catalog.clone()
    }

    fn fresh(&self, catalog: &Catalog) -> bool {
        let age = (self.config.now)() - catalog.fetched_at;
        (0..self.config.max_age.as_millis() as i64).contains(&age)
    }

    /// The catalog: the one held while it is younger than `max_age`, else
    /// fetched again; with `refresh`, fetched again in any case. A fetch that
    /// fails is the error, with no older catalog in its place.
    pub async fn apps(self: &Arc<Self>, refresh: bool) -> Result<Arc<Catalog>, StoreError> {
        use std::sync::atomic::Ordering;
        let asked = self.fetches.load(Ordering::SeqCst);
        if !refresh && let Some(held) = self.held().filter(|held| self.fresh(held)) {
            return Ok(held);
        }
        let _one = self.fetch_lock.lock().await;
        // A fetch that ended while this call waited serves it.
        let fetched_meanwhile = self.fetches.load(Ordering::SeqCst) != asked;
        if let Some(held) = self.held().filter(|held| if refresh { fetched_meanwhile } else { self.fresh(held) }) {
            return Ok(held);
        }
        let catalog = Arc::new(self.fetch_catalog().await?);
        self.fetches.fetch_add(1, Ordering::SeqCst);
        self.install_catalog(catalog.clone());
        Ok(catalog)
    }

    /// For the link: fetches the catalog again in the background when none is
    /// held or it is older than `max_age`; the device is answered from what
    /// is held meanwhile, and `catalog.changed` follows a new offer.
    pub fn refresh_if_stale(self: &Arc<Self>) {
        if self.held().is_some_and(|held| self.fresh(&held)) {
            return;
        }
        {
            let mut state = self.state.lock().unwrap();
            if state.refreshing {
                return;
            }
            state.refreshing = true;
        }
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(error) = this.apps(false).await {
                tracing::warn!(%error, "the applet store's catalog could not be fetched");
            }
            this.state.lock().unwrap().refreshing = false;
        });
    }

    async fn fetch_catalog(&self) -> Result<Catalog, StoreError> {
        let url = format!("{}/v1/apps", self.config.base_url);
        let unreachable = |why: String| StoreError::Unreachable(format!("The applet store at {} could not be reached: {why}.", self.config.base_url));
        let work = async {
            let mut response = self.http.get(&url).send().await.map_err(|error| unreachable(error_text(&error)))?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(unreachable(format!("it answered {} for its catalog", response.status())));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| unreachable(error_text(&error)))? {
                body.extend_from_slice(&chunk);
                if body.len() > CATALOG_MAX_BYTES {
                    return Err(unreachable("its catalog is longer than 8 MiB, which is not the store's answer".into()));
                }
            }
            Ok(body)
        };
        let body = tokio::time::timeout(self.config.request_timeout, work).await.map_err(|_| unreachable(format!("no catalog within {} s", self.config.request_timeout.as_secs())))??;
        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|_| unreachable("its catalog is not JSON".into()))?;
        let list = value.get("apps").and_then(serde_json::Value::as_array).ok_or_else(|| unreachable("its catalog has no apps list".into()))?;
        let mut apps: Vec<Entry> = Vec::with_capacity(list.len());
        for item in list {
            // One entry the client cannot read does not hide the others; it is logged.
            match serde_json::from_value::<Entry>(item.clone()).map_err(|error| error.to_string()).and_then(|entry| entry.check().map(|_| entry)) {
                Ok(entry) if apps.iter().any(|app| app.id == entry.id) => tracing::warn!(id = %entry.id, "the store's catalog lists an app twice; the first is kept"),
                Ok(entry) => apps.push(entry),
                Err(error) => tracing::warn!(%error, "an app of the store's catalog is left out"),
            }
        }
        Ok(Catalog { fetched_at: (self.config.now)(), apps })
    }

    /// Holds a fetched catalog: written to the cache, announced when it
    /// differs from the one held, followed by its icons, the packages of the
    /// wanted targets and the removal of files no app of it names.
    fn install_catalog(self: &Arc<Self>, catalog: Arc<Catalog>) {
        if let Err(error) = write_atomic(&self.config.cache.join("catalog.json"), &serde_json::to_vec(&*catalog).expect("a catalog serializes")) {
            tracing::warn!(%error, "the applet store's catalog could not be written to the cache");
        }
        let changed = {
            let mut state = self.state.lock().unwrap();
            let changed = state.catalog.as_ref().is_none_or(|held| held.apps != catalog.apps);
            state.catalog = Some(catalog.clone());
            changed
        };
        if changed {
            self.emit(StoreEvent::Catalog { fetched_at: catalog.fetched_at });
        }
        self.prune(&catalog);
        let this = self.clone();
        tokio::spawn(async move { this.fetch_icons().await });
        let this = self.clone();
        tokio::spawn(async move { this.fetch_details().await });
        self.prefetch((self.config.wanted_targets)());
    }

    // --- packages --------------------------------------------------------------

    pub fn package_path(&self, sha256: &str) -> PathBuf {
        self.config.cache.join("packages").join(format!("{sha256}.pocket"))
    }

    pub fn sample_file(&self, sha256: &str) -> PathBuf {
        self.config.cache.join("mocks").join(format!("{sha256}.json"))
    }

    pub fn icon_file(&self, sha256: &str) -> PathBuf {
        self.config.cache.join("icons").join(format!("{sha256}.png"))
    }

    /// The icon's path once it is in the cache.
    pub fn icon_path(&self, entry: &Entry) -> Option<PathBuf> {
        let path = self.icon_file(&entry.icon);
        path.is_file().then_some(path)
    }

    /// The package's facts when it is verified in the cache.
    pub fn ready(&self, sha256: &str) -> Option<Arc<PackageFacts>> {
        self.state.lock().unwrap().ready.get(sha256).cloned()
    }

    /// Forgets a verified package whose file no longer holds its bytes.
    pub fn invalidate(&self, sha256: &str) {
        self.state.lock().unwrap().ready.remove(sha256);
        self.emit(StoreEvent::Packages);
    }

    /// The entry's package, verified: from the cache, else downloaded. A
    /// download that runs (for the prefetch, another caller) is joined, not
    /// started twice, and goes on when the caller goes away. `progress`
    /// gets `(done, total)` bytes.
    pub async fn package(self: &Arc<Self>, entry: &Entry, mut progress: impl FnMut(u64, u64)) -> Result<Arc<PackageFacts>, StoreError> {
        let mut rx = self.start(entry);
        loop {
            let fetch = rx.borrow_and_update().clone();
            progress(fetch.done, fetch.total);
            if let Some(result) = fetch.result {
                return result;
            }
            if rx.changed().await.is_err() {
                return Err(StoreError::Unreachable(format!("The download of {} ended without an answer.", entry.id)));
            }
        }
    }

    fn start(self: &Arc<Self>, entry: &Entry) -> watch::Receiver<Fetch> {
        let mut state = self.state.lock().unwrap();
        if let Some(facts) = state.ready.get(&entry.package) {
            let (_, rx) = watch::channel(Fetch { done: facts.bytes, total: facts.bytes, result: Some(Ok(facts.clone())) });
            return rx;
        }
        if let Some(rx) = state.inflight.get(&entry.package) {
            return rx.clone();
        }
        let (tx, rx) = watch::channel(Fetch { done: 0, total: entry.bytes, result: None });
        state.inflight.insert(entry.package.clone(), rx.clone());
        drop(state);
        let (this, entry) = (self.clone(), entry.clone());
        tokio::spawn(async move {
            let result = this.obtain(&entry, &tx).await;
            match &result {
                Ok(facts) => tracing::info!(id = %entry.id, version = %entry.version, bytes = facts.bytes, "an applet package is verified in the cache"),
                Err(error) => tracing::warn!(id = %entry.id, version = %entry.version, %error, "an applet package is not in the cache"),
            }
            {
                let mut state = this.state.lock().unwrap();
                state.inflight.remove(&entry.package);
                match &result {
                    Ok(facts) => {
                        state.ready.insert(entry.package.clone(), facts.clone());
                        state.rejected.remove(&entry.package);
                    }
                    Err(StoreError::Rejected(why)) => {
                        state.rejected.insert(entry.package.clone(), why.clone());
                    }
                    Err(StoreError::Unreachable(_) | StoreError::Local(_)) => {}
                }
            }
            let ok = result.is_ok();
            tx.send_modify(|fetch| fetch.result = Some(result));
            if ok {
                this.emit(StoreEvent::Packages);
            }
        });
        rx
    }

    /// The package from the cache when its file verifies, else from the store.
    async fn obtain(&self, entry: &Entry, tx: &watch::Sender<Fetch>) -> Result<Arc<PackageFacts>, StoreError> {
        let rejected = |why: String| StoreError::Rejected(format!("The package of {} {} is rejected: {why}.", entry.id, entry.version));
        entry.verify(&self.config.official_key).map_err(rejected)?;
        if entry.bytes > INSTALL_MAX_BYTES {
            return Err(rejected(format!("it is {} bytes, more than the 24 MiB a device takes", entry.bytes)));
        }
        let path = self.package_path(&entry.package);
        if path.is_file() {
            let (entry_copy, file) = (entry.clone(), path.clone());
            match tokio::task::spawn_blocking(move || verify_file(&entry_copy, &file)).await.map_err(|_| rejected("its check failed".into()))? {
                Ok(facts) => {
                    tx.send_modify(|fetch| fetch.done = facts.bytes);
                    return Ok(Arc::new(facts));
                }
                Err(why) => {
                    tracing::warn!(id = %entry.id, %why, "a cached package does not verify; it is downloaded again");
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let bytes = self.download(entry, tx).await?;
        let (entry_copy, path_copy) = (entry.clone(), path.clone());
        tokio::task::spawn_blocking(move || -> Result<PackageFacts, StoreError> {
            let facts = package::read(&bytes).and_then(|facts| consistent(&entry_copy, &facts).map(|_| facts)).map_err(|why| StoreError::Rejected(format!("The package of {} {} is rejected: {why}.", entry_copy.id, entry_copy.version)))?;
            write_atomic(&path_copy, &bytes).map_err(|error| StoreError::Local(format!("The package of {} could not be written to the cache: {error}.", entry_copy.id)))?;
            Ok(facts)
        })
        .await
        .map_err(|_| rejected("its check failed".into()))?
        .map(Arc::new)
    }

    /// The package's bytes, checked against the entry's size and SHA-256.
    async fn download(&self, entry: &Entry, tx: &watch::Sender<Fetch>) -> Result<Vec<u8>, StoreError> {
        let url = format!("{}/v1/packages/{}", self.config.base_url, entry.package);
        let unreachable = |why: String| StoreError::Unreachable(format!("The package of {} could not be downloaded from {}: {why}.", entry.id, self.config.base_url));
        let rejected = |why: String| StoreError::Rejected(format!("The package of {} {} is rejected: {why}.", entry.id, entry.version));
        let mut response = tokio::time::timeout(self.config.request_timeout, self.http.get(&url).send())
            .await
            .map_err(|_| unreachable(format!("no answer within {} s", self.config.request_timeout.as_secs())))?
            .map_err(|error| unreachable(error_text(&error)))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(unreachable(format!("the store answered {}", response.status())));
        }
        if let Some(length) = response.content_length().filter(|length| *length != entry.bytes) {
            return Err(rejected(format!("the store sends {length} bytes and the catalog names {}", entry.bytes)));
        }
        let mut bytes = Vec::with_capacity(entry.bytes as usize);
        let mut hasher = Sha256::new();
        loop {
            let chunk = tokio::time::timeout(self.config.stall, response.chunk())
                .await
                .map_err(|_| unreachable(format!("nothing arrived for {} s", self.config.stall.as_secs())))?
                .map_err(|error| unreachable(error_text(&error)))?;
            let Some(chunk) = chunk else { break };
            if bytes.len() as u64 + chunk.len() as u64 > entry.bytes {
                return Err(rejected(format!("the store sends more than the {} bytes the catalog names", entry.bytes)));
            }
            hasher.update(&chunk);
            bytes.extend_from_slice(&chunk);
            let done = bytes.len() as u64;
            tx.send_modify(|fetch| fetch.done = done);
        }
        if bytes.len() as u64 != entry.bytes {
            return Err(unreachable(format!("the download ended at {} of {} bytes", bytes.len(), entry.bytes)));
        }
        if hex::encode(hasher.finalize()) != entry.package {
            return Err(rejected("its bytes do not match the SHA-256 the catalog names".into()));
        }
        Ok(bytes)
    }

    /// Verifies the packages the cache holds for the held catalog; nothing
    /// is downloaded.
    async fn verify_cached(self: Arc<Self>) {
        let Some(catalog) = self.held() else { return };
        let mut any = false;
        for entry in &catalog.apps {
            let path = self.package_path(&entry.package);
            if !path.is_file() || self.ready(&entry.package).is_some() || entry.verify(&self.config.official_key).is_err() {
                continue;
            }
            let (copy, file) = (entry.clone(), path.clone());
            match tokio::task::spawn_blocking(move || verify_file(&copy, &file)).await {
                Ok(Ok(facts)) => {
                    self.state.lock().unwrap().ready.insert(entry.package.clone(), Arc::new(facts));
                    any = true;
                }
                Ok(Err(why)) => {
                    tracing::warn!(id = %entry.id, %why, "a cached package does not verify and is removed");
                    let _ = std::fs::remove_file(&path);
                }
                Err(_) => {}
            }
        }
        if any {
            self.emit(StoreEvent::Packages);
        }
    }

    /// Downloads, one at a time, every package of the held catalog that runs
    /// on one of `targets` and is not verified in the cache yet. One round
    /// runs at a time; a call during it runs another round after it.
    pub fn prefetch(self: &Arc<Self>, targets: Vec<String>) {
        if targets.is_empty() {
            return;
        }
        {
            let mut state = self.state.lock().unwrap();
            if state.prefetching {
                state.prefetch_again = true;
                return;
            }
            state.prefetching = true;
        }
        let this = self.clone();
        tokio::spawn(async move {
            let mut targets = targets;
            loop {
                let wanted: Vec<Entry> = {
                    let state = this.state.lock().unwrap();
                    let apps = state.catalog.as_ref().map(|c| c.apps.clone()).unwrap_or_default();
                    apps.into_iter()
                        .filter(|entry| targets.iter().any(|target| entry.runs_on(target)))
                        .filter(|entry| !state.ready.contains_key(&entry.package) && !state.rejected.contains_key(&entry.package))
                        .collect()
                };
                for entry in wanted {
                    let _ = this.package(&entry, |_, _| {}).await;
                }
                let mut state = this.state.lock().unwrap();
                if !state.prefetch_again {
                    state.prefetching = false;
                    return;
                }
                state.prefetch_again = false;
                drop(state);
                targets = (this.config.wanted_targets)();
            }
        });
    }

    // --- details ---------------------------------------------------------------

    /// The details of the entry's package (`/v1/details/<sha256>`): the ones
    /// Hub holds, else asked for now and kept. A store that does not answer
    /// them, or not with a package's details, is `Unreachable`.
    pub async fn details(&self, entry: &Entry) -> Result<Arc<Details>, StoreError> {
        if let Some(kept) = self.held_details(entry) {
            return Ok(kept);
        }
        let _one = self.details_lock.lock().await;
        if let Some(kept) = self.held_details(entry) {
            return Ok(kept);
        }
        let details = Arc::new(self.read_details(entry).await?);
        self.state.lock().unwrap().details.insert(entry.package.clone(), details.clone());
        Ok(details)
    }

    /// The store's answer at `/v1/details/<sha256>`.
    async fn read_details(&self, entry: &Entry) -> Result<Details, StoreError> {
        let url = format!("{}/v1/details/{}", self.config.base_url, entry.package);
        let unreachable = |why: String| StoreError::Unreachable(format!("The details of {} could not be read from the applet store at {}: {why}.", entry.id, self.config.base_url));
        let work = async {
            let mut response = self.http.get(&url).send().await.map_err(|error| unreachable(error_text(&error)))?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(unreachable(format!("it answered {}", response.status())));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| unreachable(error_text(&error)))? {
                body.extend_from_slice(&chunk);
                if body.len() > DETAILS_MAX_BYTES {
                    return Err(unreachable("its answer is longer than 1 MiB, which is not the store's".into()));
                }
            }
            Ok(body)
        };
        let body = tokio::time::timeout(self.config.request_timeout, work).await.map_err(|_| unreachable(format!("no answer within {} s", self.config.request_timeout.as_secs())))??;
        details::parse(&body).map_err(unreachable)
    }

    /// The details of the entry's package when Hub holds them, without asking.
    pub fn held_details(&self, entry: &Entry) -> Option<Arc<Details>> {
        self.state.lock().unwrap().details.get(&entry.package).cloned()
    }

    /// Reads the details of every app of the held catalog again, one at a
    /// time, and forgets those of packages it no longer names. A read that
    /// fails drops what was held. `Catalog` follows when what Hub holds
    /// changed: what `store.apps` answers (each app's devices) changed with it.
    async fn fetch_details(self: Arc<Self>) {
        {
            let mut state = self.state.lock().unwrap();
            if state.fetching_details {
                return;
            }
            state.fetching_details = true;
        }
        let catalog = self.held();
        let mut changed = false;
        if let Some(catalog) = &catalog {
            let named: HashSet<&str> = catalog.apps.iter().map(|app| app.package.as_str()).collect();
            let mut state = self.state.lock().unwrap();
            let before = state.details.len();
            state.details.retain(|sha, _| named.contains(sha.as_str()));
            changed |= state.details.len() != before;
        }
        for entry in catalog.iter().flat_map(|c| c.apps.iter()) {
            let _one = self.details_lock.lock().await;
            let read = self.read_details(entry).await;
            let mut state = self.state.lock().unwrap();
            match read {
                Ok(details) => {
                    let new = state.details.get(&entry.package).is_none_or(|held| **held != details);
                    if new {
                        state.details.insert(entry.package.clone(), Arc::new(details));
                    }
                    changed |= new;
                }
                Err(error) => {
                    tracing::warn!(id = %entry.id, %error, "an applet's details are not held");
                    changed |= state.details.remove(&entry.package).is_some();
                }
            }
        }
        self.state.lock().unwrap().fetching_details = false;
        if changed && let Some(catalog) = catalog {
            self.emit(StoreEvent::Catalog { fetched_at: catalog.fetched_at });
        }
    }

    // --- sample data -------------------------------------------------------------

    /// The entry's sample data, checked against the SHA-256 the catalog names:
    /// from the cache, else downloaded. None when the version has none.
    pub async fn sample(&self, entry: &Entry) -> Result<Option<PathBuf>, StoreError> {
        let Some(sha) = entry.mock.as_deref() else { return Ok(None) };
        let path = self.sample_file(sha);
        if let Ok(bytes) = std::fs::read(&path) {
            if sha256_hex(&bytes) == sha {
                return Ok(Some(path));
            }
            tracing::warn!(id = %entry.id, "cached sample data does not match its SHA-256; it is downloaded again");
            let _ = std::fs::remove_file(&path);
        }
        let url = format!("{}/v1/mocks/{sha}", self.config.base_url);
        let unreachable = |why: String| StoreError::Unreachable(format!("The sample data of {} could not be downloaded from {}: {why}.", entry.id, self.config.base_url));
        let work = async {
            let mut response = self.http.get(&url).send().await.map_err(|error| unreachable(error_text(&error)))?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(unreachable(format!("the store answered {}", response.status())));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| unreachable(error_text(&error)))? {
                body.extend_from_slice(&chunk);
                if body.len() > SAMPLE_MAX_BYTES {
                    return Err(unreachable("it is longer than 32 MiB, which is not the store's".into()));
                }
            }
            Ok(body)
        };
        let body = tokio::time::timeout(self.config.request_timeout, work).await.map_err(|_| unreachable(format!("no answer within {} s", self.config.request_timeout.as_secs())))??;
        if sha256_hex(&body) != sha {
            return Err(StoreError::Rejected(format!("The sample data of {} {} is rejected: its bytes do not match the SHA-256 the catalog names.", entry.id, entry.version)));
        }
        write_atomic(&path, &body).map_err(|error| StoreError::Local(format!("The sample data of {} could not be written to the cache: {error}.", entry.id)))?;
        Ok(Some(path))
    }

    // --- icons -----------------------------------------------------------------

    /// Downloads the icons the cache lacks, of apps whose signature verifies
    /// (the signature covers the icon's hash). `store.changed` follows when
    /// one arrived.
    async fn fetch_icons(self: Arc<Self>) {
        {
            let mut state = self.state.lock().unwrap();
            if state.fetching_icons {
                return;
            }
            state.fetching_icons = true;
        }
        let catalog = self.held();
        let mut arrived = false;
        for entry in catalog.iter().flat_map(|c| c.apps.iter()) {
            if self.icon_path(entry).is_some() || entry.verify(&self.config.official_key).is_err() {
                continue;
            }
            match self.download_icon(entry).await {
                Ok(()) => arrived = true,
                Err(error) => tracing::warn!(id = %entry.id, %error, "an applet's icon is not in the cache"),
            }
        }
        self.state.lock().unwrap().fetching_icons = false;
        if arrived && let Some(catalog) = catalog {
            self.emit(StoreEvent::Catalog { fetched_at: catalog.fetched_at });
        }
    }

    async fn download_icon(&self, entry: &Entry) -> Result<(), String> {
        let url = format!("{}/v1/icons/{}", self.config.base_url, entry.icon);
        let work = async {
            let mut response = self.http.get(&url).send().await.map_err(|error| error_text(&error))?;
            if response.status() != reqwest::StatusCode::OK {
                return Err(format!("the store answered {}", response.status()));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| error_text(&error))? {
                body.extend_from_slice(&chunk);
                if body.len() > ICON_MAX_BYTES {
                    return Err("the icon is larger than 128 KiB".into());
                }
            }
            Ok(body)
        };
        let body = tokio::time::timeout(self.config.request_timeout, work).await.map_err(|_| "no answer in time".to_owned())??;
        if sha256_hex(&body) != entry.icon {
            return Err("the icon does not match its SHA-256".into());
        }
        write_atomic(&self.icon_file(&entry.icon), &body).map_err(|error| error.to_string())
    }

    /// Removes the cached packages, icons and sample data no app of the catalog names: the
    /// store lists only newest versions, so these are never asked for again.
    fn prune(&self, catalog: &Catalog) {
        let packages: HashSet<String> = catalog.apps.iter().map(|app| format!("{}.pocket", app.package)).collect();
        let icons: HashSet<String> = catalog.apps.iter().map(|app| format!("{}.png", app.icon)).collect();
        let samples: HashSet<String> = catalog.apps.iter().filter_map(|app| app.mock.as_ref()).map(|sha| format!("{sha}.json")).collect();
        let inflight: HashSet<String> = self.state.lock().unwrap().inflight.keys().map(|sha| format!("{sha}.pocket")).collect();
        for (dir, keep) in [(self.config.cache.join("packages"), &packages), (self.config.cache.join("icons"), &icons), (self.config.cache.join("mocks"), &samples)] {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for item in entries.flatten() {
                let name = item.file_name().to_string_lossy().into_owned();
                // Temporary files of a download are its own to remove.
                if name.starts_with(".part-") || keep.contains(&name) || inflight.contains(&name) {
                    continue;
                }
                let _ = std::fs::remove_file(item.path());
            }
        }
        let mut state = self.state.lock().unwrap();
        state.ready.retain(|sha, _| packages.contains(&format!("{sha}.pocket")));
    }

    // --- the link's offer ------------------------------------------------------

    /// The link's offer for a device target (§6.4), before the row rules of
    /// `pager::rows`: every app of the held catalog with a variant for the
    /// target whose package is verified in the cache, with that variant's
    /// output name and the file's footer hash. Title, version and summary
    /// are the catalog's, so the device shows what the computer shows; the
    /// summary's runs of white space become one space, as the reference
    /// offer does (`offer.ts`).
    pub fn offer(&self, target: &str) -> Vec<Row> {
        let state = self.state.lock().unwrap();
        let Some(catalog) = &state.catalog else { return Vec::new() };
        catalog
            .apps
            .iter()
            .filter(|entry| entry.runs_on(target))
            .filter_map(|entry| {
                let facts = state.ready.get(&entry.package)?;
                let variant = facts.variant(target)?;
                Some(Row {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    version: entry.version.clone(),
                    summary: entry.summary.split_whitespace().collect::<Vec<_>>().join(" "),
                    output: variant.output.clone(),
                    hash: facts.hash.clone(),
                    bytes: facts.bytes,
                })
            })
            .collect()
    }
}

/// Verifies a cached file against its entry: size, SHA-256, layout and
/// facts. The signature is checked by the caller.
fn verify_file(entry: &Entry, path: &Path) -> Result<PackageFacts, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 != entry.bytes {
        return Err(format!("{} bytes, the catalog names {}", bytes.len(), entry.bytes));
    }
    if sha256_hex(&bytes) != entry.package {
        return Err("its bytes do not match the SHA-256 the catalog names".into());
    }
    let facts = package::read(&bytes)?;
    consistent(entry, &facts)?;
    Ok(facts)
}

/// An error with its causes, for one line.
fn error_text(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let more = cause.to_string();
        if !text.contains(&more) {
            text.push_str(": ");
            text.push_str(&more);
        }
        source = cause.source();
    }
    text
}
