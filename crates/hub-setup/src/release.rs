//! Pocket Shell, the 3DS program: where a build comes from, its download, a
//! cache, and its checks.
//!
//! A release of `pocket-nexus/pocket-shell` holds `pocket-shell-v<date>.3dsx`,
//! `SHA256SUMS` and `build-info.json`. The program is used only after its
//! SHA-256 matches its line in `SHA256SUMS` (and `build-info.json`'s entry,
//! when it lists the file) and it starts with the `3DSX` magic; a cached copy
//! is checked the same way each time it is used.
//!
//! **No compatibility logic lives here.** A release built before Pocket Link 2
//! cannot link; `build-info.json`'s fields (`version`, `source`, `runtime`)
//! go into the install's report so the caller can warn, and a local `.3dsx`
//! (a development build) can stand in for a release.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The GitHub repository of Pocket Shell's releases.
pub const DEFAULT_REPO: &str = "pocket-nexus/pocket-shell";
pub const GITHUB_API: &str = "https://api.github.com";
/// The User-Agent of every request: generic, naming nobody.
const USER_AGENT: &str = "pocket-hub";
/// A `.3dsx` larger than this is not a program the setup installs
/// (`runtime.native.install`'s limit, §6.2).
pub const MAX_PROGRAM_BYTES: u64 = pocket_link::consts::NATIVE_MAX_BYTES;
const MAX_METADATA_BYTES: u64 = 1024 * 1024;
pub const MAGIC: &[u8; 4] = b"3DSX";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellSource {
    /// A GitHub release: the latest, or the one of `tag`. `api` is the API's
    /// base URL (a mirror or a test server can stand in for GitHub).
    GitHub { api: String, repo: String, tag: Option<String> },
    /// A `.3dsx` on this computer: a development build. Its `SHA256SUMS` and
    /// `build-info.json`, when they lie beside it, are checked and reported.
    File { path: PathBuf },
}

impl Default for ShellSource {
    fn default() -> Self {
        Self::GitHub { api: GITHUB_API.to_owned(), repo: DEFAULT_REPO.to_owned(), tag: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildFile {
    pub name: String,
    pub sha256: String,
}

/// `build-info.json`. `runtime` is the PocketJS commit the build was made
/// from, which says whether it speaks Pocket Link 2.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub files: Vec<BuildFile>,
}

/// One build of the program, known before its bytes are downloaded: its size
/// and hash decide whether the console already holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellRelease {
    /// The build's version, e.g. `2026.11.02`: `build-info.json`'s, else the
    /// tag without its `v`.
    pub version: Option<String>,
    pub tag: Option<String>,
    pub file_name: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    pub build_info: Option<BuildInfo>,
    /// Where the build comes from, e.g. `github:pocket-nexus/pocket-shell@v2026.11.02`
    /// or `file:pocket-shell.3dsx`.
    pub origin: String,
    /// The release could not be looked up and this is the last one Hub
    /// verified and cached.
    #[serde(default)]
    pub from_cache: bool,
    #[serde(skip)]
    location: Location,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Location {
    #[default]
    Cached,
    Url(String),
    File(PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum ReleaseError {
    #[error("the release could not be fetched: {0}")]
    Network(String),
    /// The server answered with this HTTP status (404: no such release).
    #[error("the release could not be fetched: HTTP {0}")]
    Status(u16),
    #[error("the release is not usable: {0}")]
    Invalid(String),
    /// The bytes are not the release's: another SHA-256 or another size.
    #[error("the program does not match its checksum")]
    Checksum,
    /// The file is not a program of the device (no `3DSX` header, too large).
    #[error("{0}")]
    NotAProgram(String),
    #[error("{0}")]
    Io(String),
}

fn invalid(message: impl Into<String>) -> ReleaseError {
    ReleaseError::Invalid(message.into())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The hash `SHA256SUMS` gives `name`: lines of `<64 hex> <space> [*]<name>`.
pub fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, rest) = line.trim_end().split_once(char::is_whitespace)?;
        let file = rest.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hash.to_ascii_lowercase())
    })
}

