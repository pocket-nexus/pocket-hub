//! The program's release: a GitHub-shaped API served on loopback, the cache,
//! the checks, and (ignored) the real GitHub release.

mod support;

use pocket_hub_setup::release::{ReleaseError, ReleaseFetcher, sha256_hex};
use pocket_hub_setup::{InstallOptions, InstallTarget, ProgramAction, ShellSource, Stage, install_3ds};
use support::*;

const TAG: &str = "v2026.01.01";
const NAME: &str = "pocket-shell-v2026.01.01.3dsx";

async fn github(program: &[u8], sums: Option<String>) -> FakeHttp {
    let sums = sums.unwrap_or_else(|| format!("{}  {NAME}\n", sha256_hex(program)));
    let info = format!(
        r#"{{"product":"pocket-shell","version":"2026.01.01","source":"aaaa","runtime":"bbbb","files":[{{"name":"{NAME}","sha256":"{}"}}]}}"#,
        sha256_hex(program)
    );
    let latest = format!(
        r#"{{"tag_name":"{TAG}","assets":[
            {{"name":"build-info.json","size":{},"browser_download_url":"{{base}}/dl/build-info.json"}},
            {{"name":"{NAME}","size":{},"browser_download_url":"{{base}}/dl/{NAME}"}},
            {{"name":"SHA256SUMS","size":{},"browser_download_url":"{{base}}/dl/SHA256SUMS"}}]}}"#,
        info.len(),
        program.len(),
        sums.len()
    );
    FakeHttp::start(vec![
        ("/repos/test/shell/releases/latest".into(), latest.into_bytes()),
        (format!("/dl/{NAME}"), program.to_vec()),
        ("/dl/SHA256SUMS".into(), sums.into_bytes()),
        ("/dl/build-info.json".into(), info.into_bytes()),
    ])
    .await
}

fn source(http: &FakeHttp) -> ShellSource {
    ShellSource::GitHub { api: http.base.clone(), repo: "test/shell".into(), tag: None }
}

#[tokio::test]
async fn a_release_is_downloaded_verified_and_cached() {
    let program = fake_3dsx(70_000, 20);
    let http = github(&program, None).await;
    let cache = temp("release-cache");
    let fetcher = ReleaseFetcher::new(&cache, &source(&http)).unwrap();
    let release = fetcher.resolve(&source(&http)).await.unwrap();
    assert_eq!(release.tag.as_deref(), Some(TAG));
    assert_eq!(release.version.as_deref(), Some("2026.01.01"));
    assert_eq!(release.file_name, NAME);
    assert_eq!(release.size, program.len() as u64);
    assert_eq!(release.sha256, sha256_hex(&program));
    assert_eq!(release.build_info.as_ref().unwrap().runtime.as_deref(), Some("bbbb"));
    assert_eq!(release.origin, format!("github:test/shell@{TAG}"));
    assert!(!release.from_cache);

    let mut last = 0;
    let bytes = fetcher.fetch(&release, &mut |done, _| last = done).await.unwrap();
    assert_eq!(bytes, program);
    assert_eq!(last, program.len() as u64);
    assert_eq!(std::fs::read(cache.join(TAG).join(NAME)).unwrap(), program);
    // The second fetch is the cache's.
    assert_eq!(fetcher.fetch(&release, &mut |_, _| {}).await.unwrap(), program);
    assert_eq!(http.hits(&format!("/dl/{NAME}")), 1);

    // Offline: the last verified release in the cache stands in.
    let offline = ShellSource::GitHub { api: "http://127.0.0.1:9".into(), repo: "test/shell".into(), tag: None };
    let fetcher = ReleaseFetcher::new(&cache, &offline).unwrap();
    let cached = fetcher.resolve(&offline).await.unwrap();
    assert!(cached.from_cache);
    assert_eq!(cached.sha256, release.sha256);
    assert_eq!(fetcher.fetch(&cached, &mut |_, _| {}).await.unwrap(), program);
}

