//! Test support: an in-process FTP server that behaves like the 3DS's ftpd
//! (mtheall/ftpd `source/ftpSession.cpp`) over a temporary folder, with
//! fault injection; a minimal HTTP server; helpers for the store and the card.
//!
//! Built with the `test-support` feature only, for this crate's tests and
//! for the end-to-end tests of the crates that drive a setup. Its helpers
//! panic on failures: they are test code.

#![allow(clippy::unwrap_used, clippy::new_without_default)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use crate::{FtpStorage, FtpTimeouts};
use pocket_hub_store::Store;
use pocket_link::pairing::{DirectoryEntry, LoadedCompanion, load_companions};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};

pub const HUB_NAME: &str = "Test Hub";
pub const HUB_PORT: u16 = 8628;

// ---------------------------------------------------------------------------
// Faults
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Then {
    /// Close the control connection.
    Kill,
    /// Send this reply line.
    Reply(&'static str),
}

#[derive(Debug, Clone)]
pub enum Action {
    /// Close the control connection without running the command.
    Kill,
    /// Never answer.
    Stall,
    /// Answer this line instead of running the command.
    Reply(&'static str),
    /// RETR/STOR: move `after` bytes on the data connection, close it, then `then`.
    DropData { after: usize, then: Then },
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub verb: &'static str,
    /// The command's argument ends with this.
    pub suffix: String,
    /// Matching commands to let through before the rule fires.
    pub skip: usize,
    pub action: Action,
}

impl Rule {
    pub fn new(verb: &'static str, suffix: impl Into<String>, action: Action) -> Self {
        Self { verb, suffix: suffix.into(), skip: 0, action }
    }
    pub fn skip(mut self, skip: usize) -> Self {
        self.skip = skip;
        self
    }
}

#[derive(Default)]
struct State {
    log: Vec<String>,
    rules: Vec<Rule>,
    commands: usize,
    kill_after: Option<usize>,
    /// ftpd on the 3DS cannot rename over an existing file (libctru's
    /// FSUSER_RenameFile fails); false emulates that.
    rename_replaces: bool,
}

// ---------------------------------------------------------------------------
// The fake ftpd
// ---------------------------------------------------------------------------

pub struct FakeFtp {
    pub address: SocketAddr,
    pub root: PathBuf,
    state: Arc<Mutex<State>>,
}

impl FakeFtp {
    pub async fn start(root: &Path) -> Self {
        std::fs::create_dir_all(root).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State::default()));
        let (served_root, served_state) = (root.to_owned(), state.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(session(stream, served_root.clone(), served_state.clone()));
            }
        });
        Self { address, root: root.to_owned(), state }
    }

    pub fn rule(&self, rule: Rule) {
        self.state.lock().unwrap().rules.push(rule);
    }
    pub fn clear_faults(&self) {
        let mut state = self.state.lock().unwrap();
        state.rules.clear();
        state.kill_after = None;
    }
    /// Close the control connection at the `n`th command from now (1-based).
    pub fn kill_after(&self, n: usize) {
        let mut state = self.state.lock().unwrap();
        state.commands = 0;
        state.kill_after = Some(n);
    }
    pub fn set_rename_replaces(&self, replaces: bool) {
        self.state.lock().unwrap().rename_replaces = replaces;
    }
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.state.lock().unwrap().log)
    }
    pub fn rules_left(&self) -> usize {
        self.state.lock().unwrap().rules.len()
    }
}

/// The commands of a log that change the card.
pub fn writes(log: &[String]) -> Vec<String> {
    log.iter().filter(|line| ["STOR", "DELE", "RNFR", "RNTO", "MKD", "APPE", "RMD"].iter().any(|verb| line.starts_with(verb))).cloned().collect()
}

async fn send(writer: &mut OwnedWriteHalf, line: &str) -> bool {
    writer.write_all(format!("{line}\r\n").as_bytes()).await.is_ok()
}

