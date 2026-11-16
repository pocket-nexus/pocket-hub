//! `pocket-hub`: Pocket Hub's command line. It is a client of the core's
//! control socket (docs/CONTROL-API.md) and nothing else: every command is
//! one or a few calls of that API.

mod connect;
mod output;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use pocket_hub_api::types::*;
use pocket_hub_api::{Client, ClientError, RpcError, methods, topics};
use serde_json::{Value, json};

use crate::output::Out;

/// Exit statuses.
pub mod status {
    pub const OK: u8 = 0;
    /// The core answered with an error, or the operation could not be done.
    pub const FAILED: u8 = 1;
    /// The command line itself is wrong (clap's own status).
    pub const USAGE: u8 = 2;
    /// No core answers and none could be started.
    pub const UNREACHABLE: u8 = 3;
}

/// How long `shutdown` waits for the core to be gone: its links' close
/// drain and its daemons' stop (5 s, then SIGTERM, 5 s, then SIGKILL) fit.
const SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

const AFTER_HELP: &str = "\
Exit status: 0 success, 1 the operation failed, 2 a usage error, 3 no Pocket Hub core could be reached.

The core (pocket-hubd) is started when none answers: $POCKET_HUBD, else the pocket-hubd next to this program.
Its home is --home, else $POCKET_HUB_HOME, else ~/Library/Application Support/nexus.pocket.hub on macOS.";

#[derive(Parser, Debug)]
#[command(name = "pocket-hub", version, about = "Manage Pocket Hub's devices and daemons from a terminal.", after_help = AFTER_HELP)]
struct Cli {
    /// Print the raw result objects of the control API as JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Do not start pocket-hubd when no core answers.
    #[arg(long, global = true)]
    no_start: bool,
    /// Hub's home folder.
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// The core: its name, home, link port and Bun.
    Status,
    /// The paired devices.
    Devices,
    /// One device: `device [<id>] <command>`. The id may be left out when exactly one device is paired.
    #[command(after_help = "Commands: status, apps, push <file.pocket>, launch <app>, remove <app>, shot [out.png], logs [-f], update [--program <file.3dsx> | --release <tag>] [--no-launch], forget.\nExample: pocket-hub device 0123456789abcdef push build/demo.pocket")]
    Device {
        /// [<id>] <command> [arguments]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true, value_name = "ARGS")]
        args: Vec<String>,
    },
    /// Set a device up for the first time: its software and this computer's pairing.
    Setup {
        #[command(subcommand)]
        kind: SetupKind,
    },
    /// Pair a device that shows a code (an M5Stack Tab5): list the devices that ask, or answer one with its code.
    #[command(after_help = "On the device, open its pairing page and choose this computer; the device shows a code of 6 digits, good for 5 minutes.\nExamples:\n  pocket-hub pair                 # the devices that ask to be paired\n  pocket-hub pair --code 314159   # the code the device shows\n  pocket-hub pair --decline       # dismiss the request")]
    Pair(PairArgs),
    /// The installed daemons.
    Daemons,
    /// One daemon.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// The applet store: what it offers (for a device, with what it holds), and installing from it.
    #[command(after_help = "Examples:\n  pocket-hub store --device 0123456789abcdef\n  pocket-hub store install dev.pocket-nexus.hn")]
    Store(StoreArgs),
    /// The Bun runtime daemons run on.
    Runtime {
        /// Download and install Bun now.
        #[arg(long)]
        install: bool,
    },
    /// Print the core's events as they happen (Ctrl-C ends).
    Watch {
        /// Only these topics (device.changed, device.log, daemon.changed, …).
        #[arg(long = "topic", value_name = "TOPIC")]
        topics: Vec<String>,
    },
    /// Stop the core: its links close, its daemons stop.
    Shutdown,
}

#[derive(Subcommand, Debug)]
enum SetupKind {
    /// A Nintendo 3DS: over its ftpd, or on its SD card in this computer's reader.
    #[command(name = "3ds")]
    ThreeDs(SetupArgs),
}