/// Checks a program's bytes against a release: size, SHA-256, `3DSX` magic.
pub fn verify(release: &ShellRelease, bytes: &[u8]) -> Result<(), ReleaseError> {
    if bytes.len() as u64 != release.size {
        tracing::info!(file = release.file_name, bytes = bytes.len(), expected = release.size, "the program is not the release's size");
        return Err(ReleaseError::Checksum);
    }
    if sha256_hex(bytes) != release.sha256 {
        return Err(ReleaseError::Checksum);
    }
    check_magic(bytes)
}

fn check_magic(bytes: &[u8]) -> Result<(), ReleaseError> {
    if bytes.len() < 32 || &bytes[..4] != MAGIC {
        return Err(ReleaseError::NotAProgram("the file is not a 3DS homebrew program (no 3DSX header)".into()));
    }
    Ok(())
}

/// The order of two program versions, or None when either is not a dotted
/// numeric version.
///
/// Pocket Shell's versions are dates in dotted numbers: a release is
/// `2026.11.02` (`build-info.json`'s `version`), and the manifest the device
/// reports from may write the same build `2026.11.2`. Each part is compared
/// as a number, so leading zeros do not count, and a missing part counts as
/// 0 (`2026.11` equals `2026.11.0`). A version with anything but ASCII digits
/// between its dots (a `v`, a suffix such as `-dev`, an empty part) has no
/// place in this order: it is never older or newer than another.
pub fn version_order(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    fn parts(version: &str) -> Option<Vec<u64>> {
        version.split('.').map(|part| if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) { part.parse().ok() } else { None }).collect()
    }
    let (a, b) = (parts(a)?, parts(b)?);
    let length = a.len().max(b.len());
    let at = |parts: &[u64], index: usize| parts.get(index).copied().unwrap_or(0);
    Some((0..length).map(|index| at(&a, index).cmp(&at(&b, index))).find(|order| order.is_ne()).unwrap_or(std::cmp::Ordering::Equal))
}

/// True when `latest` orders above `current` (`version_order`).
pub fn is_newer(latest: &str, current: &str) -> bool {
    version_order(latest, current) == Some(std::cmp::Ordering::Greater)
}

/// A name from a release used as a cache path: no folder, no `..`.
fn safe_name(name: &str) -> bool {
    crate::storage::is_safe_path(name) && !name.contains('/')
}

fn cross_check(info: &Option<BuildInfo>, name: &str, sha256: &str) -> Result<(), ReleaseError> {
    if let Some(entry) = info.as_ref().and_then(|info| info.files.iter().find(|file| file.name == name))
        && !entry.sha256.eq_ignore_ascii_case(sha256) {
            return Err(invalid(format!("build-info.json and SHA256SUMS disagree on {name}")));
        }
    Ok(())
}

/// Looks releases up and downloads them, with a cache under `cache_dir`:
/// `<cache_dir>/<tag>/<file>`, and `<cache_dir>/last-release.json` naming the
/// last release verified there.
pub struct ReleaseFetcher {
    client: reqwest::Client,
    cache_dir: PathBuf,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

impl ReleaseFetcher {
    /// `api` decides the proxy: a loopback API (a test server) is reached
    /// directly, anything else through the system's proxy settings.
    pub fn new(cache_dir: impl Into<PathBuf>, source: &ShellSource) -> Result<Self, ReleaseError> {
        let mut builder = reqwest::Client::builder().user_agent(USER_AGENT).connect_timeout(Duration::from_secs(10)).read_timeout(Duration::from_secs(30));
        if let ShellSource::GitHub { api, .. } = source
            && (api.starts_with("http://127.0.0.1") || api.starts_with("http://localhost") || api.starts_with("http://[::1]")) {
                builder = builder.no_proxy();
            }
        let client = builder.build().map_err(|error| ReleaseError::Network(error.to_string()))?;
        Ok(Self { client, cache_dir: cache_dir.into() })
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    async fn get(&self, url: &str, max: u64, progress: &mut (dyn FnMut(u64, Option<u64>) + Send)) -> Result<Vec<u8>, ReleaseError> {
        let network = |error: reqwest::Error| ReleaseError::Network(error.without_url().to_string());
        let mut response = self.client.get(url).header("Accept", "application/vnd.github+json, application/octet-stream, */*").send().await.map_err(network)?;
        if !response.status().is_success() {
            return Err(ReleaseError::Status(response.status().as_u16()));
        }
        let total = response.content_length();
        if total.is_some_and(|total| total > max) {
            return Err(invalid(format!("a file of more than {max} bytes")));
        }
        let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(max) as usize);
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            if bytes.len() as u64 + chunk.len() as u64 > max {
                return Err(invalid(format!("a file of more than {max} bytes")));
            }
            bytes.extend_from_slice(&chunk);
            progress(bytes.len() as u64, total);
        }
        Ok(bytes)
    }

