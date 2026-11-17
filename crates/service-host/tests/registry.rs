//! Installing service packages from folders and archives.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use service_host::{BunRuntime, DependencyInstall, InstallError, ManifestError, Registry};

fn registry(home: &Path, bun: Option<PathBuf>) -> Registry {
    // Without a Bun the registry still installs packages that need none; a
    // missing override fails only when `bun install` would run.
    let bun = bun.unwrap_or_else(|| home.join("no-bun"));
    Registry::new(home, BunRuntime::new(home, Some(bun)), Duration::from_secs(60))
}

fn package(parent: &Path, name: &str, manifest: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = parent.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("pocket.service.json"), manifest).unwrap();
    for (path, content) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    dir
}

fn manifest(id: &str, version: &str) -> String {
    format!(r#"{{"service":1,"id":"{id}","version":"{version}","entry":"src/service.ts","idleSeconds":12}}"#)
}

/// A `.tar.gz` of `dir`, under `prefix` in the archive.
fn tar_gz(dir: &Path, prefix: &str, to: &Path) {
    let file = fs::File::create(to).unwrap();
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::fast()));
    builder.append_dir_all(prefix, dir).unwrap();
    builder.into_inner().unwrap().finish().unwrap();
}

/// A `.tar.gz` whose entries are written with raw names, which `tar`'s own
/// builder refuses: `(name, kind, link target, content)`.
type RawEntry<'a> = (&'a str, tar::EntryType, &'a str, &'a [u8]);

fn hostile_tar_gz(to: &Path, entries: &[RawEntry]) {
    let file = fs::File::create(to).unwrap();
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::fast()));
    for (name, kind, link, content) in entries {
        let mut header = tar::Header::new_old();
        let raw = header.as_old_mut();
        raw.name[..name.len()].copy_from_slice(name.as_bytes());
        raw.linkname[..link.len()].copy_from_slice(link.as_bytes());
        header.set_entry_type(*kind);
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, *content).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
}

#[tokio::test]
async fn a_folder_without_dependencies_is_installed() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let source = package(home.path(), "src", &manifest("com.example.plain", "1.0.0"), &[("src/service.ts", "1"), ("bin/run", "#!/bin/sh")]);
    fs::set_permissions(source.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir(source.join(".git")).unwrap();
    fs::write(source.join(".git/HEAD"), "x").unwrap();

    let report = registry.install(&source, &[]).await.unwrap();
    assert_eq!(report.dependencies, DependencyInstall::None);
    assert_eq!(report.previous, None);
    let service = report.service;
    assert_eq!((service.id.as_str(), service.version.as_str(), service.entry.as_str(), service.idle_seconds), ("com.example.plain", "1.0.0", "src/service.ts", 12));
    assert_eq!(service.dir, home.path().join("services/com.example.plain/1.0.0"));
    assert_eq!(fs::read_to_string(service.dir.join("src/service.ts")).unwrap(), "1");
    assert_eq!(fs::metadata(service.dir.join("bin/run")).unwrap().permissions().mode() & 0o777, 0o755);
    assert!(!service.dir.join(".git").exists());
    assert_eq!(fs::metadata(&service.data).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(registry.list(), vec![service.clone()]);
    assert_eq!(registry.get("com.example.plain"), Some(service));
    assert_eq!(registry.get("com.example.other"), None);
    // Nothing is left in staging.
    assert_eq!(fs::read_dir(home.path().join("services-staging")).unwrap().count(), 0);
}

#[tokio::test]
async fn a_package_json_without_a_lockfile_installs_nothing() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let source = package(home.path(), "src", &manifest("a", "1.0.0"), &[("src/service.ts", ""), ("package.json", "{}")]);
    let report = registry.install(&source, &[]).await.unwrap();
    assert_eq!(report.dependencies, DependencyInstall::NoLockfile);
    assert!(!report.service.dir.join("node_modules").exists());
}

