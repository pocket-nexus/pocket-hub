//! Hub provides the daemon SDK (docs/DAEMON-PROTOCOL.md §1): the runtime
//! files of `sdk/daemon` are built into this binary and written under
//! `<home>/sdk/daemon` at each start, so the SDK a daemon runs is always the
//! one of the Hub that started it. `hub-daemons` links that folder into a
//! package that has no `node_modules/@pocket-hub/daemon` of its own.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The package's runtime files, by their path in the package.
pub const FILES: &[(&str, &str)] = &[
    ("package.json", include_str!("../../../sdk/daemon/package.json")),
    ("README.md", include_str!("../../../sdk/daemon/README.md")),
    ("src/channel.ts", include_str!("../../../sdk/daemon/src/channel.ts")),
    ("src/connection.ts", include_str!("../../../sdk/daemon/src/connection.ts")),
    ("src/errors.ts", include_str!("../../../sdk/daemon/src/errors.ts")),
    ("src/frame.ts", include_str!("../../../sdk/daemon/src/frame.ts")),
    ("src/index.ts", include_str!("../../../sdk/daemon/src/index.ts")),
    ("src/serve.ts", include_str!("../../../sdk/daemon/src/serve.ts")),
    ("src/streams.ts", include_str!("../../../sdk/daemon/src/streams.ts")),
];

/// `<home>/sdk/daemon`.
pub fn folder(home: &Path) -> PathBuf {
    home.join("sdk").join("daemon")
}

/// Writes the SDK under `home`: each file whose content differs, and a file
/// of `src/` this build does not hold is removed. Returns the folder.
pub fn materialize(home: &Path) -> io::Result<PathBuf> {
    let root = folder(home);
    fs::create_dir_all(root.join("src"))?;
    for (path, content) in FILES {
        let target = root.join(path);
        if fs::read(&target).is_ok_and(|current| current == content.as_bytes()) {
            continue;
        }
        let temporary = target.with_extension("tmp");
        fs::write(&temporary, content)?;
        fs::rename(&temporary, &target)?;
    }
    for entry in fs::read_dir(root.join("src"))? {
        let entry = entry?;
        let name = format!("src/{}", entry.file_name().to_string_lossy());
        if !FILES.iter().any(|(path, _)| *path == name) {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source file added to the SDK must be added to `FILES` too.
    #[test]
    fn every_sdk_source_is_embedded() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/daemon/src");
        let mut on_disk: Vec<String> = fs::read_dir(src).unwrap().map(|e| format!("src/{}", e.unwrap().file_name().to_string_lossy())).collect();
        on_disk.sort();
        let mut embedded: Vec<String> = FILES.iter().filter(|(path, _)| path.starts_with("src/")).map(|(path, _)| path.to_string()).collect();
        embedded.sort();
        assert_eq!(on_disk, embedded);
    }

    #[test]
    fn materialize_writes_and_prunes() {
        let home = tempfile::tempdir().unwrap();
        let root = materialize(home.path()).unwrap();
        assert!(root.join("src/index.ts").is_file());
        fs::write(root.join("src/stale.ts"), "x").unwrap();
        fs::write(root.join("src/index.ts"), "changed").unwrap();
        materialize(home.path()).unwrap();
        assert!(!root.join("src/stale.ts").exists());
        assert_eq!(fs::read_to_string(root.join("src/index.ts")).unwrap(), FILES.iter().find(|(p, _)| *p == "src/index.ts").unwrap().1);
    }
}