#[tokio::test]
async fn a_program_that_does_not_match_its_checksum_is_refused() {
    let program = fake_3dsx(10_000, 21);
    let cache = temp("release-bad");

    // build-info.json and SHA256SUMS disagree: refused before the download.
    let http = github(&program, Some(format!("{}  {NAME}\n", "0".repeat(64)))).await;
    let fetcher = ReleaseFetcher::new(&cache, &source(&http)).unwrap();
    assert!(matches!(fetcher.resolve(&source(&http)).await, Err(ReleaseError::Invalid(_))));

    // SHA256SUMS alone, naming other bytes than the server sends: the
    // download is refused and nothing is cached.
    let bare = FakeHttp::start(vec![
        (
            "/repos/test/shell/releases/latest".into(),
            format!(
                r#"{{"tag_name":"{TAG}","assets":[{{"name":"{NAME}","size":10000,"browser_download_url":"{{base}}/dl/{NAME}"}},{{"name":"SHA256SUMS","size":1,"browser_download_url":"{{base}}/dl/SHA256SUMS"}}]}}"#
            )
            .into_bytes(),
        ),
        (format!("/dl/{NAME}"), program.clone()),
        ("/dl/SHA256SUMS".into(), format!("{}  {NAME}\n", sha256_hex(&fake_3dsx(10_000, 22))).into_bytes()),
    ])
    .await;
    let release = fetcher.resolve(&source(&bare)).await.unwrap();
    assert!(release.build_info.is_none());
    assert!(matches!(fetcher.fetch(&release, &mut |_, _| {}).await, Err(ReleaseError::Checksum)));
    assert!(!cache.join(TAG).join(NAME).exists());
    assert!(!cache.join("last-release.json").exists());
}

#[tokio::test]
async fn an_install_downloads_the_release_only_when_it_installs() {
    let program = fake_3dsx(90_000, 24);
    let http = github(&program, None).await;
    let card = temp("release-console");
    let fake = FakeFtp::start(&card).await;
    let store = open_store();
    let mut options = InstallOptions::new(temp("release-install-cache"));
    options.source = source(&http);
    options.ftp_timeouts = timeouts();
    let target = InstallTarget::Ftp { address: fake.address };

    let mut events = Vec::new();
    let report = install_3ds(&target, &store, &options, &mut |event| events.push(event)).await;
    assert!(report.ok, "{report:?}");
    assert_eq!(report.program.action, ProgramAction::Installed);
    assert_eq!(report.program.release.as_ref().unwrap().build_info.as_ref().unwrap().runtime.as_deref(), Some("bbbb"));
    assert_eq!(std::fs::read(card.join("3ds/pocket-shell.3dsx")).unwrap(), program);
    let downloads: Vec<_> = events.iter().filter(|event| event.stage == Stage::Downloading).collect();
    assert_eq!(downloads.last().unwrap().done, program.len() as u64);

    // Installed already: the description is fetched again, the program not.
    let report = install_3ds(&target, &store, &options, &mut |_| {}).await;
    assert_eq!(report.program.action, ProgramAction::AlreadyInstalled);
    assert_eq!(http.hits(&format!("/dl/{NAME}")), 1);
}

/// The real release on GitHub. Run with
/// `cargo test -p pocket-hub-setup --test release -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "downloads the real release from GitHub"]
async fn the_github_release() {
    let cache = temp("github");
    let source = ShellSource::default();
    let fetcher = ReleaseFetcher::new(&cache, &source).unwrap();
    let release = fetcher.resolve(&source).await.unwrap();
    let bytes = fetcher.fetch(&release, &mut |_, _| {}).await.unwrap();
    assert_eq!(&bytes[..4], b"3DSX");
    assert_eq!(sha256_hex(&bytes), release.sha256);
    println!("origin:   {}", release.origin);
    println!("file:     {} ({} bytes)", release.file_name, release.size);
    println!("sha256:   {}", release.sha256);
    println!("version:  {:?}", release.version);
    println!("build:    {}", serde_json::to_string(&release.build_info).unwrap());
    println!("cached:   {}", cache.join(release.tag.as_deref().unwrap()).join(&release.file_name).exists());
}
