//! A small FTP client for the 3DS's ftpd (github.com/mtheall/ftpd), and the
//! setup's storage over it (docs/POCKET-LINK.md §3.2, §12.1 item 7).
//!
//! What the client depends on in ftpd (`source/ftpSession.cpp`):
//!
//! - The banner is `220 Hello!`. `USER` with any name answers `230` when no
//!   user or password is configured (the default); `PASS` is sent only after
//!   a `331`, since ftpd answers an unexpected `PASS` with `503`.
//! - The session starts in ASCII mode: `TYPE I` is required, or `SIZE`
//!   answers `504` and `RETR` converts line ends.
//! - Passive mode only through `PASV` (no `EPSV`); the 3DS binds the passive
//!   port on the control socket's address from 5001 upward, with a backlog
//!   of one. The client connects to the control connection's peer address
//!   and the port of the `227` reply, before it sends the transfer command.
//! - A missing file: `SIZE` answers `550 <strerror>`, `RETR` answers
//!   **`450 No such file or directory`** (ftpd stats before it opens), `DELE`
//!   `550`, `RNFR` `450`, `NLST` of a missing folder `550`, `MKD` of an
//!   existing folder `550`.
//! - `NLST <dir>` lists full path names, one per CRLF line.
//! - A transfer ends with `226 OK`; an upload is complete when the client
//!   closes the data connection.
//! - An idle session is dropped after 60 s, so an install never keeps a
//!   session open across a download.
//!
//! **Failures map to the three read outcomes exactly**: a `550` (or a `450`
//! whose text says the file is missing) is "absent"; a timeout, a dropped
//! control or data connection, a transfer shorter than `SIZE` announced, or
//! any other answer is "failed". After a timeout or a lost connection the
//! control connection is not trusted again: every later operation fails, so a
//! late reply is never taken for the answer to another command.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::timeout;

use crate::storage::{FileRead, FileStat, SetupStorage, StorageError, Via, check_path};

/// The 3DS's ftpd listens here by default.
pub const FTPD_PORT: u16 = 5000;

/// The largest file `read` takes into memory: pairing files are bytes long,
/// and a program is checked with `stat`, never read back.
pub const MAX_READ: u64 = 1024 * 1024;

const MAX_LINE: u64 = 8 * 1024;
const MAX_REPLY_LINES: usize = 256;
const CHUNK: usize = 32 * 1024;

/// A bound on every step, so that a console that stops answering ends the
/// operation as failed instead of hanging the setup.
#[derive(Debug, Clone, Copy)]
pub struct FtpTimeouts {
    /// The TCP connect of the control connection.
    pub connect: Duration,
    /// One reply on the control connection.
    pub reply: Duration,
    /// The TCP connect of a passive data connection.
    pub data_connect: Duration,
    /// A data connection on which no byte moves for this long has failed.
    pub idle: Duration,
    /// The final reply after an upload: ftpd answers once the file is closed
    /// on the SD card, which is slow (about 145 KB/s, §13.3).
    pub finish: Duration,
}

impl Default for FtpTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(4),
            reply: Duration::from_secs(10),
            data_connect: Duration::from_secs(5),
            idle: Duration::from_secs(15),
            finish: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FtpError {
    #[error("the console did not answer in time ({0})")]
    Timeout(&'static str),
    #[error("the connection to the console was lost: {0}")]
    Io(String),
    /// The command was refused before any transfer started.
    #[error("ftpd answered {code} {text}")]
    Reply { code: u16, text: String },
    /// A transfer that started ended with an error reply.
    #[error("the transfer ended with {code} {text}")]
    Incomplete { code: u16, text: String },
    #[error("ftpd sent something that is not FTP: {0}")]
    Protocol(String),
    #[error("the transfer stopped after {got} of {expected} bytes")]
    Short { got: u64, expected: u64 },
    #[error("the file is larger than {0} bytes")]
    TooLarge(u64),
}

impl FtpError {
    fn io(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
    /// Whether the control connection can still be trusted after this error.
    fn breaks_control(&self) -> bool {
        matches!(self, Self::Timeout(_) | Self::Io(_) | Self::Protocol(_))
    }
}

impl From<FtpError> for StorageError {
    fn from(error: FtpError) -> Self {
        StorageError(error.to_string())
    }
}

/// One reply: its code and its text (the lines joined by `\n`, codes removed
/// from the first line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub code: u16,
    pub text: String,
}

impl Reply {
    fn class(&self) -> u16 {
        self.code / 100
    }
    fn into_error(self) -> FtpError {
        FtpError::Reply { code: self.code, text: self.text }
    }
}

fn reply_code(line: &str) -> Option<u16> {
    let digits = line.get(..3)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|code| (100..600).contains(code))
}

