//! Hub's home folder and the control socket's path (§1).
//!
//! `<home>` is `$POCKET_HUB_HOME`, else `~/Library/Application
//! Support/nexus.pocket.hub` on macOS and `$XDG_STATE_HOME/pocket-hub` (else
//! `~/.local/state/pocket-hub`) elsewhere. The folders of the earlier Pocket
//! Hub (`~/Library/Application Support/Pocket Hub`, `~/Library/Logs/Pocket
//! Hub`) are never named here.
//!
//! The socket is `<home>/hubd.sock`, and nowhere else: a socket address holds
//! about 104 bytes of path, so a home whose socket path is longer than 100
//! bytes is refused rather than served from a second place a client would
//! have to guess.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const HOME_ENV: &str = "POCKET_HUB_HOME";
pub const SOCKET_FILE: &str = "hubd.sock";
/// The longest socket path a home may give.
pub const MAX_SOCKET_PATH: usize = 100;

/// The home folder from the environment: `$POCKET_HUB_HOME`, else the
/// platform's default. None when neither it nor `$HOME` is set.
pub fn home_from_env() -> Option<PathBuf> {
    match std::env::var_os(HOME_ENV) {
        Some(home) if !home.is_empty() => Some(PathBuf::from(home)),
        _ => default_home(std::env::var_os("HOME"), std::env::var_os("XDG_STATE_HOME")),
    }
}

/// The platform's default home, from `$HOME` (and `$XDG_STATE_HOME` off macOS).
pub fn default_home(home: Option<OsString>, xdg_state: Option<OsString>) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let home = home.filter(|h| !h.is_empty())?;
        return Some(PathBuf::from(home).join("Library/Application Support/nexus.pocket.hub"));
    }
    if let Some(state) = xdg_state.filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(state).join("pocket-hub"));
    }
    Some(PathBuf::from(home.filter(|h| !h.is_empty())?).join(".local/state/pocket-hub"))
}

/// `<home>/hubd.sock`, where the core listens and every client connects.
pub fn socket_path(home: &Path) -> PathBuf {
    home.join(SOCKET_FILE)
}

/// The socket path of `home`, or the sentence that refuses the home.
pub fn checked_socket_path(home: &Path) -> Result<PathBuf, String> {
    let socket = socket_path(home);
    let length = socket.as_os_str().len();
    if length > MAX_SOCKET_PATH {
        return Err(format!(
            "The home folder {} is too long: its socket path {} has {length} bytes and a socket path may have at most {MAX_SOCKET_PATH}; choose a shorter POCKET_HUB_HOME.",
            home.display(),
            socket.display()
        ));
    }
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_long_homes() {
        assert_eq!(checked_socket_path(Path::new("/tmp/h")).unwrap(), PathBuf::from("/tmp/h/hubd.sock"));
        let edge = PathBuf::from(format!("/tmp/{}", "y".repeat(MAX_SOCKET_PATH - "/tmp/".len() - "/hubd.sock".len())));
        assert_eq!(checked_socket_path(&edge).unwrap().as_os_str().len(), MAX_SOCKET_PATH);
        let long = PathBuf::from(format!("{}y", edge.display()));
        let refused = checked_socket_path(&long).unwrap_err();
        assert!(refused.contains("101 bytes"), "{refused}");
    }

    #[test]
    fn default_homes() {
        let home = default_home(Some("/Users/a".into()), Some("/state".into())).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(home, PathBuf::from("/Users/a/Library/Application Support/nexus.pocket.hub"));
        } else {
            assert_eq!(home, PathBuf::from("/state/pocket-hub"));
            assert_eq!(default_home(Some("/home/a".into()), None).unwrap(), PathBuf::from("/home/a/.local/state/pocket-hub"));
        }
        assert_eq!(default_home(None, None), None);
    }
}
