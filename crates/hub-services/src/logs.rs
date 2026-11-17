//! §2: a service's standard output and error are its log. Hub keeps the last
//! lines of each service in a ring, across its restarts, and broadcasts each
//! line as it comes.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::broadcast;

use crate::util::{cut, epoch_ms};

/// Lines kept per service.
pub const RING_LINES: usize = 4000;
/// A longer line is cut to this many bytes and marked.
pub const MAX_LINE_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogStream {
    Stdout,
    Stderr,
    /// A line of Hub's own about the process: its start, its end.
    Hub,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub app: String,
    /// Increases by one per line of this service, across its restarts.
    pub seq: u64,
    /// Epoch milliseconds.
    pub at: i64,
    pub stream: LogStream,
    pub text: String,
    /// The line was longer than MAX_LINE_BYTES and `text` is its start.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

pub struct LogRing {
    app: String,
    lines: Mutex<(u64, VecDeque<LogLine>)>,
    live: broadcast::Sender<LogLine>,
}

impl LogRing {
    pub fn new(app: &str, live: broadcast::Sender<LogLine>) -> Self {
        Self { app: app.to_owned(), lines: Mutex::new((0, VecDeque::new())), live }
    }

    pub fn push(&self, stream: LogStream, text: &str, was_cut: bool) {
        let was_cut = was_cut || text.len() > MAX_LINE_BYTES;
        let text = cut(text, MAX_LINE_BYTES);
        let mut guard = self.lines.lock().expect("log ring");
        let (seq, lines) = &mut *guard;
        *seq += 1;
        let line = LogLine { app: self.app.clone(), seq: *seq, at: epoch_ms(), stream, text: text.to_owned(), cut: was_cut };
        if lines.len() == RING_LINES {
            lines.pop_front();
        }
        lines.push_back(line.clone());
        drop(guard);
        let _ = self.live.send(line);
    }

    /// The lines kept, oldest first; with `after`, only those after that seq.
    pub fn lines(&self, after: Option<u64>) -> Vec<LogLine> {
        let guard = self.lines.lock().expect("log ring");
        guard.1.iter().filter(|line| after.is_none_or(|after| line.seq > after)).cloned().collect()
    }
}

/// Reads `from` to its end and pushes each line. A line over MAX_LINE_BYTES
/// is pushed cut, and the rest of it up to its newline is dropped, so one
/// endless line costs a bounded buffer.
pub async fn pump(mut from: impl AsyncRead + Unpin, ring: std::sync::Arc<LogRing>, stream: LogStream) {
    let mut line: Vec<u8> = Vec::with_capacity(256);
    let mut dropping = false;
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = match from.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &byte in &chunk[..n] {
            if byte == b'\n' {
                if !dropping {
                    emit(&ring, stream, &line, false);
                }
                line.clear();
                dropping = false;
            } else if !dropping {
                line.push(byte);
                if line.len() > MAX_LINE_BYTES {
                    emit(&ring, stream, &line, true);
                    line.clear();
                    dropping = true;
                }
            }
        }
    }
    if !line.is_empty() && !dropping {
        emit(&ring, stream, &line, false);
    }
}

fn emit(ring: &LogRing, stream: LogStream, bytes: &[u8], was_cut: bool) {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    ring.push(stream, &String::from_utf8_lossy(bytes), was_cut);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn lines_are_split_cut_and_bounded() {
        let (live, _) = broadcast::channel(16);
        let ring = Arc::new(LogRing::new("a", live));
        let mut input = Vec::new();
        input.extend_from_slice(b"one\r\ntwo\n");
        input.extend(std::iter::repeat_n(b'x', MAX_LINE_BYTES * 3));
        input.extend_from_slice(b"\nthree");
        pump(&input[..], ring.clone(), LogStream::Stdout).await;
        let lines = ring.lines(None);
        let texts: Vec<_> = lines.iter().map(|line| (line.text.len(), line.cut)).collect();
        assert_eq!(texts, vec![(3, false), (3, false), (MAX_LINE_BYTES, true), (5, false)]);
        assert_eq!(lines[3].text, "three");
        assert_eq!(ring.lines(Some(2)).len(), 2);

        for i in 0..RING_LINES + 10 {
            ring.push(LogStream::Hub, &i.to_string(), false);
        }
        let lines = ring.lines(None);
        assert_eq!(lines.len(), RING_LINES);
        assert_eq!(lines[0].text, "10");
    }
}
