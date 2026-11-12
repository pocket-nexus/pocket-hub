//! §6.2 Runtime operations, as typed methods on a device's handle, and the
//! device mutex around the state-changing ones (§8.3).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use bytes::Bytes;
use pocket_link::consts::*;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::api::{DeviceSnapshot, LaunchTarget, OpError};
use crate::apps::InstalledApp;
use crate::conn::{ConnMsg, Input, Op, OpOutcome};
use crate::endpoint::Shared;
use crate::lock::LockGuard;
use crate::screenshot::{Screenshot, compose, surfaces_of};

/// The footer a `.pocket` stores: the FNV-1a64 hash of the bytes before it,
/// as 16 hex digits (`tools/pocket-link/v2/offer.ts` `packageFooter`).
pub const PACKAGE_FOOTER_BYTES: usize = 8;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, &byte| (hash ^ byte as u64).wrapping_mul(FNV_PRIME))
}

/// The footer hash a package stores, checked against the bytes before it.
pub fn package_hash(package: &[u8]) -> Result<String, String> {
    if package.len() < PACKAGE_FOOTER_BYTES + 16 {
        return Err("the package is truncated".into());
    }
    let (body, footer) = package.split_at(package.len() - PACKAGE_FOOTER_BYTES);
    let stored = u64::from_le_bytes(footer.try_into().expect("8 bytes"));
    if stored != fnv1a64(body) {
        return Err("the package's footer hash does not match its bytes".into());
    }
    Ok(format!("{stored:016x}"))
}

/// The footer hash of a package `runtime.push` takes: 1 byte to 24 MiB (§6.2).
fn push_hash(package: &[u8]) -> Result<String, OpError> {
    if package.is_empty() || package.len() as u64 > PUSH_MAX_BYTES {
        return Err(OpError::Invalid { message: "a package is 1 byte to 24 MiB".into() });
    }
    package_hash(package).map_err(|message| OpError::Invalid { message })
}

/// A `.3dsx` name (§6.2): 6 to 64 bytes of `[A-Za-z0-9._-]`, ending in
/// `.3dsx`, not starting with `.`: one file in `sdmc:/3ds/` itself.
pub fn valid_native_name(name: &str) -> bool {
    (NATIVE_NAME_MIN..=NATIVE_NAME_MAX).contains(&name.len())
        && name.ends_with(".3dsx")
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The limits of `runtime.native.install`'s request (§6.2), checked before
/// anything is sent.
fn check_native(name: &str, file: &[u8]) -> Result<(), OpError> {
    if !valid_native_name(name) {
        return Err(OpError::Invalid { message: "a .3dsx name is 6 to 64 bytes of [A-Za-z0-9._-], ends in .3dsx and does not start with .".into() });
    }
    if (file.len() as u64) < NATIVE_MIN_BYTES || file.len() as u64 > NATIVE_MAX_BYTES {
        return Err(OpError::Invalid { message: "a .3dsx is 32 bytes to 32 MiB".into() });
    }
    Ok(())
}

/// Sends `CANCEL` for the operation's request when its caller goes away
/// before the reply (§8.3: the holder's local client went away).
struct CancelOnDrop {
    tx: mpsc::UnboundedSender<Input>,
    key: u64,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.tx.send(Input::Msg(ConnMsg::CancelOp(self.key)));
        }
    }
}

/// One paired device, for the rest of Hub.
#[derive(Clone)]
pub struct DeviceHandle {
    shared: Arc<Shared>,
    id: String,
}

