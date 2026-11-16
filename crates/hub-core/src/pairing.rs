//! §3.11: a device's request to be paired by the code it shows.
//!
//! **A connection whose first `HANDSHAKE` is `pair` is a request to be
//! paired.** Hub keeps one for a device id, a later one replacing the one
//! before; it counts among the connections of §2.4 (the connection's own
//! count), and Hub closes it `pair_window` after the `pair` in place of the
//! 6 s of an unfinished handshake. The request waits here for Hub's owner:
//!
//! ```text
//! pair ──▶ waiting ──(the owner's code)──▶ offered ──(pair.accept verifies)──▶ record stored, pair.done
//!             │                               ├─(CLOSE auth)──▶ the code is not the device's
//!             ├─(dismissed)──▶ refused declined   └─(any other message)──▶ refused bad_proof
//!             └─(the window, the device leaving, a later pair of its id)──▶ gone
//! ```
//!
//! **Hub stores the device's record when `pair.accept` verifies, and not
//! before**: an offer under a wrong code replaces no key Hub holds for that
//! device id. No key appears in a log line or an event.

use std::net::IpAddr;
use std::sync::Arc;

use pocket_hub_store::DeviceRecord;
use pocket_link::CloseCode;
use pocket_link::frame::{self, Decoded, FrameDecoder, FrameType};
use pocket_link::handshake::{self, Refusal, Refused};
use pocket_link::pair::{self, Pair, PairDone, PairOffer};
use pocket_link::text::{new_key, new_nonce};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::api::{HubEvent, PairError, PairRequest};
use crate::endpoint::{Shared, store_call, write};
use crate::family;

