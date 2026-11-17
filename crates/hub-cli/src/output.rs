//! What the command line prints: results on standard output (one JSON
//! object per line with `--json`, the raw result of the call), notes,
//! progress bars and errors on standard error. A progress bar is drawn only
//! on a terminal.

use std::cell::RefCell;
use std::io::{IsTerminal, Write};

use pocket_hub_api::RpcError;
use pocket_hub_api::types::*;
use serde_json::{Value, json};

use crate::Failure;

pub struct Out {
    pub json: bool,
    terminal: bool,
}

impl Out {
    pub fn new(json: bool) -> Self {
        Self { json, terminal: std::io::stderr().is_terminal() }
    }

    pub fn print_json(&self, value: &Value) {
        println!("{value}");
    }

    /// A call's result: as it came with `--json`, else as `human` words it.
    pub fn result(&self, value: &Value, human: impl FnOnce() -> Result<String, Failure>) -> Result<(), Failure> {
        if self.json {
            self.print_json(value);
        } else {
            let text = human()?;
            if !text.is_empty() {
                println!("{text}");
            }
        }
        Ok(())
    }

    /// One event: its params with `--json`, else one line.
    pub fn event(&self, event: &Value, human: impl FnOnce(&Value) -> String) {
        if self.json {
            self.print_json(event);
        } else {
            println!("{}", human(event));
        }
        let _ = std::io::stdout().flush();
    }

    pub fn note(&self, text: &str) {
        if !self.json {
            eprintln!("{text}");
        }
    }

    pub fn error(&self, error: &RpcError) {
        if self.json {
            println!("{}", json!({"error": error}));
            return;
        }
        let mut line = format!("pocket-hub: {}", error.message);
        if let Some(data) = &error.data {
            let mut detail = vec![data.kind.as_str().to_owned()];
            if let Some(holder) = &data.holder {
                detail.push(format!("held by {} ({}) running {}", holder.name, holder.kind.as_str(), holder.method));
            }
            for (name, value) in [("code", &data.code), ("phase", &data.phase), ("step", &data.step), ("reason", &data.reason), ("device", &data.device), ("id", &data.id)] {
                if let Some(value) = value {
                    detail.push(format!("{name} {value}"));
                }
            }
            if data.confirmed == Some(false) {
                detail.push("not confirmed".into());
            }
            line.push_str(&format!(" [{}]", detail.join(", ")));
        }
        eprintln!("{line}");
        if let Some(detail) = error.data.as_ref().and_then(|data| data.detail.as_deref()) {
            eprintln!("  detail: {detail}");
        }
    }

    pub fn failure(&self, message: &str) {
        if message.is_empty() {
            return;
        }
        if self.json {
            println!("{}", json!({"error": {"message": message}}));
        } else {
            eprintln!("pocket-hub: {message}");
        }
    }

    pub fn bar(&self, label: &str) -> Bar {
        Bar { label: label.to_owned(), on: self.terminal && !self.json, drawn: RefCell::new(false) }
    }
}

pub struct Bar {
    label: String,
    on: bool,
    drawn: RefCell<bool>,
}

impl Bar {
    pub fn set(&self, done: u64, total: Option<u64>, phase: &str) {
        if !self.on {
            return;
        }
        const WIDTH: u64 = 30;
        let line = match total {
            Some(total) if total > 0 => {
                let filled = (done.min(total) * WIDTH / total) as usize;
                format!("{} [{}{}] {:>3}% {}", self.label, "#".repeat(filled), " ".repeat(WIDTH as usize - filled), done.min(total) * 100 / total, phase)
            }
            _ => format!("{} {}", self.label, phase),
        };
        eprint!("\r\x1b[2K{line}");
        let _ = std::io::stderr().flush();
        *self.drawn.borrow_mut() = true;
    }

    pub fn done(&self) {
        if *self.drawn.borrow() {
            eprintln!();
        }
    }
}

