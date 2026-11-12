//! §5.3 The four steps by which Hub reads an `EVENT`, the header id first.
//!
//! One test of the id's range decides "late" (step 3): the reader keeps no
//! mark and no per-id memory. The table of system events and the rule each
//! one's header id follows is §5.3's, as `tools/pocket-link/v2/requests.ts`
//! `SYSTEM_EVENTS` lists it.

use pocket_link::frame::{Side, id_range};

/// How a system event's header id is read (§5.3, step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The id is 0.
    Zero,
    /// An open request Hub sent and that has no `REPLY` yet.
    Request,
    /// A channel open on the connection.
    Channel,
    /// A fresh id of the device: the event opens a stream.
    Stream,
    /// `runtime.screenshot`: a request's progress, or a fresh id that opens an unsolicited stream.
    Screenshot,
}

pub fn system_rule(t: &str) -> Option<Rule> {
    Some(match t {
        "features" | "catalog.changed" | "runtime.status" | "runtime.apps" | "log" | "devtools.listen" | "agent.hello" | "agent.turn" | "agent.audio.end" | "agent.cancel" | "agent.reset"
        | "agent.result" | "agent.state" | "agent.transcript" | "agent.request" | "agent.delta" | "agent.step" | "agent.tool" | "agent.usage" | "agent.done" | "agent.error" => Rule::Zero,
        "runtime.install" | "runtime.native" | "install.head" | "net.head" => Rule::Request,
        "channel.close" => Rule::Channel,
        "agent.audio" | "agent.shot" | "agent.screen" | "agent.glyphs" => Rule::Stream,
        "runtime.screenshot" => Rule::Screenshot,
        _ => return None,
    })
}

