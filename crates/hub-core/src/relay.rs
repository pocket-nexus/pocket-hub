//! §7 Applet channels, relayed to services (docs/SERVICE-PROTOCOL.md §5).
//!
//! Each accepted channel is one local connection to the applet's service, in
//! the local dialect: the connection is the channel, so no frame carries the
//! `APP` flag or a channel id. The relay adds and removes them and maps ids:
//!
//! - **the applet's ids pass through** unchanged: they are of the device's
//!   range on both connections;
//! - **the service's ids are of Hub's range on its own connection**, from a
//!   counter of its own; Hub gives each request and each event-opened stream
//!   of the service an id from the link's counter and maps both ways.
//!
//! Credit is the service's and the applet's, passed through: a `CREDIT` from
//! one side goes to the other. **Hub verifies both sides against what it
//! passed**: device bytes past the service's credit close the link with
//! `flow` (§5.6), a device event past the service's units closes the channel
//! with `overflow`; a service that writes past the applet's credit or units,
//! or sends a malformed frame, loses its channel (`overflow` to it, `daemon`,
//! the link's reason, to the device). Where Hub itself drops an event that
//! its sender spent units on (an object over LINK_APP_MSG, late progress,
//! progress for a cancelled request), it gives the units back to that sender
//! at once: the receiver never saw the event and would never regrant them
//! (§5.6, "for any reason").
//!
//! Hub reads no applet JSON beyond the check that it is one object with a
//! string `t` (§7.2).

use std::collections::HashMap;

use bytes::Bytes;
use pocket_link::consts::*;
use pocket_link::frame::{self, Dialect, FLAG_BODY, FLAG_END, FLAG_ERROR, Frame, FrameDecoder, FrameType, JsonFrame, Side, id_range};
use pocket_link::text::encoded_prefix;
use pocket_link::{LinkClose, ResetCode};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::api::{ChannelInfo, ChannelStream, HubEvent};
use crate::conn::{Conn, ConnMsg, InRequest, Input, OutKind, OutRequest, RecvSink, RecvStream, SendStream};

/// The `channel.close` reason a device is sent when the channel's service
/// ended, broke a rule or is being stopped: "the channel's daemon ended or
/// failed to start" (link specification §7.5). It is the wire's name, which
/// says daemon until the specification renames it and this repository moves
/// its pin.
pub(crate) const CLOSE_BY_SERVICE: &str = "daemon";

/// One open channel at Hub.
pub(crate) struct Channel {
    /// Unique on the connection, so a service's frames of a closed channel
    /// never reach a newer one.
    pub key: u64,
    /// The channel id on the wire, which is also its id on the service's connection.
    pub id: u32,
    pub app: String,
    pub hash: String,
    pub generation: u32,
    /// Epoch milliseconds of `channel.ready`.
    pub opened_at: i64,
    /// LINK_APP_MSG of the link.
    pub msg_limit: usize,
    service: Option<mpsc::UnboundedSender<Bytes>>,
    reader: JoinHandle<()>,
    /// Units the service granted for the device's events, not yet spent.
    device_units: u64,
    /// Units the device granted for the service's events, not yet spent.
    service_units: u64,
    /// The service's ids (Hub's range on its connection) to the link's, and back.
    to_wire: HashMap<u32, u32>,
    to_local: HashMap<u32, u32>,
}

impl Channel {
    pub fn info(&self) -> ChannelInfo {
        ChannelInfo { channel: self.id, app: self.app.clone(), hash: self.hash.clone(), generation: self.generation, since: self.opened_at }
    }

    /// The service's id for a link id.
    fn local(&self, wire: u32) -> Option<u32> {
        match id_range(wire) {
            Some(Side::Device) => Some(wire),
            Some(Side::Hub) => self.to_local.get(&wire).copied(),
            None => None,
        }
    }

    /// The link's id for a service id.
    fn wire(&self, local: u32) -> Option<u32> {
        match id_range(local) {
            Some(Side::Device) => Some(local),
            Some(Side::Hub) => self.to_wire.get(&local).copied(),
            None => None,
        }
    }

    pub fn unmap(&mut self, wire: u32) {
        if let Some(local) = self.to_local.remove(&wire) {
            self.to_wire.remove(&local);
        }
    }