#[derive(Args, Debug)]
struct SetupArgs {
    /// The console running ftpd: an address, with :port when it is not 5000. Without --host or --card, Hub looks for one.
    #[arg(long, value_name = "IP[:PORT]", conflicts_with = "card")]
    host: Option<String>,
    /// The console's SD card, mounted at this folder.
    #[arg(long, value_name = "DIR")]
    card: Option<PathBuf>,
    /// Install this .3dsx instead of the latest release of the 3DS's software.
    #[arg(long, value_name = "FILE", conflicts_with = "release")]
    program: Option<PathBuf>,
    /// Install this release of the 3DS's software (a tag) instead of the latest.
    #[arg(long, value_name = "TAG")]
    release: Option<String>,
}

#[derive(Args, Debug)]
struct PairArgs {
    /// The 6 digits the device shows.
    #[arg(long, value_name = "DIGITS", conflicts_with = "decline")]
    code: Option<String>,
    /// Dismiss the device's request instead of pairing it.
    #[arg(long)]
    decline: bool,
    /// The device, by the id `pocket-hub pair` lists; may be left out when exactly one device asks.
    #[arg(long, value_name = "ID")]
    device: Option<String>,
}

#[derive(Args, Debug)]
struct StoreArgs {
    /// The device: list the applets for its target with what it holds; install on it. `install` takes the one paired device without it.
    #[arg(long, global = true, value_name = "ID")]
    device: Option<String>,
    /// Fetch the catalog again now instead of using the one Hub holds.
    #[arg(long)]
    refresh: bool,
    #[command(subcommand)]
    command: Option<StoreCommand>,
}

#[derive(Subcommand, Debug)]
enum StoreCommand {
    /// Install an applet from the store on a device, downloading it first when Hub has not.
    Install {
        /// The applet's id (dev.pocket-nexus.hn).
        app: String,
    },
}

/// `device [<id>] …`, parsed after the id is taken off.
#[derive(Parser, Debug)]
#[command(name = "pocket-hub device [<id>]", no_binary_name = true)]
struct DeviceCli {
    #[command(subcommand)]
    command: DeviceCommand,
}

#[derive(Subcommand, Debug)]
enum DeviceCommand {
    /// The device's fresh runtime status.
    Status,
    /// The applets the device reported.
    Apps,
    /// Install a .pocket package, or replace the running guest with it.
    Push { file: PathBuf },
    /// Launch an installed applet by its output name or id.
    Launch { app: String },
    /// Delete an installed applet, and its data on the device, by its id or output name.
    Remove { app: String },
    /// Save a screenshot of both screens (into Hub's cache without OUT).
    Shot { out: Option<PathBuf> },
    /// Print the device's log lines as they come (Ctrl-C ends; logs go off when no client wants them).
    Logs {
        /// Follow the log (the device keeps no history, so this is what `logs` always does).
        #[arg(short, long)]
        follow: bool,
    },
    /// Update the 3DS's software over the link: its latest release, a release, or a local .3dsx. The device then restarts.
    Update {
        /// Install this .3dsx instead of the latest release. It replaces the 3DS's software whatever the file is called.
        #[arg(long, value_name = "FILE", conflicts_with = "release")]
        program: Option<PathBuf>,
        /// Install this release (a tag) instead of the latest.
        #[arg(long, value_name = "TAG")]
        release: Option<String>,
        /// Do not restart the device into the new program; it runs at the next start.
        #[arg(long)]
        no_launch: bool,
    },
    /// Delete the device's pairing on this computer and close its link.
    Forget,
}

const DEVICE_COMMANDS: [&str; 9] = ["status", "apps", "push", "launch", "remove", "shot", "logs", "update", "forget"];

