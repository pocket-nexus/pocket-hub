//! §1: `pocket.service.json`, checked before anything of a package is used.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MANIFEST_FILE: &str = "pocket.service.json";

/// The manifest versions this Hub reads.
const MANIFEST_VERSION: u64 = 1;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 64;
const MAX_ENTRY_BYTES: usize = 1024;
pub const DEFAULT_IDLE_SECONDS: u64 = 30;
/// A process may stay up idle for at most a day: one that never idles out is
/// a resident program, which a service is not.
const MAX_IDLE_SECONDS: u64 = 86_400;

/// A checked manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub version: String,
    /// Relative to the package root, with `/` separators, inside the package.
    pub entry: String,
    pub idle_seconds: u64,
}

/// The file as written. Unknown fields are refused: a field this Hub does not
/// know may change what the package means.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawManifest {
    service: u64,
    id: String,
    version: String,
    entry: String,
    #[serde(default)]
    idle_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("{MANIFEST_FILE} is missing")]
    Missing,
    #[error("{MANIFEST_FILE} cannot be read: {0}")]
    Unreadable(String),
    #[error("{MANIFEST_FILE} is over {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("{MANIFEST_FILE} is not a valid manifest: {0}")]
    Malformed(String),
    #[error("manifest version {0} is not supported (this Hub reads version {MANIFEST_VERSION})")]
    Version(u64),
    #[error("id {0:?}: 1 to {MAX_ID_BYTES} bytes of A-Z a-z 0-9 . _ -, and not . or ..")]
    Id(String),
    #[error("version {0:?} is not a SemVer version of at most {MAX_VERSION_BYTES} bytes")]
    SemVer(String),
    #[error("entry {0:?} is not a relative path inside the package")]
    EntryPath(String),
    #[error("entry {0:?} is not a file in the package")]
    EntryMissing(String),
    #[error("idleSeconds {0} is not between 1 and {MAX_IDLE_SECONDS}")]
    Idle(u64),
}

/// Whether `id` is an applet id a service may have (§1). `.` and `..` are
/// refused besides the charset, since the id names folders.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id != "."
        && id != ".."
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// SemVer 2.0.0: `MAJOR.MINOR.PATCH`, numbers without leading zeros, then an
/// optional `-pre.release` and `+build`. The version names a folder, and this
/// charset is safe for one.
pub fn valid_semver(version: &str) -> bool {
    if version.is_empty() || version.len() > MAX_VERSION_BYTES {
        return false;
    }
    let (rest, build) = match version.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (version, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) && (part == "0" || !part.starts_with('0'));
    let ident = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let core: Vec<&str> = core.split('.').collect();
    if core.len() != 3 || !core.iter().all(|part| numeric(part)) {
        return false;
    }
    if let Some(pre) = pre
        && !pre.split('.').all(|part| ident(part) && (!part.bytes().all(|b| b.is_ascii_digit()) || numeric(part)))
    {
        return false;
    }
    if let Some(build) = build
        && !build.split('.').all(ident)
    {
        return false;
    }
    true
}