/// What Hub holds, asked by header id.
pub trait Holdings {
    /// An open request an `APP` frame started, in either direction, with no `REPLY` yet.
    fn channel_request(&self, id: u32) -> bool;
    /// An open request Hub sent (system or forwarded for a daemon) with no `REPLY` yet.
    fn own_request(&self, id: u32) -> bool;
    /// A channel open on the connection.
    fn channel(&self, id: u32) -> bool;
    /// Any id in use on the connection: a request, a stream or a channel.
    fn in_use(&self, id: u32) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// Step 1: progress of an applet channel's request, whatever its `t`.
    AppProgress,
    /// Step 2: an `APP` event; `opens_stream` when its id is a fresh id of the device.
    App { opens_stream: bool },
    /// Step 3, or a late frame of step 4: ignored.
    Ignore,
    /// Step 4: a system event to act on; `opens_stream` for a fresh id.
    System { rule: Rule, opens_stream: bool },
    /// An unknown `t` on a fresh id of the device: ignored, with `RESET refused`.
    ResetRefused,
    /// The id breaks the event's rule: `CLOSE protocol`.
    Close(&'static str),
}

/// Reads an event that came from the device. `t` is the object's `t`, or
/// None when the payload is not a JSON object with a string `t`.
pub fn read_event(id: u32, app: bool, t: Option<&str>, holds: &impl Holdings) -> Reading {
    let range = id_range(id);
    if id != 0 && range.is_none() {
        return Reading::Close("EVENT id 0x80000000");
    }
    // Step 1: an open request of an applet channel.
    if id != 0 && holds.channel_request(id) {
        return Reading::AppProgress;
    }
    // Step 2: the APP flag.
    if app {
        if id == 0 {
            return Reading::App { opens_stream: false };
        }
        if range != Some(Side::Device) || holds.in_use(id) {
            return Reading::Close("an APP event's stream id is not a fresh id of the device");
        }
        return Reading::App { opens_stream: true };
    }
    // Step 3: a non-zero id of Hub's own range that names nothing held.
    if id != 0 && range == Some(Side::Hub) && !holds.in_use(id) {
        return Reading::Ignore;
    }
    // Step 4: a system event; its `t` defines what the id means.
    let fresh = id != 0 && range == Some(Side::Device) && !holds.in_use(id);
    let Some(rule) = t.and_then(system_rule) else {
        return if fresh { Reading::ResetRefused } else { Reading::Ignore };
    };
    let system = |opens_stream| Reading::System { rule, opens_stream };
    match rule {
        Rule::Zero => {
            if id == 0 {
                system(false)
            } else {
                Reading::Close("a system event that requires id 0 carries another")
            }
        }
        Rule::Request => {
            if holds.own_request(id) {
                system(false)
            } else {
                Reading::Close("a progress event names no open request of Hub")
            }
        }
        Rule::Channel => {
            if holds.channel(id) {
                system(false)
            } else if id == 0 {
                Reading::Close("channel.close with id 0")
            } else if fresh {
                // A close that crossed Hub's own: the channel is already closed.
                Reading::Ignore
            } else {
                Reading::Close("channel.close on an id that is not a channel")
            }
        }
        Rule::Stream => {
            if fresh {
                system(true)
            } else {
                Reading::Close("a stream-opening event's id is 0, outside the device's range, or in use")
            }
        }
        Rule::Screenshot => {
            if holds.own_request(id) {
                system(false)
            } else if fresh {
                system(true)
            } else {
                Reading::Close("runtime.screenshot: bad id")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Held {
        channel_requests: Vec<u32>,
        own: Vec<u32>,
        channels: Vec<u32>,
        used: Vec<u32>,
    }
    impl Holdings for Held {
        fn channel_request(&self, id: u32) -> bool {
            self.channel_requests.contains(&id)
        }
        fn own_request(&self, id: u32) -> bool {
            self.own.contains(&id)
        }
        fn channel(&self, id: u32) -> bool {
            self.channels.contains(&id)
        }
        fn in_use(&self, id: u32) -> bool {
            self.used.contains(&id) || self.own.contains(&id) || self.channels.contains(&id) || self.channel_requests.contains(&id)
        }
    }

    #[test]
    fn the_four_steps() {
        let held = Held { channel_requests: vec![0x8000_0002], own: vec![0x8000_0001], channels: vec![3], used: vec![5] };
        // A progress event named `log` on an applet request is the applet's.
        assert_eq!(read_event(0x8000_0002, false, Some("log"), &held), Reading::AppProgress);
        assert_eq!(read_event(0, true, Some("x"), &held), Reading::App { opens_stream: false });
        assert_eq!(read_event(9, true, Some("x"), &held), Reading::App { opens_stream: true });
        assert!(matches!(read_event(5, true, Some("x"), &held), Reading::Close(_)));
        assert!(matches!(read_event(0x8000_0009, true, Some("x"), &held), Reading::Close(_)));
        // Late: an id of Hub's range that names nothing, whatever its `t`.
        assert_eq!(read_event(0x8000_0009, false, Some("features"), &held), Reading::Ignore);
        assert!(matches!(read_event(0, false, Some("runtime.status"), &held), Reading::System { rule: Rule::Zero, .. }));
        assert!(matches!(read_event(7, false, Some("runtime.status"), &held), Reading::Close(_)));
        assert!(matches!(read_event(0x8000_0001, false, Some("runtime.install"), &held), Reading::System { rule: Rule::Request, .. }));
        assert!(matches!(read_event(0, false, Some("runtime.install"), &held), Reading::Close(_)));
        assert!(matches!(read_event(0x8000_0001, false, Some("runtime.screenshot"), &held), Reading::System { opens_stream: false, .. }));
        assert!(matches!(read_event(11, false, Some("runtime.screenshot"), &held), Reading::System { opens_stream: true, .. }));
        assert!(matches!(read_event(5, false, Some("runtime.screenshot"), &held), Reading::Close(_)));
        assert!(matches!(read_event(3, false, Some("channel.close"), &held), Reading::System { rule: Rule::Channel, .. }));
        assert_eq!(read_event(13, false, Some("channel.close"), &held), Reading::Ignore);
        assert!(matches!(read_event(0, false, Some("channel.close"), &held), Reading::Close(_)));
        assert_eq!(read_event(15, false, Some("nope"), &held), Reading::ResetRefused);
        assert_eq!(read_event(0, false, None, &held), Reading::Ignore);
        assert!(matches!(read_event(0x8000_0000, false, Some("log"), &held), Reading::Close(_)));
    }
}