/// Parses one reply from its lines (RFC 959 §4.2): `ddd text`, or `ddd-`
/// followed by lines up to one that starts with the same code and a space.
pub fn parse_reply(lines: &[String]) -> Result<Reply, FtpError> {
    let first = lines.first().ok_or_else(|| FtpError::Protocol("an empty reply".into()))?;
    let code = reply_code(first).ok_or_else(|| FtpError::Protocol(format!("a reply line without a code: {:?}", truncate(first))))?;
    let rest = &first[3..];
    if !rest.starts_with('-') {
        return Ok(Reply { code, text: rest.trim_start_matches(' ').to_owned() });
    }
    let mut text = vec![rest[1..].to_owned()];
    for line in &lines[1..] {
        if reply_code(line) == Some(code) && line[3..].starts_with(' ') {
            text.push(line[4..].to_owned());
            return Ok(Reply { code, text: text.join("\n") });
        }
        text.push(line.clone());
    }
    Err(FtpError::Protocol("a multi-line reply without its last line".into()))
}

fn truncate(text: &str) -> String {
    text.chars().take(60).collect()
}

/// Whether a `450` or `550` text says the file is not there. ftpd sends
/// `strerror(ENOENT)`, "No such file or directory".
fn names_missing(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    ["no such file", "not found", "does not exist", "doesn't exist"].iter().any(|needle| text.contains(needle))
}

/// The passive address of a `227` reply: six numbers, the last two the port.
pub fn parse_pasv(text: &str) -> Option<u16> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let numbers: Vec<u16> = text[start..]
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .take(6)
        .map(|part| part.parse::<u16>().ok().filter(|n| *n <= 255))
        .collect::<Option<_>>()?;
    if numbers.len() != 6 {
        return None;
    }
    let port = numbers[4] * 256 + numbers[5];
    (port != 0).then_some(port)
}

/// The port of a `229` reply: `(|||port|)`.
fn parse_epsv(text: &str) -> Option<u16> {
    let inner = &text[text.find('(')? + 1..text.rfind(')')?];
    let delimiter = inner.chars().next()?;
    let parts: Vec<&str> = inner.split(delimiter).collect();
    parts.get(3)?.parse().ok().filter(|port| *port != 0)
}

/// The control connection.
pub struct FtpClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    peer: SocketAddr,
    local: SocketAddr,
    timeouts: FtpTimeouts,
    broken: Option<String>,
    /// The banner's text, for discovery and logs.
    pub banner: String,
}