impl DeviceHandle {
    pub(crate) fn new(shared: Arc<Shared>, id: String) -> Self {
        Self { shared, id }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The device with its live state; None once it is forgotten.
    pub fn snapshot(&self) -> Option<DeviceSnapshot> {
        self.shared.store.device(&self.id).map(|record| self.shared.snapshot(&record))
    }

    /// The last `runtime.status` the device sent, by event or reply.
    pub fn cached_status(&self) -> Option<Value> {
        self.shared.registry.lock().unwrap().devices.get(&self.id).and_then(|s| s.status.clone())
    }

    /// The device's installed applets, from its last complete report.
    pub fn apps(&self) -> Option<Vec<InstalledApp>> {
        self.shared.registry.lock().unwrap().devices.get(&self.id).and_then(|s| s.apps.clone())
    }

    /// The latest unsolicited screenshot (the dev menu's), PNG included.
    pub fn latest_screenshot(&self) -> Option<Arc<Screenshot>> {
        self.shared.registry.lock().unwrap().devices.get(&self.id).and_then(|s| s.screenshot.clone())
    }

    async fn request(
        &self,
        request: Value,
        group: &'static str,
        body: Option<Bytes>,
        progress: Option<mpsc::UnboundedSender<Value>>,
        sent: Option<mpsc::UnboundedSender<u64>>,
    ) -> Result<OpOutcome, OpError> {
        let state_changing = matches!(request["t"].as_str(), Some("runtime.push" | "runtime.native.install" | "runtime.launch"));
        if self.shared.store.device(&self.id).is_none() {
            return Err(OpError::Unknown);
        }
        let tx = self.shared.link_tx(&self.id).ok_or(OpError::NotLinked)?;
        let key = self.shared.next_op.fetch_add(1, Ordering::SeqCst);
        let (done, result) = oneshot::channel();
        let Value::Object(request) = request else { unreachable!("requests are objects") };
        tx.send(Input::Msg(ConnMsg::Op(Op { key, request, group, body, progress, sent, state_changing, done }))).map_err(|_| OpError::NotLinked)?;
        let mut guard = CancelOnDrop { tx, key, armed: true };
        let outcome = result.await.unwrap_or_else(|_| Err(OpError::link("closed", "the connection closed")));
        guard.armed = false;
        outcome
    }

    async fn lock(&self, holder: &str) -> Result<LockGuard, OpError> {
        if self.shared.store.device(&self.id).is_none() {
            return Err(OpError::Unknown);
        }
        let lock = self.shared.lock_of(&self.id);
        let timing = &self.shared.config.timing;
        lock.acquire(holder, timing.mutex_wait, self.shared.config.mutex_queue).await.map_err(|held| OpError::Busy { holder: held.holder })
    }

    /// Ends one of the device's applet channels by the owner's or Hub's
    /// policy: the device gets `channel.close` `policy`, the daemon its one
    /// close notice with the same reason (§7.5). False when the device is not linked.
    pub fn close_channel(&self, channel: u32) -> bool {
        self.shared.link_tx(&self.id).is_some_and(|tx| tx.send(Input::Msg(ConnMsg::CloseChannel(channel))).is_ok())
    }

    /// `runtime.status` (§6.2): the reply, which also becomes the cached status.
    pub async fn status(&self) -> Result<Value, OpError> {
        let outcome = self.request(json!({"t": "runtime.status"}), "runtime", None, None, None).await?;
        let status = Value::Object(outcome.reply);
        self.shared.set_status(&self.id, status.clone());
        Ok(status)
    }

    /// `runtime.push`: a `.pocket`'s bytes and its footer hash, under the
    /// device mutex. `runtime.install` events go to `progress`.
    pub async fn push(&self, holder: &str, package: Bytes, progress: Option<mpsc::UnboundedSender<Value>>) -> Result<Map<String, Value>, OpError> {
        self.push_counted(holder, package, progress, None).await
    }

    /// `push`, and the count of the package's bytes written to the carrier
    /// after each frame goes to `sent`: a progress bar of the upload itself.
    pub async fn push_counted(
        &self,
        holder: &str,
        package: Bytes,
        progress: Option<mpsc::UnboundedSender<Value>>,
        sent: Option<mpsc::UnboundedSender<u64>>,
    ) -> Result<Map<String, Value>, OpError> {
        let hash = push_hash(&package)?;
        let _guard = self.lock(holder).await?;
        self.push_request(&hash, package, progress, sent).await
    }

    /// Takes the device mutex (§8.3) for an operation that does work of its
    /// own before its request (`store.install` downloads first): a second
    /// caller waits up to the mutex wait and then gets `Busy`. The mutex is
    /// released when the guard drops.
    pub async fn hold(&self, holder: &str) -> Result<LockGuard, OpError> {
        self.lock(holder).await
    }

    /// `push_counted` under a mutex the caller already holds (`hold`): the
    /// guard is borrowed so it outlives the request.
    pub async fn push_held(
        &self,
        _guard: &LockGuard,
        package: Bytes,
        progress: Option<mpsc::UnboundedSender<Value>>,
        sent: Option<mpsc::UnboundedSender<u64>>,
    ) -> Result<Map<String, Value>, OpError> {
        let hash = push_hash(&package)?;
        self.push_request(&hash, package, progress, sent).await
    }

    async fn push_request(&self, hash: &str, package: Bytes, progress: Option<mpsc::UnboundedSender<Value>>, sent: Option<mpsc::UnboundedSender<u64>>) -> Result<Map<String, Value>, OpError> {
        let request = json!({"t": "runtime.push", "bytes": package.len(), "hash": hash});
        Ok(self.request(request, "runtime", Some(package), progress, sent).await?.reply)
    }

    /// `runtime.native.install`: a `.3dsx`, with its CRC-32, under the device
    /// mutex. A `launching` reply is followed by the device's `CLOSE
    /// restart`; the mutex is then held until it links again or 60 s pass.
    pub async fn native_install(&self, holder: &str, name: &str, file: Bytes, launch: bool, progress: Option<mpsc::UnboundedSender<Value>>) -> Result<Map<String, Value>, OpError> {
        self.native_install_counted(holder, name, file, launch, progress, None).await
    }

    /// `native_install`, with the count of bytes written going to `sent` as in `push_counted`.
    pub async fn native_install_counted(
        &self,
        holder: &str,
        name: &str,
        file: Bytes,
        launch: bool,
        progress: Option<mpsc::UnboundedSender<Value>>,
        sent: Option<mpsc::UnboundedSender<u64>>,
    ) -> Result<Map<String, Value>, OpError> {
        check_native(name, &file)?;
        let guard = self.lock(holder).await?;
        self.native_install_held(guard, name, file, launch, progress, sent).await
    }

    /// `native_install_counted` under a mutex the caller took with `hold`
    /// (an update fetches its release first). The guard is taken by value:
    /// after a `launching` reply it becomes the launch hold.
    pub async fn native_install_held(
        &self,
        guard: LockGuard,
        name: &str,
        file: Bytes,
        launch: bool,
        progress: Option<mpsc::UnboundedSender<Value>>,
        sent: Option<mpsc::UnboundedSender<u64>>,
    ) -> Result<Map<String, Value>, OpError> {
        check_native(name, &file)?;
        let crc32 = crc32fast::hash(&file);
        let request = json!({"t": "runtime.native.install", "name": name, "bytes": file.len(), "crc32": crc32, "launch": launch});
        let reply = self.request(request, "runtime.native", Some(file), progress, sent).await?.reply;
        self.after_launch(guard, &reply);
        Ok(reply)
    }

    /// `runtime.launch` of a `.3dsx` or an installed applet, under the mutex.
    pub async fn launch(&self, holder: &str, target: LaunchTarget) -> Result<Map<String, Value>, OpError> {
        let (request, group) = match &target {
            LaunchTarget::Native(name) => {
                if !valid_native_name(name) {
                    return Err(OpError::Invalid { message: "not a valid .3dsx name".into() });
                }
                (json!({"t": "runtime.launch", "native": name}), "runtime.native")
            }
            LaunchTarget::App(output) => {
                if output.is_empty() || pocket_link::text::encoded_bytes(output) > APP_OUTPUT_BYTES {
                    return Err(OpError::Invalid { message: "an output name is 1 to 64 bytes".into() });
                }
                (json!({"t": "runtime.launch", "app": output}), "runtime")
            }
        };
        let guard = self.lock(holder).await?;
        let reply = self.request(request, group, None, None, None).await?.reply;
        self.after_launch(guard, &reply);
        Ok(reply)
    }

    fn after_launch(&self, guard: LockGuard, reply: &Map<String, Value>) {
        if reply.get("phase").and_then(Value::as_str) == Some("launching") {
            // The device exits into another program (§6.2, §8.3).
            guard.hold(self.shared.config.timing.launch_hold);
        }
    }

    /// `runtime.screenshot`: both screens, composed into one PNG. Takes no lock.
    pub async fn screenshot(&self) -> Result<Screenshot, OpError> {
        let outcome = self.request(json!({"t": "runtime.screenshot"}), "runtime", None, None, None).await?;
        let head = outcome.head.ok_or_else(|| OpError::link("transfer", "the device sent no screenshot head"))?;
        let (surfaces, _) = surfaces_of(&head).ok_or_else(|| OpError::link("transfer", "the screenshot head lists no surfaces"))?;
        let frame = outcome.reply.get("frame").or_else(|| head.get("frame")).and_then(Value::as_u64).unwrap_or(0);
        let format = head.get("format").and_then(Value::as_str).unwrap_or("").to_owned();
        let data = outcome.data;
        tokio::task::spawn_blocking(move || compose(frame, &format, surfaces, &data))
            .await
            .map_err(|_| OpError::link("internal", "the PNG task failed"))?
            .map_err(|error| OpError::link("transfer", error.to_string()))
    }

    /// `runtime.logs` on or off; `log` events reach `Endpoint::subscribe`.
    /// Logs are off at every new connection.
    pub async fn logs(&self, on: bool) -> Result<(), OpError> {
        self.request(json!({"t": "runtime.logs", "on": on}), "runtime", None, None, None).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_names_and_footers() {
        assert!(valid_native_name("pocket.3dsx"));
        assert!(!valid_native_name(".a.3dsx"));
        assert!(valid_native_name("a.3dsx"));
        assert!(!valid_native_name("x.3ds"));
        assert!(!valid_native_name("dir/x.3dsx"));
        let mut package = vec![7u8; 40];
        let hash = fnv1a64(&package);
        package.extend_from_slice(&hash.to_le_bytes());
        assert_eq!(package_hash(&package).unwrap(), format!("{hash:016x}"));
        package[0] = 8;
        assert!(package_hash(&package).is_err());
    }
}