#[tokio::test]
async fn a_lockfile_is_installed_frozen() {
    let Some(bun) = system_bun() else { return };
    let home = temp_home();
    let registry = registry(home.path(), Some(bun.clone()));
    // A stale node_modules in the source is not copied: the lockfile makes it.
    let source = home.path().join("with-lock");
    copy_dir(&fixtures().join("with-lock"), &source);
    fs::create_dir_all(source.join("node_modules/stale")).unwrap();
    let report = registry.install(&source, &[]).await.unwrap();
    let DependencyInstall::Installed { output, .. } = &report.dependencies else { panic!("{:?}", report.dependencies) };
    assert!(output.contains("1 package installed"), "{output}");
    let dir = &report.service.dir;
    assert_eq!(fs::read_to_string(dir.join("node_modules/tiny/index.js")).unwrap(), "module.exports = { word: \"tiny-ok\" };\n");
    assert!(!dir.join("node_modules/stale").exists());
    // The installed package runs and finds its dependency.
    let run = std::process::Command::new(&bun).args(["run", "./service.mjs"]).current_dir(dir).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "tiny-ok");

    // A lockfile that does not match package.json fails the install, with
    // Bun's output, and leaves the current version as it was.
    let broken = home.path().join("broken");
    copy_dir(&fixtures().join("with-lock"), &broken);
    fs::write(broken.join("pocket.service.json"), r#"{"service":1,"id":"com.example.locked","version":"1.1.0","entry":"service.mjs"}"#).unwrap();
    fs::write(broken.join("package.json"), r#"{"name":"with-lock","dependencies":{"tiny":"file:./tiny","other":"file:./tiny"}}"#).unwrap();
    let error = registry.install(&broken, &[]).await.unwrap_err();
    let InstallError::Dependencies { output, .. } = &error else { panic!("{error}") };
    assert!(output.to_lowercase().contains("lockfile"), "{output}");
    assert_eq!(registry.get("com.example.locked").unwrap().version, "1.0.0");
    assert!(!home.path().join("services/com.example.locked/1.1.0").exists());
}

#[tokio::test]
async fn an_archive_is_installed_with_or_without_a_top_folder() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let source = package(home.path(), "src", &manifest("com.example.tar", "1.0.0"), &[("src/service.ts", "x")]);
    let nested = home.path().join("nested.tar.gz");
    tar_gz(&source, "package", &nested);
    let report = registry.install(&nested, &[]).await.unwrap();
    assert_eq!(fs::read_to_string(report.service.dir.join("src/service.ts")).unwrap(), "x");

    fs::write(source.join("pocket.service.json"), manifest("com.example.tar", "1.0.1")).unwrap();
    let flat = home.path().join("flat.tgz");
    tar_gz(&source, ".", &flat);
    let report = registry.install(&flat, &[]).await.unwrap();
    assert_eq!(report.previous.as_deref(), Some("1.0.0"));
    assert_eq!(report.service.version, "1.0.1");
    // The replaced version is gone; nothing keeps it.
    assert!(!home.path().join("services/com.example.tar/1.0.0").exists());
}

#[tokio::test]
async fn an_upgrade_keeps_the_data_folder_and_remove_keeps_it_unless_purged() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let v1 = package(home.path(), "v1", &manifest("com.example.up", "1.0.0"), &[("src/service.ts", "1")]);
    let data = registry.install(&v1, &[]).await.unwrap().service.data;
    fs::write(data.join("state.json"), "kept").unwrap();
    let v2 = package(home.path(), "v2", &manifest("com.example.up", "2.0.0"), &[("src/service.ts", "2")]);
    // A version a process still runs is kept.
    let report = registry.install(&v2, &["1.0.0".to_owned()]).await.unwrap();
    assert_eq!(report.service.data, data);
    assert_eq!(fs::read_to_string(data.join("state.json")).unwrap(), "kept");
    assert!(home.path().join("services/com.example.up/1.0.0").is_dir());
    registry.prune("com.example.up", &["2.0.0".to_owned()]).await;
    assert!(!home.path().join("services/com.example.up/1.0.0").exists());
    // The same version again replaces it.
    fs::write(v2.join("src/service.ts"), "2b").unwrap();
    let report = registry.install(&v2, &[]).await.unwrap();
    assert_eq!(fs::read_to_string(report.service.dir.join("src/service.ts")).unwrap(), "2b");

    assert!(registry.remove("com.example.up", false).await.unwrap());
    assert!(registry.get("com.example.up").is_none());
    assert!(!home.path().join("services/com.example.up").exists());
    assert_eq!(fs::read_to_string(data.join("state.json")).unwrap(), "kept");
    assert!(!registry.remove("com.example.up", true).await.unwrap());
    assert!(!data.exists());
}