impl FtpClient {
    /// Connects, reads the banner, logs in as `anonymous` and selects binary
    /// type.
    pub async fn connect(address: SocketAddr, timeouts: FtpTimeouts) -> Result<Self, FtpError> {
        let stream = match timeout(timeouts.connect, TcpStream::connect(address)).await {
            Err(_) => return Err(FtpError::Timeout("connect")),
            Ok(Err(error)) => return Err(FtpError::io(error)),
            Ok(Ok(stream)) => stream,
        };
        let _ = stream.set_nodelay(true);
        let peer = stream.peer_addr().map_err(FtpError::io)?;
        let local = stream.local_addr().map_err(FtpError::io)?;
        let (read, write) = stream.into_split();
        let mut client = Self { reader: BufReader::new(read), writer: write, peer, local, timeouts, broken: None, banner: String::new() };

        let mut banner = client.reply(timeouts.reply).await?;
        // 120: "service ready in a few minutes", followed by the 220.
        if banner.code == 120 {
            banner = client.reply(timeouts.reply).await?;
        }
        if banner.code != 220 {
            return Err(banner.into_error());
        }
        client.banner = banner.text;

        let user = client.command("USER anonymous").await?;
        match user.code {
            230 => {}
            331 | 332 => {
                let pass = client.command("PASS anonymous@").await?;
                if pass.class() != 2 {
                    return Err(pass.into_error());
                }
            }
            _ => return Err(user.into_error()),
        }
        let binary = client.command("TYPE I").await?;
        if binary.class() != 2 {
            return Err(binary.into_error());
        }
        Ok(client)
    }