#[derive(Subcommand, Debug)]
enum DaemonCommand {
    /// Install a daemon package: a folder or a .tar.gz.
    Install { path: PathBuf },
    Start { id: String },
    Stop { id: String },
    Restart { id: String },
    /// The daemon's output.
    Logs {
        id: String,
        /// Keep printing new lines.
        #[arg(short, long)]
        follow: bool,
        /// How many of the kept lines to print first.
        #[arg(long, default_value_t = 200)]
        tail: usize,
    },
    /// Remove the daemon (its data folder is kept unless --purge).
    Remove {
        id: String,
        /// Remove the daemon's data folder too.
        #[arg(long)]
        purge: bool,
    },
}

/// How a command ended.
enum Failure {
    Rpc(RpcError),
    /// A usage error found after clap (in `device …`); already printed when empty.
    Usage(String),
    Unreachable(String),
    Other(String),
}

impl From<ClientError> for Failure {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Rpc(error) => Failure::Rpc(error),
            ClientError::Unreachable { .. } | ClientError::Closed => Failure::Unreachable(error.to_string()),
            ClientError::Decode(text) => Failure::Other(format!("The core's answer could not be read: {text}")),
        }
    }
}

impl From<connect::Unreachable> for Failure {
    fn from(error: connect::Unreachable) -> Self {
        Failure::Unreachable(error.0)
    }
}

type Outcome = Result<(), Failure>;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = if error.use_stderr() { status::USAGE } else { status::OK };
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    let out = Out::new(cli.json);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let result = runtime.block_on(run(cli, &out));
    match result {
        Ok(()) => ExitCode::from(status::OK),
        Err(Failure::Rpc(error)) => {
            out.error(&error);
            ExitCode::from(status::FAILED)
        }
        Err(Failure::Other(message)) => {
            out.failure(&message);
            ExitCode::from(status::FAILED)
        }
        Err(Failure::Unreachable(message)) => {
            out.failure(&message);
            ExitCode::from(status::UNREACHABLE)
        }
        Err(Failure::Usage(message)) => {
            out.failure(&message);
            ExitCode::from(status::USAGE)
        }
    }
}

