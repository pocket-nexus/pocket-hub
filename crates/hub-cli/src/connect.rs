//! Reaching the core: connect to `<home>/hubd.sock`, and when nothing
//! answers, start `pocket-hubd` (unless `--no-start`) and wait for it.
//!
//! The core is `$POCKET_HUBD`, else the `pocket-hubd` next to this program.
//! It is started in its own session with no terminal and the CLI's
//! environment; its standard error is read until it answers, so a core that
//! exits first (a port in use, a home it refuses) has its one error line
//! shown here.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pocket_hub_api::types::{ClientInfo, ClientKind};
use pocket_hub_api::{Client, ClientError, paths};

/// How long a started core has to answer.
const START_WAIT: Duration = Duration::from_secs(15);

pub fn client_info() -> ClientInfo {
    ClientInfo { kind: ClientKind::Cli, name: "pocket-hub".into(), version: env!("CARGO_PKG_VERSION").into() }
}

/// Why no core could be reached; the CLI exits with "core unreachable".
pub struct Unreachable(pub String);

pub fn home(explicit: Option<PathBuf>) -> Result<PathBuf, Unreachable> {
    explicit.or_else(paths::home_from_env).ok_or_else(|| Unreachable("Pocket Hub has no home folder: set POCKET_HUB_HOME or HOME.".into()))
}

/// The core program: `$POCKET_HUBD`, else `pocket-hubd` beside this program.
pub fn core_program() -> Result<PathBuf, Unreachable> {
    if let Some(path) = std::env::var_os("POCKET_HUBD").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let me = std::env::current_exe().map_err(|error| Unreachable(format!("This program cannot find where it is: {error}.")))?;
    let beside = me.with_file_name("pocket-hubd");
    if beside.is_file() {
        return Ok(beside);
    }
    Err(Unreachable(format!("pocket-hubd is not next to this program ({}); set POCKET_HUBD to its path.", beside.display())))
}

pub async fn connect(home: &Path, start: bool) -> Result<Client, Unreachable> {
    let socket = paths::checked_socket_path(home).map_err(Unreachable)?;
    match Client::connect(&socket, client_info()).await {
        Ok(client) => return Ok(client),
        Err(ClientError::Unreachable { .. }) if start => {}
        Err(ClientError::Unreachable { .. }) => {
            return Err(Unreachable(format!("No Pocket Hub core answers on {}. Start one with pocket-hubd, or run this command without --no-start.", socket.display())));
        }
        Err(error) => return Err(Unreachable(format!("The Pocket Hub core on {} did not accept this client: {error}.", socket.display()))),
    }
    start_core(home)?;
    let deadline = Instant::now() + START_WAIT;
    loop {
        if let Ok(client) = Client::connect(&socket, client_info()).await {
            return Ok(client);
        }
        if Instant::now() >= deadline {
            return Err(Unreachable(format!("pocket-hubd was started but did not answer on {} within {} s.", socket.display(), START_WAIT.as_secs())));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Starts the core, detached, and returns once it answers or has exited.
fn start_core(home: &Path) -> Result<(), Unreachable> {
    let program = core_program()?;
    let mut command = Command::new(&program);
    command.arg("--home").arg(home).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    // SAFETY: setsid is async-signal-safe and touches no memory of the parent.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| Unreachable(format!("{} could not be started: {error}.", program.display())))?;
    let socket = paths::socket_path(home);
    let deadline = Instant::now() + START_WAIT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            if status.success() {
                // Another core won the start, or this one ran and ended: the
                // caller connects to whichever answers.
                return Ok(());
            }
            let mut text = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut text);
            }
            // The core's own error line, else the first thing it said (an
            // argument it refused, in clap's words).
            let line = text
                .lines()
                .find(|line| line.starts_with("pocket-hubd:"))
                .or_else(|| text.lines().find(|line| !line.trim().is_empty()))
                .unwrap_or("pocket-hubd exited without saying why.");
            return Err(Unreachable(line.trim().to_owned()));
        }
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() || Instant::now() >= deadline {
            // The core runs on its own from here; its standard error closes
            // with this process, and it logs to `<home>/logs/hubd.log`.
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
}