/// The entry as a relative path of normal components, or None when it is
/// absolute, empty, or climbs out with `..`.
fn entry_path(entry: &str) -> Option<PathBuf> {
    if entry.is_empty() || entry.len() > MAX_ENTRY_BYTES || entry.contains('\0') || entry.contains('\\') {
        return None;
    }
    let mut path = PathBuf::new();
    for component in Path::new(entry).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

impl Manifest {
    /// Reads and checks the manifest at the root of the package in `root`.
    /// The entry must be a regular file there, not a link.
    pub fn read(root: &Path) -> Result<Self, ManifestError> {
        let path = root.join(MANIFEST_FILE);
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(ManifestError::Missing),
            Err(error) => return Err(ManifestError::Unreadable(error.to_string())),
        };
        if !meta.is_file() {
            return Err(ManifestError::Unreadable("not a regular file".into()));
        }
        if meta.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge);
        }
        let bytes = fs::read(&path).map_err(|error| ManifestError::Unreadable(error.to_string()))?;
        let manifest = Self::parse(&bytes)?;
        let entry = root.join(entry_path(&manifest.entry).expect("checked by parse"));
        // Every component below the root must be a real folder or the final
        // regular file: a link could point the entry out of the package.
        let mut walk = root.to_path_buf();
        for component in Path::new(&manifest.entry).components() {
            if let Component::Normal(part) = component {
                walk.push(part);
                match fs::symlink_metadata(&walk) {
                    Ok(meta) if meta.file_type().is_symlink() => return Err(ManifestError::EntryPath(manifest.entry)),
                    Ok(_) => {}
                    Err(_) => return Err(ManifestError::EntryMissing(manifest.entry)),
                }
            }
        }
        if !fs::symlink_metadata(&entry).is_ok_and(|meta| meta.is_file()) {
            return Err(ManifestError::EntryMissing(manifest.entry));
        }
        Ok(manifest)
    }

    /// Checks a manifest's content, without the files it names.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge);
        }
        // The version first, so a manifest of a later version is reported as
        // such rather than by a field this version does not know.
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| ManifestError::Malformed(error.to_string()))?;
        match value.get("service") {
            Some(serde_json::Value::Number(n)) if n.as_u64() == Some(MANIFEST_VERSION) => {}
            Some(serde_json::Value::Number(n)) if n.as_u64().is_some() => return Err(ManifestError::Version(n.as_u64().unwrap_or(0))),
            _ => return Err(ManifestError::Malformed("`service` must be the integer 1".into())),
        }
        let raw: RawManifest = serde_json::from_value(value).map_err(|error| ManifestError::Malformed(error.to_string()))?;
        debug_assert_eq!(raw.service, MANIFEST_VERSION);
        if !valid_id(&raw.id) {
            return Err(ManifestError::Id(raw.id));
        }
        if !valid_semver(&raw.version) {
            return Err(ManifestError::SemVer(raw.version));
        }
        let Some(entry) = entry_path(&raw.entry) else {
            return Err(ManifestError::EntryPath(raw.entry));
        };
        let idle_seconds = raw.idle_seconds.unwrap_or(DEFAULT_IDLE_SECONDS);
        if !(1..=MAX_IDLE_SECONDS).contains(&idle_seconds) {
            return Err(ManifestError::Idle(idle_seconds));
        }
        let entry = entry.to_str().ok_or_else(|| ManifestError::EntryPath(raw.entry.clone()))?.to_owned();
        Ok(Self { id: raw.id, version: raw.version, entry, idle_seconds })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Manifest, ManifestError> {
        Manifest::parse(json.as_bytes())
    }

    #[test]
    fn a_complete_manifest_is_read() {
        let manifest = parse(r#"{"service":1,"id":"com.example.app","version":"1.2.3-beta.1+b5","entry":"./src/service.ts","idleSeconds":5}"#).unwrap();
        assert_eq!(manifest.entry, "src/service.ts");
        assert_eq!(manifest.idle_seconds, 5);
        assert_eq!(parse(r#"{"service":1,"id":"a","version":"0.0.0","entry":"d.ts"}"#).unwrap().idle_seconds, DEFAULT_IDLE_SECONDS);
    }

    #[test]
    fn every_bad_field_is_refused() {
        let with = |field: &str| parse(&format!(r#"{{"service":1,"id":"a","version":"1.0.0","entry":"d.ts",{field}}}"#));
        assert_eq!(parse(r#"{"service":2,"id":"a","version":"1.0.0","entry":"d.ts"}"#), Err(ManifestError::Version(2)));
        assert!(matches!(parse(r#"{"service":"1","id":"a","version":"1.0.0","entry":"d.ts"}"#), Err(ManifestError::Malformed(_))));
        assert!(matches!(parse(r#"{"id":"a","version":"1.0.0","entry":"d.ts"}"#), Err(ManifestError::Malformed(_))));
        assert!(matches!(parse("[]"), Err(ManifestError::Malformed(_))));
        assert!(matches!(with(r#""extra":true"#), Err(ManifestError::Malformed(_))));
        assert!(matches!(with(r#""idleSeconds":0"#), Err(ManifestError::Idle(0))));
        assert!(matches!(with(r#""idleSeconds":86401"#), Err(ManifestError::Idle(_))));
        assert!(matches!(with(r#""idleSeconds":-1"#), Err(ManifestError::Malformed(_))));
        for id in ["", ".", "..", "a/b", "a b", "ü", &"x".repeat(129)] {
            let json = format!(r#"{{"service":1,"id":{},"version":"1.0.0","entry":"d.ts"}}"#, serde_json::to_string(id).unwrap());
            assert!(matches!(parse(&json), Err(ManifestError::Id(_))), "{id:?}");
        }
        assert!(parse(&format!(r#"{{"service":1,"id":"{}","version":"1.0.0","entry":"d.ts"}}"#, "x".repeat(128))).is_ok());
        for version in ["1", "1.0", "01.0.0", "1.0.0-", "1.0.0-01", "1.0.0+", "1.0.0/x", "v1.0.0", "1.0.0-a..b"] {
            let json = format!(r#"{{"service":1,"id":"a","version":"{version}","entry":"d.ts"}}"#);
            assert!(matches!(parse(&json), Err(ManifestError::SemVer(_))), "{version}");
        }
        for entry in ["", "/etc/passwd", "../d.ts", "a/../../d.ts", "a/../d.ts", ".", "a\\b"] {
            let json = format!(r#"{{"service":1,"id":"a","version":"1.0.0","entry":{}}}"#, serde_json::to_string(entry).unwrap());
            assert!(matches!(parse(&json), Err(ManifestError::EntryPath(_))), "{entry}");
        }
    }

    #[test]
    fn the_entry_must_be_a_file_in_the_package_and_not_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(MANIFEST_FILE), r#"{"service":1,"id":"a","version":"1.0.0","entry":"lib/d.ts"}"#).unwrap();
        assert_eq!(Manifest::read(root), Err(ManifestError::EntryMissing("lib/d.ts".into())));
        fs::create_dir(root.join("lib")).unwrap();
        fs::create_dir(root.join("lib/d.ts")).unwrap();
        assert_eq!(Manifest::read(root), Err(ManifestError::EntryMissing("lib/d.ts".into())));
        fs::remove_dir(root.join("lib/d.ts")).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("lib/d.ts")).unwrap();
        assert_eq!(Manifest::read(root), Err(ManifestError::EntryPath("lib/d.ts".into())));
        fs::remove_file(root.join("lib/d.ts")).unwrap();
        fs::write(root.join("lib/d.ts"), "").unwrap();
        assert!(Manifest::read(root).is_ok());
        assert_eq!(Manifest::read(&root.join("lib")), Err(ManifestError::Missing));
    }
}