    /// This computer's address on the control connection: the address the
    /// console reaches Hub at (§3.2 step 3).
    pub fn local_address(&self) -> IpAddr {
        self.local.ip()
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    fn fail<T>(&mut self, error: FtpError) -> Result<T, FtpError> {
        if error.breaks_control() && self.broken.is_none() {
            self.broken = Some(error.to_string());
        }
        Err(error)
    }

    async fn line(&mut self, wait: Duration) -> Result<String, FtpError> {
        let mut bytes = Vec::new();
        let read = timeout(wait, (&mut self.reader).take(MAX_LINE).read_until(b'\n', &mut bytes)).await;
        match read {
            Err(_) => Err(FtpError::Timeout("reply")),
            Ok(Err(error)) => Err(FtpError::io(error)),
            Ok(Ok(0)) => Err(FtpError::Io("ftpd closed the connection".into())),
            Ok(Ok(_)) if !bytes.ends_with(b"\n") => Err(FtpError::Protocol("a reply line without its end".into())),
            Ok(Ok(_)) => {
                let text = String::from_utf8_lossy(&bytes);
                Ok(text.trim_end_matches(['\r', '\n']).to_owned())
            }
        }
    }

    async fn reply(&mut self, wait: Duration) -> Result<Reply, FtpError> {
        if let Some(reason) = &self.broken {
            return Err(FtpError::Io(reason.clone()));
        }
        let mut lines = Vec::new();
        loop {
            let line = match self.line(wait).await {
                Ok(line) => line,
                Err(error) => return self.fail(error),
            };
            lines.push(line);
            let multi = lines[0].as_bytes().get(3) == Some(&b'-');
            let last = lines.last().unwrap();
            let ends = !multi || (lines.len() > 1 && reply_code(last).is_some() && reply_code(last) == reply_code(&lines[0]) && last.as_bytes().get(3) == Some(&b' '));
            if ends {
                break;
            }
            if lines.len() > MAX_REPLY_LINES {
                return self.fail(FtpError::Protocol("a reply of too many lines".into()));
            }
        }
        match parse_reply(&lines) {
            Ok(reply) => Ok(reply),
            Err(error) => self.fail(error),
        }
    }

    /// Sends one command and reads its reply.
    pub async fn command(&mut self, line: &str) -> Result<Reply, FtpError> {
        if let Some(reason) = &self.broken {
            return Err(FtpError::Io(reason.clone()));
        }
        debug_assert!(!line.contains(['\r', '\n']));
        tracing::trace!(command = line, "ftp");
        let mut bytes = line.as_bytes().to_vec();
        bytes.extend_from_slice(b"\r\n");
        match timeout(self.timeouts.reply, self.writer.write_all(&bytes)).await {
            Err(_) => return self.fail(FtpError::Timeout("command")),
            Ok(Err(error)) => return self.fail(FtpError::io(error)),
            Ok(Ok(())) => {}
        }
        let wait = self.timeouts.reply;
        self.reply(wait).await
    }

    /// Opens a passive data connection: `PASV`, else `EPSV` where a server
    /// has no `PASV`. The address is the control connection's peer, so a
    /// server that names another address in its reply is still reached.
    async fn passive(&mut self) -> Result<TcpStream, FtpError> {
        let reply = self.command("PASV").await?;
        let port = match reply.code {
            227 => parse_pasv(&reply.text).ok_or_else(|| FtpError::Protocol(format!("a PASV reply without an address: {:?}", truncate(&reply.text))))?,
            500 | 502 => {
                let reply = self.command("EPSV").await?;
                if reply.code != 229 {
                    return Err(reply.into_error());
                }
                parse_epsv(&reply.text).ok_or_else(|| FtpError::Protocol("an EPSV reply without a port".into()))?
            }
            _ => return Err(reply.into_error()),
        };
        let address = SocketAddr::new(self.peer.ip(), port);
        match timeout(self.timeouts.data_connect, TcpStream::connect(address)).await {
            Err(_) => self.fail(FtpError::Timeout("data connection")),
            Ok(Err(error)) => self.fail(FtpError::io(error)),
            Ok(Ok(stream)) => Ok(stream),
        }
    }

    /// Starts a transfer: the data connection, then the command and its
    /// preliminary reply. An answer other than 125 or 150 is returned as
    /// `FtpError::Reply` with the control connection still usable.
    async fn start(&mut self, line: &str) -> Result<TcpStream, FtpError> {
        let data = self.passive().await?;
        let reply = self.command(line).await?;
        match reply.code {
            125 | 150 => Ok(data),
            _ if reply.class() == 1 => {
                // Another preliminary reply: the transfer state is unknown.
                self.fail(FtpError::Protocol(format!("an unexpected preliminary reply {}", reply.code)))
            }
            _ => Err(reply.into_error()),
        }
    }

    async fn finish(&mut self, wait: Duration) -> Result<(), FtpError> {
        let reply = self.reply(wait).await?;
        if reply.class() == 2 { Ok(()) } else { Err(FtpError::Incomplete { code: reply.code, text: reply.text }) }
    }

    /// `RETR`, `NLST` or `LIST`: reads the data connection to its end, at most
    /// `max` bytes, then the final reply.
    async fn retrieve(&mut self, line: &str, max: u64) -> Result<Vec<u8>, FtpError> {
        let mut data = self.start(line).await?;
        let mut bytes = Vec::new();
        let mut buffer = vec![0u8; CHUNK];
        loop {
            let read = match timeout(self.timeouts.idle, data.read(&mut buffer)).await {
                Err(_) => return self.fail(FtpError::Timeout("data transfer")),
                Ok(Err(error)) => return self.fail(FtpError::io(error)),
                Ok(Ok(read)) => read,
            };
            if read == 0 {
                break;
            }
            if bytes.len() as u64 + read as u64 > max {
                // The server is still sending: the control connection's next
                // reply is not known, so the connection is not reused.
                return self.fail(FtpError::Protocol(format!("more than {max} bytes")));
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        drop(data);
        let wait = self.timeouts.reply;
        self.finish(wait).await?;
        Ok(bytes)
    }

    /// `STOR`: writes `bytes`, closes the data connection and waits for the
    /// final reply, which ftpd sends once the file is closed.
    async fn store(&mut self, path: &str, bytes: &[u8], progress: &mut (dyn FnMut(u64) + Send)) -> Result<(), FtpError> {
        let mut data = self.start(&format!("STOR {path}")).await?;
        let mut sent = 0u64;
        for chunk in bytes.chunks(CHUNK) {
            match timeout(self.timeouts.idle, data.write_all(chunk)).await {
                Err(_) => return self.fail(FtpError::Timeout("upload")),
                Ok(Err(error)) => return self.fail(FtpError::io(error)),
                Ok(Ok(())) => {}
            }
            sent += chunk.len() as u64;
            progress(sent);
        }
        match timeout(self.timeouts.idle, data.shutdown()).await {
            Err(_) => return self.fail(FtpError::Timeout("upload")),
            Ok(Err(error)) => return self.fail(FtpError::io(error)),
            Ok(Ok(())) => {}
        }
        drop(data);
        let wait = self.timeouts.finish;
        self.finish(wait).await
    }

    /// Sends `QUIT` and closes, without waiting long for the answer.
    pub async fn quit(mut self) {
        if self.broken.is_none() {
            self.timeouts.reply = self.timeouts.reply.min(Duration::from_secs(2));
            let _ = self.command("QUIT").await;
        }
    }
}

/// The setup's storage over ftpd: the card's root is the FTP root.
pub struct FtpStorage {
    client: FtpClient,
}

impl FtpStorage {
    pub async fn connect(address: SocketAddr, timeouts: FtpTimeouts) -> Result<Self, FtpError> {
        let client = FtpClient::connect(address, timeouts).await?;
        tracing::debug!(console = %address, local = %client.local_address(), "ftpd session open");
        Ok(Self { client })
    }

    pub fn local_address(&self) -> IpAddr {
        self.client.local_address()
    }

    pub fn banner(&self) -> &str {
        &self.client.banner
    }

    pub async fn close(self) {
        self.client.quit().await;
    }

    /// `SIZE`: Some for a file, None when the server answered that it is
    /// absent, an error otherwise. `Ok(Err(()))` when `SIZE` is not supported.
    async fn size(&mut self, remote: &str) -> Result<Result<Option<u64>, ()>, FtpError> {
        let reply = self.client.command(&format!("SIZE {remote}")).await?;
        match reply.code {
            213 => Ok(Ok(Some(reply.text.trim().parse().map_err(|_| FtpError::Protocol("a SIZE reply without a number".into()))?))),
            550 => Ok(Ok(None)),
            500 | 501 | 502 | 504 => Ok(Err(())),
            _ => Err(reply.into_error()),
        }
    }

    /// `MDTM`, None when the server has none to give.
    async fn modified(&mut self, remote: &str) -> Result<Option<String>, FtpError> {
        let reply = self.client.command(&format!("MDTM {remote}")).await?;
        Ok(match reply.code {
            213 => Some(reply.text.trim().to_owned()),
            _ if reply.class() >= 4 => None,
            _ => return Err(reply.into_error()),
        })
    }

    async fn read_inner(&mut self, path: &str) -> Result<FileRead, FtpError> {
        let remote = format!("/{path}");
        let size = match self.size(&remote).await? {
            Ok(None) => return Ok(FileRead::Absent),
            Ok(Some(size)) if size > MAX_READ => return Err(FtpError::TooLarge(MAX_READ)),
            Ok(size) => size,
            Err(()) => None,
        };
        match self.client.retrieve(&format!("RETR {remote}"), MAX_READ).await {
            Ok(bytes) => match size {
                Some(expected) if bytes.len() as u64 != expected => Err(FtpError::Short { got: bytes.len() as u64, expected }),
                _ => Ok(FileRead::File(bytes)),
            },
            // ftpd answers a missing file's RETR with 450; RFC 959's 550 is
            // "file unavailable". Both only before any transfer started.
            Err(FtpError::Reply { code: 550, .. }) => Ok(FileRead::Absent),
            Err(FtpError::Reply { code: 450, text }) if names_missing(&text) => Ok(FileRead::Absent),
            Err(error) => Err(error),
        }
    }

    /// Whether `/dir` exists as a folder: `CWD` into it and back to `/`.
    async fn is_folder(&mut self, remote: &str) -> Result<bool, FtpError> {
        let reply = self.client.command(&format!("CWD {remote}")).await?;
        if reply.class() != 2 {
            return if reply.code == 550 || reply.code == 450 || reply.code == 501 { Ok(false) } else { Err(reply.into_error()) };
        }
        let back = self.client.command("CWD /").await?;
        if back.class() != 2 {
            return Err(back.into_error());
        }
        Ok(true)
    }

    async fn list_inner(&mut self, dir: &str) -> Result<Vec<String>, FtpError> {
        let remote = format!("/{dir}");
        let (text, long) = match self.client.retrieve(&format!("NLST {remote}"), MAX_READ).await {
            Ok(bytes) => (bytes, false),
            Err(FtpError::Reply { code: 500 | 502, .. }) => (self.client.retrieve(&format!("LIST {remote}"), MAX_READ).await?, true),
            // A missing folder lists as empty; a folder that is there and
            // did not list is a failed listing.
            Err(FtpError::Reply { code: code @ (450 | 550), text }) => {
                return if self.is_folder(&remote).await? { Err(FtpError::Reply { code, text }) } else { Ok(Vec::new()) };
            }
            Err(error) => return Err(error),
        };
        let text = String::from_utf8_lossy(&text);
        let names = text
            .split('\n')
            .map(|line| line.trim_end_matches('\r'))
            .filter_map(|line| if long { line.split_whitespace().last() } else { Some(line) })
            .map(|line| line.rsplit('/').next().unwrap_or(line))
            .filter(|name| !name.is_empty() && *name != "." && *name != "..")
            .map(str::to_owned)
            .collect();
        Ok(names)
    }

    /// `MKD` of each folder of `path` that is not there yet.
    async fn make_parents(&mut self, path: &str) -> Result<(), FtpError> {
        let Some((parent, _)) = path.rsplit_once('/') else { return Ok(()) };
        let mut prefix = String::new();
        for segment in parent.split('/') {
            prefix.push('/');
            prefix.push_str(segment);
            let reply = self.client.command(&format!("MKD {prefix}")).await?;
            if reply.class() == 2 {
                continue;
            }
            // ftpd answers 550 "File exists" for a folder that is there.
            if !self.is_folder(&prefix).await? {
                return Err(reply.into_error());
            }
        }
        Ok(())
    }

    async fn write_inner(&mut self, path: &str, bytes: &[u8], progress: &mut (dyn FnMut(u64) + Send)) -> Result<(), FtpError> {
        let remote = format!("/{path}");
        match self.client.store(&remote, bytes, progress).await {
            // ftpd's fopen fails with 450 when a parent folder is missing;
            // other servers answer 550 or 553. Make the folders, try once more.
            Err(FtpError::Reply { code: 450 | 550 | 553, .. }) => {
                self.make_parents(path).await?;
                self.client.store(&remote, bytes, progress).await
            }
            other => other,
        }
    }

    async fn delete_inner(&mut self, path: &str) -> Result<(), FtpError> {
        let remote = format!("/{path}");
        let reply = self.client.command(&format!("DELE {remote}")).await?;
        // A refused DELE is what a missing file gets; the read decides.
        if !(reply.class() == 2 || reply.code == 550 || reply.code == 450) {
            return Err(reply.into_error());
        }
        match self.read_inner(path).await? {
            FileRead::Absent => Ok(()),
            FileRead::File(_) => Err(FtpError::Reply { code: reply.code, text: format!("{path} is still there after its delete") }),
            FileRead::Failed(error) => Err(FtpError::Io(error.0)),
        }
    }

    async fn rename_once(&mut self, from: &str, to: &str) -> Result<Result<(), Reply>, FtpError> {
        let reply = self.client.command(&format!("RNFR {from}")).await?;
        if reply.code != 350 {
            return Err(reply.into_error());
        }
        let reply = self.client.command(&format!("RNTO {to}")).await?;
        Ok(if reply.class() == 2 { Ok(()) } else { Err(reply) })
    }

    async fn rename_inner(&mut self, from: &str, to: &str) -> Result<(), FtpError> {
        let (remote_from, remote_to) = (format!("/{from}"), format!("/{to}"));
        let refused = match self.rename_once(&remote_from, &remote_to).await? {
            Ok(()) => return Ok(()),
            Err(reply) => reply,
        };
        // The 3DS's file system does not rename over an existing file (libctru
        // `FSUSER_RenameFile` fails on an existing destination), so ftpd
        // answers 550. Delete the destination, confirmed, and rename again.
        if !matches!(self.size(&remote_to).await?, Ok(Some(_))) {
            return Err(refused.into_error());
        }
        tracing::debug!(to, "rename refused over an existing file; deleting it first");
        self.delete_inner(to).await?;
        match self.rename_once(&remote_from, &remote_to).await? {
            Ok(()) => Ok(()),
            Err(reply) => Err(reply.into_error()),
        }
    }
}

impl SetupStorage for FtpStorage {
    fn via(&self) -> Via {
        Via::Ftp { local_address: self.local_address().to_string() }
    }

    async fn read(&mut self, path: &str) -> FileRead {
        if let Err(error) = check_path(path) {
            return FileRead::Failed(error);
        }
        match self.read_inner(path).await {
            Ok(read) => read,
            Err(error) => {
                tracing::debug!(path, %error, "read failed");
                FileRead::Failed(StorageError(format!("{path}: {error}")))
            }
        }
    }

    async fn list(&mut self, dir: &str) -> Result<Vec<String>, StorageError> {
        check_path(dir)?;
        self.list_inner(dir).await.map_err(|error| StorageError(format!("{dir}/: {error}")))
    }

    async fn stat(&mut self, path: &str) -> Result<Option<FileStat>, StorageError> {
        check_path(path)?;
        let remote = format!("/{path}");
        let wrap = |error: FtpError| StorageError(format!("{path}: {error}"));
        let size = match self.size(&remote).await.map_err(wrap)? {
            Ok(None) => return Ok(None),
            Ok(Some(size)) => size,
            Err(()) => return Err(StorageError(format!("{path}: the server does not answer SIZE"))),
        };
        let modified = self.modified(&remote).await.map_err(wrap)?;
        Ok(Some(FileStat { size, modified }))
    }

    async fn write(&mut self, path: &str, bytes: &[u8], progress: &mut (dyn FnMut(u64) + Send)) -> Result<(), StorageError> {
        check_path(path)?;
        self.write_inner(path, bytes, progress).await.map_err(|error| StorageError(format!("{path} could not be written: {error}")))
    }

    async fn delete(&mut self, path: &str) -> Result<(), StorageError> {
        check_path(path)?;
        self.delete_inner(path).await.map_err(|error| StorageError(format!("{path} could not be deleted: {error}")))
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<(), StorageError> {
        check_path(from)?;
        check_path(to)?;
        self.rename_inner(from, to).await.map_err(|error| StorageError(format!("{from} could not be renamed to {to}: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn replies() {
        assert_eq!(parse_reply(&lines(&["220 Hello!"])).unwrap(), Reply { code: 220, text: "Hello!".into() });
        assert_eq!(parse_reply(&lines(&["230"])).unwrap(), Reply { code: 230, text: "".into() });
        let multi = parse_reply(&lines(&["211-", " MDTM", " 211 not the end", "211 End"])).unwrap();
        assert_eq!(multi.code, 211);
        assert_eq!(multi.text, "\n MDTM\n 211 not the end\nEnd");
        assert!(parse_reply(&lines(&["211-", " MDTM"])).is_err());
        assert!(parse_reply(&lines(&["hello"])).is_err());
        assert!(parse_reply(&lines(&["099 x"])).is_err());
    }

    #[test]
    fn passive_replies() {
        assert_eq!(parse_pasv("Entering Passive Mode (192,168,1,30,19,137)."), Some(5001));
        assert_eq!(parse_pasv("=192,168,1,30,0,21"), Some(21));
        assert_eq!(parse_pasv("(192,168,1,30,0,0)"), None);
        assert_eq!(parse_pasv("(192,168,1,30,300,1)"), None);
        assert_eq!(parse_pasv("(1,2,3)"), None);
        assert_eq!(parse_epsv("Entering Extended Passive Mode (|||6446|)"), Some(6446));
    }

    #[test]
    fn missing_texts() {
        assert!(names_missing("No such file or directory"));
        assert!(!names_missing("Input/output error"));
    }
}
