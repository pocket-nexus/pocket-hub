//! The Bun runtime: a download from a local release server laid out as
//! GitHub's, and (ignored by default) the real download.

mod common;

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::temp_home;
use service_host::{BunRuntime, RuntimeError, BUN_VERSION};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FAKE_VERSION: &str = "0.0.1-test";

/// A release zip as Bun publishes it, holding a `bun` that prints `version`.
fn fake_release(version: &str) -> Vec<u8> {
    let asset = BunRuntime::asset_name().unwrap();
    let folder = asset.trim_end_matches(".zip");
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(0o644);
    zip.add_directory(format!("{folder}/"), options).unwrap();
    zip.start_file(format!("{folder}/bun"), options).unwrap();
    zip.write_all(format!("#!/bin/sh\necho {version}\n").as_bytes()).unwrap();
    zip.write_all(&vec![b'#'; 256 * 1024]).unwrap();
    zip.finish().unwrap().into_inner()
}

/// Serves `files` by path over HTTP/1.1 and counts the requests per path.
/// Bodies go out in two halves 150 ms apart, so concurrent callers overlap.
async fn serve(files: HashMap<String, Vec<u8>>) -> (String, Arc<Mutex<HashMap<String, usize>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(Mutex::new(HashMap::new()));
    let files = Arc::new(files);
    let counted = hits.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (files, hits) = (files.clone(), counted.clone());
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_owned();
                let path = line.split(' ').nth(1).unwrap_or("").to_owned();
                *hits.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
                match files.get(&path) {
                    Some(body) => {
                        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        stream.write_all(head.as_bytes()).await.unwrap();
                        let half = body.len() / 2;
                        stream.write_all(&body[..half]).await.unwrap();
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        stream.write_all(&body[half..]).await.unwrap();
                    }
                    None => {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    }
                }
            });
        }
    });
    (base, hits)
}

fn release_files(zip: &[u8], published: Option<String>) -> HashMap<String, Vec<u8>> {
    let asset = BunRuntime::asset_name().unwrap();
    let sum = published.unwrap_or_else(|| hex::encode(Sha256::digest(zip)));
    let sums = format!("0000000000000000000000000000000000000000000000000000000000000000  bun-other.zip\n{sum}  {asset}\n");
    HashMap::from([
        (format!("/bun-v{FAKE_VERSION}/SHASUMS256.txt"), sums.into_bytes()),
        (format!("/bun-v{FAKE_VERSION}/{asset}"), zip.to_vec()),
    ])
}

#[tokio::test]
async fn concurrent_callers_share_one_checked_download() {
    let home = temp_home();
    let zip = fake_release(FAKE_VERSION);
    let (base, hits) = serve(release_files(&zip, None)).await;
    let runtime = BunRuntime::with_source(home.path(), None, &base, FAKE_VERSION);
    assert_eq!(runtime.available(), None);

    let progress = Arc::new(Mutex::new(Vec::new()));
    let seen = progress.clone();
    let other = runtime.clone();
    let (a, b, c) = tokio::join!(runtime.ensure_with(move |p| seen.lock().unwrap().push(p)), runtime.ensure(), other.ensure());
    let path = a.unwrap();
    assert_eq!(b.unwrap(), path);
    assert_eq!(c.unwrap(), path);
    assert_eq!(path, home.path().join(format!("runtimes/bun-{FAKE_VERSION}/bun")));
    let asset = BunRuntime::asset_name().unwrap();
    assert_eq!(hits.lock().unwrap().get(&format!("/bun-v{FAKE_VERSION}/{asset}")), Some(&1));

    let progress = progress.lock().unwrap().clone();
    let last = progress.last().unwrap();
    assert_eq!((last.done, last.total), (zip.len() as u64, Some(zip.len() as u64)));
    assert!(progress.windows(2).all(|pair| pair[0].done <= pair[1].done));

    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
    let printed = std::process::Command::new(&path).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&printed.stdout).trim(), FAKE_VERSION);
    // Nothing but the runtime is left.
    let names: Vec<_> = std::fs::read_dir(home.path().join("runtimes")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, vec![std::ffi::OsString::from(format!("bun-{FAKE_VERSION}"))]);

    // Present now: no request.
    assert_eq!(runtime.ensure().await.unwrap(), path);
    assert_eq!(hits.lock().unwrap().values().sum::<usize>(), 2);
}

#[tokio::test]
async fn a_download_that_does_not_match_its_checksum_installs_nothing() {
    let home = temp_home();
    let zip = fake_release(FAKE_VERSION);
    let (base, _) = serve(release_files(&zip, Some("ab".repeat(32)))).await;
    let runtime = BunRuntime::with_source(home.path(), None, &base, FAKE_VERSION);
    let error = runtime.ensure().await.unwrap_err();
    assert!(matches!(error, RuntimeError::Checksum { .. }), "{error}");
    assert_eq!(std::fs::read_dir(home.path().join("runtimes")).unwrap().count(), 0);
}

#[tokio::test]
async fn a_binary_that_is_not_the_pinned_version_installs_nothing() {
    let home = temp_home();
    let zip = fake_release("9.9.9");
    let (base, _) = serve(release_files(&zip, None)).await;
    let runtime = BunRuntime::with_source(home.path(), None, &base, FAKE_VERSION);
    let error = runtime.ensure().await.unwrap_err();
    assert!(matches!(error, RuntimeError::Verify(_)), "{error}");
    assert_eq!(std::fs::read_dir(home.path().join("runtimes")).unwrap().count(), 0);
    // A failed download is not remembered: the next call tries again.
    assert!(matches!(runtime.ensure().await, Err(RuntimeError::Verify(_))));
}

#[tokio::test]
async fn a_configured_bun_is_used_as_is() {
    let home = temp_home();
    let runtime = BunRuntime::new(home.path(), Some(home.path().join("missing-bun")));
    assert!(matches!(runtime.ensure().await, Err(RuntimeError::Override(_))));
    std::fs::write(home.path().join("bun"), "").unwrap();
    let runtime = BunRuntime::new(home.path(), Some(home.path().join("bun")));
    assert_eq!(runtime.ensure().await.unwrap(), home.path().join("bun"));
}

/// The real download from GitHub. Run with
/// `cargo test -p pocket-service-host --test runtime -- --ignored`.
#[tokio::test]
#[ignore = "downloads Bun from GitHub"]
async fn the_pinned_bun_downloads_and_runs() {
    let home = temp_home();
    let runtime = BunRuntime::new(home.path(), None);
    let mut last = None;
    let path = runtime.ensure_with(|p| last = Some(p)).await.unwrap();
    let last = last.unwrap();
    eprintln!("downloaded {} of {:?} bytes to {}", last.done, last.total, path.display());
    let printed = std::process::Command::new(&path).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&printed.stdout).trim(), BUN_VERSION);
}