#[tokio::test]
async fn invalid_manifests_are_refused_and_install_nothing() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let cases: &[(&str, &[(&str, &str)])] = &[
        (r#"{"service":1,"id":"a/b","version":"1.0.0","entry":"d.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":2,"id":"a","version":"1.0.0","entry":"d.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","version":"latest","entry":"d.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","version":"1.0.0","entry":"../d.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","version":"1.0.0","entry":"missing.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","version":"1.0.0","entry":"d.ts","idleSeconds":0}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","version":"1.0.0","entry":"d.ts","name":"x"}"#, &[("d.ts", "")]),
        ("not json", &[("d.ts", "")]),
    ];
    for (i, (manifest, files)) in cases.iter().enumerate() {
        let source = package(home.path(), &format!("bad{i}"), manifest, files);
        let error = registry.install(&source, &[]).await.unwrap_err();
        assert!(matches!(error, InstallError::Manifest(_)), "{manifest}: {error}");
    }
    let empty = home.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(matches!(registry.install(&empty, &[]).await, Err(InstallError::Manifest(ManifestError::Missing))));
    let not_archive = home.path().join("package.zip");
    fs::write(&not_archive, "x").unwrap();
    assert!(matches!(registry.install(&not_archive, &[]).await, Err(InstallError::Source(_))));
    assert!(registry.list().is_empty());
}

/// A package of Pocket Hub 0.2.2 and earlier, written before services took
/// their names, is not read under its old names: the line names the file Hub
/// expects, and an old version key in the new file is refused as malformed.
#[tokio::test]
async fn a_package_under_the_old_names_is_refused_with_the_file_hub_expects() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let old = home.path().join("old");
    fs::create_dir(&old).unwrap();
    fs::write(old.join("pocket.daemon.json"), r#"{"daemon":1,"id":"a","version":"1.0.0","entry":"d.ts"}"#).unwrap();
    fs::write(old.join("d.ts"), "").unwrap();
    let error = registry.install(&old, &[]).await.unwrap_err();
    assert!(matches!(error, InstallError::Manifest(ManifestError::Missing)), "{error}");
    assert_eq!(error.to_string(), "pocket.service.json is missing");

    let key = package(home.path(), "key", r#"{"daemon":1,"id":"a","version":"1.0.0","entry":"d.ts"}"#, &[("d.ts", "")]);
    let error = registry.install(&key, &[]).await.unwrap_err();
    assert!(error.to_string().contains("`service` must be the integer 1"), "{error}");
    assert!(registry.list().is_empty());
}

#[tokio::test]
async fn hostile_archives_are_refused() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let ok_manifest = br#"{"service":1,"id":"a","version":"1.0.0","entry":"d.ts"}"#;
    use tar::EntryType::{Link, Regular, Symlink};
    let cases: &[(&str, &[RawEntry])] = &[
        ("climb", &[("pocket.service.json", Regular, "", ok_manifest), ("d.ts", Regular, "", b""), ("../evil", Regular, "", b"x")]),
        ("nested climb", &[("pocket.service.json", Regular, "", ok_manifest), ("d.ts", Regular, "", b""), ("a/../../evil", Regular, "", b"x")]),
        ("absolute", &[("pocket.service.json", Regular, "", ok_manifest), ("/tmp/evil", Regular, "", b"x")]),
        ("link out", &[("pocket.service.json", Regular, "", ok_manifest), ("d.ts", Symlink, "/etc/passwd", b"")]),
        ("relative link out", &[("pocket.service.json", Regular, "", ok_manifest), ("d.ts", Symlink, "../../x", b"")]),
        ("write through a link", &[("pocket.service.json", Regular, "", ok_manifest), ("dir", Symlink, ".", b""), ("dir/d.ts", Regular, "", b"")]),
        ("hard link", &[("pocket.service.json", Regular, "", ok_manifest), ("d.ts", Link, "pocket.service.json", b"")]),
        ("twice", &[("pocket.service.json", Regular, "", ok_manifest), ("pocket.service.json", Regular, "", ok_manifest)]),
    ];
    for (name, entries) in cases {
        let archive = home.path().join(format!("{}.tar.gz", name.replace(' ', "-")));
        hostile_tar_gz(&archive, entries);
        let error = registry.install(&archive, &[]).await.unwrap_err();
        assert!(matches!(error, InstallError::Archive(_)), "{name}: {error}");
    }
    assert!(!home.path().parent().unwrap().join("evil").exists());
    assert!(registry.list().is_empty());

    // An entry link that stays inside is fine to unpack, but the entry must
    // be a real file.
    let archive = home.path().join("inner-link.tar.gz");
    hostile_tar_gz(&archive, &[("pocket.service.json", Regular, "", ok_manifest), ("real.ts", Regular, "", b""), ("d.ts", Symlink, "real.ts", b"")]);
    let error = registry.install(&archive, &[]).await.unwrap_err();
    assert!(matches!(error, InstallError::Manifest(ManifestError::EntryPath(_))), "{error}");
}