fn local_time(ms: i64) -> String {
    let seconds = ms / 1000;
    // The C library's local time: no time zone database is needed here.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t: libc::time_t = seconds as libc::time_t;
    // SAFETY: localtime_r writes into `tm`, which lives for the call.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return ms.to_string();
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

pub fn status(hub: &HubStatus, devices: &DeviceList, daemons: &DaemonList) -> Result<String, Failure> {
    let carriers: Vec<String> = hub.carriers.iter().map(|c| match c.port {
        Some(port) => format!("{} port {port}", c.kind),
        None => c.kind.clone(),
    }).collect();
    let linked = devices.devices.iter().filter(|d| d.link.is_some()).count();
    let running = daemons.daemons.iter().filter(|d| d.state == DaemonState::Running).count();
    Ok(format!(
        "Pocket Hub \"{}\" ({}) {}\nHome:     {}\nStarted:  {}\nLink:     {}\nBun:      {} ({})\nDevices:  {} paired, {} linked\nDaemons:  {} installed, {} running",
        hub.name,
        hub.id,
        hub.version,
        hub.home,
        local_time(hub.started_at),
        carriers.join(", "),
        hub.bun.version,
        if hub.bun.installed { "installed" } else { "not installed" },
        devices.devices.len(),
        linked,
        daemons.daemons.len(),
        running,
    ))
}

fn table(header: &[&str], rows: Vec<Vec<String>>) -> String {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let line = |cells: Vec<String>| {
        let mut text = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                text.push_str(cell);
            } else {
                text.push_str(&format!("{cell:<width$}  ", width = widths[i]));
            }
        }
        text.trim_end().to_owned()
    };
    let mut lines = vec![line(header.iter().map(|h| h.to_string()).collect())];
    lines.extend(rows.into_iter().map(line));
    lines.join("\n")
}

pub fn device_state(device: &Device) -> String {
    let mut state = match &device.link {
        Some(link) => format!("linked ({} {})", link.carrier, link.peer),
        None => "not linked".to_owned(),
    };
    if let Some(busy) = &device.busy {
        if busy.launching {
            state.push_str(&format!(", restarting into a program ({} ran {})", busy.holder.name, busy.holder.method));
        } else {
            state.push_str(&format!(", busy: {} running {}", busy.holder.name, busy.holder.method));
        }
    }
    if device.setup_unconfirmed {
        state.push_str(", setup not confirmed");
    }
    if device.duplicate_id {
        state.push_str(", two consoles share this id");
    }
    state
}

pub fn devices(devices: &[Device]) -> String {
    if devices.is_empty() {
        return "No device is paired. Set a 3DS up with: pocket-hub setup 3ds\nA device that shows a code (an M5Stack Tab5) is paired with: pocket-hub pair".into();
    }
    let rows = devices.iter().map(|d| vec![d.id.clone(), d.name.clone(), d.model.clone(), program(&d.program), device_state(d)]).collect();
    table(&["ID", "NAME", "MODEL", "PROGRAM", "STATE"], rows)
}

/// `pair`: the devices that ask to be paired, and how to answer one.
pub fn pair_requests(requests: &[PairRequest]) -> String {
    if requests.is_empty() {
        return "No device asks to be paired. On the device, open its pairing page and choose this computer; then run this again.".into();
    }
    let rows = requests.iter().map(|r| vec![r.device.clone(), r.name.clone(), r.model.clone(), r.address.clone(), local_time(r.expires)]).collect();
    format!("{}\nEnter the code a device shows with: pocket-hub pair --code <digits>", table(&["ID", "NAME", "MODEL", "ADDRESS", "ASKS UNTIL"], rows))
}

/// What `pair --code` did.
pub fn paired(device: &Device) -> String {
    let name = if device.name.is_empty() { &device.id } else { &device.name };
    format!("Paired {name} ({}). It connects by itself, now and whenever it is on.", device.id)
}

