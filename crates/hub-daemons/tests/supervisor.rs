//! The supervisor against the fixture daemon (tests/fixtures/daemon.mjs) on
//! the system Bun.

mod common;

use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use pocket_hub_daemons::{DaemonState, LogStream, OpenError, Supervisor, SupervisorError};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

macro_rules! bun_or_skip {
    () => {
        match system_bun() {
            Some(bun) => bun,
            None => return,
        }
    };
}

fn log_texts(supervisor: &Supervisor, id: &str) -> Vec<String> {
    supervisor.logs(id, None).into_iter().map(|line| line.text).collect()
}

fn logged(supervisor: &Supervisor, id: &str, needle: &str) -> bool {
    log_texts(supervisor, id).iter().any(|line| line.contains(needle))
}

#[tokio::test]
async fn a_channel_opens_on_demand_and_carries_bytes_untouched() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.echo", json!({}))], |_| {}).await;
    assert_eq!(supervisor.status("app.echo").unwrap().state, DaemonState::Stopped);

    let mut first = supervisor.open_channel(offer("app.echo")).await.expect("first channel");
    let status = supervisor.status("app.echo").unwrap();
    assert_eq!(status.state, DaemonState::Running);
    assert_eq!(status.channels, 1);
    assert_eq!(status.pid, Some(first.info.pid));

    // The daemon wrote its attach and a first frame in one write: the first
    // frame is the relay's to read, whole.
    let greeting = read_frame(&mut first.reader).await.unwrap();
    assert_eq!(greeting, frame(0x12, 0, 0, br#"{"t":"greeting"}"#));

    // What the relay writes comes back from the echoing daemon unchanged.
    let mut sent = frame(0x04, 0, 0, br#"{"t":"channel.ready","channel":1,"messageLimit":8192}"#);
    sent.extend(frame(0x12, 0, 5, br#"{"t":"tick","n":1}"#));
    sent.extend(frame(0x20, 0x01, 5, &[0, 1, 2, 3, 255]));
    first.writer.write_all(&sent).await.unwrap();
    let mut echoed = vec![0u8; sent.len()];
    first.reader.read_exact(&mut echoed).await.unwrap();
    assert_eq!(echoed, sent);

    // A second channel goes to the same process.
    let mut second = supervisor.open_channel(offer("app.echo")).await.expect("second channel");
    assert_eq!(second.info.pid, first.info.pid);
    assert_ne!(second.info.channel, first.info.channel);
    assert_eq!(read_frame(&mut second.reader).await.unwrap(), greeting);
    assert_eq!(supervisor.status("app.echo").unwrap().channels, 2);

    drop(first);
    assert!(eventually(Duration::from_secs(2), || supervisor.status("app.echo").unwrap().channels == 1).await);
    let status = supervisor.status("app.echo").unwrap();
    assert_eq!(status.state, DaemonState::Running);
    assert_eq!(status.restarts, 0);

    let texts = log_texts(&supervisor, "app.echo");
    assert!(texts.iter().any(|line| line.starts_with("env POCKET_HUB_DAEMON_APP,POCKET_HUB_DAEMON_DATA,POCKET_HUB_DAEMON_SOCKET,POCKET_HUB_DAEMON_TOKEN")), "{texts:#?}");
    assert!(texts.iter().any(|line| line == "offer device=0123456789abcdef name=Test 3DS generation=7 limit=16384"), "{texts:#?}");
    drop(second);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_second_app_gets_a_second_process() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.one", json!({})), ("app.two", json!({}))], |_| {}).await;
    let (one, two) = tokio::join!(supervisor.open_channel(offer("app.one")), supervisor.open_channel(offer("app.two")));
    let (one, two) = (one.unwrap(), two.unwrap());
    assert_ne!(one.info.pid, two.info.pid);
    assert_eq!(one.info.app, "app.one");
    assert_eq!(two.info.app, "app.two");
    assert_eq!(supervisor.statuses().iter().filter(|status| status.state == DaemonState::Running).count(), 2);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn an_idle_daemon_is_asked_to_stop_and_exits() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.idle", json!({}))], |_| {}).await;
    let channel = supervisor.open_channel(offer("app.idle")).await.unwrap();
    // A channel open keeps it up past its idle time.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(supervisor.status("app.idle").unwrap().state, DaemonState::Running);
    drop(channel);
    assert!(eventually(Duration::from_secs(3), || supervisor.status("app.idle").unwrap().state == DaemonState::Stopped).await);
    let status = supervisor.status("app.idle").unwrap();
    assert_eq!(status.last_exit.as_ref().unwrap().code, Some(0));
    assert_eq!(status.crashes, 0);
    assert!(logged(&supervisor, "app.idle", "stopping"));
    assert!(!logged(&supervisor, "app.idle", "sending SIGTERM"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_daemon_that_ignores_daemon_stop_gets_sigterm() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.stubborn", json!({ "ignoreStop": true }))], |_| {}).await;
    drop(supervisor.open_channel(offer("app.stubborn")).await.unwrap());
    assert!(eventually(Duration::from_secs(4), || supervisor.status("app.stubborn").unwrap().state == DaemonState::Stopped).await);
    let status = supervisor.status("app.stubborn").unwrap();
    assert_eq!(status.last_exit.as_ref().unwrap().signal, Some(libc::SIGTERM));
    assert!(logged(&supervisor, "app.stubborn", "ignoring daemon.stop"));
    assert!(logged(&supervisor, "app.stubborn", "sending SIGTERM"));
    assert!(!logged(&supervisor, "app.stubborn", "sending SIGKILL"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_daemon_that_ignores_sigterm_gets_sigkill() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.deaf", json!({ "ignoreTerm": true }))], |_| {}).await;
    drop(supervisor.open_channel(offer("app.deaf")).await.unwrap());
    assert!(eventually(Duration::from_secs(5), || supervisor.status("app.deaf").unwrap().state == DaemonState::Stopped).await);
    let status = supervisor.status("app.deaf").unwrap();
    assert_eq!(status.last_exit.as_ref().unwrap().signal, Some(libc::SIGKILL));
    assert!(logged(&supervisor, "app.deaf", "ignoring SIGTERM"));
    assert!(logged(&supervisor, "app.deaf", "sending SIGKILL"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_wrong_token_or_protocol_is_refused() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let daemons = [("app.token", json!({ "mode": "badToken" })), ("app.proto", json!({ "mode": "badProtocol" }))];
    let supervisor = supervisor_with(home.path(), bun, &daemons, |_| {}).await;

    let started = Instant::now();
    let error = supervisor.open_channel(offer("app.token")).await.err().unwrap();
    assert!(matches!(&error, OpenError::Busy(why) if why.contains("before its daemon.hello")), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(logged(&supervisor, "app.token", "refused: unknown token"));
    assert_eq!(supervisor.status("app.token").unwrap().state, DaemonState::Failed);

    let error = supervisor.open_channel(offer("app.proto")).await.err().unwrap();
    assert!(matches!(&error, OpenError::Busy(why) if why.contains("protocol 2 is not supported")), "{error}");
    assert!(eventually(Duration::from_secs(2), || logged(&supervisor, "app.proto", "refused: protocol 2 is not supported")).await);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_daemon_that_never_says_hello_is_ended_and_busy() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.silent", json!({ "mode": "silent" }))], |config| {
        config.timings.start = Duration::from_millis(1500);
    })
    .await;
    let started = Instant::now();
    let error = supervisor.open_channel(offer("app.silent")).await.err().unwrap();
    assert!(matches!(&error, OpenError::Busy(why) if why.contains("no daemon.hello within 1.5 s")), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(eventually(Duration::from_secs(2), || supervisor.status("app.silent").unwrap().state == DaemonState::Failed).await);
    let status = supervisor.status("app.silent").unwrap();
    assert_eq!(status.last_exit.as_ref().unwrap().signal, Some(libc::SIGTERM));
    assert!(logged(&supervisor, "app.silent", "never connecting"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_ticket_never_claimed_is_busy_in_time() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.mute", json!({ "mode": "noAttach" }))], |_| {}).await;
    // Started first, so the time measured is the attach wait alone.
    supervisor.start("app.mute").await.unwrap();
    let started = Instant::now();
    let error = supervisor.open_channel(offer("app.mute")).await.err().unwrap();
    let waited = started.elapsed();
    assert!(matches!(&error, OpenError::Busy(why) if why.contains("did not attach")), "{error}");
    assert!(waited >= Duration::from_millis(900) && waited < Duration::from_millis(2000), "{waited:?}");
    assert!(logged(&supervisor, "app.mute", "no channel.attach within"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn an_unknown_ticket_is_refused() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let fixture = json!({ "attachTicket": "00000000000000000000000000000000" });
    let supervisor = supervisor_with(home.path(), bun, &[("app.forger", fixture)], |_| {}).await;
    let error = supervisor.open_channel(offer("app.forger")).await.err().unwrap();
    assert!(matches!(error, OpenError::Busy(_)));
    assert!(eventually(Duration::from_secs(2), || logged(&supervisor, "app.forger", "channel refused: unknown or expired ticket")).await);
    assert!(eventually(Duration::from_secs(2), || logged(&supervisor, "app.forger", "channel closed")).await);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn an_app_without_a_daemon_is_no_daemon() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[], |_| {}).await;
    assert_eq!(supervisor.open_channel(offer("app.none")).await.err(), Some(OpenError::NoDaemon));
    assert_eq!(supervisor.open_channel(offer("../etc")).await.err(), Some(OpenError::NoDaemon));
    assert!(supervisor.statuses().is_empty());
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_daemon_that_exits_at_start_is_busy_with_its_log_and_not_respawned() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.boom", json!({ "mode": "exitAtStart" }))], |_| {}).await;
    let error = supervisor.open_channel(offer("app.boom")).await.err().unwrap();
    assert!(matches!(&error, OpenError::Busy(why) if why.contains("exited with code 3")), "{error}");
    let lines = supervisor.logs("app.boom", None);
    assert!(lines.iter().any(|line| line.stream == LogStream::Stderr && line.text == "boom: exiting at start"), "{lines:#?}");
    let status = supervisor.status("app.boom").unwrap();
    assert_eq!(status.state, DaemonState::Failed);
    assert_eq!(status.crashes, 1);
    assert!(status.retry_at.is_some());
    assert_eq!(status.last_exit.as_ref().unwrap().code, Some(3));

    // Opens during the hold are answered at once, without a start.
    for _ in 0..5 {
        let started = Instant::now();
        let error = supervisor.open_channel(offer("app.boom")).await.err().unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(matches!(&error, OpenError::Busy(why) if why.contains("starts it again in")), "{error}");
    }
    let status = supervisor.status("app.boom").unwrap();
    assert_eq!((status.restarts, status.crashes), (0, 1));
    let starts = log_texts(&supervisor, "app.boom").iter().filter(|line| line.starts_with("starting ")).count();
    assert_eq!(starts, 1);

    // After the hold, the next open starts it again, and the hold doubles.
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(supervisor.open_channel(offer("app.boom")).await.is_err());
    let status = supervisor.status("app.boom").unwrap();
    assert_eq!((status.restarts, status.crashes), (1, 2));
    let wait = status.retry_at.unwrap() - pocket_hub_epoch_ms();
    assert!(wait > 3000 && wait <= 4000, "{wait}");
    supervisor.shutdown().await;
}

fn pocket_hub_epoch_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}

#[tokio::test]
async fn a_daemon_that_dies_with_a_channel_open_ends_the_stream() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.dies", json!({ "dieAfterAttachMs": 300 }))], |_| {}).await;
    let mut channel = supervisor.open_channel(offer("app.dies")).await.unwrap();
    read_frame(&mut channel.reader).await.unwrap();
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(3), channel.reader.read_to_end(&mut rest)).await;
    assert!(matches!(read, Ok(Ok(0))), "{read:?}");
    assert!(eventually(Duration::from_secs(2), || supervisor.status("app.dies").unwrap().state == DaemonState::Failed).await);
    assert_eq!(supervisor.status("app.dies").unwrap().last_exit.unwrap().code, Some(5));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn a_closed_control_connection_ends_the_process_and_its_channels() {
    let bun = bun_or_skip!();
    let home = temp_home();
    // The fixture keeps running after it closes its control connection.
    let fixture = json!({ "closeControlAfterAttachMs": 300, "ignoreTerm": true });
    let supervisor = supervisor_with(home.path(), bun, &[("app.lease", fixture)], |_| {}).await;
    let mut channel = supervisor.open_channel(offer("app.lease")).await.unwrap();
    let pid = channel.info.pid;
    read_frame(&mut channel.reader).await.unwrap();
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(2), channel.reader.read_to_end(&mut rest)).await;
    assert!(matches!(read, Ok(Ok(0))), "{read:?}");
    assert!(eventually(Duration::from_secs(3), || !process_exists(pid)).await);
    assert!(logged(&supervisor, "app.lease", "the daemon closed its control connection"));
    assert!(logged(&supervisor, "app.lease", "sending SIGKILL"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn logs_are_captured_in_order() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.chatty", json!({ "logLines": 1000 }))], |_| {}).await;
    let mut live = supervisor.subscribe_logs();
    let _channel = supervisor.open_channel(offer("app.chatty")).await.unwrap();
    assert!(eventually(Duration::from_secs(3), || logged(&supervisor, "app.chatty", "line 999")).await);
    let lines: Vec<String> = supervisor
        .logs("app.chatty", None)
        .into_iter()
        .filter(|line| line.stream == LogStream::Stdout && line.text.starts_with("line "))
        .map(|line| line.text)
        .collect();
    assert_eq!(lines, (0..1000).map(|i| format!("line {i}")).collect::<Vec<_>>());
    let seqs: Vec<u64> = supervisor.logs("app.chatty", None).iter().map(|line| line.seq).collect();
    assert!(seqs.windows(2).all(|pair| pair[1] == pair[0] + 1));
    let first = live.recv().await.unwrap();
    assert_eq!((first.app.as_str(), first.stream), ("app.chatty", LogStream::Hub));
    assert!(first.text.starts_with("starting app.chatty 1.0.0"));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn start_stop_and_restart_by_the_owner() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.owner", json!({}))], |config| {
        config.timings.idle_override = Some(Duration::from_secs(30));
    })
    .await;
    let mut events = supervisor.subscribe();
    let running = supervisor.start("app.owner").await.unwrap();
    assert_eq!(running.state, DaemonState::Running);
    let pid = running.pid.unwrap();
    let restarted = supervisor.restart("app.owner").await.unwrap();
    assert_ne!(restarted.pid.unwrap(), pid);
    assert_eq!(restarted.restarts, 1);
    assert!(!process_exists(pid));
    supervisor.stop("app.owner").await;
    let stopped = supervisor.status("app.owner").unwrap();
    assert_eq!((stopped.state, stopped.pid, stopped.crashes), (DaemonState::Stopped, None, 0));

    let mut states = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let pocket_hub_daemons::DaemonEvent::State(status) = event {
            states.push(status.state);
        }
    }
    use DaemonState::*;
    assert_eq!(states, vec![Starting, Running, Stopping, Stopped, Starting, Running, Stopping, Stopped]);
    supervisor.shutdown().await;
}

#[tokio::test]
async fn an_install_over_an_idle_daemon_stops_it_and_the_next_start_runs_the_new_version() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[("app.up", json!({}))], |config| {
        config.timings.idle_override = Some(Duration::from_secs(30));
    })
    .await;
    // With a channel open, the old version keeps running.
    let channel = supervisor.open_channel(offer("app.up")).await.unwrap();
    let source = fixture_package(home.path(), "app.up", "2.0.0", json!({}));
    let report = supervisor.install(&source).await.unwrap();
    assert_eq!(report.previous.as_deref(), Some("1.0.0"));
    let status = supervisor.status("app.up").unwrap();
    assert_eq!((status.state, status.version.as_deref(), status.running_version.as_deref()), (DaemonState::Running, Some("2.0.0"), Some("1.0.0")));
    assert!(home.path().join("daemons/app.up/1.0.0").is_dir());
    drop(channel);
    assert!(eventually(Duration::from_secs(2), || supervisor.status("app.up").unwrap().channels == 0).await);

    // With none, an install stops it.
    let source = fixture_package(home.path(), "app.up", "3.0.0", json!({}));
    supervisor.install(&source).await.unwrap();
    assert_eq!(supervisor.status("app.up").unwrap().state, DaemonState::Stopped);
    // The versions no process runs are gone.
    assert!(eventually(Duration::from_secs(2), || !home.path().join("daemons/app.up/1.0.0").exists()).await);
    assert!(!home.path().join("daemons/app.up/2.0.0").exists());
    let channel = supervisor.open_channel(offer("app.up")).await.unwrap();
    assert_eq!(channel.info.version, "3.0.0");
    drop(channel);

    // Remove stops it first.
    assert!(supervisor.remove("app.up", false).await.unwrap());
    assert!(supervisor.status("app.up").is_none());
    assert_eq!(supervisor.open_channel(offer("app.up")).await.err(), Some(OpenError::NoDaemon));
    supervisor.shutdown().await;
}

#[tokio::test]
async fn shutdown_leaves_no_process_behind() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let daemons = [("app.parent", json!({ "spawnChild": true, "ignoreStop": true })), ("app.plain", json!({}))];
    let supervisor = supervisor_with(home.path(), bun, &daemons, |_| {}).await;
    let parent = supervisor.open_channel(offer("app.parent")).await.unwrap();
    let plain = supervisor.open_channel(offer("app.plain")).await.unwrap();
    let child: u32 = log_texts(&supervisor, "app.parent")
        .iter()
        .find_map(|line| line.strip_prefix("child pid ").map(|pid| pid.parse().unwrap()))
        .expect("the fixture's child");
    let pids = [parent.info.pid, plain.info.pid, child];
    assert!(pids.iter().all(|&pid| process_exists(pid)));
    let mut stop = parent.stop.clone();

    supervisor.shutdown().await;
    assert!(stop.is_requested());
    tokio::time::timeout(Duration::from_secs(1), stop.requested()).await.unwrap();
    assert!(eventually(Duration::from_secs(2), || pids.iter().all(|&pid| !process_exists(pid))).await, "{pids:?}");
    assert!(!home.path().join("d.sock").exists());
    assert!(matches!(supervisor.open_channel(offer("app.plain")).await, Err(OpenError::Busy(_))));
}

#[tokio::test]
async fn the_socket_is_private_and_a_live_one_is_not_taken() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let socket = home.path().join("d.sock");
    // A socket file nobody serves is replaced. It is made by a socket that
    // never listens: on macOS std creates a socket and sets close-on-exec in
    // two steps, so a listener bound here could be inherited by a Bun that
    // another test of this binary spawns in between, and would then still
    // accept a connection after it is dropped. A socket that never listened
    // accepts none, inherited or not.
    drop(std::os::unix::net::UnixDatagram::bind(&socket).unwrap());
    let supervisor = Supervisor::new(config(home.path(), bun.clone())).await.expect("a stale socket is replaced");
    let meta = std::fs::symlink_metadata(&socket).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    let second = Supervisor::new(config(home.path(), bun.clone())).await;
    assert!(matches!(second, Err(SupervisorError::SocketInUse(_))));
    // The refused start did not touch the live socket.
    assert!(socket.exists());
    supervisor.shutdown().await;

    std::fs::write(&socket, b"").unwrap();
    assert!(matches!(Supervisor::new(config(home.path(), bun)).await, Err(SupervisorError::NotASocket(_))));
}

#[tokio::test]
async fn a_connection_that_says_nothing_valid_is_closed() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let supervisor = supervisor_with(home.path(), bun, &[], |_| {}).await;
    // A hello with no process waiting for it.
    let mut stream = tokio::net::UnixStream::connect(supervisor.socket()).await.unwrap();
    let hello = br#"{"t":"daemon.hello","protocol":1,"token":"00000000000000000000000000000000","app":"x"}"#;
    stream.write_all(&frame(0x04, 0, 0, hello)).await.unwrap();
    let refused = read_frame(&mut stream).await.unwrap();
    assert_eq!(&refused[12..], br#"{"t":"daemon.refused","message":"unknown token"}"#);
    let mut rest = Vec::new();
    assert_eq!(stream.read_to_end(&mut rest).await.unwrap(), 0);

    // A first frame that is not a HANDSHAKE.
    let mut stream = tokio::net::UnixStream::connect(supervisor.socket()).await.unwrap();
    stream.write_all(&frame(0x12, 0, 0, b"{}")).await.unwrap();
    assert_eq!(stream.read_to_end(&mut rest).await.unwrap(), 0);
    supervisor.shutdown().await;
}

/// Descriptors this test holds open WITHOUT close-on-exec, at numbers no
/// runtime takes by itself: a file and a listening socket, as a link
/// connection or a download would be on macOS between its creation and its
/// close-on-exec flag.
fn leaky_descriptors(dir: &Path) -> (Vec<libc::c_int>, std::fs::File, std::net::TcpListener) {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::create(dir.join("leak.txt")).unwrap();
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let wanted = [(file.as_raw_fd(), 150), (socket.as_raw_fd(), 151)];
    for (fd, at) in wanted {
        // SAFETY: dup2 onto numbers this test owns; the copy has no close-on-exec.
        assert_eq!(unsafe { libc::dup2(fd, at) }, at);
        assert_eq!(unsafe { libc::fcntl(at, libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
    }
    (wanted.iter().map(|(_, at)| *at).collect(), file, socket)
}

#[tokio::test]
async fn a_daemon_inherits_no_descriptor_of_hubs() {
    let bun = bun_or_skip!();
    let home = temp_home();
    let (leaked, _file, _socket) = leaky_descriptors(home.path());
    let supervisor = supervisor_with(home.path(), bun, &[("app.fds", json!({ "listFds": true }))], |_| {}).await;
    let channel = supervisor.open_channel(offer("app.fds")).await.unwrap();
    let listing = || log_texts(&supervisor, "app.fds").into_iter().find_map(|line| line.strip_prefix("fds ").map(str::to_owned));
    assert!(eventually(Duration::from_secs(5), || listing().is_some()).await, "the fixture lists its descriptors");
    let line = listing().unwrap();
    let held: Vec<libc::c_int> = line.split(',').map(|fd| fd.parse().unwrap()).collect();
    assert!(held.starts_with(&[0, 1, 2]), "{held:?}");
    for fd in &leaked {
        assert!(!held.contains(fd), "the daemon holds descriptor {fd} of the test: {held:?}");
    }
    drop(channel);
    supervisor.shutdown().await;
    for fd in leaked {
        // SAFETY: closes this test's own copies.
        unsafe { libc::close(fd) };
    }
}
