//! The endpoint's settings and its timers.
//!
//! Every timer of the specification is a field of `Timing`, so a test can run
//! the 30 s and 10 s rules in a fraction of a second against a real peer
//! instead of on a paused clock (a paused tokio clock would also jump the
//! timers of the live sockets the tests use).

use std::time::Duration;

use pocket_link::consts::*;

/// The timers of docs/POCKET-LINK.md, at the values of §9 by default.
#[derive(Debug, Clone)]
pub struct Timing {
    /// §2.4, §3.3: Hub closes an unfinished handshake after this long.
    pub handshake: Duration,
    /// §3.11: Hub closes a device's request to be paired this long after its
    /// `pair`, in place of `handshake`: the time the device keeps its code.
    pub pair_window: Duration,
    /// §4.6: `CLOSE silence` after this long without a frame of any type.
    pub silence: Duration,
    /// The silence limit in place of `silence` while a state-changing request
    /// of Hub is open, and for a while after a `switching` reply (`conn`'s
    /// "patient window"). **Hub's own deviation from §4.6**: a 3DS running a
    /// Pocket Shell from before PocketJS #690 stops sending and reading for
    /// more than 10 s while it creates the folders of a new applet's slot on
    /// its SD card (measured: three folders of about 4.2 s each).
    pub state_silence: Duration,
    /// How often Hub sends its own `PING` during that window, so a device
    /// that resumes finds a frame waiting before it checks its own silence.
    pub state_ping: Duration,
    /// §4.8: the side that sent `CLOSE` closes its socket this long after at the latest.
    pub close_drain: Duration,
    /// §5.5: a request fails with `timeout` after this long without a frame for its id.
    pub request_stall: Duration,
    /// §5.4: a cancelled request is freed this long after its `CANCEL` without a reply.
    pub cancel_grace: Duration,
    /// §2.4: a replaced connection has this long to answer Hub's `PING`.
    pub replaced_ping: Duration,
    /// §2.4: the window in which 3 counted replacements make a duplicate id.
    pub replaced_window: Duration,
    /// §6.2, §8.3: no state-changing request this long after a `launching` reply.
    pub launch_hold: Duration,
    /// §7.1: LINK_CHANNEL_OPEN_MS, Hub's longest wait before it answers `channel.open`.
    pub channel_open: Duration,
    /// §8.3, §11: how long a second local caller waits for the device mutex
    /// before it gets `busy` naming the holder.
    pub mutex_wait: Duration,
    /// How long a device request's handler keeps running after its cancel
    /// signal (a `CANCEL`, the link's end) before Hub drops its future.
    /// Hub has answered the device by then; this bounds what is left running.
    pub handler_wind_down: Duration,
    /// How often a connection runs its timed rules. Every rule above is a
    /// multiple of it, so its granularity is the error of each timer.
    pub tick: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            handshake: Duration::from_millis(HANDSHAKE_TIMEOUT_MS),
            pair_window: Duration::from_millis(PAIR_WINDOW_MS),
            silence: Duration::from_millis(SILENCE_MS),
            state_silence: Duration::from_millis(REQUEST_STALL_MS),
            state_ping: Duration::from_millis(PING_MS),
            close_drain: Duration::from_millis(CLOSE_DRAIN_MS),
            request_stall: Duration::from_millis(REQUEST_STALL_MS),
            cancel_grace: Duration::from_millis(CANCEL_GRACE_MS),
            replaced_ping: Duration::from_millis(REPLACED_PING_MS),
            replaced_window: Duration::from_millis(REPLACED_WINDOW_MS),
            launch_hold: Duration::from_millis(LAUNCH_HOLD_MS),
            channel_open: Duration::from_millis(CHANNEL_OPEN_MS),
            mutex_wait: Duration::from_secs(30),
            handler_wind_down: Duration::from_secs(1),
            tick: Duration::from_millis(100),
        }
    }
}

/// The endpoint's settings.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    pub timing: Timing,
    /// §2.4, §9: connections Hub holds at a time; the next `hello` is refused `busy`.
    pub max_connections: usize,
    /// Local callers that may wait in the device mutex's queue at once; one
    /// more is answered `busy` at once (§11 recommends a bounded FIFO wait).
    pub mutex_queue: usize,
    /// Send beacons (§2.2). Off in tests.
    pub beacons: bool,
    /// The UDP port beacons go to; `None` for the link's 8627.
    pub beacon_port: Option<u16>,
    /// Keep a device's unsolicited screenshot as its latest (§6.2); when off,
    /// Hub answers the stream with `RESET refused`.
    pub keep_screenshots: bool,
}

impl Default for EndpointConfig {
    fn default() -> Self {
        Self { timing: Timing::default(), max_connections: MAX_CONNECTIONS, mutex_queue: 8, beacons: true, beacon_port: None, keep_screenshots: true }
    }
}