async fn run(cli: Cli, out: &Out) -> Outcome {
    let home = connect::home(cli.home.clone())?;
    // `shutdown` never starts a core only to stop it.
    let start = !cli.no_start && !matches!(cli.command, Command::Shutdown);
    // `device` is parsed before connecting, so a usage error needs no core.
    let device = match &cli.command {
        Command::Device { args } => Some(parse_device(args)?),
        _ => None,
    };
    let client = connect::connect(&home, start).await?;
    match cli.command {
        Command::Status => {
            let value = client.call(methods::STATUS, &json!({})).await?;
            let devices = client.call_as::<DeviceList>(methods::DEVICES_LIST, &json!({})).await?;
            let daemons = client.call_as::<DaemonList>(methods::DAEMONS_LIST, &json!({})).await?;
            out.result(&value, || output::status(&decode(&value)?, &devices, &daemons))
        }
        Command::Devices => {
            let value = client.call(methods::DEVICES_LIST, &json!({})).await?;
            out.result(&value, || Ok(output::devices(&decode::<DeviceList>(&value)?.devices)))
        }
        Command::Device { .. } => {
            let (id, command) = device.expect("parsed above");
            device_command(&client, out, &id, command).await
        }
        Command::Setup { kind: SetupKind::ThreeDs(args) } => setup(&client, out, args).await,
        Command::Pair(args) => pair(&client, out, args).await,
        Command::Daemons => {
            let value = client.call(methods::DAEMONS_LIST, &json!({})).await?;
            out.result(&value, || Ok(output::daemons(&decode::<DaemonList>(&value)?.daemons)))
        }
        Command::Daemon { command } => daemon_command(&client, out, command).await,
        Command::Runtime { install } => {
            let call = client.start(methods::DAEMONS_RUNTIME, &RuntimeParams { install: Some(install) }).await?;
            let bar = out.bar("Downloading Bun");
            let value = call
                .finish_with(|progress| {
                    if let Ok(progress) = serde_json::from_value::<RuntimeProgress>(progress) {
                        bar.set(progress.done, progress.total, "");
                    }
                })
                .await;
            bar.done();
            let value = value?;
            out.result(&value, || {
                let info: RuntimeInfo = decode(&value)?;
                Ok(if info.installed { format!("Bun {} is installed at {}.", info.version, info.path) } else { format!("Bun {} is not installed; Hub installs it at {} (pocket-hub runtime --install).", info.version, info.path) })
            })
        }
        Command::Store(args) => store(&client, out, args).await,
        Command::Watch { topics } => watch(&client, out, topics).await,
        Command::Shutdown => {
            let value = client.call(methods::SHUTDOWN, &json!({})).await?;
            // The answer comes before the core has stopped its links and
            // daemons: it is gone when it has closed this connection and
            // removed its socket (§1 "Starting and stopping the core").
            let socket = pocket_hub_api::paths::socket_path(&home);
            let gone = async {
                client.closed().await;
                while socket.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            };
            if tokio::time::timeout(SHUTDOWN_WAIT, gone).await.is_err() {
                return Err(Failure::Other(format!(
                    "The Pocket Hub core accepted the shutdown but had not stopped {} s later ({} is still there).",
                    SHUTDOWN_WAIT.as_secs(),
                    socket.display()
                )));
            }
            out.result(&value, || Ok("The Pocket Hub core stopped.".into()))
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, Failure> {
    serde_json::from_value(value.clone()).map_err(|error| Failure::Other(format!("The core's answer could not be read: {error}")))
}

/// An absolute path for the core, which does not share this shell's folder.
fn absolute(path: &Path) -> Result<String, Failure> {
    let path = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().map_err(|error| Failure::Other(format!("The current folder cannot be read: {error}")))?.join(path) };
    Ok(path.display().to_string())
}

fn parse_device(args: &[String]) -> Result<(String, DeviceCommand), Failure> {
    let (id, rest) = match args.first() {
        Some(first) if !DEVICE_COMMANDS.contains(&first.as_str()) && !first.starts_with('-') => (first.clone(), &args[1..]),
        _ => ("*".to_owned(), args),
    };
    match DeviceCli::try_parse_from(rest) {
        Ok(parsed) => Ok((id, parsed.command)),
        Err(error) => {
            let _ = error.print();
            Err(Failure::Usage(String::new()))
        }
    }
}

async fn device_command(client: &Client, out: &Out, id: &str, command: DeviceCommand) -> Outcome {
    match command {
        DeviceCommand::Status => {
            let value = client.call(methods::DEVICES_STATUS, &DeviceParams { device: id.into() }).await?;
            if out.json {
                return out.result(&value, || Ok(String::new()));
            }
            // The device's own program, beside its fresh runtime status.
            let device: Device = client.call_as(methods::DEVICES_GET, &DeviceParams { device: id.into() }).await?;
            out.result(&value, || Ok(format!("{}\nprogram: {}", output::fields(&value), output::program(&device.program))))
        }
        DeviceCommand::Apps => {
            let value = client.call(methods::DEVICES_GET, &DeviceParams { device: id.into() }).await?;
            let device: Device = decode(&value)?;
            let apps = serde_json::to_value(&device.apps).expect("apps serialize");
            out.result(&apps, || Ok(output::apps(&device)))
        }
        DeviceCommand::Push { file } => {
            let call = client.start(methods::DEVICES_PUSH, &PathParams { device: id.into(), path: absolute(&file)? }).await?;
            let value = transfer(out, call, "Pushing").await?;
            out.result(&value, || {
                let pushed: PushResult = decode(&value)?;
                Ok(format!("Pushed {} ({}): generation {}, hash {}.", pushed.app.title, pushed.app.id, pushed.generation, pushed.hash))
            })
        }
        DeviceCommand::Launch { app } => {
            let value = client.call(methods::DEVICES_LAUNCH, &LaunchParams { device: id.into(), app }).await?;
            out.result(&value, || Ok(format!("Launched {}.", decode::<LaunchResult>(&value)?.output)))
        }
        DeviceCommand::Remove { app } => {
            let value = client.call(methods::DEVICES_REMOVE, &RemoveParams { device: id.into(), app: app.clone() }).await?;
            out.result(&value, || Ok(format!("Removed {app}.")))
        }
        DeviceCommand::Shot { out: path } => {
            let path = path.map(|path| absolute(&path)).transpose()?;
            let value = client.call(methods::DEVICES_SCREENSHOT, &ScreenshotParams { device: id.into(), path }).await?;
            out.result(&value, || {
                let shot: ScreenshotResult = decode(&value)?;
                Ok(format!("Saved {} ({}x{}, frame {}).", shot.path, shot.width, shot.height, shot.frame))
            })
        }
        DeviceCommand::Logs { follow: _ } => device_logs(client, out, id).await,
        DeviceCommand::Update { program, release, no_launch } => {
            let program = match (program, release) {
                (Some(path), _) => Some(ProgramChoice::Path { path: absolute(&path)? }),
                (None, Some(tag)) => Some(ProgramChoice::Release { release: tag }),
                (None, None) => None,
            };
            let call = client.start(methods::DEVICES_INSTALL_PROGRAM, &InstallProgramParams { device: id.into(), program, launch: Some(!no_launch) }).await?;
            let value = transfer(out, call, "Updating").await?;
            out.result(&value, || Ok(output::program_installed(&decode(&value)?)))
        }
        DeviceCommand::Forget => {
            let value = client.call(methods::DEVICES_FORGET, &DeviceParams { device: id.into() }).await?;
            out.result(&value, || Ok("The device is forgotten: its pairing on this computer is deleted.".into()))
        }
    }
}

async fn store(client: &Client, out: &Out, args: StoreArgs) -> Outcome {
    match args.command {
        None => {
            let for_device = args.device.is_some();
            let value = client.call(methods::STORE_APPS, &StoreAppsParams { device: args.device, refresh: args.refresh.then_some(true) }).await?;
            out.result(&value, || Ok(output::store(&decode(&value)?, for_device)))
        }
        Some(StoreCommand::Install { app }) => {
            let device = args.device.unwrap_or_else(|| "*".into());
            let call = client.start(methods::STORE_INSTALL, &StoreInstallParams { device, app }).await?;
            let value = transfer(out, call, "Installing").await?;
            out.result(&value, || {
                let installed: PushResult = decode(&value)?;
                Ok(format!("Installed {} ({}): generation {}, hash {}.", installed.app.title, installed.app.id, installed.generation, installed.hash))
            })
        }
    }
}

async fn transfer(out: &Out, call: pocket_hub_api::Call, label: &str) -> Result<Value, Failure> {
    let bar = out.bar(label);
    let result = call
        .finish_with(|progress| {
            if let Ok(progress) = serde_json::from_value::<TransferProgress>(progress) {
                bar.set(progress.done, Some(progress.total), &progress.phase);
            }
        })
        .await;
    bar.done();
    Ok(result?)
}

/// Waits for Ctrl-C.
async fn interrupted() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn device_logs(client: &Client, out: &Out, id: &str) -> Outcome {
    client.call(methods::SUBSCRIBE, &SubscribeParams { topics: Some(vec![topics::DEVICE_LOG.into(), topics::DEVICE_UNLINKED.into()]) }).await?;
    // The id `*` names the one paired device; the events name it by its id.
    let device: Device = client.call_as(methods::DEVICES_GET, &DeviceParams { device: id.into() }).await?;
    client.call(methods::DEVICES_LOGS, &LogsParams { device: device.id.clone(), on: true }).await?;
    out.note(&format!("Logs of {} (Ctrl-C ends).", if device.name.is_empty() { &device.id } else { &device.name }));
    loop {
        tokio::select! {
            _ = interrupted() => break,
            event = client.next_event() => {
                let Some(event) = event else { return Err(Failure::Unreachable("The connection to the Pocket Hub core closed.".into())) };
                if event.get("id").and_then(Value::as_str) != Some(device.id.as_str()) {
                    continue;
                }
                out.event(&event, output::event_line);
            }
        }
    }
    let _ = client.call(methods::DEVICES_LOGS, &LogsParams { device: device.id, on: false }).await;
    Ok(())
}

async fn setup(client: &Client, out: &Out, args: SetupArgs) -> Outcome {
    let program = match (&args.program, &args.release) {
        (Some(path), _) => Some(ProgramChoice::Path { path: absolute(path)? }),
        (None, Some(tag)) => Some(ProgramChoice::Release { release: tag.clone() }),
        (None, None) => None,
    };
    let call = if let Some(card) = &args.card {
        client.start(methods::SETUP_CARD, &SetupCardParams { kind: "3ds".into(), path: absolute(card)?, program }).await?
    } else {
        let (host, port) = match &args.host {
            Some(host) => split_host(host)?,
            None => {
                out.note("Looking for a console running ftpd on this network…");
                let found: DiscoverResult = client.call_as(methods::SETUP_DISCOVER, &KindParams { kind: "3ds".into() }).await?;
                // ftpd's own banners first: the one such console is taken;
                // without one, the one host that answered like FTP.
                let ftpd: Vec<&Candidate> = found.candidates.iter().filter(|c| c.ftpd).collect();
                let pool: Vec<&Candidate> = if ftpd.is_empty() { found.candidates.iter().collect() } else { ftpd };
                match pool.as_slice() {
                    [] => return Err(Failure::Other("No console running ftpd answered on this network. Open ftpd on the console, or give its address with --host.".into())),
                    [only] => {
                        out.note(&format!("Found {}:{} ({}).", only.address, only.port, only.banner));
                        (only.address.clone(), Some(only.port))
                    }
                    several => {
                        let list: Vec<String> = several.iter().map(|c| format!("  {}:{}  {}{}", c.address, c.port, c.banner, if c.ftpd { "  (ftpd)" } else { "" })).collect();
                        return Err(Failure::Other(format!("Several hosts answered like ftpd; choose one with --host:\n{}", list.join("\n"))));
                    }
                }
            }
        };
        client.start(methods::SETUP_INSTALL, &SetupInstallParams { kind: "3ds".into(), host, port, program }).await?
    };
    let bar = out.bar("Setting up");
    let result = call
        .finish_with(|progress| {
            if let Ok(progress) = serde_json::from_value::<SetupProgress>(progress) {
                bar.set(progress.done.unwrap_or(0), progress.total, &progress.message);
            }
        })
        .await;
    bar.done();
    let value = result?;
    out.result(&value, || Ok(output::report(&decode(&value)?)))
}

/// `pair`: the devices that ask to be paired by the code they show, an
/// answer to one with its code, or its dismissal (CONTROL-API.md §3 Pairing).
async fn pair(client: &Client, out: &Out, args: PairArgs) -> Outcome {
    let device = args.device.unwrap_or_else(|| "*".into());
    if let Some(code) = args.code {
        let value = client.call(methods::PAIRING_ANSWER, &PairAnswerParams { device, code }).await?;
        return out.result(&value, || Ok(output::paired(&decode(&value)?)));
    }
    if args.decline {
        let value = client.call(methods::PAIRING_DECLINE, &DeviceParams { device }).await?;
        return out.result(&value, || Ok("The request was dismissed; the device says so.".into()));
    }
    let value = client.call(methods::PAIRING_LIST, &json!({})).await?;
    out.result(&value, || Ok(output::pair_requests(&decode::<PairRequests>(&value)?.requests)))
}

fn split_host(text: &str) -> Result<(String, Option<u16>), Failure> {
    match text.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            let port = port.parse().map_err(|_| Failure::Usage(format!("{port} is not a port number.")))?;
            Ok((host.to_owned(), Some(port)))
        }
        _ => Ok((text.to_owned(), None)),
    }
}

