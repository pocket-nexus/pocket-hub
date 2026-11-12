//! Hub's log: `tracing` to standard error and to `<home>/logs/hubd.log`,
//! which is bounded: at 4 MiB it becomes `hubd.log.1` (replacing the one
//! before) and a new file starts, so the log never takes more than 8 MiB.
//!
//! No line carries a link key: Hub logs ids, names, addresses and codes,
//! never a `DeviceRecord` or a pairing file's text.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The log file's size at which it is rotated.
pub const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// The target of the one line a failed start writes: it goes to the file
/// through `tracing`, and to standard error as the bare sentence.
pub const FATAL_TARGET: &str = "pocket_hubd::fatal";

pub fn log_path(home: &Path) -> PathBuf {
    home.join("logs").join("hubd.log")
}

struct State {
    path: PathBuf,
    file: File,
    size: u64,
    max: u64,
}

/// The size-bounded log file.
#[derive(Clone)]
pub struct LogFile(Arc<Mutex<State>>);

impl LogFile {
    pub fn open(path: &Path, max: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let size = file.metadata()?.len();
        Ok(Self(Arc::new(Mutex::new(State { path: path.to_owned(), file, size, max }))))
    }
}

pub struct LogWriter(LogFile);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.0.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.size > 0 && state.size + bytes.len() as u64 > state.max {
            let rotated = state.path.with_extension("log.1");
            let _ = fs::rename(&state.path, &rotated);
            state.file = OpenOptions::new().create(true).append(true).open(&state.path)?;
            state.size = 0;
        }
        state.file.write_all(bytes)?;
        state.size += bytes.len() as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogFile {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> LogWriter {
        LogWriter(self.clone())
    }
}

/// Installs the subscriber: the file always, standard error when `stderr`.
/// `$POCKET_HUB_LOG` is the filter (`info` by default).
pub fn init(home: &Path, stderr: bool) -> io::Result<()> {
    let file = LogFile::open(&log_path(home), MAX_LOG_BYTES)?;
    let filter = || EnvFilter::try_from_env("POCKET_HUB_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = tracing_subscriber::fmt::layer().log_internal_errors(false).with_ansi(false).with_writer(file).with_filter(filter());
    let stderr_layer = stderr.then(|| {
        // A client that started this core reads its standard error only
        // until the core answers; writes after that fail and are dropped,
        // and must not be reported on that same standard error.
        tracing_subscriber::fmt::layer()
            .log_internal_errors(false)
            .with_writer(io::stderr)
            .with_ansi(std::io::IsTerminal::is_terminal(&io::stderr()))
            .with_filter(filter())
            .with_filter(tracing_subscriber::filter::filter_fn(|meta| meta.target() != FATAL_TARGET))
    });
    let _ = tracing_subscriber::registry().with(file_layer).with(stderr_layer).try_init();
    Ok(())
}

use tracing_subscriber::Layer as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs/hubd.log");
        let file = LogFile::open(&path, 100).unwrap();
        let mut writer = file.make_writer();
        for i in 0..30 {
            writer.write_all(format!("line {i:04}\n").as_bytes()).unwrap();
        }
        assert!(fs::metadata(&path).unwrap().len() <= 100);
        assert!(fs::metadata(path.with_extension("log.1")).unwrap().len() <= 100);
        assert!(fs::read_to_string(&path).unwrap().ends_with("line 0029\n"));
    }
}