fn resolve(root: &Path, argument: &str) -> Option<PathBuf> {
    let relative = argument.trim_start_matches('/');
    let path = Path::new(relative);
    if path.components().any(|component| !matches!(component, Component::Normal(_))) {
        return None;
    }
    Some(root.join(path))
}

async fn accept(listener: &mut Option<TcpListener>) -> Option<TcpStream> {
    let listener = listener.take()?;
    tokio::time::timeout(Duration::from_secs(5), listener.accept()).await.ok()?.ok().map(|(stream, _)| stream)
}

fn modified(path: &Path) -> String {
    std::fs::metadata(path).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
}

async fn session(stream: TcpStream, root: PathBuf, state: Arc<Mutex<State>>) {
    let (read, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read);
    if !send(&mut writer, "220 Hello!").await {
        return;
    }
    let mut ascii = true;
    let mut pasv: Option<TcpListener> = None;
    let mut rename_from: Option<PathBuf> = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\r', '\n']).to_owned();
        let (verb, argument) = match line.split_once(' ') {
            Some((verb, argument)) => (verb.to_ascii_uppercase(), argument.to_owned()),
            None => (line.to_ascii_uppercase(), String::new()),
        };
        let action = {
            let mut state = state.lock().unwrap();
            state.log.push(line.clone());
            state.commands += 1;
            if state.kill_after.is_some_and(|limit| state.commands >= limit) {
                state.kill_after = None;
                Some(Action::Kill)
            } else {
                let mut fired = None;
                for (at, rule) in state.rules.iter_mut().enumerate() {
                    if rule.verb == verb && argument.ends_with(&rule.suffix) {
                        if rule.skip > 0 {
                            rule.skip -= 1;
                            continue;
                        }
                        fired = Some(at);
                        break;
                    }
                }
                fired.map(|at| state.rules.remove(at).action)
            }
        };
        let mut drop_data = None;
        match action {
            Some(Action::Kill) => return,
            Some(Action::Stall) => {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                return;
            }
            Some(Action::Reply(reply)) => {
                send(&mut writer, reply).await;
                continue;
            }
            Some(Action::DropData { after, then }) => drop_data = Some((after, then)),
            None => {}
        }
        let rename_replaces = state.lock().unwrap().rename_replaces;
        let path = resolve(&root, &argument);
        let ok = match verb.as_str() {
            "USER" => send(&mut writer, "230 OK").await,
            "PASS" => send(&mut writer, "503 Send USER before PASS").await,
            "TYPE" => {
                match argument.as_str() {
                    "I" => ascii = false,
                    "A" | "A N" => ascii = true,
                    _ => {
                        send(&mut writer, "504 Unavailable").await;
                        continue;
                    }
                }
                send(&mut writer, "200 OK").await
            }
            "NOOP" => send(&mut writer, "200 OK").await,
            "SYST" => send(&mut writer, "215 UNIX Type: L8").await,
            "QUIT" => {
                send(&mut writer, "221 Disconnecting").await;
                return;
            }
            "PASV" => {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let port = listener.local_addr().unwrap().port();
                pasv = Some(listener);
                send(&mut writer, &format!("227 Entering Passive Mode (127,0,0,1,{},{}).", port >> 8, port & 0xff)).await
            }
            "CWD" => match &path {
                _ if argument == "/" => send(&mut writer, "200 OK").await,
                Some(path) if path.is_dir() => send(&mut writer, "200 OK").await,
                Some(path) if path.exists() => send(&mut writer, "550 Not a directory").await,
                _ => send(&mut writer, "550 No such file or directory").await,
            },
            "SIZE" => match &path {
                _ if ascii => send(&mut writer, "504 SIZE unavailable in ASCII mode").await,
                Some(path) if path.is_file() => send(&mut writer, &format!("213 {}", std::fs::metadata(path).unwrap().len())).await,
                Some(path) if path.exists() => send(&mut writer, "550 Not a file").await,
                _ => send(&mut writer, "550 No such file or directory").await,
            },
            "MDTM" => match &path {
                Some(path) if path.is_file() => send(&mut writer, &format!("213 {}", modified(path))).await,
                _ => send(&mut writer, "550 No such file or directory").await,
            },
            "MKD" => match &path {
                Some(path) => match std::fs::create_dir(path) {
                    Ok(()) => send(&mut writer, &format!("257 \"{argument}\" directory created")).await,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => send(&mut writer, "550 File exists").await,
                    Err(_) => send(&mut writer, "550 No such file or directory").await,
                },
                None => send(&mut writer, "553 Invalid argument").await,
            },
            "DELE" => match &path {
                Some(path) if path.is_file() => {
                    std::fs::remove_file(path).unwrap();
                    send(&mut writer, "250 OK").await
                }
                _ => send(&mut writer, "550 No such file or directory").await,
            },
            "RNFR" => match &path {
                Some(path) if path.exists() => {
                    rename_from = Some(path.clone());
                    send(&mut writer, "350 OK").await
                }
                _ => send(&mut writer, "450 No such file or directory").await,
            },
            "RNTO" => match (rename_from.take(), &path) {
                (None, _) => send(&mut writer, "503 Bad sequence of commands").await,
                (Some(_), None) => send(&mut writer, "554 Invalid argument").await,
                (Some(_), Some(to)) if to.exists() && !rename_replaces => send(&mut writer, "550 File exists").await,
                (Some(from), Some(to)) => match std::fs::rename(&from, to) {
                    Ok(()) => send(&mut writer, "250 OK").await,
                    Err(_) => send(&mut writer, "550 No such file or directory").await,
                },
            },
            "RETR" => {
                // ftpd stats the file before the transfer and answers 450.
                let Some(path) = path.filter(|path| path.is_file()) else {
                    send(&mut writer, "450 No such file or directory").await;
                    continue;
                };
                let bytes = std::fs::read(&path).unwrap();
                let Some(mut data) = accept(&mut pasv).await else {
                    send(&mut writer, "425 Failed to establish connection").await;
                    continue;
                };
                send(&mut writer, "150 Ready").await;
                match drop_data {
                    Some((after, then)) => {
                        let _ = data.write_all(&bytes[..after.min(bytes.len())]).await;
                        drop(data);
                        match then {
                            Then::Kill => return,
                            Then::Reply(reply) => send(&mut writer, reply).await,
                        }
                    }
                    None => {
                        let _ = data.write_all(&bytes).await;
                        let _ = data.shutdown().await;
                        drop(data);
                        send(&mut writer, "226 OK").await
                    }
                }
            }
            "STOR" => {
                let Some(path) = path.filter(|path| path.parent().is_some_and(Path::is_dir)) else {
                    send(&mut writer, "450 No such file or directory").await;
                    continue;
                };
                let Some(mut data) = accept(&mut pasv).await else {
                    send(&mut writer, "425 Failed to establish connection").await;
                    continue;
                };
                send(&mut writer, "150 Ready").await;
                let mut bytes = Vec::new();
                match drop_data {
                    Some((after, then)) => {
                        let mut buffer = vec![0u8; after];
                        let mut got = 0;
                        while got < after {
                            match data.read(&mut buffer[got..]).await {
                                Ok(0) | Err(_) => break,
                                Ok(read) => got += read,
                            }
                        }
                        std::fs::write(&path, &buffer[..got]).unwrap();
                        drop(data);
                        match then {
                            Then::Kill => return,
                            Then::Reply(reply) => send(&mut writer, reply).await,
                        }
                    }
                    None => {
                        if data.read_to_end(&mut bytes).await.is_err() {
                            send(&mut writer, "451 Connection reset").await;
                            continue;
                        }
                        std::fs::write(&path, &bytes).unwrap();
                        send(&mut writer, "226 OK").await
                    }
                }
            }
            "NLST" | "LIST" => {
                let Some(path) = path.filter(|path| path.is_dir()) else {
                    send(&mut writer, "550 No such file or directory").await;
                    continue;
                };
                let Some(mut data) = accept(&mut pasv).await else {
                    send(&mut writer, "425 Failed to establish connection").await;
                    continue;
                };
                send(&mut writer, "150 Ready").await;
                let mut text = String::new();
                for entry in std::fs::read_dir(&path).unwrap() {
                    let entry = entry.unwrap();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if verb == "NLST" {
                        // ftpd's NLST gives whole path names.
                        text.push_str(&format!("{}/{name}\r\n", argument.trim_end_matches('/')));
                    } else {
                        text.push_str(&format!("-rw-rw-rw- 1 3DS 3DS {} Jan 1 00:00 {name}\r\n", entry.metadata().unwrap().len()));
                    }
                }
                if let Some((after, then)) = drop_data {
                    let _ = data.write_all(&text.as_bytes()[..after.min(text.len())]).await;
                    drop(data);
                    match then {
                        Then::Kill => return,
                        Then::Reply(reply) => send(&mut writer, reply).await,
                    }
                } else {
                    let _ = data.write_all(text.as_bytes()).await;
                    drop(data);
                    send(&mut writer, "226 OK").await
                }
            }
            _ => send(&mut writer, "502 Command not implemented").await,
        };
        if !ok {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// A minimal HTTP server: GET of fixed bodies, one request per connection.
// ---------------------------------------------------------------------------

pub struct FakeHttp {
    pub base: String,
    hits: Arc<Mutex<BTreeMap<String, usize>>>,
    routes: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    /// When set, every request is answered with this status and no body.
    failing: Arc<Mutex<Option<u16>>>,
}

impl FakeHttp {
    /// `routes` maps a path to its body; `{base}` in a body is replaced by the
    /// server's base URL.
    pub async fn start(routes: Vec<(String, Vec<u8>)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = Self { base, hits: Arc::default(), routes: Arc::default(), failing: Arc::default() };
        for (path, body) in routes {
            server.set(&path, body);
        }
        let (routes, hits, failing) = (server.routes.clone(), server.hits.clone(), server.failing.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (routes, hits, failing) = (routes.clone(), hits.clone(), failing.clone());
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut reader = BufReader::new(read);
                    let mut request = String::new();
                    if reader.read_line(&mut request).await.is_err() {
                        return;
                    }
                    loop {
                        let mut header = String::new();
                        match reader.read_line(&mut header).await {
                            Ok(0) | Err(_) => break,
                            Ok(_) if header == "\r\n" || header == "\n" => break,
                            Ok(_) => {}
                        }
                    }
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    *hits.lock().unwrap().entry(path.clone()).or_default() += 1;
                    let failing = *failing.lock().unwrap();
                    let body = routes.lock().unwrap().get(&path).cloned();
                    let response = match (failing, body) {
                        (Some(status), _) => format!("HTTP/1.1 {status} Failing\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes(),
                        (None, Some(body)) => {
                            let mut response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                            response.extend_from_slice(&body);
                            response
                        }
                        (None, None) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = write.write_all(&response).await;
                    let _ = write.shutdown().await;
                });
            }
        });
        server
    }

    /// Serves `body` at `path` from now on, `{base}` replaced as in `start`.
    pub fn set(&self, path: &str, body: Vec<u8>) {
        let body = match String::from_utf8(body.clone()) {
            Ok(text) if text.contains("{base}") => text.replace("{base}", &self.base).into_bytes(),
            _ => body,
        };
        self.routes.lock().unwrap().insert(path.to_owned(), body);
    }

    /// Answers every request with `status` (Some), or serves the routes again (None).
    pub fn fail(&self, status: Option<u16>) {
        *self.failing.lock().unwrap() = status;
    }

    pub fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

/// A Pocket Shell release as GitHub's API describes it, for `FakeHttp`: the
/// release at `/repos/<repo>/releases/tags/<tag>` (and at `.../latest` when
/// `latest`), its `.3dsx` named `pocket-shell-<tag>.3dsx`, `SHA256SUMS` and
/// `build-info.json` naming `version`. Returns the routes.
pub fn github_release(repo: &str, tag: &str, version: &str, program: &[u8], latest: bool) -> Vec<(String, Vec<u8>)> {
    let name = format!("pocket-shell-{tag}.3dsx");
    let sha = crate::release::sha256_hex(program);
    let sums = format!("{sha}  {name}\n");
    let info = format!(r#"{{"product":"pocket-shell","version":"{version}","source":"aaaa","runtime":"bbbb","files":[{{"name":"{name}","sha256":"{sha}"}}]}}"#);
    let release = format!(
        r#"{{"tag_name":"{tag}","assets":[
            {{"name":"build-info.json","size":{},"browser_download_url":"{{base}}/dl/{tag}/build-info.json"}},
            {{"name":"{name}","size":{},"browser_download_url":"{{base}}/dl/{tag}/{name}"}},
            {{"name":"SHA256SUMS","size":{},"browser_download_url":"{{base}}/dl/{tag}/SHA256SUMS"}}]}}"#,
        info.len(),
        program.len(),
        sums.len()
    );
    let mut routes = vec![
        (format!("/repos/{repo}/releases/tags/{tag}"), release.clone().into_bytes()),
        (format!("/dl/{tag}/{name}"), program.to_vec()),
        (format!("/dl/{tag}/SHA256SUMS"), sums.into_bytes()),
        (format!("/dl/{tag}/build-info.json"), info.into_bytes()),
    ];
    if latest {
        routes.push((format!("/repos/{repo}/releases/latest"), release.into_bytes()));
    }
    routes
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pocket-hub-setup-{tag}-{}", pocket_link::text::new_link_id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn open_store() -> Store {
    let store = Store::open(temp("store")).unwrap();
    store.set_hub_name(HUB_NAME).unwrap();
    store
}

