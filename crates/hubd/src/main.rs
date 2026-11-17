//! `pocket-hubd`: Pocket Hub's resident core.
//!
//! With no argument it is the core for the home of `$POCKET_HUB_HOME` (or the
//! platform's default) and stays in the foreground of whatever started it: a
//! client that starts it detaches it. It ends at `SIGINT`, `SIGTERM` or
//! `hub.shutdown`. A second start for a home whose core runs exits 0 at once.
//!
//! Exit status: 0 after a shutdown, or when a core already runs for the home;
//! 1 when the core could not start, after one line on standard error saying
//! why (also written to `<home>/logs/hubd.log` when the home is usable).

use std::io::Write;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;
use pocket_hub_setup::ShellSource;
use pocket_hubd::{Options, StartError, logging};

#[derive(Parser, Debug)]
#[command(name = "pocket-hubd", version, about = "Pocket Hub's resident core: devices, services and the control socket.", long_about = None)]
struct Args {
    /// Run in the foreground. This is what pocket-hubd always does; the flag
    /// is accepted for scripts and development sessions.
    #[arg(long)]
    foreground: bool,

    /// Hub's home folder (default: $POCKET_HUB_HOME, else ~/Library/Application Support/nexus.pocket.hub on macOS).
    #[arg(long, value_name = "DIR")]
    home: Option<PathBuf>,

    /// The TCP link port; 0 for an ephemeral one. A port in use stops the start.
    #[arg(long, value_name = "PORT", env = "POCKET_HUB_LINK_PORT")]
    link_port: Option<u16>,

    /// Send no beacons (for development and tests).
    #[arg(long, env = "POCKET_HUB_NO_BEACONS", value_parser = clap::builder::BoolishValueParser::new())]
    no_beacons: bool,

    /// The Hub's name when its home is new (default: this computer's name). A home that has its identity keeps its name.
    #[arg(long, value_name = "NAME", env = "POCKET_HUB_NAME")]
    name: Option<String>,

    /// Run services on this Bun instead of the one Hub downloads.
    #[arg(long, value_name = "PATH", env = "POCKET_HUB_BUN")]
    bun: Option<PathBuf>,

    /// Use this local .3dsx as Pocket Shell's "latest" release.
    #[arg(long, value_name = "FILE", env = "POCKET_HUB_SHELL_FILE", conflicts_with = "shell_api")]
    shell_file: Option<PathBuf>,

    /// Look Pocket Shell's releases up at this GitHub API base URL (a mirror or a test server).
    #[arg(long, value_name = "URL", env = "POCKET_HUB_SHELL_API")]
    shell_api: Option<String>,

    /// The GitHub repository of Pocket Shell's releases, owner/name.
    #[arg(long, value_name = "REPO", env = "POCKET_HUB_SHELL_REPO")]
    shell_repo: Option<String>,

    /// The shortest time between two looks for the newest Pocket Shell
    /// release, in milliseconds (default 6 hours; development and tests).
    #[arg(long, value_name = "MS", hide = true)]
    program_look_ms: Option<u64>,

    /// Replace every service's idleSeconds (development and tests).
    #[arg(long, value_name = "SECONDS", hide = true)]
    service_idle_seconds: Option<f64>,

    /// How long a second caller waits for a device's mutex, in milliseconds.
    #[arg(long, value_name = "MS", hide = true, default_value_t = 10_000)]
    mutex_wait_ms: u64,

    /// The link's silence limit in milliseconds (development and tests).
    #[arg(long, value_name = "MS", hide = true)]
    link_silence_ms: Option<u64>,

    /// The silence limit while a state-changing request is open, in milliseconds (development and tests).
    #[arg(long, value_name = "MS", hide = true)]
    link_state_silence_ms: Option<u64>,

    /// Hub's PING interval while a state-changing request is open, in milliseconds (development and tests).
    #[arg(long, value_name = "MS", hide = true)]
    link_state_ping_ms: Option<u64>,

    /// setup.discover probes these IPv4 addresses (comma separated) instead of the local subnets.
    #[arg(long, value_name = "IPS", hide = true, value_delimiter = ',')]
    discover_targets: Option<Vec<Ipv4Addr>>,

    /// setup.discover probes this port instead of 5000.
    #[arg(long, value_name = "PORT", hide = true)]
    discover_port: Option<u16>,

    /// Send a device's net.http requests for HOST to ADDRESS:PORT, a private
    /// or loopback address allowed for that host alone (development and
    /// tests). Repeatable.
    #[arg(long = "net-resolve", value_name = "HOST=ADDRESS:PORT", hide = true)]
    net_resolve: Vec<pocket_hub_net::ResolveOverride>,