async fn daemon_command(client: &Client, out: &Out, command: DaemonCommand) -> Outcome {
    let simple = |method: &'static str, id: String| async move {
        let value = client.call(method, &DaemonIdParams { id }).await?;
        out.result(&value, || Ok(output::daemons(&[decode::<Daemon>(&value)?])))
    };
    match command {
        DaemonCommand::Install { path } => {
            let call = client.start(methods::DAEMONS_INSTALL, &DaemonPathParams { path: absolute(&path)? }).await?;
            let value = call
                .finish_with(|progress| {
                    if let Ok(progress) = serde_json::from_value::<DaemonInstallProgress>(progress) {
                        out.note(&progress.message);
                    }
                })
                .await?;
            out.result(&value, || {
                let daemon: Daemon = decode(&value)?;
                Ok(format!("Installed {} {}.", daemon.id, daemon.version))
            })
        }
        DaemonCommand::Start { id } => simple(methods::DAEMONS_START, id).await,
        DaemonCommand::Stop { id } => simple(methods::DAEMONS_STOP, id).await,
        DaemonCommand::Restart { id } => simple(methods::DAEMONS_RESTART, id).await,
        DaemonCommand::Remove { id, purge } => {
            let value = client.call(methods::DAEMONS_REMOVE, &DaemonRemoveParams { id: id.clone(), purge: Some(purge) }).await?;
            out.result(&value, || Ok(if purge { format!("Removed {id} and its data.") } else { format!("Removed {id}; its data folder is kept.") }))
        }
        DaemonCommand::Logs { id, follow, tail } => {
            if follow {
                client.call(methods::SUBSCRIBE, &SubscribeParams { topics: Some(vec![topics::DAEMON_LOG.into()]) }).await?;
            }
            let value = client.call(methods::DAEMONS_LOGS, &DaemonLogsParams { id: id.clone(), tail: Some(tail) }).await?;
            let logs: DaemonLogs = decode(&value)?;
            if out.json && !follow {
                out.print_json(&value);
            } else {
                for line in &logs.lines {
                    out.event(&json!({"topic": topics::DAEMON_LOG, "id": id, "at": line.at, "stream": line.stream, "text": line.text}), output::event_line);
                }
            }
            if !follow {
                return Ok(());
            }
            // Lines already printed from the kept ones are skipped once.
            let mut seen: std::collections::VecDeque<(i64, String)> = logs.lines.iter().map(|l| (l.at, l.text.clone())).collect();
            loop {
                tokio::select! {
                    _ = interrupted() => return Ok(()),
                    event = client.next_event() => {
                        let Some(event) = event else { return Err(Failure::Unreachable("The connection to the Pocket Hub core closed.".into())) };
                        if event.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                            continue;
                        }
                        let key = (event["at"].as_i64().unwrap_or(0), event["text"].as_str().unwrap_or("").to_owned());
                        if let Some(at) = seen.iter().position(|old| *old == key) {
                            seen.drain(..=at);
                            continue;
                        }
                        seen.clear();
                        out.event(&event, output::event_line);
                    }
                }
            }
        }
    }
}

async fn watch(client: &Client, out: &Out, topics: Vec<String>) -> Outcome {
    let topics = if topics.is_empty() { None } else { Some(topics) };
    client.call(methods::SUBSCRIBE, &SubscribeParams { topics }).await?;
    out.note("Watching the core's events (Ctrl-C ends).");
    loop {
        tokio::select! {
            _ = interrupted() => return Ok(()),
            event = client.next_event() => {
                let Some(event) = event else { return Err(Failure::Unreachable("The connection to the Pocket Hub core closed.".into())) };
                out.event(&event, output::event_line);
            }
        }
    }
}
