//! Installing service packages from folders and archives.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use service_host::{BunRuntime, DependencyInstall, FailPoint, InstallError, ManifestError, Registry, Removed};

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

fn manifest(id: &str) -> String {
    format!(r#"{{"service":1,"id":"{id}","entry":"src/service.ts","idleSeconds":12}}"#)
}

/// What Pocket Hub's protocol 2 installed for `id` (the layout of #23):
/// `<version>/` with its manifest, `current.json`, and a data folder.
fn plant_earlier(home: &Path, id: &str) -> PathBuf {
    let folder = home.join("services").join(id);
    fs::create_dir_all(folder.join("1.0.0/src")).unwrap();
    fs::write(folder.join("1.0.0/pocket.service.json"), format!(r#"{{"service":1,"id":"{id}","version":"1.0.0","entry":"src/service.ts"}}"#)).unwrap();
    fs::write(folder.join("1.0.0/src/service.ts"), "old").unwrap();
    fs::write(folder.join("current.json"), r#"{"version":"1.0.0","installedAt":1}"#).unwrap();
    fs::create_dir_all(home.join("service-data").join(id)).unwrap();
    fs::write(home.join("service-data").join(id).join("state.json"), "kept").unwrap();
    folder
}

/// Every path under `dir` with its size and modification time.
fn snapshot(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            out.push((entry.path(), meta.len(), meta.modified().unwrap()));
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    out.sort();
    out
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
async fn a_folder_without_dependencies_is_installed_as_the_dev_release() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let source = package(home.path(), "src", &manifest("com.example.plain"), &[("src/service.ts", "1"), ("bin/run", "#!/bin/sh")]);
    fs::set_permissions(source.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir(source.join(".git")).unwrap();
    fs::write(source.join(".git/HEAD"), "x").unwrap();

    let report = registry.install(&source).await.unwrap();
    assert_eq!(report.dependencies, DependencyInstall::None);
    assert!(!report.replaced_earlier);
    let service = report.service;
    assert_eq!((service.id.as_str(), service.release.as_str(), service.entry.as_str(), service.idle_seconds), ("com.example.plain", "dev", "src/service.ts", 12));
    assert_eq!(service.dir, home.path().join("services/com.example.plain/dev"));
    let index: serde_json::Value = serde_json::from_slice(&fs::read(home.path().join("services/com.example.plain/releases.json")).unwrap()).unwrap();
    assert_eq!(index["releases"][0]["release"], "dev");
    assert_eq!(fs::read_to_string(service.dir.join("src/service.ts")).unwrap(), "1");
    assert_eq!(fs::metadata(service.dir.join("bin/run")).unwrap().permissions().mode() & 0o777, 0o755);
    assert!(!service.dir.join(".git").exists());
    assert_eq!(fs::metadata(&service.data).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(registry.list(), vec![service.clone()]);
    assert_eq!(registry.get("com.example.plain"), Some(service.clone()));
    assert_eq!(registry.release("com.example.plain", "dev"), Some(service));
    assert_eq!(registry.release("com.example.plain", "0123456789abcdef"), None);
    assert_eq!(registry.get("com.example.other"), None);
    assert!(!registry.is_earlier("com.example.plain"));
    // Nothing is left in staging.
    assert_eq!(fs::read_dir(home.path().join("services-staging")).unwrap().count(), 0);
}

#[tokio::test]
async fn a_package_json_without_a_lockfile_installs_nothing() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let source = package(home.path(), "src", &manifest("a"), &[("src/service.ts", ""), ("package.json", "{}")]);
    let report = registry.install(&source).await.unwrap();
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
    let report = registry.install(&source).await.unwrap();
    let DependencyInstall::Installed { output, .. } = &report.dependencies else { panic!("{:?}", report.dependencies) };
    assert!(output.contains("1 package installed"), "{output}");
    let dir = &report.service.dir;
    assert_eq!(fs::read_to_string(dir.join("node_modules/tiny/index.js")).unwrap(), "module.exports = { word: \"tiny-ok\" };\n");
    assert!(!dir.join("node_modules/stale").exists());
    // The installed package runs and finds its dependency.
    let run = std::process::Command::new(&bun).args(["run", "./service.mjs"]).current_dir(dir).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "tiny-ok");

    // A lockfile that does not match package.json fails the install, with
    // Bun's output, and leaves the installed release as it was.
    let broken = home.path().join("broken");
    copy_dir(&fixtures().join("with-lock"), &broken);
    fs::write(broken.join("service.mjs"), "console.log('the broken one');\n").unwrap();
    fs::write(broken.join("package.json"), r#"{"name":"with-lock","dependencies":{"tiny":"file:./tiny","other":"file:./tiny"}}"#).unwrap();
    let error = registry.install(&broken).await.unwrap_err();
    let InstallError::Dependencies { output, .. } = &error else { panic!("{error}") };
    assert!(output.to_lowercase().contains("lockfile"), "{output}");
    assert!(error.to_string().starts_with("`bun install --frozen-lockfile --ignore-scripts` failed"), "{error}");
    let kept = registry.get("com.example.locked").unwrap();
    assert_eq!(fs::read_to_string(kept.dir.join("service.mjs")).unwrap(), fs::read_to_string(fixtures().join("with-lock/service.mjs")).unwrap());
}

/// SERVICE-PROTOCOL.md §1, Dependencies: no dependency's install script runs.
/// The fixture's `tiny` is a trusted dependency whose `postinstall` makes the
/// `built.js` its `index.js` loads: `bun install --frozen-lockfile` runs it
/// (shown first, on a copy), the host's install does not, and the service
/// then fails when it loads the dependency, with the dependency's own error.
#[tokio::test]
async fn a_dependencys_install_script_does_not_run() {
    let Some(bun) = system_bun() else { return };
    let home = temp_home();
    let plain = home.path().join("plain");
    copy_dir(&fixtures().join("with-script"), &plain);
    let ran = std::process::Command::new(&bun).args(["install", "--frozen-lockfile"]).current_dir(&plain).output().unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    assert!(plain.join("node_modules/tiny/built.js").exists(), "without --ignore-scripts, Bun runs the trusted dependency's script");

    let registry = registry(home.path(), Some(bun.clone()));
    let source = home.path().join("with-script");
    copy_dir(&fixtures().join("with-script"), &source);
    let report = registry.install(&source).await.unwrap();
    assert!(matches!(report.dependencies, DependencyInstall::Installed { .. }), "{:?}", report.dependencies);
    let dir = &report.service.dir;
    assert!(dir.join("node_modules/tiny/index.js").exists(), "the dependency is installed");
    assert!(!dir.join("node_modules/tiny/built.js").exists(), "its install script did not run");
    let run = std::process::Command::new(&bun).args(["run", "./service.mjs"]).current_dir(dir).output().unwrap();
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("built.js"), "the service meets the dependency's own error: {stderr}");
}

#[tokio::test]
async fn an_archive_is_installed_with_or_without_a_top_folder() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let source = package(home.path(), "src", &manifest("com.example.tar"), &[("src/service.ts", "x")]);
    let nested = home.path().join("nested.tar.gz");
    tar_gz(&source, "package", &nested);
    let report = registry.install(&nested).await.unwrap();
    assert_eq!(fs::read_to_string(report.service.dir.join("src/service.ts")).unwrap(), "x");

    fs::write(source.join("src/service.ts"), "y").unwrap();
    let flat = home.path().join("flat.tgz");
    tar_gz(&source, ".", &flat);
    let report = registry.install(&flat).await.unwrap();
    assert_eq!(report.service.release, "dev");
    assert_eq!(fs::read_to_string(report.service.dir.join("src/service.ts")).unwrap(), "y");
    // One release folder and the index.
    let mut children: Vec<String> = fs::read_dir(home.path().join("services/com.example.tar")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    children.sort();
    assert_eq!(children, ["dev", "releases.json"]);
}

#[tokio::test]
async fn an_install_keeps_the_data_folder_and_remove_keeps_it_unless_purged() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let first = package(home.path(), "first", &manifest("com.example.up"), &[("src/service.ts", "1")]);
    let data = registry.install(&first).await.unwrap().service.data;
    fs::write(data.join("state.json"), "kept").unwrap();
    let second = package(home.path(), "second", &manifest("com.example.up"), &[("src/service.ts", "2")]);
    let report = registry.install(&second).await.unwrap();
    assert_eq!(report.service.data, data);
    assert_eq!(fs::read_to_string(data.join("state.json")).unwrap(), "kept");
    assert_eq!(fs::read_to_string(report.service.dir.join("src/service.ts")).unwrap(), "2");

    // A release that is not installed is not removed; the one that is, is.
    assert_eq!(registry.remove("com.example.up", Some("0123456789abcdef"), true).await.unwrap(), Removed::Nothing);
    assert_eq!(fs::read_to_string(data.join("state.json")).unwrap(), "kept", "nothing removed, nothing purged");
    assert_eq!(registry.remove("com.example.up", Some("dev"), false).await.unwrap(), Removed::Releases(vec!["dev".into()]));
    assert!(registry.get("com.example.up").is_none());
    assert!(!home.path().join("services/com.example.up").exists());
    assert_eq!(fs::read_to_string(data.join("state.json")).unwrap(), "kept");
    assert_eq!(registry.remove("com.example.up", None, true).await.unwrap(), Removed::Nothing);
    assert!(!data.exists());
}

#[tokio::test]
async fn invalid_manifests_are_refused_and_install_nothing() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let cases: &[(&str, &[(&str, &str)])] = &[
        (r#"{"service":1,"id":"a/b","entry":"d.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":2,"id":"a","entry":"d.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","version":"1.0.0","entry":"d.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","entry":"../d.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","entry":"missing.ts"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","entry":"d.ts","idleSeconds":0}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","entry":"d.ts","name":"x"}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","entry":"d.ts","requires":["net"]}"#, &[("d.ts", "")]),
        (r#"{"service":1,"id":"a","entry":"d.ts","hosts":["*.co.uk"]}"#, &[("d.ts", "")]),
        ("not json", &[("d.ts", "")]),
    ];
    for (i, (manifest, files)) in cases.iter().enumerate() {
        let source = package(home.path(), &format!("bad{i}"), manifest, files);
        let error = registry.install(&source).await.unwrap_err();
        assert!(matches!(error, InstallError::Manifest(_)), "{manifest}: {error}");
    }
    let empty = home.path().join("empty");
    fs::create_dir(&empty).unwrap();
    assert!(matches!(registry.install(&empty).await, Err(InstallError::Manifest(ManifestError::Missing))));
    let not_archive = home.path().join("package.zip");
    fs::write(&not_archive, "x").unwrap();
    assert!(matches!(registry.install(&not_archive).await, Err(InstallError::Source(_))));
    assert!(registry.list().is_empty());
    assert!(!home.path().join("services/a").exists());
}

/// SERVICE-PROTOCOL.md §1, "How a release folder appears": an install that
/// stops between its staging and its rename (a crash; here a failure point
/// that leaves the staging folder as a crash would) leaves no id without its
/// index, and the next start cleans what it left.
#[tokio::test]
async fn an_install_stopped_before_its_rename_leaves_nothing_at_the_next_start() {
    let home = temp_home();
    let source = package(home.path(), "src", &manifest("com.example.crash"), &[("src/service.ts", "1")]);
    {
        let registry = registry(home.path(), None);
        registry.set_fail_point(Some(FailPoint::BeforeRename));
        let error = registry.install(&source).await.unwrap_err();
        assert!(error.to_string().contains("injected failure before the rename"), "{error}");
        assert!(!home.path().join("services/com.example.crash").exists(), "no id folder without its index");
        let staged: Vec<_> = fs::read_dir(home.path().join("services-staging")).unwrap().collect();
        assert_eq!(staged.len(), 1, "the staged install is left as a crash leaves it");
        let staged = staged[0].as_ref().unwrap().path();
        assert!(staged.join("id/releases.json").is_file() && staged.join("id/dev/pocket.service.json").is_file(), "staged with its index");
    }
    // The next start.
    let registry = registry(home.path(), None);
    assert!(!home.path().join("services-staging").exists());
    assert!(registry.list().is_empty() && registry.earlier().is_empty());
    assert!(!registry.is_earlier("com.example.crash"));
}

/// A later release is renamed in before the index is replaced; one the
/// index does not list is removed at the next start, a listed one whose
/// folder is gone is dropped, a half-written index is removed, and the
/// installed release is untouched.
#[tokio::test]
async fn the_next_start_removes_what_the_index_does_not_list() {
    let home = temp_home();
    let first = package(home.path(), "first", &manifest("com.example.later"), &[("src/service.ts", "1")]);
    let second = package(home.path(), "second", &manifest("com.example.later"), &[("src/service.ts", "2")]);
    let folder = home.path().join("services/com.example.later");
    {
        let registry = registry(home.path(), None);
        registry.install(&first).await.unwrap();
        registry.set_fail_point(Some(FailPoint::BeforeIndex));
        let error = registry.install(&second).await.unwrap_err();
        assert!(error.to_string().contains("injected failure before the index"), "{error}");
        // The new folder is in place; the index was not replaced.
        assert_eq!(fs::read_to_string(folder.join("dev/src/service.ts")).unwrap(), "2");
    }
    // What a crashed install of a store release, and a crashed index write,
    // would leave beside it.
    fs::create_dir_all(folder.join("0123456789abcdef/src")).unwrap();
    fs::write(folder.join("0123456789abcdef/pocket.service.json"), manifest("com.example.later")).unwrap();
    fs::write(folder.join(".releases.json.0f0f.tmp"), "{").unwrap();
    let mut index: serde_json::Value = serde_json::from_slice(&fs::read(folder.join("releases.json")).unwrap()).unwrap();
    index["releases"].as_array_mut().unwrap().push(serde_json::json!({"release": "fedcba9876543210", "installedAt": 1}));
    fs::write(folder.join("releases.json"), serde_json::to_vec(&index).unwrap()).unwrap();

    let registry = registry(home.path(), None);
    assert!(!folder.join("0123456789abcdef").exists(), "an unlisted release folder is removed");
    assert!(!folder.join(".releases.json.0f0f.tmp").exists());
    let index: serde_json::Value = serde_json::from_slice(&fs::read(folder.join("releases.json")).unwrap()).unwrap();
    let releases: Vec<&str> = index["releases"].as_array().unwrap().iter().map(|r| r["release"].as_str().unwrap()).collect();
    assert_eq!(releases, ["dev"], "a listed release whose folder is gone is dropped");
    assert_eq!(registry.get("com.example.later").unwrap().release, "dev");
}

/// HOST-PROTOCOL's one rule: an id without `releases.json` is earlier. It is
/// listed, never read or deleted, until the owner removes it or installs the
/// id again (which replaces it); its data folder is kept as any other's.
#[tokio::test]
async fn what_an_earlier_hub_left_is_kept_unread_until_the_owner_acts() {
    let home = temp_home();
    let earlier = plant_earlier(home.path(), "dev.old");
    let other = plant_earlier(home.path(), "dev.again");
    let before = snapshot(&home.path().join("services"));
    let registry = registry(home.path(), None);
    assert_eq!(snapshot(&home.path().join("services")), before, "the start reads, moves and deletes nothing of it");
    assert!(registry.is_earlier("dev.old"));
    assert_eq!(registry.earlier(), ["dev.again", "dev.old"]);
    assert_eq!(registry.get("dev.old"), None);
    assert!(registry.list().is_empty());
    assert_eq!(registry.remove("dev.old", Some("dev"), false).await.unwrap(), Removed::Nothing, "it has no release to name");
    assert!(earlier.exists());

    // The owner removes one: the folder goes, the data stays unless purged.
    assert_eq!(registry.remove("dev.old", None, false).await.unwrap(), Removed::Earlier);
    assert!(!earlier.exists());
    assert_eq!(fs::read_to_string(home.path().join("service-data/dev.old/state.json")).unwrap(), "kept");

    // The owner installs the other again: it replaces what was there.
    let source = package(home.path(), "again", &manifest("dev.again"), &[("src/service.ts", "new")]);
    let report = registry.install(&source).await.unwrap();
    assert!(report.replaced_earlier);
    assert!(!other.join("1.0.0").exists() && !other.join("current.json").exists());
    assert_eq!(registry.get("dev.again").unwrap().release, "dev");
    assert!(registry.earlier().is_empty());
    assert_eq!(fs::read_to_string(home.path().join("service-data/dev.again/state.json")).unwrap(), "kept");
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
    fs::write(old.join("pocket.daemon.json"), r#"{"daemon":1,"id":"a","entry":"d.ts"}"#).unwrap();
    fs::write(old.join("d.ts"), "").unwrap();
    let error = registry.install(&old).await.unwrap_err();
    assert!(matches!(error, InstallError::Manifest(ManifestError::Missing)), "{error}");
    assert_eq!(error.to_string(), "pocket.service.json is missing");

    let key = package(home.path(), "key", r#"{"daemon":1,"id":"a","entry":"d.ts"}"#, &[("d.ts", "")]);
    let error = registry.install(&key).await.unwrap_err();
    assert!(error.to_string().contains("`service` must be the integer 1"), "{error}");
    assert!(registry.list().is_empty());
}

#[tokio::test]
async fn hostile_archives_are_refused() {
    let home = temp_home();
    let registry = registry(home.path(), None);
    let ok_manifest = br#"{"service":1,"id":"a","entry":"d.ts"}"#;
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
        let error = registry.install(&archive).await.unwrap_err();
        assert!(matches!(error, InstallError::Archive(_)), "{name}: {error}");
    }
    assert!(!home.path().parent().unwrap().join("evil").exists());
    assert!(registry.list().is_empty());

    // An entry link that stays inside is fine to unpack, but the entry must
    // be a real file.
    let archive = home.path().join("inner-link.tar.gz");
    hostile_tar_gz(&archive, &[("pocket.service.json", Regular, "", ok_manifest), ("real.ts", Regular, "", b""), ("d.ts", Symlink, "real.ts", b"")]);
    let error = registry.install(&archive).await.unwrap_err();
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
    let error = registry.install(&archive).await.unwrap_err();
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

    let plain = package(temp.path(), "plain", &manifest("dev.plain"), &[("src/service.ts", "")]);
    let installed = registry.install(&plain).await.unwrap().service;
    let link = installed.dir.join(service_host::SDK_LINK);
    assert_eq!(fs::read_link(&link).unwrap(), sdk);
    assert!(!plain.join("node_modules").exists(), "the source is not touched");

    let own = package(
        temp.path(),
        "own",
        &manifest("dev.own"),
        &[("src/service.ts", ""), ("node_modules/@pocketjs/service/package.json", r#"{"name":"@pocketjs/service","version":"9.9.9"}"#)],
    );
    let installed_own = registry.install(&own).await.unwrap().service;
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