    async fn get_text(&self, url: &str) -> Result<String, ReleaseError> {
        let bytes = self.get(url, MAX_METADATA_BYTES, &mut |_, _| {}).await?;
        String::from_utf8(bytes).map_err(|_| invalid("a metadata file that is not UTF-8"))
    }

    /// The build `source` names, without its program bytes (a local file is
    /// read and hashed). A GitHub release that cannot be looked up falls
    /// back to the last release verified in the cache, marked `from_cache`.
    pub async fn resolve(&self, source: &ShellSource) -> Result<ShellRelease, ReleaseError> {
        match source {
            ShellSource::File { path } => resolve_file(path).await,
            ShellSource::GitHub { api, repo, tag } => match self.resolve_github(api, repo, tag.as_deref()).await {
                Ok(release) => Ok(release),
                Err(error) => match self.last_cached().await {
                    Some(mut cached) if tag.is_none() || cached.tag == *tag => {
                        tracing::info!(%error, tag = cached.tag.as_deref().unwrap_or(""), "the release could not be looked up; using the cached one");
                        cached.from_cache = true;
                        Ok(cached)
                    }
                    _ => Err(error),
                },
            },
        }
    }

    /// The build `source` names as the source answers now, without the
    /// cached stand-in of `resolve`: an update over the link and the look for
    /// the newest release need the release's own answer, and the cached one
    /// is most often the build the device already runs. The bytes still come
    /// from the cache through `fetch` when a verified copy is there.
    pub async fn resolve_fresh(&self, source: &ShellSource) -> Result<ShellRelease, ReleaseError> {
        match source {
            ShellSource::File { path } => resolve_file(path).await,
            ShellSource::GitHub { api, repo, tag } => self.resolve_github(api, repo, tag.as_deref()).await,
        }
    }

    async fn resolve_github(&self, api: &str, repo: &str, tag: Option<&str>) -> Result<ShellRelease, ReleaseError> {
        if !repo.split('/').count().eq(&2) || !repo.split('/').all(safe_name) {
            return Err(invalid(format!("not a GitHub repository: {repo}")));
        }
        let api = api.trim_end_matches('/');
        let url = match tag {
            None => format!("{api}/repos/{repo}/releases/latest"),
            Some(tag) if safe_name(tag) => format!("{api}/repos/{repo}/releases/tags/{tag}"),
            Some(tag) => return Err(invalid(format!("not a release tag: {tag}"))),
        };
        let text = self.get_text(&url).await?;
        let release: GitHubRelease = serde_json::from_str(&text).map_err(|error| invalid(format!("the release's description: {error}")))?;
        if !safe_name(&release.tag_name) {
            return Err(invalid("the release's tag is not a plain name"));
        }
        let asset = |name: &str| release.assets.iter().find(|asset| asset.name == name);
        let sums = asset("SHA256SUMS").ok_or_else(|| invalid("the release has no SHA256SUMS"))?;
        let sums = self.get_text(&sums.browser_download_url).await?;
        let build_info = match asset("build-info.json") {
            Some(info) => Some(serde_json::from_str::<BuildInfo>(&self.get_text(&info.browser_download_url).await?).map_err(|error| invalid(format!("build-info.json: {error}")))?),
            None => None,
        };
        // The program: the one `.3dsx` asset; with several, the one
        // build-info.json lists.
        let programs: Vec<&GitHubAsset> = release.assets.iter().filter(|asset| asset.name.ends_with(".3dsx")).collect();
        let program = match programs.as_slice() {
            [one] => *one,
            [] => return Err(invalid("the release has no .3dsx")),
            many => many
                .iter()
                .copied()
                .find(|asset| build_info.as_ref().is_some_and(|info| info.files.iter().any(|file| file.name == asset.name)))
                .ok_or_else(|| invalid("the release has several .3dsx files"))?,
        };
        if !safe_name(&program.name) {
            return Err(invalid("the program's name is not a plain name"));
        }
        if program.size > MAX_PROGRAM_BYTES {
            return Err(ReleaseError::NotAProgram(format!("the program is larger than {MAX_PROGRAM_BYTES} bytes")));
        }
        let sha256 = checksum_for(&sums, &program.name).ok_or_else(|| invalid(format!("SHA256SUMS has no line for {}", program.name)))?;
        cross_check(&build_info, &program.name, &sha256)?;
        let version = build_info.as_ref().and_then(|info| info.version.clone()).or_else(|| Some(release.tag_name.trim_start_matches('v').to_owned()));
        Ok(ShellRelease {
            version,
            tag: Some(release.tag_name.clone()),
            file_name: program.name.clone(),
            size: program.size,
            sha256,
            build_info,
            origin: format!("github:{repo}@{}", release.tag_name),
            from_cache: false,
            location: Location::Url(program.browser_download_url.clone()),
        })
    }