#[tokio::test]
async fn an_archive_over_the_size_limit_is_refused() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    // A header that claims more than the limit is refused before any byte.
    let archive = home.path().join("huge.tar.gz");
    let file = fs::File::create(&archive).unwrap();
    let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut header = tar::Header::new_gnu();
    header.set_path("big.bin").unwrap();
    header.set_size(600 * 1024 * 1024);
    header.set_mode(0o644);
    header.set_cksum();
    std::io::Write::write_all(&mut encoder, header.as_bytes()).unwrap();
    std::io::Write::write_all(&mut encoder, &vec![0u8; 1024 * 1024]).unwrap();
    encoder.finish().unwrap();
    let error = registry.install(&archive, &[]).await.unwrap_err();
    assert!(matches!(&error, InstallError::Archive(_)) && error.to_string().contains("over 512 MiB"), "{error}");
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Hub provides the SDK (§1): a package without its own
/// `node_modules/@pocketjs/service` gets a link to Hub's copy at install; a
/// package that brings its own keeps it; a link removed after the install is
/// made again by `link_sdk`, which the supervisor runs before each start.
#[tokio::test]
async fn hubs_sdk_is_linked_into_packages_without_their_own() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let sdk = temp.path().join("sdk");
    fs::create_dir_all(&sdk).unwrap();
    fs::write(sdk.join("package.json"), r#"{"name":"@pocketjs/service"}"#).unwrap();
    let registry = registry(&home, None).with_sdk(Some(sdk.clone()));

    let plain = package(temp.path(), "plain", &manifest("dev.plain", "1.0.0"), &[("src/service.ts", "")]);
    let installed = registry.install(&plain, &[]).await.unwrap().service;
    let link = installed.dir.join(service_host::SDK_LINK);
    assert_eq!(fs::read_link(&link).unwrap(), sdk);
    assert!(!plain.join("node_modules").exists(), "the source is not touched");

    let own = package(
        temp.path(),
        "own",
        &manifest("dev.own", "1.0.0"),
        &[("src/service.ts", ""), ("node_modules/@pocketjs/service/package.json", r#"{"name":"@pocketjs/service","version":"9.9.9"}"#)],
    );
    let installed_own = registry.install(&own, &[]).await.unwrap().service;
    let kept = installed_own.dir.join(service_host::SDK_LINK);
    assert!(!fs::symlink_metadata(&kept).unwrap().file_type().is_symlink());
    assert!(fs::read_to_string(kept.join("package.json")).unwrap().contains("9.9.9"));
    assert!(!service_host::link_sdk(&installed_own.dir, &sdk).unwrap());

    // Removed, and dangling: both are made again.
    fs::remove_file(&link).unwrap();
    assert!(service_host::link_sdk(&installed.dir, &sdk).unwrap());
    assert_eq!(fs::read_link(&link).unwrap(), sdk);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(temp.path().join("gone"), &link).unwrap();
    assert!(service_host::link_sdk(&installed.dir, &sdk).unwrap());
    assert_eq!(fs::read_link(&link).unwrap(), sdk);
}