    /// One frame toward the service, in the local dialect.
    fn send(&self, ty: FrameType, flags: u8, id: u32, payload: &[u8]) {
        let limit = if ty == FrameType::Handshake { MAX_HANDSHAKE } else { u32::MAX };
        match frame::encode_in(Dialect::Local, ty, flags, id, payload, limit) {
            Ok(bytes) => {
                if let Some(service) = &self.service {
                    let _ = service.send(bytes);
                }
            }
            Err(error) => tracing::warn!(app = %self.app, "a {ty:?} frame to the service was not sent: {error}"),
        }
    }

    fn send_json(&self, ty: FrameType, flags: u8, id: u32, object: &serde_json::Value) {
        self.send(ty, flags, id, &serde_json::to_vec(object).expect("JSON"));
    }
}

/// Units an applet event costs: one per started 512 bytes of its JSON (§5.6).
pub(crate) fn units(json_bytes: usize) -> u64 {
    json_bytes.div_ceil(APP_EVENT_UNIT).max(1) as u64
}

/// A JSON object with a string `t`, read without building the object: the
/// one check Hub makes of applet JSON.
fn has_t(raw: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Named<'a> {
        #[serde(borrow)]
        #[allow(dead_code)]
        t: std::borrow::Cow<'a, str>,
    }
    serde_json::from_slice::<Named>(raw).is_ok()
}

impl Conn {
    fn channel_by_key(&self, key: u64) -> Option<&Channel> {
        self.channels.values().find(|c| c.key == key)
    }
    fn channel_by_key_mut(&mut self, key: u64) -> Option<&mut Channel> {
        self.channels.values_mut().find(|c| c.key == key)
    }
    fn inbound_streams(&self, key: u64) -> usize {
        self.recv.values().filter(|s| matches!(s.sink, RecvSink::Relay { key: k } if k == key)).count()
    }
    fn outbound_streams(&self, key: u64) -> usize {
        self.send.values().filter(|s| s.channel == Some(key) && s.counted).count()
    }

    /// Remembers a released service request for its late events.
    pub(crate) fn released(&mut self, wire: u32, key: u64) {
        const RELEASED_KEPT: usize = 256;
        self.released.insert(wire, key);
        self.released_order.push_back(wire);
        while self.released_order.len() > RELEASED_KEPT {
            if let Some(old) = self.released_order.pop_front() {
                self.released.remove(&old);
            }
        }
    }

    /// A device event Hub drops on a released id: counted, then given back.
    pub(crate) fn late_device_event(&mut self, key: u64, json_bytes: usize) {
        let cost = units(json_bytes);
        let Some(channel) = self.channel_by_key_mut(key) else { return };
        channel.device_units = channel.device_units.saturating_sub(cost);
        self.regrant_device(key, cost);
    }

    // --- open and close ------------------------------------------------------

