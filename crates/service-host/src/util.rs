//! Small pieces shared by the modules.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::RngCore;

/// 32 lowercase hex digits from the OS's random source: tokens and tickets.
pub fn random_hex() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn epoch_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Compares two secrets in time that does not depend on where they differ.
pub fn same_secret(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Writes a file through a temporary file in the same folder, fsynced, and a
/// rename: a reader sees the old content or the new, never a part.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no parent folder"))?;
    let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_string_lossy();
    let temp = dir.join(format!(".{name}.{}.tmp", random_hex()));
    let result = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Cuts `text` to at most `max` bytes on a character boundary.
pub fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The variables a child of Hub (a service, `bun install`) keeps from Hub's
/// own environment: what Bun needs to find programs, write temporary files,
/// reach the network through the owner's proxy and trust their certificates,
/// and print in their locale. Nothing else of Hub's environment reaches it.
pub fn inherited_env() -> Vec<(OsString, OsString)> {
    const NAMES: &[&str] = &[
        "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "TZ", "LANG", "LANGUAGE",
        "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "no_proxy", "all_proxy",
        "SSL_CERT_FILE", "SSL_CERT_DIR", "NODE_EXTRA_CA_CERTS",
    ];
    std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            NAMES.contains(&name.as_ref()) || name.starts_with("LC_")
        })
        .collect()
}

/// `PATH` with the folder of the Bun Hub runs first, so a service that starts
/// `bun` itself gets the same runtime.
pub fn path_with(bun: &Path) -> OsString {
    let mut parts: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = bun.parent() {
        parts.push(dir.to_path_buf());
    }
    if let Some(path) = std::env::var_os("PATH") {
        parts.extend(std::env::split_paths(&path));
    } else {
        parts.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(Into::into));
    }
    std::env::join_paths(parts).unwrap_or_default()
}

/// Sends `signal` to the process group `pgid`. A group that no longer exists
/// is not an error: the process is already gone.
#[cfg(unix)]
pub fn signal_group(pgid: u32, signal: i32) {
    if pgid == 0 || pgid > i32::MAX as u32 {
        return;
    }
    // SAFETY: killpg only reads its arguments.
    unsafe {
        libc::killpg(pgid as libc::pid_t, signal);
    }
}

/// Makes a folder with mode 0700, or sets an existing one to 0700.
#[cfg(unix)]
pub fn private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