pub fn timeouts() -> FtpTimeouts {
    FtpTimeouts { connect: Duration::from_secs(2), reply: Duration::from_secs(5), data_connect: Duration::from_secs(2), idle: Duration::from_secs(5), finish: Duration::from_secs(5) }
}

/// Timeouts for the stall cases: a step that hangs fails in a fraction of a second.
pub fn short_timeouts() -> FtpTimeouts {
    FtpTimeouts { connect: Duration::from_millis(500), reply: Duration::from_millis(300), data_connect: Duration::from_millis(500), idle: Duration::from_millis(300), finish: Duration::from_millis(300) }
}

pub async fn connect(fake: &FakeFtp) -> FtpStorage {
    FtpStorage::connect(fake.address, timeouts()).await.unwrap()
}

/// Every file under `root`, by its path relative to `root`.
pub fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, into);
            } else {
                into.insert(path.strip_prefix(root).unwrap().to_string_lossy().into_owned(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

pub fn companions(root: &Path) -> PathBuf {
    root.join("pocketjs/link/companions")
}

pub fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// What the device would load from the card (§3.6), in directory order.
pub fn device_load(root: &Path) -> Vec<LoadedCompanion> {
    let mut entries = Vec::new();
    if let Ok(read) = std::fs::read_dir(companions(root)) {
        for entry in read {
            let entry = entry.unwrap();
            entries.push(DirectoryEntry { name: entry.file_name().to_string_lossy().into_owned(), text: std::fs::read_to_string(entry.path()).unwrap_or_default() });
        }
    }
    let (loaded, error) = load_companions(&entries);
    assert_eq!(error, None);
    loaded
}

/// A program with the 3DSX magic and `size` deterministic bytes.
pub fn fake_3dsx(size: usize, seed: u8) -> Vec<u8> {
    let mut bytes = b"3DSX".to_vec();
    bytes.extend((4..size).map(|at| (at as u8).wrapping_mul(31).wrapping_add(seed)));
    bytes
}

pub fn write(path: &Path, text: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
