//! SERVICE-PROTOCOL.md §2, "The enclosure": every service process runs
//! inside it, and nothing runs a service outside it.
//!
//! **macOS.** The process is started as `/usr/bin/sandbox-exec -p <profile>
//! <bun> …`: `sandbox-exec` applies the profile to itself and then `exec`s
//! Bun, so the process Hub supervises (its pid, its process group) is Bun's.
//! The profile is generated for each start from canonical paths (a home
//! behind a symbolic link, as test homes under `/var/folders` are, or with a
//! space in its path): Bun's executable, the release's folder, Hub's copy of
//! the SDK and the service socket, and nothing else of the home. Its fixed
//! part is the profile of the proposal's `TRUST.md` §5 as measured there;
//! what was changed to run the shipped Bun and this SDK is said beside it.
//!
//! **Any other system.** No enclosure is built yet, so no service starts
//! there: the start fails with the error `service`, `reason: "enclosure"`
//! (the owner's rule for a device with no measured enclosure, in its
//! narrowest form, `TRUST.md` §5).

use std::path::{Path, PathBuf};

/// Why a service's process cannot be started inside the enclosure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct EnclosureError(pub String);

/// The program and arguments that start Bun with `args` inside the
/// enclosure, and the socket path the process must be given (the canonical
/// one: the profile names it, and a connection by another path is refused).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enclosed {
    pub program: PathBuf,
    pub args: Vec<std::ffi::OsString>,
    /// The release's folder, canonical: the working directory.
    pub package: PathBuf,
    pub socket: PathBuf,
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    /// The canonical form of `socket`: its folder resolved, its name kept
    /// (the socket file is replaced at each of Hub's starts).
    fn canonical_socket(socket: &Path) -> std::io::Result<PathBuf> {
        let parent = socket.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let name = socket.file_name().ok_or_else(|| std::io::Error::other("the socket path has no file name"))?;
        Ok(std::fs::canonicalize(parent)?.join(name))
    }
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStrExt;

    /// The launcher that applies a profile and runs a program under it.
    pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

    /// A path as a string of the profile language: within double quotes,
    /// `\` and `"` escaped. A path that is not UTF-8 cannot be named.
    fn literal(path: &Path) -> Result<String, EnclosureError> {
        let text = std::str::from_utf8(path.as_os_str().as_bytes()).map_err(|_| EnclosureError(format!("{} is not a UTF-8 path", path.display())))?;
        Ok(format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")))
    }

    /// The folders above `path`, `/` excluded, outermost first.
    fn ancestors(path: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = path.ancestors().skip(1).filter(|folder| *folder != Path::new("/") && !folder.as_os_str().is_empty()).map(Path::to_path_buf).collect();
        found.reverse();
        found
    }

    /// The profile for one start. `bun`, `package`, `sdk` and `socket` are
    /// canonical.
    pub fn profile(bun: &Path, package: &Path, sdk: &Path, socket: &Path) -> Result<String, EnclosureError> {
        let (bun_l, package_l, sdk_l, socket_l) = (literal(bun)?, literal(package)?, literal(sdk)?, literal(socket)?);
        let mut metadata: Vec<PathBuf> = [bun, package, sdk, socket].iter().flat_map(|path| ancestors(path)).collect();
        metadata.sort();
        metadata.dedup();
        let metadata = metadata.iter().map(|folder| literal(folder).map(|text| format!("(literal {text})"))).collect::<Result<Vec<_>, _>>()?.join(" ");
        Ok(format!(
            "(version 1)\n\
             (deny default)\n\
             (allow process-exec (literal {bun_l}))\n\
             (allow file-read* (literal {bun_l}))\n\
             (allow file-read* (subpath {package_l}) (subpath {sdk_l}))\n\
             (allow file-read-metadata (literal \"/\") {metadata})\n\
             (allow file-read-data (literal \"/\"))\n\
             (allow file-read* (subpath \"/usr/lib\") (subpath \"/System/Library\")\n  \
               (subpath \"/usr/share/zoneinfo\") (subpath \"/usr/share/icu\")\n  \
               (subpath \"/private/var/db/timezone\") (literal \"/private/etc/localtime\")\n  \
               (literal \"/dev/urandom\") (literal \"/dev/random\") (literal \"/dev/null\"))\n\
             (allow sysctl-read (sysctl-name-prefix \"hw.\") (sysctl-name-prefix \"machdep.\")\n  \
               (sysctl-name-prefix \"kern.os\") (sysctl-name \"kern.version\") (sysctl-name \"kern.boottime\")\n  \
               (sysctl-name \"kern.argmax\") (sysctl-name \"kern.hostname\") (sysctl-name \"kern.maxfilesperproc\")\n  \
               (sysctl-name \"kern.usrstack64\") (sysctl-name \"sysctl.proc_translated\")\n  \
               (sysctl-name \"kern.secure_kernel\") (sysctl-name \"vm.footprint_suspend\"))\n\
             (deny process-info* (target others))\n\
             (allow network-outbound (remote unix-socket (path-literal {socket_l})))\n"
        ))
    }

    pub fn enclose(bun: &Path, package: &Path, sdk: &Path, socket: &Path, args: &[OsString]) -> Result<Enclosed, EnclosureError> {
        if !Path::new(SANDBOX_EXEC).is_file() {
            return Err(EnclosureError(format!("{SANDBOX_EXEC} is not on this Mac")));
        }
        let canonical = |path: &Path| std::fs::canonicalize(path).map_err(|error| EnclosureError(format!("{}: {error}", path.display())));
        let (bun, package, sdk) = (canonical(bun)?, canonical(package)?, canonical(sdk)?);
        let socket = canonical_socket(socket).map_err(|error| EnclosureError(format!("{}: {error}", socket.display())))?;
        let profile = profile(&bun, &package, &sdk, &socket)?;
        let mut all: Vec<OsString> = vec!["-p".into(), profile.into(), bun.clone().into_os_string()];
        all.extend(args.iter().cloned());
        Ok(Enclosed { program: SANDBOX_EXEC.into(), args: all, package, socket })
    }
}

