//! §5: the channel connection the supervisor hands to the relay.
//!
//! The relay gets the connection's two halves right after the service's
//! `channel.attach`, with nothing past that frame read: it writes
//! `channel.ready` (or closes) itself and owns every byte after. Both halves
//! hold one lease on the channel; when the relay has dropped both, the
//! supervisor counts the channel closed, which is what lets an idle service
//! stop.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch;

use crate::supervisor::Inner;

/// Which process a channel connection belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelInfo {
    pub app: String,
    /// The release the process runs.
    pub release: String,
    pub pid: u32,
    /// The supervisor's number for this channel, unique while Hub runs.
    pub channel: u64,
}

/// One accepted channel connection. Drop both halves to end the channel.
pub struct ChannelStream {
    pub reader: ChannelReader,
    pub writer: ChannelWriter,
    pub info: ChannelInfo,
    /// Fires when Hub stops the service (its owner, an upgrade, Hub quitting):
    /// the relay sends `channel.close` `stop` to the service and tells the
    /// device. A service that ends on its own is seen as the end of the
    /// connection instead.
    pub stop: StopSignal,
}

impl ChannelStream {
    pub fn into_split(self) -> (ChannelReader, ChannelWriter) {
        (self.reader, self.writer)
    }
}

/// The read half: everything the service sends after its `channel.attach`.
pub struct ChannelReader {
    half: OwnedReadHalf,
    _lease: Arc<ChannelLease>,
}

/// The write half.
pub struct ChannelWriter {
    half: OwnedWriteHalf,
    _lease: Arc<ChannelLease>,
}

impl AsyncRead for ChannelReader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.half).poll_read(cx, buf)
    }
}

impl AsyncWrite for ChannelWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.half).poll_write(cx, buf)
    }
    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.half).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.half.is_write_vectored()
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.half).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.half).poll_shutdown(cx)
    }
}

/// Hub's request that a channel end because its service is being stopped.
#[derive(Clone)]
pub struct StopSignal(watch::Receiver<bool>);

impl StopSignal {
    pub fn is_requested(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves when the stop is requested, or when the service is gone.
    pub async fn requested(&mut self) {
        let _ = self.0.wait_for(|stop| *stop).await;
    }
}

/// Tells the supervisor when the relay has let go of a channel.
pub(crate) struct ChannelLease {
    pub inner: Weak<Inner>,
    pub app: String,
    pub serial: u64,
    pub channel: u64,
}

impl Drop for ChannelLease {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.channel_ended(&self.app, self.serial, self.channel);
        }
    }
}

pub(crate) fn make_stream(
    stream: tokio::net::UnixStream,
    lease: ChannelLease,
    info: ChannelInfo,
    stop: watch::Receiver<bool>,
) -> ChannelStream {
    let lease = Arc::new(lease);
    let (read, write) = stream.into_split();
    ChannelStream {
        reader: ChannelReader { half: read, _lease: lease.clone() },
        writer: ChannelWriter { half: write, _lease: lease },
        info,
        stop: StopSignal(stop),
    }
}