    fn cached_path(&self, release: &ShellRelease) -> Option<PathBuf> {
        let tag = release.tag.as_deref()?;
        (safe_name(tag) && safe_name(&release.file_name)).then(|| self.cache_dir.join(tag).join(&release.file_name))
    }

    async fn last_cached(&self) -> Option<ShellRelease> {
        let text = tokio::fs::read_to_string(self.cache_dir.join("last-release.json")).await.ok()?;
        let mut release: ShellRelease = serde_json::from_str(&text).ok()?;
        release.location = Location::Cached;
        let path = self.cached_path(&release)?;
        let bytes = tokio::fs::read(&path).await.ok()?;
        verify(&release, &bytes).ok()?;
        Some(release)
    }

    /// The program's bytes, verified: from the cache when a verified copy is
    /// there, else downloaded, verified and cached. `progress` receives
    /// (bytes so far, total when known).
    pub async fn fetch(&self, release: &ShellRelease, progress: &mut (dyn FnMut(u64, Option<u64>) + Send)) -> Result<Vec<u8>, ReleaseError> {
        if let Location::File(path) = &release.location {
            let bytes = tokio::fs::read(path).await.map_err(|error| ReleaseError::Io(format!("{}: {error}", release.file_name)))?;
            verify(release, &bytes)?;
            progress(bytes.len() as u64, Some(bytes.len() as u64));
            return Ok(bytes);
        }
        let cached = self.cached_path(release);
        if let Some(path) = &cached
            && let Ok(bytes) = tokio::fs::read(path).await {
                if verify(release, &bytes).is_ok() {
                    progress(bytes.len() as u64, Some(bytes.len() as u64));
                    return Ok(bytes);
                }
                tracing::warn!(file = release.file_name, "the cached program does not match its release; downloading it again");
            }
        let Location::Url(url) = &release.location else {
            return Err(ReleaseError::Io("the cached program is gone".into()));
        };
        let bytes = self.get(url, MAX_PROGRAM_BYTES, progress).await?;
        verify(release, &bytes)?;
        if let Some(path) = &cached
            && let Err(error) = self.store_cache(release, path, &bytes) {
                // A cache that cannot be written costs the next download only.
                tracing::warn!(%error, "the program could not be cached");
            }
        Ok(bytes)
    }

    fn store_cache(&self, release: &ShellRelease, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let folder = path.parent().expect("a cache path has a folder");
        std::fs::create_dir_all(folder)?;
        let temporary = path.with_extension("part");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, path)?;
        let pointer = self.cache_dir.join("last-release.json");
        let temporary = pointer.with_extension("part");
        std::fs::write(&temporary, serde_json::to_vec_pretty(release)?)?;
        std::fs::rename(&temporary, pointer)
    }
}

