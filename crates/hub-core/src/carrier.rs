//! Carriers (docs/POCKET-LINK.md §2.0).
//!
//! Pocket Link is defined over a carrier: one reliable, ordered, two-way byte
//! stream between one device and Hub. Everything from the preamble on is the
//! same bytes on every carrier, so the endpoint, the applets and the daemons
//! never learn which one a device is on. A carrier adapter supplies the
//! stream and the largest frame it offers; TCP over Wi-Fi is the one adapter
//! of phase one, and USB, UART and Bluetooth adapters produce the same
//! `CarrierStream`.

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;

use pocket_link::consts::{PORT, TCP_PAYLOAD};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

/// The kind of carrier a device is linked over, for display only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CarrierKind {
    Tcp,
    Usb,
    Uart,
    Ble,
}

/// One device's byte stream, ready for the preamble.
pub struct CarrierStream {
    pub kind: CarrierKind,
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    /// The largest frame payload this carrier offers (`ready.maxPayload`, §4.4).
    pub max_payload: u32,
    /// Where the device is, as text for the owner.
    pub peer: String,
    /// The device's address on an IP carrier: its last address in the record.
    pub peer_ip: Option<IpAddr>,
}

pub type AcceptFuture<'a> = Pin<Box<dyn Future<Output = io::Result<CarrierStream>> + Send + 'a>>;

/// A carrier adapter's listening side: it hands the endpoint one stream per
/// device that reaches Hub on it.
pub trait CarrierListener: Send + 'static {
    fn kind(&self) -> CarrierKind;
    /// The port a beacon names for this carrier, when it has one.
    fn port(&self) -> Option<u16>;
    fn accept(&mut self) -> AcceptFuture<'_>;
}

/// TCP over Wi-Fi: the device connects to Hub's listener (§2.1).
pub struct TcpCarrier {
    listener: TcpListener,
    port: u16,
}

impl TcpCarrier {
    /// Binds exactly the port it is given: the link port 8628 when `None`,
    /// an ephemeral one for `Some(0)`. **A port that is taken is an error**,
    /// not a reason to listen elsewhere: a device paired with this Hub looks
    /// for it on the port its pairing names, and the owner has to learn that
    /// another program holds it.
    pub async fn bind(port: Option<u16>) -> io::Result<Self> {
        let wanted = port.unwrap_or(PORT);
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), wanted)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }
}

impl CarrierListener for TcpCarrier {
    fn kind(&self) -> CarrierKind {
        CarrierKind::Tcp
    }

    fn port(&self) -> Option<u16> {
        Some(self.port)
    }

    fn accept(&mut self) -> AcceptFuture<'_> {
        Box::pin(async move {
            let (socket, from) = self.listener.accept().await?;
            // A small message must not wait behind Nagle for the device's next frame.
            let _ = socket.set_nodelay(true);
            let (reader, writer) = socket.into_split();
            Ok(CarrierStream {
                kind: CarrierKind::Tcp,
                reader: Box::new(reader),
                writer: Box::new(writer),
                max_payload: TCP_PAYLOAD,
                peer: from.to_string(),
                peer_ip: Some(from.ip()),
            })
        })
    }
}