    /// The applet store's base URL.
    #[arg(long, value_name = "URL", env = "POCKET_HUB_STORE_URL", default_value = pocket_hub_applet_store::DEFAULT_URL)]
    store_url: String,

    /// The applet store's official publisher key, base64url (a test store's own).
    #[arg(long, value_name = "KEY", env = "POCKET_HUB_STORE_KEY", hide = true, default_value = pocket_hub_applet_store::OFFICIAL_KEY)]
    store_key: String,
}

/// The one line of a failed start: to standard error as a bare sentence,
/// and through the log to `hubd.log` when logging runs.
fn fatal(message: &str) -> ExitCode {
    tracing::error!(target: logging::FATAL_TARGET, "{message}");
    let _ = writeln!(std::io::stderr(), "pocket-hubd: {message}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    // Nothing of whatever started this core (the command line, the app)
    // stays open in it for its life (`service_host::child`).
    service_host::child::close_inherited();
    let args = Args::parse();
    let Some(home) = args.home.clone().or_else(pocket_hub_api::paths::home_from_env) else {
        return fatal("Pocket Hub has no home folder: set POCKET_HUB_HOME or HOME.");
    };
    let home = if home.is_absolute() { home } else { std::env::current_dir().map(|dir| dir.join(&home)).unwrap_or(home) };
    if let Err(message) = pocket_hub_api::paths::checked_socket_path(&home) {
        return fatal(&message);
    }
    if let Err(error) = std::fs::create_dir_all(&home) {
        return fatal(&format!("Pocket Hub cannot make its home folder {}: {error}.", home.display()));
    }
    if let Err(error) = logging::init(&home, true) {
        return fatal(&format!("Pocket Hub cannot open its log {}: {error}.", logging::log_path(&home).display()));
    }

    let mut options = Options::new(&home);
    options.link_port = args.link_port;
    options.beacons = !args.no_beacons;
    options.bun = args.bun;
    options.name = match args.name.as_deref().map(str::trim) {
        Some(name) if name.is_empty() || pocket_link::text::encoded_bytes(name) > pocket_link::consts::NAME_BYTES => {
            return fatal(&format!("A Hub's name is 1 to {} bytes.", pocket_link::consts::NAME_BYTES));
        }
        name => name.map(str::to_owned),
    };
    options.shell_source = match (args.shell_file, args.shell_api, args.shell_repo) {
        (Some(path), _, _) => Some(ShellSource::File { path }),
        (None, None, None) => None,
        (None, api, repo) => {
            let ShellSource::GitHub { api: default_api, repo: default_repo, .. } = ShellSource::default() else { unreachable!("the default is GitHub") };
            Some(ShellSource::GitHub { api: api.unwrap_or(default_api), repo: repo.unwrap_or(default_repo), tag: None })
        }
    };
    if let Some(ms) = args.program_look_ms {
        options.program_look_interval = Duration::from_millis(ms);
    }
    options.service_idle = args.service_idle_seconds.map(Duration::from_secs_f64);
    options.mutex_wait = Duration::from_millis(args.mutex_wait_ms);
    for (value, field) in [(args.link_silence_ms, &mut options.link_silence), (args.link_state_silence_ms, &mut options.link_state_silence), (args.link_state_ping_ms, &mut options.link_state_ping)] {
        if let Some(ms) = value {
            *field = Duration::from_millis(ms);
        }
    }
    options.discover_targets = args.discover_targets;
    options.discover_port = args.discover_port;
    options.net_overrides = args.net_resolve;
    options.store_url = args.store_url;
    options.store_key = args.store_key;

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return fatal(&format!("Pocket Hub cannot start its runtime: {error}.")),
    };
    runtime.block_on(async move {
        let running = match pocket_hubd::start(options).await {
            Ok(running) => running,
            Err(StartError::AlreadyRunning(home)) => {
                tracing::info!(home = %home.display(), "a core already runs for this home; this one exits");
                return ExitCode::SUCCESS;
            }
            Err(error) => return fatal(&error.to_string()),
        };
        running.run(signals()).await;
        ExitCode::SUCCESS
    })
}

/// Resolves at the first `SIGINT` or `SIGTERM`.
async fn signals() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut interrupt), Ok(mut terminate)) = (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) else {
        tracing::warn!("signals cannot be caught; only hub.shutdown stops this core");
        return std::future::pending().await;
    };
    tokio::select! {
        _ = interrupt.recv() => tracing::info!("SIGINT"),
        _ = terminate.recv() => tracing::info!("SIGTERM"),
    }
}