/// A local `.3dsx`: read, hashed, magic checked. A `SHA256SUMS` beside it
/// that names the file must agree; a `build-info.json` beside it is reported.
async fn resolve_file(path: &Path) -> Result<ShellRelease, ReleaseError> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = tokio::fs::read(path).await.map_err(|error| ReleaseError::Io(format!("{name}: {error}")))?;
    if bytes.len() as u64 > MAX_PROGRAM_BYTES {
        return Err(ReleaseError::NotAProgram(format!("the program is larger than {MAX_PROGRAM_BYTES} bytes")));
    }
    check_magic(&bytes)?;
    let sha256 = sha256_hex(&bytes);
    let folder = path.parent().unwrap_or(Path::new("."));
    if let Ok(sums) = tokio::fs::read_to_string(folder.join("SHA256SUMS")).await
        && let Some(expected) = checksum_for(&sums, &name)
            && expected != sha256 {
                return Err(ReleaseError::Checksum);
            }
    let build_info = match tokio::fs::read_to_string(folder.join("build-info.json")).await {
        Ok(text) => serde_json::from_str::<BuildInfo>(&text).ok().filter(|info| info.files.is_empty() || info.files.iter().any(|file| file.name == name)),
        Err(_) => None,
    };
    cross_check(&build_info, &name, &sha256)?;
    Ok(ShellRelease {
        version: build_info.as_ref().and_then(|info| info.version.clone()),
        tag: None,
        file_name: name.clone(),
        size: bytes.len() as u64,
        sha256,
        build_info,
        origin: format!("file:{name}"),
        from_cache: false,
        location: Location::File(path.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums() {
        let sums = "6a9a86d131d712d3abfac6481b3a65772c5d45e39598d6f1c656d9cc8e4c24a6  pocket-shell-v2026.11.02.3dsx\n\
                    00000000000000000000000000000000000000000000000000000000000000AA *other.3dsx\n";
        assert_eq!(checksum_for(sums, "pocket-shell-v2026.11.02.3dsx").as_deref(), Some("6a9a86d131d712d3abfac6481b3a65772c5d45e39598d6f1c656d9cc8e4c24a6"));
        assert_eq!(checksum_for(sums, "other.3dsx").as_deref(), Some("00000000000000000000000000000000000000000000000000000000000000aa"));
        assert_eq!(checksum_for(sums, "pocket-shell"), None);
    }

    #[test]
    fn dotted_numeric_versions_order_by_number() {
        use std::cmp::Ordering::*;
        assert_eq!(version_order("2026.11.11", "2026.11.2"), Some(Greater));
        assert_eq!(version_order("2026.11.02", "2026.11.2"), Some(Equal));
        assert_eq!(version_order("2026.11", "2026.11.0"), Some(Equal));
        assert_eq!(version_order("2026.11.11", "2026.12.1"), Some(Less));
        assert_eq!(version_order("2027.1.1", "2026.12.31"), Some(Greater));
        assert_eq!(version_order("10", "9"), Some(Greater));
        // Another form is never older or newer.
        for other in ["v2026.11.11", "2026.11.11-dev", "", "2026..11", "abc", "2026.11.x", "99999999999999999999999"] {
            assert_eq!(version_order(other, "2026.11.1"), None, "{other}");
            assert_eq!(version_order("2026.11.1", other), None, "{other}");
            assert!(!is_newer(other, "2026.11.1") && !is_newer("2026.11.1", other));
        }
        assert!(is_newer("2026.11.11", "2026.11.1"));
        assert!(!is_newer("2026.11.11", "2026.11.11"));
        assert!(!is_newer("2026.11.1", "2026.11.11"));
    }

    #[test]
    fn build_info_parses_the_release_file() {
        let text = r#"{"product":"pocket-shell","version":"2026.11.02","manifestVersion":"2026.11.2","source":"458d","runtime":"5d5f","run":"1","files":[{"name":"a.3dsx","sha256":"ab"}]}"#;
        let info: BuildInfo = serde_json::from_str(text).unwrap();
        assert_eq!(info.version.as_deref(), Some("2026.11.02"));
        assert_eq!(info.runtime.as_deref(), Some("5d5f"));
        assert_eq!(info.files.len(), 1);
    }
}