    pub(crate) fn channel_accepted(&mut self, id: u32, app: String, hash: String, generation: u32, stream: ChannelStream) {
        let key = self.next_key;
        self.next_key += 1;
        // The REPLY opens the channel; no frame of it goes before (§7.1). The
        // service's first grant of units can only follow `channel.ready`, which
        // is written below, so it reaches the wire behind this REPLY (§5.6).
        self.send_json(FrameType::Reply, id, &json!({"generation": generation}), JsonFrame::default());
        let msg_limit = frame::app_message_limit(self.limit) as usize;
        let (service_tx, mut service_rx) = mpsc::unbounded_channel::<Bytes>();
        let ChannelStream { mut reader, mut writer, stop } = stream;
        if let Some(stop) = stop {
            let input = self.tx.clone();
            let stopped = tokio::spawn(async move {
                stop.await;
                let _ = input.send(Input::Msg(ConnMsg::ServiceStop { key }));
            });
            self.stop_watchers.insert(key, stopped);
        }
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            while let Some(bytes) = service_rx.recv().await {
                if writer.write_all(&bytes).await.is_err() {
                    break;
                }
                if service_rx.is_empty() && writer.flush().await.is_err() {
                    break;
                }
            }
            let _ = writer.shutdown().await;
        });
        let input = self.tx.clone();
        let limit = self.limit;
        let reader = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut decoder = FrameDecoder::local(limit);
            let mut buffer = vec![0u8; 64 * 1024];
            loop {
                let n = match reader.read(&mut buffer).await {
                    Ok(0) | Err(_) => {
                        let _ = input.send(Input::Msg(ConnMsg::Service { key, frame: None }));
                        return;
                    }
                    Ok(n) => n,
                };
                decoder.push(&buffer[..n]);
                loop {
                    match decoder.next() {
                        Ok(None) => break,
                        Ok(Some(frame::Decoded::Skipped)) => {}
                        Ok(Some(frame::Decoded::Frame(frame))) => {
                            if input.send(Input::Msg(ConnMsg::Service { key, frame: Some(Ok(frame)) })).is_err() {
                                return;
                            }
                        }
                        Err(close) => {
                            let _ = input.send(Input::Msg(ConnMsg::Service { key, frame: Some(Err(close)) }));
                            return;
                        }
                    }
                }
            }
        });
        let channel = Channel {
            key,
            id,
            app,
            hash,
            generation,
            opened_at: chrono::Utc::now().timestamp_millis(),
            msg_limit,
            service: Some(service_tx),
            reader,
            device_units: 0,
            service_units: 0,
            to_wire: HashMap::new(),
            to_local: HashMap::new(),
        };
        channel.send_json(FrameType::Handshake, 0, 0, &json!({"t": "channel.ready", "channel": id, "messageLimit": msg_limit}));
        let info = channel.info();
        self.channels.insert(id, channel);
        self.publish_channels();
        self.shared.publish(HubEvent::ChannelOpened { device: self.device.clone(), channel: info });
    }

    /// Ends a channel (§7.5). `to_device` is the reason sent on the link (None
    /// when the device closed it or the link is gone); `to_service` is the one
    /// close notice the service gets (None when its connection is already gone).
    /// Bytes not yet in a queued DATA frame are dropped, and nothing of the
    /// channel is sent or delivered afterwards.
    pub(crate) fn close_channel(&mut self, id: u32, to_device: Option<&str>, to_service: Option<&str>) {
        let Some(mut channel) = self.channels.remove(&id) else { return };
        let key = channel.key;
        self.inr.retain(|_, r| !matches!(r, InRequest::Applet { key: k, .. } if *k == key));
        self.out.retain(|_, r| !matches!(r.kind, OutKind::Channel { key: k, .. } if k == key));
        self.send.retain(|_, s| s.channel != Some(key));
        self.recv.retain(|_, s| !matches!(s.sink, RecvSink::Relay { key: k } if k == key));
        if let Some(reason) = to_device {
            self.send_json(FrameType::Event, id, &json!({"t": "channel.close", "reason": reason}), JsonFrame::default());
        }
        if let Some(reason) = to_service {
            channel.send_json(FrameType::Event, 0, id, &json!({"t": "channel.close", "reason": reason}));
        }
        // The writer sends what is queued, the notice last, and closes the socket.
        channel.service = None;
        channel.reader.abort();
        if let Some(watcher) = self.stop_watchers.remove(&key) {
            watcher.abort();
        }
        let released = &mut self.released;
        self.released_order.retain(|wire| released.get(wire) != Some(&key));
        released.retain(|_, k| *k != key);
        self.publish_channels();
        let reason = to_service.or(to_device).unwrap_or(CLOSE_BY_SERVICE).to_owned();
        self.shared.publish(HubEvent::ChannelClosed { device: self.device.clone(), channel: channel.info(), reason });
    }

    pub(crate) fn service_reply_error(&mut self, key: u64, local: u32, code: &str, message: &str) {
        if let Some(channel) = self.channel_by_key(key) {
            channel.send_json(FrameType::Reply, FLAG_ERROR, local, &json!({"code": code, "message": encoded_prefix(message, ERROR_MESSAGE_BYTES)}));
        }
    }

    /// Gives an event's units back to the device: Hub dropped the event, so
    /// the service never sees it and never regrants them.
    fn regrant_device(&mut self, key: u64, cost: u64) {
        let Some(channel) = self.channel_by_key_mut(key) else { return };
        channel.device_units += cost;
        let id = channel.id;
        self.send_credit(id, cost);
    }

    /// Gives an event's units back to the service: Hub dropped its event.
    fn regrant_service(&mut self, key: u64, cost: u64) {
        let Some(channel) = self.channel_by_key_mut(key) else { return };
        channel.service_units += cost;
        channel.send(FrameType::Credit, 0, channel.id, &(cost.min(u32::MAX as u64) as u32).to_le_bytes());
    }

    // --- frames of the device ------------------------------------------------

    /// A REQUEST with APP: the applet asks its service.
    pub(crate) fn applet_request(&mut self, frame: Frame) -> Result<(), LinkClose> {
        let id = frame.id;
        let (channel_id, raw) = frame::split_app(&frame);
        let Some(channel) = channel_id.and_then(|c| self.channels.get(&c)) else {
            self.reply_error(id, "no_channel", "the channel is not open");
            return Ok(());
        };
        let key = channel.key;
        if raw.len() > channel.msg_limit {
            self.reply_error(id, "too_large", "the object is over LINK_APP_MSG");
            return Ok(());
        }
        if !has_t(&raw) {
            self.reply_error(id, "malformed", "not a JSON object with a string t");
            return Ok(());
        }
        let open = self.inr.values().filter(|r| matches!(r, InRequest::Applet { key: k, .. } if *k == key)).count();
        if open >= CHANNEL_REQUESTS {
            self.reply_error(id, "busy", "8 requests are open on the channel");
            return Ok(());
        }
        let body = frame.flags & FLAG_BODY != 0;
        self.inr.insert(id, InRequest::Applet { key, head: false, refused: false });
        let full = body && self.inbound_streams(key) >= CHANNEL_STREAMS;
        let channel = self.channels.get(&channel_id.unwrap()).expect("checked");
        channel.send(FrameType::Request, if body { FLAG_BODY } else { 0 }, id, &raw);
        if full {
            // §7.6: the fifth stream is refused; the request still ends with its REPLY.
            channel.send(FrameType::Reset, 0, id, &[ResetCode::Refused as u8, 0, 0, 0]);
            self.send_reset(id, ResetCode::Refused);
        } else if body {
            self.recv.insert(id, RecvStream { outstanding: 0, received: 0, sink: RecvSink::Relay { key } });
        }
        Ok(())
    }

    /// The applet cancelled one of its requests: Hub answers it at once and
    /// tells the service, whose later frames for the id are dropped (§5.4).
    pub(crate) fn applet_cancel(&mut self, id: u32) {
        let Some(InRequest::Applet { key, .. }) = self.inr.remove(&id) else { return };
        self.reply_error(id, "cancelled", "cancelled");
        self.send.remove(&id);
        self.recv.remove(&id);
        if let Some(channel) = self.channel_by_key(key) {
            channel.send(FrameType::Cancel, 0, id, &[]);
        }
    }

    /// Step 1 of §5.3: an event on an open request of a channel. From the
    /// device that is the progress of a service's request.
    pub(crate) fn device_progress(&mut self, id: u32, raw: Bytes, named: bool) {
        let Some(OutRequest { kind: OutKind::Channel { key, local, .. }, .. }) = self.out.get(&id) else {
            // Progress on the device's own request: it is the requester there. Ignored.
            return;
        };
        let (key, local) = (*key, *local);
        let Some(channel) = self.channel_by_key_mut(key) else { return };
        let cost = units(raw.len());
        if cost > channel.device_units {
            let channel_id = channel.id;
            self.close_channel(channel_id, Some("overflow"), Some("overflow"));
            return;
        }
        channel.device_units -= cost;
        let msg_limit = channel.msg_limit;
        let request = self.out.get_mut(&id).expect("held");
        request.last_frame = Instant::now();
        let discard = request.cancelled_at.is_some() || matches!(request.kind, OutKind::Channel { answered: true, .. });
        if discard || !named || raw.len() > msg_limit {
            // A request already cancelled, or no name, or over LINK_APP_MSG: dropped, units returned.
            self.regrant_device(key, cost);
            return;
        }
        // The head step: the response stream starts at the first progress event.
        if let OutKind::Channel { early_credit, .. } = &mut request.kind
            && !self.recv.contains_key(&id)
            && *early_credit != u64::MAX
        {
            let credit = *early_credit;
            *early_credit = u64::MAX;
            if self.inbound_streams(key) >= CHANNEL_STREAMS {
                self.send_reset(id, ResetCode::Refused);
                if let Some(channel) = self.channel_by_key(key) {
                    channel.send(FrameType::Reset, 0, local, &[ResetCode::Refused as u8, 0, 0, 0]);
                }
            } else {
                self.recv.insert(id, RecvStream { outstanding: credit, received: 0, sink: RecvSink::Relay { key } });
            }
        }
        if let Some(channel) = self.channel_by_key(key) {
            channel.send(FrameType::Event, 0, local, &raw);
        }
    }

    /// Step 2 of §5.3: an EVENT with APP.
    pub(crate) fn applet_event(&mut self, channel_id: u32, id: u32, raw: Bytes, named: bool, opens_stream: bool) {
        let Some(channel) = self.channels.get_mut(&channel_id) else {
            if opens_stream {
                self.send_reset(id, ResetCode::Refused);
            }
            return;
        };
        let key = channel.key;
        let cost = units(raw.len());
        if cost > channel.device_units {
            self.close_channel(channel_id, Some("overflow"), Some("overflow"));
            return;
        }
        channel.device_units -= cost;
        if !named || raw.len() > channel.msg_limit {
            self.regrant_device(key, cost);
            if opens_stream {
                self.send_reset(id, ResetCode::Refused);
            }
            return;
        }
        let mut stream_id = 0;
        if opens_stream {
            if self.inbound_streams(key) >= CHANNEL_STREAMS {
                self.send_reset(id, ResetCode::Refused);
            } else {
                stream_id = id;
                self.recv.insert(id, RecvStream { outstanding: 0, received: 0, sink: RecvSink::Relay { key } });
            }
        }
        if let Some(channel) = self.channels.get(&channel_id) {
            channel.send(FrameType::Event, 0, stream_id, &raw);
        }
    }

    /// Device bytes toward the service, already checked against its credit.
    pub(crate) fn relay_data(&mut self, key: u64, id: u32, payload: Bytes, end: bool) {
        let Some(channel) = self.channel_by_key(key) else { return };
        let Some(local) = channel.local(id) else { return };
        channel.send(FrameType::Data, if end { FLAG_END } else { 0 }, local, &payload);
    }

    /// A CREDIT on the channel id from the device: units for the service's events.
    pub(crate) fn units_from_device(&mut self, key: u64, amount: u64) {
        let Some(channel) = self.channel_by_key_mut(key) else { return };
        channel.service_units += amount;
        channel.send(FrameType::Credit, 0, channel.id, &(amount as u32).to_le_bytes());
    }

    pub(crate) fn relay_credit_to_service(&mut self, key: u64, wire: u32, amount: u64) {
        let Some(channel) = self.channel_by_key(key) else { return };
        if let Some(local) = channel.local(wire) {
            channel.send(FrameType::Credit, 0, local, &(amount as u32).to_le_bytes());
        }
    }

    pub(crate) fn relay_reset_to_service(&mut self, key: u64, wire: u32, code: ResetCode) {
        self.send.remove(&wire);
        self.recv.remove(&wire);
        let is_request = self.out.contains_key(&wire);
        let Some(channel) = self.channel_by_key_mut(key) else { return };
        if let Some(local) = channel.local(wire) {
            channel.send(FrameType::Reset, 0, local, &[code as u8, 0, 0, 0]);
        }
        if !is_request {
            channel.unmap(wire);
        }
    }

    /// The device's REPLY to a service's request.
    pub(crate) fn channel_reply(&mut self, frame: Frame) {
        let id = frame.id;
        let Some(request) = self.out.remove(&id) else { return };
        let OutKind::Channel { key, local, answered, .. } = request.kind else { return };
        self.send.remove(&id);
        self.recv.remove(&id);
        self.released(id, key);
        let Some(channel) = self.channel_by_key_mut(key) else { return };
        channel.unmap(id);
        if answered {
            // Hub already gave the service its outcome (`timeout`).
            return;
        }
        if frame.payload.len() > channel.msg_limit {
            // §4.7: the requester's endpoint keeps none of it and reports `too_large`.
            channel.send_json(FrameType::Reply, FLAG_ERROR, local, &json!({"code": "too_large", "message": "the reply is over LINK_APP_MSG"}));
            return;
        }
        channel.send(FrameType::Reply, frame.flags & FLAG_ERROR, local, &frame.payload);
    }

    // --- frames of the service ------------------------------------------------

    pub(crate) fn service_frame(&mut self, key: u64, frame: Option<Result<Frame, LinkClose>>) {
        let Some(channel_id) = self.channel_by_key(key).map(|c| c.id) else { return };
        let frame = match frame {
            None => return self.close_channel(channel_id, Some(CLOSE_BY_SERVICE), None),
            Some(Err(close)) => {
                tracing::warn!(device = %self.device, "a service's frame broke a rule: {close}");
                return self.close_channel(channel_id, Some(CLOSE_BY_SERVICE), Some("overflow"));
            }
            Some(Ok(frame)) => frame,
        };
        let outcome = match frame.ty {
            FrameType::Request => self.service_request(key, frame),
            FrameType::Reply => self.service_reply(key, frame),
            FrameType::Event => self.service_event(key, frame),
            FrameType::Cancel => self.service_cancel(key, frame.id),
            FrameType::Data => self.service_data(key, frame),
            FrameType::Credit => self.service_credit(key, frame.id, frame::parse_credit(&frame.payload) as u64),
            FrameType::Reset => self.service_reset(key, frame.id, frame::parse_reset(&frame.payload)),
            // The opening exchange is over; nothing else has a meaning here.
            FrameType::Handshake | FrameType::Ping | FrameType::Pong | FrameType::Close => Ok(()),
        };
        if let Err(why) = outcome {
            tracing::warn!(device = %self.device, "a service broke a channel rule: {why}");
            self.close_channel(channel_id, Some(CLOSE_BY_SERVICE), Some("overflow"));
        }
    }

    fn service_request(&mut self, key: u64, frame: Frame) -> Result<(), &'static str> {
        let local = frame.id;
        let channel = self.channel_by_key(key).expect("open");
        if id_range(local) != Some(Side::Hub) || channel.to_wire.contains_key(&local) {
            return Err("a REQUEST id is not a fresh id of Hub's range");
        }
        if frame.payload.len() > channel.msg_limit {
            self.service_reply_error(key, local, "too_large", "the object is over the channel's messageLimit");
            return Ok(());
        }
        if !has_t(&frame.payload) {
            self.service_reply_error(key, local, "malformed", "not a JSON object with a string t");
            return Ok(());
        }
        let open = self.out.values().filter(|r| matches!(r.kind, OutKind::Channel { key: k, answered: false, .. } if k == key)).count();
        if open >= CHANNEL_REQUESTS {
            self.service_reply_error(key, local, "busy", "8 requests of the service are open");
            return Ok(());
        }
        let wire = match self.take_id() {
            Ok(id) => id,
            Err(_) => return Err("the link's ids are exhausted"),
        };
        let body = frame.flags & FLAG_BODY != 0;
        let full = body && self.outbound_streams(key) >= CHANNEL_STREAMS;
        let channel = self.channel_by_key_mut(key).expect("open");
        channel.to_wire.insert(local, wire);
        channel.to_local.insert(wire, local);
        let channel_id = channel.id;
        match frame::encode_json_bytes(FrameType::Request, wire, &frame.payload, JsonFrame { channel: Some(channel_id), body, error: false }, self.limit) {
            Ok(bytes) => self.enqueue(bytes),
            Err(_) => return Err("an object over the limit passed the check"),
        }
        self.out.insert(
            wire,
            OutRequest { group: "channel", last_frame: Instant::now(), cancelled_at: None, cause: "cancelled", kind: OutKind::Channel { key, local, early_credit: 0, answered: false } },
        );
        if full {
            self.send_reset(wire, ResetCode::Refused);
            if let Some(channel) = self.channel_by_key(key) {
                channel.send(FrameType::Reset, 0, local, &[ResetCode::Refused as u8, 0, 0, 0]);
            }
        } else if body {
            self.send.insert(wire, SendStream::new(Some(key)));
        }
        Ok(())
    }

    fn service_reply(&mut self, key: u64, frame: Frame) -> Result<(), &'static str> {
        let id = frame.id;
        if !matches!(self.inr.get(&id), Some(InRequest::Applet { key: k, .. }) if *k == key) {
            // Answered by Hub already (the applet cancelled), or not a request of the applet.
            return Ok(());
        }
        self.inr.remove(&id);
        self.recv.remove(&id);
        let msg_limit = self.channel_by_key(key).expect("open").msg_limit;
        let reply = if frame.payload.len() > msg_limit {
            // The sender's endpoint refuses an object over the limit (§7.2).
            frame::encode_json(FrameType::Reply, id, &json!({"code": "too_large", "message": "the service's reply is over LINK_APP_MSG"}), JsonFrame { error: true, ..Default::default() }, self.limit)
        } else {
            frame::encode(FrameType::Reply, frame.flags & FLAG_ERROR, id, &frame.payload, self.limit)
        };
        let Ok(reply) = reply else { return Err("a reply over the payload limit") };
        // The REPLY goes behind every DATA frame of the request (§5.2).
        match self.send.get_mut(&id) {
            Some(stream) if stream.channel == Some(key) => {
                stream.trailer = Some(reply);
                self.pump(id);
            }
            _ => self.enqueue(reply),
        }
        Ok(())
    }

    fn service_event(&mut self, key: u64, frame: Frame) -> Result<(), &'static str> {
        let local = frame.id;
        let channel = self.channel_by_key(key).expect("open");
        let channel_id = channel.id;
        if local == channel_id {
            // The service closes its channel; it closes its socket after. The device is told `daemon` (CLOSE_BY_SERVICE).
            self.close_channel(channel_id, Some(CLOSE_BY_SERVICE), None);
            return Ok(());
        }
        let opens_stream = id_range(local) == Some(Side::Hub);
        if opens_stream && channel.to_wire.contains_key(&local) {
            return Err("an EVENT id of the service's range is not fresh");
        }
        let cost = units(frame.payload.len());
        if cost > channel.service_units {
            return Err("an event past the units the applet granted");
        }
        let valid = frame.payload.len() <= channel.msg_limit && has_t(&frame.payload);
        let channel = self.channel_by_key_mut(key).expect("open");
        channel.service_units -= cost;
        if local != 0 && !opens_stream {
            // Progress on one of the applet's requests.
            let Some(InRequest::Applet { key: k, head, .. }) = self.inr.get_mut(&local) else {
                // Late: the applet's request ended (cancelled). Dropped, units returned.
                self.regrant_service(key, cost);
                return Ok(());
            };
            if *k != key || !valid {
                self.regrant_service(key, cost);
                return Ok(());
            }
            let first = !*head;
            *head = true;
            if first && !self.send.contains_key(&local) {
                // The head step: the response stream exists from here, to hold the
                // applet's grants; it counts toward the bound from its first DATA.
                let mut stream = SendStream::new(Some(key));
                stream.counted = false;
                self.send.insert(local, stream);
            }
            if let Ok(bytes) = frame::encode(FrameType::Event, 0, local, &frame.payload, self.limit) {
                self.enqueue(bytes);
            }
            return Ok(());
        }
        if !valid {
            self.regrant_service(key, cost);
            if opens_stream && let Some(channel) = self.channel_by_key(key) {
                channel.send(FrameType::Reset, 0, local, &[ResetCode::Refused as u8, 0, 0, 0]);
            }
            return Ok(());
        }
        let mut wire = 0;
        if opens_stream {
            if self.outbound_streams(key) >= CHANNEL_STREAMS {
                if let Some(channel) = self.channel_by_key(key) {
                    channel.send(FrameType::Reset, 0, local, &[ResetCode::Refused as u8, 0, 0, 0]);
                }
            } else {
                wire = self.take_id().map_err(|_| "the link's ids are exhausted")?;
                let channel = self.channel_by_key_mut(key).expect("open");
                channel.to_wire.insert(local, wire);
                channel.to_local.insert(wire, local);
                self.send.insert(wire, SendStream::new(Some(key)));
            }
        }
        match frame::encode_json_bytes(FrameType::Event, wire, &frame.payload, JsonFrame { channel: Some(channel_id), ..Default::default() }, self.limit) {
            Ok(bytes) => self.enqueue(bytes),
            Err(_) => return Err("an object over the limit passed the check"),
        }
        Ok(())
    }

    fn service_cancel(&mut self, key: u64, local: u32) -> Result<(), &'static str> {
        let Some(wire) = self.channel_by_key(key).and_then(|c| if id_range(local) == Some(Side::Hub) { c.wire(local) } else { None }) else { return Ok(()) };
        let Some(request) = self.out.get_mut(&wire) else { return Ok(()) };
        if request.cancelled_at.is_some() || matches!(request.kind, OutKind::Channel { answered: true, .. }) {
            return Ok(());
        }
        request.cancelled_at = Some(Instant::now());
        request.cause = "cancelled";
        self.enqueue(frame::encode_cancel(wire));
        // The requester stops its body (§5.4); the cancel grace runs in `tick`.
        self.send.remove(&wire);
        Ok(())
    }

    fn service_data(&mut self, key: u64, frame: Frame) -> Result<(), &'static str> {
        let local = frame.id;
        let Some(wire) = self.channel_by_key(key).and_then(|c| c.wire(local)) else { return Ok(()) };
        let end = frame.flags & FLAG_END != 0;
        let uncounted = self.send.get(&wire).is_some_and(|s| s.channel == Some(key) && !s.counted);
        if uncounted {
            // A response's first DATA opens it toward the applet: the fifth is refused (§7.6).
            if self.outbound_streams(key) >= CHANNEL_STREAMS {
                self.send.remove(&wire);
                if let Some(InRequest::Applet { refused, .. }) = self.inr.get_mut(&wire) {
                    *refused = true;
                }
                self.send_reset(wire, ResetCode::Refused);
                if let Some(channel) = self.channel_by_key(key) {
                    channel.send(FrameType::Reset, 0, local, &[ResetCode::Refused as u8, 0, 0, 0]);
                }
                return Ok(());
            }
            if let Some(stream) = self.send.get_mut(&wire) {
                stream.counted = true;
            }
        }
        match self.send.get_mut(&wire) {
            Some(stream) if stream.channel == Some(key) => {
                if stream.ending {
                    return Ok(());
                }
                if frame.payload.len() as u64 > stream.credit.saturating_sub(stream.pending_len) {
                    return Err("DATA past the applet's credit");
                }
                stream.pending_len += frame.payload.len() as u64;
                if !frame.payload.is_empty() {
                    stream.pending.push_back(frame.payload);
                }
                stream.ending = end;
                self.pump(wire);
            }
            _ => {
                // Bytes on an applet request before any progress event: the head
                // step is refused before a byte reaches the wire (§7.2).
                if let Some(InRequest::Applet { key: k, head: false, refused, .. }) = self.inr.get_mut(&wire)
                    && *k == key
                    && !*refused
                {
                    *refused = true;
                    if let Some(channel) = self.channel_by_key(key) {
                        channel.send(FrameType::Reset, 0, local, &[ResetCode::Refused as u8, 0, 0, 0]);
                    }
                }
            }
        }
        Ok(())
    }

    fn service_credit(&mut self, key: u64, local: u32, amount: u64) -> Result<(), &'static str> {
        let channel = self.channel_by_key_mut(key).expect("open");
        if local == channel.id {
            channel.device_units += amount;
            let id = channel.id;
            self.send_credit(id, amount);
            return Ok(());
        }
        let Some(wire) = channel.wire(local) else { return Ok(()) };
        if let Some(stream) = self.recv.get_mut(&wire)
            && matches!(stream.sink, RecvSink::Relay { key: k } if k == key)
        {
            if stream.outstanding + amount > MAX_CREDIT {
                return Err("a grant over 16 MiB outstanding");
            }
            stream.outstanding += amount;
            self.send_credit(wire, amount);
            return Ok(());
        }
        // A grant for the response of the service's own request, before its head event.
        if let Some(OutRequest { kind: OutKind::Channel { key: k, early_credit, .. }, .. }) = self.out.get_mut(&wire)
            && *k == key
            && *early_credit != u64::MAX
        {
            if *early_credit + amount > MAX_CREDIT {
                return Err("a grant over 16 MiB outstanding");
            }
            *early_credit += amount;
            self.send_credit(wire, amount);
        }
        Ok(())
    }

    fn service_reset(&mut self, key: u64, local: u32, code: Option<ResetCode>) -> Result<(), &'static str> {
        let Some(wire) = self.channel_by_key(key).and_then(|c| c.wire(local)) else { return Ok(()) };
        let owned = self.send.get(&wire).is_some_and(|s| s.channel == Some(key))
            || matches!(self.recv.get(&wire), Some(RecvStream { sink: RecvSink::Relay { key: k }, .. }) if *k == key)
            || matches!(self.inr.get(&wire), Some(InRequest::Applet { key: k, .. }) if *k == key)
            || matches!(self.out.get(&wire), Some(OutRequest { kind: OutKind::Channel { key: k, .. }, .. }) if *k == key);
        if !owned {
            return Ok(());
        }
        // RESET abandons both directions; a request still ends with its REPLY.
        let trailer = self.send.remove(&wire).and_then(|s| s.trailer);
        self.recv.remove(&wire);
        self.send_reset(wire, code.unwrap_or(ResetCode::Cancelled));
        if let Some(trailer) = trailer {
            self.enqueue(trailer);
        }
        if !self.out.contains_key(&wire)
            && let Some(channel) = self.channel_by_key_mut(key)
        {
            channel.unmap(wire);
        }
        Ok(())
    }
}
