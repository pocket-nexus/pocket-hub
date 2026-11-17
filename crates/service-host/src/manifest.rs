//! SERVICE-PROTOCOL.md §1: `pocket.service.json`, checked before anything of
//! a package is used. Each field is read on its own, so a refusal names the
//! field it is about (the control API's error `service`, `reason: "manifest"`,
//! the field in `detail`).

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::hosts::{self, EntryFault};

pub const MANIFEST_FILE: &str = "pocket.service.json";

/// The manifest versions this host reads.
const MANIFEST_VERSION: u64 = 1;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024;
const MAX_ID_BYTES: usize = 128;
const MAX_ENTRY_BYTES: usize = 1024;
pub const DEFAULT_IDLE_SECONDS: u64 = 30;
/// A process may stay up idle for at most a day: one that never idles out is
/// a resident program, which a service is not.
const MAX_IDLE_SECONDS: u64 = 86_400;
const MAX_HOSTS: usize = 32;
/// `store.maxBytes`: at most 256 MiB, 16 MiB when it is not given.
const MAX_STORE_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_STORE_BYTES: u64 = 16 * 1024 * 1024;

/// Every capability a manifest may name in `requires`.
pub const CAPABILITIES: &[&str] = &["net", "store", "image", "text", "media", "native"];
/// The capabilities this host provides. None yet: a service that requires
/// one is refused at install, with its name, so it never runs without it.
pub const PROVIDED: &[&str] = &[];

/// The fields of the manifest's table, in its order.
const FIELDS: &[&str] = &["service", "id", "entry", "requires", "hosts", "store", "idleSeconds"];

/// A checked manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    /// Relative to the package root, with `/` separators, inside the package.
    pub entry: String,
    /// Sorted: the order carries no meaning.
    pub requires: Vec<String>,
    /// As the manifest lists them.
    pub hosts: Vec<String>,
    /// `store.maxBytes`; None without `store` in `requires`.
    pub store_max_bytes: Option<u64>,
    pub idle_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error("{MANIFEST_FILE} is missing")]
    Missing,
    #[error("{MANIFEST_FILE} cannot be read: {0}")]
    Unreadable(String),
    #[error("{MANIFEST_FILE} is over {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("{MANIFEST_FILE} is not JSON: {0}")]
    NotJson(String),
    #[error("{MANIFEST_FILE} is not a JSON object")]
    NotObject,
    #[error("`service` must be the integer 1")]
    ServiceKey,
    #[error("manifest version {0} is not supported (this Hub reads version {MANIFEST_VERSION})")]
    Version(u64),
    #[error("{MANIFEST_FILE} names `{0}`, a field this Hub does not read")]
    UnknownField(String),
    #[error("`{field}` must be {expected}")]
    Type { field: &'static str, expected: &'static str },
    #[error("id {0:?}: 1 to {MAX_ID_BYTES} bytes of A-Z a-z 0-9 . _ -, and not . or ..")]
    Id(String),
    #[error("entry {0:?} is not a relative path inside the package")]
    EntryPath(String),
    #[error("entry {0:?} is not a file in the package")]
    EntryMissing(String),
    #[error("requires names {0:?}, which is not a capability (net, store, image, text, media, native)")]
    RequiresUnknown(String),
    #[error("requires {0:?}, which this Hub does not provide yet")]
    RequiresNotProvided(String),
    #[error("requires names {0:?} twice")]
    RequiresRepeated(String),
    #[error("hosts has {0} entries, more than {MAX_HOSTS}")]
    HostsTooMany(usize),
    #[error("hosts entry {0:?} is not a lower-case host name, or `*.` and one")]
    HostsEntry(String),
    #[error("hosts names {0:?} twice")]
    HostsRepeated(String),
    #[error("hosts entry {entry:?} covers {suffix}, a public suffix")]
    HostsPublicSuffix { entry: String, suffix: String },
    #[error("`store` is allowed only with \"store\" in requires")]
    StoreWithoutRequires,
    #[error("store.maxBytes {0} is not between 1 and {MAX_STORE_BYTES}")]
    StoreBytes(u64),
    #[error("idleSeconds {0} is not between 1 and {MAX_IDLE_SECONDS}")]
    Idle(u64),
}