/// The device's own program: its version (`-` before it reported one) and
/// the newest release when that is an update.
pub fn program(program: &ProgramState) -> String {
    let version = program.version.as_deref().unwrap_or("-");
    match (&program.latest, program.update) {
        (Some(latest), true) => format!("{version} (update available: {latest})"),
        _ => version.to_owned(),
    }
}

/// What `device update` installed. The file's name on the device is not
/// the owner's concern: it is in the `--json` result alone.
pub fn program_installed(done: &InstallProgramResult) -> String {
    let what = match &done.version {
        Some(version) => format!("Version {version} of the 3DS's software was installed."),
        None => "The 3DS's software was installed.".to_owned(),
    };
    let next = match done.phase.as_str() {
        "launching" => "The device restarts and connects again by itself.",
        "staged" => "It takes over when the software running on the device exits.",
        _ => "It runs at the device's next start.",
    };
    format!("{what} {next}")
}

pub fn apps(device: &Device) -> String {
    match &device.apps {
        None => "The device has not reported its applets yet.".into(),
        Some(apps) if apps.is_empty() => "The device has no applets installed.".into(),
        Some(apps) => table(
            &["OUTPUT", "ID", "VERSION", "HASH"],
            apps.iter().map(|a| vec![format!("{}{}", a.output, if a.home == Some(true) { " (home)" } else { "" }), a.id.clone(), a.version.clone(), a.hash.clone()]).collect(),
        ),
    }
}

/// A size for people: 2.2 MB.
fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} kB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

/// `store`: one row per applet; with a device, what it holds of each.
pub fn store(apps: &StoreApps, for_device: bool) -> String {
    let when = local_time(apps.fetched_at);
    if apps.apps.is_empty() {
        let none = if for_device { "The applet store offers no applet for this device" } else { "The applet store offers no applet" };
        return format!("{none} (catalog of {when}).");
    }
    let mut header = vec!["TITLE", "ID", "VERSION", "SIZE"];
    if for_device {
        header.push("ON DEVICE");
    }
    let rows = apps
        .apps
        .iter()
        .map(|app| {
            let title = if app.official { app.title.clone() } else { format!("{} (by {})", app.title, app.author) };
            // `*`: Hub has not downloaded the package yet.
            let mut row = vec![title, app.id.clone(), app.version.clone(), format!("{}{}", size(app.bytes), if app.cached { "" } else { " *" })];
            if for_device {
                row.push(match (&app.installed, app.update) {
                    (Some(Some(copy)), Some(true)) => format!("{} (update)", copy.version),
                    (Some(Some(copy)), _) => format!("{} installed", copy.version),
                    _ => "-".into(),
                });
            }
            row
        })
        .collect();
    let note = if apps.apps.iter().any(|app| !app.cached) { "\n* not downloaded by Hub yet: a device's own store page lists it once Hub has it." } else { "" };
    format!("{}{note}\nCatalog of {when}.", table(&header, rows))
}

/// `store details`: one line per member, ids as the store gives them; a
/// member the package names nothing for is left out.
pub fn store_details(details: &StoreDetails) -> String {
    let devices: Vec<String> = details
        .devices
        .iter()
        .map(|device| match device.host_abi {
            Some(abi) => format!("{} ({}, host ABI {abi})", device.name, device.target),
            None => format!("{} ({})", device.name, device.target),
        })
        .collect();
    let ids = |list: &[String]| if list.is_empty() { "nothing".to_owned() } else { list.join(", ") };
    let mut lines = vec![
        format!("{} {}", details.id, details.version),
        format!("Built for:   {}", devices.join(", ")),
        format!("Needs:       {}", ids(&details.needs)),
        format!("Uses:        {}", ids(&details.uses)),
    ];
    if let Some(surface) = &details.surface {
        let what = if surface.touch { "a touch surface of its own" } else { "the device's screen" };
        lines.push(match &surface.size {
            Some(size) => format!("Surface:     {what}, {} × {} to {} × {} points", size.min[0], size.min[1], size.max[0], size.max[1]),
            None => format!("Surface:     {what}"),
        });
    }
    if let Some(framework) = &details.framework {
        lines.push(format!("Framework:   {framework}"));
    }
    if let Some(api) = details.api {
        lines.push(format!("Applet API:  {api}"));
    }
    lines.join("\n")
}