/// What the endpoint tells the connection that holds a request.
pub(crate) enum Command {
    /// Hub's owner entered `code`; `done` gets how the pairing ended.
    Offer { code: String, done: oneshot::Sender<Result<(), PairError>> },
    /// Hub's owner dismissed the request: `refused` `declined`.
    Decline,
    /// The request ends: silently for a later `pair` of the same device id,
    /// with this `CLOSE` when Hub quits.
    End(Option<(CloseCode, &'static str)>),
}

/// One device's request, as the endpoint keeps it by device id.
pub(crate) struct Slot {
    /// The connection that holds it, so a replaced connection's end does not
    /// remove its successor.
    conn: u64,
    request: PairRequest,
    tx: mpsc::UnboundedSender<Command>,
    /// An offer went out: the request waits for the device, not for a code.
    offered: bool,
}

/// The requests that wait for a code, oldest first.
pub(crate) fn requests(shared: &Shared) -> Vec<PairRequest> {
    let mut waiting: Vec<PairRequest> = shared.pairing.lock().unwrap().values().filter(|slot| !slot.offered).map(|slot| slot.request.clone()).collect();
    waiting.sort_by(|a, b| (a.since, &a.device).cmp(&(b.since, &b.device)));
    waiting
}

/// Hub's owner entered `code` for `device`: Hub mints a link key and offers
/// it under the code. Resolves when the device proved that it holds the key
/// and its record is stored.
pub(crate) async fn offer(shared: &Shared, device: &str, code: &str) -> Result<(), PairError> {
    if !pair::is_code(code) {
        return Err(PairError::NotACode);
    }
    let (done, ended) = oneshot::channel();
    {
        let mut slots = shared.pairing.lock().unwrap();
        let slot = slots.get_mut(device).filter(|slot| !slot.offered).ok_or(PairError::NoRequest)?;
        slot.tx.send(Command::Offer { code: code.to_owned(), done }).map_err(|_| PairError::NoRequest)?;
        slot.offered = true;
    }
    // The request no longer waits for a code.
    shared.publish(HubEvent::PairRequests);
    ended.await.unwrap_or(Err(PairError::Gone))
}

/// Hub's owner dismissed the request of `device`. False when none waits for
/// a code.
pub(crate) fn decline(shared: &Shared, device: &str) -> bool {
    let slots = shared.pairing.lock().unwrap();
    slots.get(device).filter(|slot| !slot.offered).is_some_and(|slot| slot.tx.send(Command::Decline).is_ok())
}

/// Hub quits: every request's connection closes with `going_away`.
pub(crate) fn close_all(shared: &Shared) {
    for slot in shared.pairing.lock().unwrap().values() {
        let _ = slot.tx.send(Command::End(Some((CloseCode::GoingAway, "Hub quits"))));
    }
}

/// The offer Hub sent: its nonce, the key it carries, and its caller.
struct Sent {
    nonce: String,
    key: String,
    done: oneshot::Sender<Result<(), PairError>>,
}

/// How a request ended.
enum End {
    /// The record is stored and `pair.done` sent.
    Paired,
    /// An offer was out and did not leave a pairing.
    Failed(PairError),
    /// Dismissed, replaced, run out, or the device left: no pairing, and no
    /// caller that needs more than "gone".
    Gone,
}

/// Holds one request until it ends. `asked` is the device's `pair`, already
/// read; `decoder` may hold bytes that followed it.
pub(crate) async fn serve(
    shared: Arc<Shared>,
    asked: Pair,
    mut decoder: FrameDecoder,
    reader: &mut (dyn AsyncRead + Send + Unpin),
    writer: &mut (dyn AsyncWrite + Send + Unpin),
    peer: &str,
    peer_ip: Option<IpAddr>,
) {
    let conn = shared.next_conn_id();
    let window = shared.config.timing.pair_window;
    let since = chrono::Utc::now().timestamp_millis();
    let request = PairRequest {
        device: asked.device.clone(),
        name: asked.name.clone(),
        model: asked.model.clone(),
        target: asked.target.clone(),
        peer: peer.to_owned(),
        address: peer_ip.map_or_else(|| peer.to_owned(), |ip| ip.to_string()),
        since,
        expires: since + window.as_millis() as i64,
    };
    let (tx, mut commands) = mpsc::unbounded_channel();
    {
        // One request a device id: a later one replaces the one it left behind.
        let mut slots = shared.pairing.lock().unwrap();
        if let Some(earlier) = slots.insert(asked.device.clone(), Slot { conn, request, tx, offered: false }) {
            let _ = earlier.tx.send(Command::End(None));
        }
    }
    shared.publish(HubEvent::PairRequests);
    tracing::info!(device = %asked.device, name = %asked.name, model = %asked.model, target = %asked.target, %peer, "a device asks to be paired by the code it shows");

    let hub_id = shared.store.hub_id();
    let deadline = Instant::now() + window;
    let refuse = |code: Refusal, message: &str| {
        shared.publish(HubEvent::Refused { device: Some(asked.device.clone()), peer: peer.to_owned(), code: code.code().to_owned(), message: message.to_owned() });
        handshake::encode(&Refused::new(code, message)).expect("a refusal fits a handshake frame")
    };
    let mut sent: Option<Sent> = None;
    let mut buffer = vec![0u8; 4096];
    // Every write happens between two waits, never inside one: a wait that
    // loses to another (the owner's code against the device's bytes) is then
    // only a read that took nothing, and no frame is cut short.
    let end = 'request: loop {
        loop {
            match decoder.next() {
                Ok(Some(Decoded::Frame(frame))) => match frame.ty {
                    FrameType::Handshake => {
                        // After `pair` the device sends one more message: `pair.accept`, and only to an offer.
                        let proof = pair::parse_pair_accept(frame::parse_json(&frame).object.as_ref());
                        let expected = sent.as_ref().and_then(|sent| pair::console_proof(&sent.key, &asked.nonce, &sent.nonce, &asked.device, &hub_id));
                        let (Some(proof), Some(expected), Some(offered)) = (proof, expected, sent.as_ref()) else {
                            let _ = write(writer, &refuse(Refusal::BadProof, "the pairing proof did not verify")).await;
                            break 'request End::Failed(PairError::BadProof);
                        };
                        if !handshake::proof_equals(&proof, &expected) {
                            let _ = write(writer, &refuse(Refusal::BadProof, "the pairing proof did not verify")).await;
                            break 'request End::Failed(PairError::BadProof);
                        }
                        // The proof is keyed with the link key: the device
                        // unmasked it, so it holds the key this record gets.
                        break 'request match store(&shared, &asked, &offered.key, peer_ip).await {
                            Ok(()) => {
                                let _ = write(writer, &handshake::encode(&PairDone::new()).expect("pair.done fits a handshake frame")).await;
                                End::Paired
                            }
                            Err(error) => {
                                // The device wrote its key file and Hub holds
                                // no record of it: its next `hello` is
                                // answered `unknown_device`, and Hub's owner
                                // reads why from the caller's error.
                                let _ = write(writer, &frame::encode_close(CloseCode::GoingAway, "Hub could not store the pairing")).await;
                                End::Failed(PairError::Store(error))
                            }
                        };
                    }
                    FrameType::Ping => {
                        if write(writer, &frame::encode_pong(&frame.payload)).await.is_err() {
                            break 'request End::Gone;
                        }
                    }
                    FrameType::Close => {
                        // The device read the offer's proof with its own
                        // code: `auth` says the code Hub's owner entered is another.
                        let (code, _) = frame::parse_close(&frame.payload);
                        break 'request if code == CloseCode::Auth { End::Failed(PairError::WrongCode) } else { End::Gone };
                    }
                    _ => {}
                },
                Ok(Some(Decoded::Skipped)) => {}
                Ok(None) => break,
                Err(close) => {
                    let _ = write(writer, &frame::encode_close(close.code, &close.reason)).await;
                    break 'request End::Gone;
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep_until(deadline) => {
                let _ = write(writer, &frame::encode_close(CloseCode::HandshakeTimeout, "the request to be paired ran out")).await;
                break End::Gone;
            }
            command = commands.recv() => match command {
                Some(Command::Offer { code, done }) => {
                    let (key, nonce) = (new_key(), new_nonce());
                    let offer = PairOffer::new(&code, &key, &asked, &hub_id, &shared.store.hub_name(), &nonce).and_then(|offer| handshake::encode(&offer).ok());
                    let Some(offer) = offer else {
                        // `offer` took the code for one, so this is not reached.
                        let _ = done.send(Err(PairError::NotACode));
                        break End::Gone;
                    };
                    sent = Some(Sent { nonce, key, done });
                    if write(writer, &offer).await.is_err() {
                        break End::Gone;
                    }
                }
                Some(Command::Decline) => {
                    let _ = write(writer, &refuse(Refusal::Declined, "this computer's owner dismissed the request")).await;
                    break End::Gone;
                }
                Some(Command::End(close)) => {
                    if let Some((code, reason)) = close {
                        let _ = write(writer, &frame::encode_close(code, reason)).await;
                    }
                    break End::Gone;
                }
                None => break End::Gone,
            },
            read = reader.read(&mut buffer) => match read {
                Ok(n) if n > 0 => decoder.push(&buffer[..n]),
                _ => break End::Gone,
            },
        }
    };

    {
        let mut slots = shared.pairing.lock().unwrap();
        if slots.get(&asked.device).is_some_and(|slot| slot.conn == conn) {
            slots.remove(&asked.device);
        }
    }
    let paired = matches!(end, End::Paired);
    let how = match &end {
        End::Paired => "paired".to_owned(),
        End::Failed(error) => error.to_string(),
        End::Gone => "ended without a pairing".to_owned(),
    };
    tracing::info!(device = %asked.device, %peer, %how, "a request to be paired ended");
    if let Some(sent) = sent {
        let _ = sent.done.send(match end {
            End::Paired => Ok(()),
            End::Failed(error) => Err(error),
            End::Gone => Err(PairError::Gone),
        });
    }
    shared.publish(HubEvent::PairRequests);
    if paired {
        shared.publish(HubEvent::Paired { device: asked.device.clone() });
    }
}

/// Stores the record of a device that proved it holds `key`: what its `pair`
/// said, and where it is. Its family is its target's when Hub knows that
/// target, else the one its record had; `hostAbi` and the last `ready` stay
/// as an earlier record had them, since a `pair` carries neither.
async fn store(shared: &Arc<Shared>, asked: &Pair, key: &str, peer_ip: Option<IpAddr>) -> Result<(), String> {
    let (asked, key) = (asked.clone(), key.to_owned());
    let stored = store_call(&shared.store, move |store| {
        let earlier = store.device(&asked.device);
        let kind = match family::kind_of_target(&asked.target) {
            "" => earlier.as_ref().map(|record| record.kind.clone()).unwrap_or_default(),
            known => known.to_owned(),
        };
        store.put_device(DeviceRecord {
            id: asked.device.clone(),
            kind,
            name: asked.name.clone(),
            model: asked.model.clone(),
            key,
            target: asked.target.clone(),
            host_abi: earlier.as_ref().map_or(0, |record| record.host_abi),
            last_address: peer_ip.map(|ip| ip.to_string()).or_else(|| earlier.as_ref().and_then(|record| record.last_address.clone())),
            last_seen: earlier.as_ref().and_then(|record| record.last_seen),
            setup_unconfirmed: false,
        })
    })
    .await;
    match stored {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(error.to_string()),
        None => Err("the store task failed".to_owned()),
    }
}
