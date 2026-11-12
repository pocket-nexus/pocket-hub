//! Pocket Link 2: the one link between a device's runtime and Pocket Hub.
//!
//! This crate is the wire contract with no sockets, clocks or files: the
//! beacon, the preamble, the 12-byte frame header and its streaming decoder,
//! the handshake messages and proofs, and the pairing files with Hub's setup
//! as a plan of storage operations. Section numbers in the comments are those
//! of `docs/POCKET-LINK.md` in PocketJS (`vendor/pocketjs`), which is the
//! specification; `tests/vectors.rs` checks this crate against its vectors.

pub mod beacon;
pub mod consts;
pub mod frame;
pub mod handshake;
pub mod pairing;
pub mod preamble;
pub mod text;

pub use frame::{CloseCode, Dialect, Frame, FrameDecoder, FrameType, LinkClose, ResetCode};