pub fn daemons(daemons: &[Daemon]) -> String {
    if daemons.is_empty() {
        return "No daemon is installed.".into();
    }
    let rows = daemons
        .iter()
        .map(|d| {
            let state = match (&d.state, &d.error) {
                (DaemonState::Failed, Some(error)) => format!("failed: {error}"),
                (state, _) => serde_json::to_value(state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(),
            };
            vec![d.id.clone(), d.version.clone(), state, d.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()), d.channels.to_string()]
        })
        .collect();
    table(&["ID", "VERSION", "STATE", "PID", "CHANNELS"], rows)
}

/// A free-form object (a device's runtime status) as `key: value` lines.
pub fn fields(value: &Value) -> String {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) => format!("{key}: {text}"),
                other => format!("{key}: {other}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

pub fn report(report: &InstallReport) -> String {
    let pairing = match report.pairing {
        PairingResult::Key => "a new pairing was written",
        PairingResult::Kept => "the pairing was in place",
        PairingResult::Address => "this computer's address was updated",
    };
    let program = match report.program.action {
        ProgramAction::Installed => match &report.program.version {
            Some(version) => format!("version {version} of the 3DS's software was installed"),
            None => "the 3DS's software was installed".to_owned(),
        },
        ProgramAction::Kept => "the 3DS's software was already there".to_owned(),
    };
    format!("Device {}: {pairing}; {program}.\nNext: {}", report.device, report.next)
}

/// One event as one line.
pub fn event_line(event: &Value) -> String {
    let text = |key: &str| event.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    match event.get("topic").and_then(Value::as_str).unwrap_or("") {
        "device.changed" => match serde_json::from_value::<Device>(event["device"].clone()) {
            Ok(device) => format!("device {} {}: {}", device.id, device.name, device_state(&device)),
            Err(_) => format!("device.changed {}", event["device"]),
        },
        "device.removed" => format!("device {} forgotten", text("id")),
        "device.unlinked" => format!("device {} unlinked: {} {}", text("id"), text("code"), text("reason")),
        "device.log" => format!("[{}] {} {}: {}", event["frame"], text("level"), text("source"), text("text")),
        "device.screenshot" => format!("device {} screenshot {} ({}x{})", text("id"), text("path"), event["width"], event["height"]),
        "pairing.changed" => match serde_json::from_value::<Vec<PairRequest>>(event["requests"].clone()) {
            Ok(requests) if requests.is_empty() => "pairing: no device asks to be paired".to_owned(),
            Ok(requests) => format!("pairing: {} ask(s) to be paired", requests.iter().map(|r| format!("{} {} ({})", r.device, r.name, r.address)).collect::<Vec<_>>().join(", ")),
            Err(_) => format!("pairing.changed {}", event["requests"]),
        },
        "daemon.changed" => match serde_json::from_value::<Daemon>(event["daemon"].clone()) {
            Ok(daemon) => {
                let pid = daemon.pid.map(|pid| format!(" pid {pid}")).unwrap_or_default();
                let state = serde_json::to_value(daemon.state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
                format!("daemon {} {}: {state}{pid}, {} channel(s)", daemon.id, daemon.version, daemon.channels)
            }
            Err(_) => format!("daemon.changed {}", event["daemon"]),
        },
        "daemon.removed" => format!("daemon {} removed", text("id")),
        "daemon.log" => format!("{} {} {}: {}", local_time(event["at"].as_i64().unwrap_or(0)), text("id"), text("stream"), text("text")),
        "hub.changed" => format!("hub renamed \"{}\"", event["hub"]["name"].as_str().unwrap_or("")),
        other => format!("{other} {event}"),
    }
}