impl ManifestError {
    /// The field the refusal is about, for the error's `detail`; None for a
    /// fault of the file as a whole.
    pub fn field(&self) -> Option<String> {
        Some(match self {
            Self::Missing | Self::Unreadable(_) | Self::TooLarge | Self::NotJson(_) | Self::NotObject => return None,
            Self::ServiceKey | Self::Version(_) => "service".into(),
            Self::UnknownField(name) => name.clone(),
            Self::Type { field, .. } => (*field).into(),
            Self::Id(_) => "id".into(),
            Self::EntryPath(_) | Self::EntryMissing(_) => "entry".into(),
            Self::RequiresUnknown(_) | Self::RequiresNotProvided(_) | Self::RequiresRepeated(_) => "requires".into(),
            Self::HostsTooMany(_) | Self::HostsEntry(_) | Self::HostsRepeated(_) | Self::HostsPublicSuffix { .. } => "hosts".into(),
            Self::StoreWithoutRequires | Self::StoreBytes(_) => "store".into(),
            Self::Idle(_) => "idleSeconds".into(),
        })
    }
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

fn string<'a>(object: &'a Map<String, Value>, field: &'static str) -> Result<&'a str, ManifestError> {
    object.get(field).and_then(Value::as_str).ok_or(ManifestError::Type { field, expected: "a string" })
}

/// An optional array of strings; empty when absent.
fn strings(object: &Map<String, Value>, field: &'static str) -> Result<Vec<String>, ManifestError> {
    let wrong = ManifestError::Type { field, expected: "an array of strings" };
    match object.get(field) {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items.iter().map(|item| item.as_str().map(str::to_owned).ok_or(wrong.clone())).collect(),
        Some(_) => Err(wrong),
    }
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

    /// Checks a manifest's content, without the files it names. The version
    /// first, so a manifest of a later version is reported as such rather
    /// than by a field this version does not know; then any field the table
    /// does not name (`version` of the earlier manifest and `setup` among
    /// them); then each field in the table's order.
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge);
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|error| ManifestError::NotJson(error.to_string()))?;
        let Value::Object(object) = value else { return Err(ManifestError::NotObject) };
        match object.get("service") {
            Some(Value::Number(n)) if n.as_u64() == Some(MANIFEST_VERSION) => {}
            Some(Value::Number(n)) if n.as_u64().is_some() => return Err(ManifestError::Version(n.as_u64().unwrap_or(0))),
            _ => return Err(ManifestError::ServiceKey),
        }
        if let Some(unknown) = object.keys().find(|key| !FIELDS.contains(&key.as_str())) {
            return Err(ManifestError::UnknownField(unknown.clone()));
        }

        let id = string(&object, "id")?;
        if !valid_id(id) {
            return Err(ManifestError::Id(id.to_owned()));
        }
        let raw_entry = string(&object, "entry")?;
        let entry = entry_path(raw_entry).and_then(|path| path.to_str().map(str::to_owned)).ok_or_else(|| ManifestError::EntryPath(raw_entry.to_owned()))?;

        let mut requires = strings(&object, "requires")?;
        for (i, name) in requires.iter().enumerate() {
            if !CAPABILITIES.contains(&name.as_str()) {
                return Err(ManifestError::RequiresUnknown(name.clone()));
            }
            if requires[..i].contains(name) {
                return Err(ManifestError::RequiresRepeated(name.clone()));
            }
        }
        if let Some(missing) = requires.iter().find(|name| !PROVIDED.contains(&name.as_str())) {
            return Err(ManifestError::RequiresNotProvided(missing.clone()));
        }
        requires.sort();

        let hosts = strings(&object, "hosts")?;
        if hosts.len() > MAX_HOSTS {
            return Err(ManifestError::HostsTooMany(hosts.len()));
        }
        for (i, entry) in hosts.iter().enumerate() {
            match hosts::check_entry(entry) {
                Some(EntryFault::Syntax) => return Err(ManifestError::HostsEntry(entry.clone())),
                Some(EntryFault::PublicSuffix(suffix)) => return Err(ManifestError::HostsPublicSuffix { entry: entry.clone(), suffix }),
                None => {}
            }
            if hosts[..i].contains(entry) {
                return Err(ManifestError::HostsRepeated(entry.clone()));
            }
        }

        let store_max_bytes = match object.get("store") {
            None => requires.iter().any(|name| name == "store").then_some(DEFAULT_STORE_BYTES),
            Some(_) if !requires.iter().any(|name| name == "store") => return Err(ManifestError::StoreWithoutRequires),
            Some(Value::Object(store)) => {
                if let Some(unknown) = store.keys().find(|key| *key != "maxBytes") {
                    return Err(ManifestError::UnknownField(format!("store.{unknown}")));
                }
                let bytes = match store.get("maxBytes") {
                    None => DEFAULT_STORE_BYTES,
                    Some(value) => value.as_u64().ok_or(ManifestError::Type { field: "store", expected: "{\"maxBytes\": <integer>}" })?,
                };
                if !(1..=MAX_STORE_BYTES).contains(&bytes) {
                    return Err(ManifestError::StoreBytes(bytes));
                }
                Some(bytes)
            }
            Some(_) => return Err(ManifestError::Type { field: "store", expected: "{\"maxBytes\": <integer>}" }),
        };

        let idle_seconds = match object.get("idleSeconds") {
            None => DEFAULT_IDLE_SECONDS,
            Some(value) => value.as_u64().ok_or(ManifestError::Type { field: "idleSeconds", expected: "an integer" })?,
        };
        if !(1..=MAX_IDLE_SECONDS).contains(&idle_seconds) {
            return Err(ManifestError::Idle(idle_seconds));
        }

        Ok(Self { id: id.to_owned(), entry, requires, hosts, store_max_bytes, idle_seconds })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Manifest, ManifestError> {
        Manifest::parse(json.as_bytes())
    }

    /// The manifest of `fields` added to a minimal one.
    fn with(fields: &str) -> Result<Manifest, ManifestError> {
        parse(&format!(r#"{{"service":1,"id":"a","entry":"d.ts"{}{fields}}}"#, if fields.is_empty() { "" } else { "," }))
    }

    #[test]
    fn a_complete_manifest_is_read() {
        let manifest = parse(r#"{"service":1,"id":"com.example.app","entry":"./src/service.ts","requires":[],"hosts":["x.com","*.googlevideo.com"],"idleSeconds":5}"#).unwrap();
        assert_eq!(manifest.entry, "src/service.ts");
        assert_eq!(manifest.idle_seconds, 5);
        assert_eq!(manifest.hosts, ["x.com", "*.googlevideo.com"]);
        assert_eq!((manifest.requires.len(), manifest.store_max_bytes), (0, None));
        let minimal = with("").unwrap();
        assert_eq!((minimal.idle_seconds, minimal.requires.len(), minimal.hosts.len()), (DEFAULT_IDLE_SECONDS, 0, 0));
    }

    /// Every refusal of the manifest's table, with its sentence and its field.
    #[test]
    fn every_refusal_of_the_table_has_its_sentence_and_field() {
        let cases: Vec<(String, &str, Option<&str>)> = vec![
            ("[]".into(), "pocket.service.json is not a JSON object", None),
            ("{".into(), "pocket.service.json is not JSON: EOF while parsing an object at line 1 column 1", None),
            (r#"{"id":"a","entry":"d.ts"}"#.into(), "`service` must be the integer 1", Some("service")),
            (r#"{"service":"1","id":"a","entry":"d.ts"}"#.into(), "`service` must be the integer 1", Some("service")),
            (r#"{"service":2,"id":"a","entry":"d.ts","whatever":1}"#.into(), "manifest version 2 is not supported (this Hub reads version 1)", Some("service")),
            (r#"{"service":1,"id":"a","version":"1.0.0","entry":"d.ts"}"#.into(), "pocket.service.json names `version`, a field this Hub does not read", Some("version")),
            (r#"{"service":1,"id":"a","entry":"d.ts","setup":"setup/SKILL.md"}"#.into(), "pocket.service.json names `setup`, a field this Hub does not read", Some("setup")),
            (r#"{"service":1,"entry":"d.ts"}"#.into(), "`id` must be a string", Some("id")),
            (r#"{"service":1,"id":"a/b","entry":"d.ts"}"#.into(), r#"id "a/b": 1 to 128 bytes of A-Z a-z 0-9 . _ -, and not . or .."#, Some("id")),
            (r#"{"service":1,"id":"a","entry":"../d.ts"}"#.into(), r#"entry "../d.ts" is not a relative path inside the package"#, Some("entry")),
            (r#"{"service":1,"id":"a","entry":7}"#.into(), "`entry` must be a string", Some("entry")),
            (r#"{"service":1,"id":"a","entry":"d.ts","requires":"net"}"#.into(), "`requires` must be an array of strings", Some("requires")),
            (r#"{"service":1,"id":"a","entry":"d.ts","requires":["signin"]}"#.into(), r#"requires names "signin", which is not a capability (net, store, image, text, media, native)"#, Some("requires")),
            (r#"{"service":1,"id":"a","entry":"d.ts","requires":["net","net"]}"#.into(), r#"requires names "net" twice"#, Some("requires")),
            (r#"{"service":1,"id":"a","entry":"d.ts","requires":["image","net"]}"#.into(), r#"requires "image", which this Hub does not provide yet"#, Some("requires")),
            (r#"{"service":1,"id":"a","entry":"d.ts","requires":["store"],"store":{"maxBytes":1}}"#.into(), r#"requires "store", which this Hub does not provide yet"#, Some("requires")),
            (r#"{"service":1,"id":"a","entry":"d.ts","hosts":"x.com"}"#.into(), "`hosts` must be an array of strings", Some("hosts")),
            (format!(r#"{{"service":1,"id":"a","entry":"d.ts","hosts":[{}]}}"#, (0..33).map(|i| format!("\"h{i}.example\"")).collect::<Vec<_>>().join(",")), "hosts has 33 entries, more than 32", Some("hosts")),
            (r#"{"service":1,"id":"a","entry":"d.ts","hosts":["X.com"]}"#.into(), r#"hosts entry "X.com" is not a lower-case host name, or `*.` and one"#, Some("hosts")),
            (r#"{"service":1,"id":"a","entry":"d.ts","hosts":["192.0.2.1"]}"#.into(), r#"hosts entry "192.0.2.1" is not a lower-case host name, or `*.` and one"#, Some("hosts")),
            (r#"{"service":1,"id":"a","entry":"d.ts","hosts":["x.com:443"]}"#.into(), r#"hosts entry "x.com:443" is not a lower-case host name, or `*.` and one"#, Some("hosts")),
            (r#"{"service":1,"id":"a","entry":"d.ts","hosts":["https://x.com"]}"#.into(), r#"hosts entry "https://x.com" is not a lower-case host name, or `*.` and one"#, Some("hosts")),
            (r#"{"service":1,"id":"a","entry":"d.ts","hosts":["x.com","x.com"]}"#.into(), r#"hosts names "x.com" twice"#, Some("hosts")),
            (r#"{"service":1,"id":"a","entry":"d.ts","hosts":["*.co.uk"]}"#.into(), r#"hosts entry "*.co.uk" covers co.uk, a public suffix"#, Some("hosts")),
            (r#"{"service":1,"id":"a","entry":"d.ts","hosts":["*.github.io"]}"#.into(), r#"hosts entry "*.github.io" covers github.io, a public suffix"#, Some("hosts")),
            (r#"{"service":1,"id":"a","entry":"d.ts","store":{"maxBytes":1}}"#.into(), "`store` is allowed only with \"store\" in requires", Some("store")),
            (r#"{"service":1,"id":"a","entry":"d.ts","idleSeconds":0}"#.into(), "idleSeconds 0 is not between 1 and 86400", Some("idleSeconds")),
            (r#"{"service":1,"id":"a","entry":"d.ts","idleSeconds":86401}"#.into(), "idleSeconds 86401 is not between 1 and 86400", Some("idleSeconds")),
            (r#"{"service":1,"id":"a","entry":"d.ts","idleSeconds":-1}"#.into(), "`idleSeconds` must be an integer", Some("idleSeconds")),
        ];
        for (json, sentence, field) in cases {
            let error = parse(&json).unwrap_err();
            assert_eq!((error.to_string().as_str(), error.field().as_deref()), (sentence, field), "{json}");
        }
        assert_eq!(parse(&"x".repeat(16 * 1024 + 1)).unwrap_err().to_string(), "pocket.service.json is over 16384 bytes");
        for id in ["", ".", "..", "a b", "ü", &"x".repeat(129)] {
            let json = format!(r#"{{"service":1,"id":{},"entry":"d.ts"}}"#, serde_json::to_string(id).unwrap());
            assert!(matches!(parse(&json), Err(ManifestError::Id(_))), "{id:?}");
        }
        assert!(parse(&format!(r#"{{"service":1,"id":"{}","entry":"d.ts"}}"#, "x".repeat(128))).is_ok());
        for entry in ["", "/etc/passwd", "a/../../d.ts", "a/../d.ts", ".", "a\\b"] {
            let json = format!(r#"{{"service":1,"id":"a","entry":{}}}"#, serde_json::to_string(entry).unwrap());
            assert!(matches!(parse(&json), Err(ManifestError::EntryPath(_))), "{entry}");
        }
        let hosts: Vec<String> = (0..32).map(|i| format!("h{i}.example")).collect();
        assert!(with(&format!(r#""hosts":{}"#, serde_json::to_string(&hosts).unwrap())).is_ok(), "32 hosts are allowed");
    }

    #[test]
    fn the_entry_must_be_a_file_in_the_package_and_not_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(MANIFEST_FILE), r#"{"service":1,"id":"a","entry":"lib/d.ts"}"#).unwrap();
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