#[cfg(target_os = "macos")]
pub use macos::profile;

/// How to start Bun with `args` (after Bun's own path) inside the
/// enclosure, for the release in `package`, Hub's SDK copy in `sdk` and the
/// service socket `socket`.
#[cfg(target_os = "macos")]
pub fn enclose(bun: &Path, package: &Path, sdk: &Path, socket: &Path, args: &[std::ffi::OsString]) -> Result<Enclosed, EnclosureError> {
    macos::enclose(bun, package, sdk, socket, args)
}

/// No enclosure is built for this system: no service starts on it.
#[cfg(not(target_os = "macos"))]
pub fn enclose(_bun: &Path, _package: &Path, _sdk: &Path, _socket: &Path, _args: &[std::ffi::OsString]) -> Result<Enclosed, EnclosureError> {
    Err(EnclosureError("this system's enclosure for services is not built yet, so no service runs on it".into()))
}

/// The line `sandbox-exec` writes to standard error when it cannot apply a
/// profile or exec the program, before any line of the service's.
pub const LAUNCHER_PREFIX: &str = "sandbox-exec:";

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// A path with a space, a double quote and a backslash is named exactly:
    /// `sandbox-exec` takes the profile and runs the program it allows.
    #[test]
    fn awkward_paths_are_named_exactly() {
        let temp = tempfile::Builder::new().prefix("phe").tempdir_in("/tmp").unwrap();
        let package = temp.path().join("a \"quoted\\ name");
        std::fs::create_dir_all(&package).unwrap();
        let socket = package.join("s.sock");
        let enclosed = enclose(Path::new("/usr/bin/true"), &package, &package, &socket, &[]).unwrap();
        let profile = enclosed.args[1].to_string_lossy().into_owned();
        assert!(profile.contains(r#"(subpath "/private/tmp/"#) && profile.contains(r#"a \"quoted\\ name""#), "{profile}");
        assert!(profile.contains(r#"(literal "/private")"#) && profile.contains(r#"(literal "/private/tmp")"#), "{profile}");
        let status = std::process::Command::new(&enclosed.program).args(&enclosed.args).status().unwrap();
        assert!(status.success());
    }
}
