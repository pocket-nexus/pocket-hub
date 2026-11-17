//! Bringing a package's files into Hub's staging folder: a copy of a folder or
//! the unpacking of a `.tar.gz`, under the same rules.
//!
//! Only regular files, folders and links that stay inside the package are
//! taken; every path is relative and climbs out nowhere; nothing is written
//! through a link; the total size and the number of entries are bounded so a
//! hostile archive cannot fill the disk.

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

/// Every file of a package together, after decompression. Generous for a
/// service with its `node_modules`.
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 100_000;
const MAX_PATH_BYTES: usize = 1024;
const MAX_DEPTH: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("{0}: the path is absolute, climbs out of the package or is too long")]
    Path(String),
    #[error("{0}: a link that points outside the package")]
    Link(String),
    #[error("{0}: only files, folders and links are allowed in a package")]
    Kind(String),
    #[error("{0}: written twice")]
    Duplicate(String),
    #[error("the package is over {} MiB", MAX_TOTAL_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("the package has over {MAX_ENTRIES} entries")]
    TooMany,
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// A path inside the package, checked: relative, only normal components.
fn checked(path: &Path) -> Result<PathBuf, ArchiveError> {
    let shown = || ArchiveError::Path(path.display().to_string());
    if path.as_os_str().len() > MAX_PATH_BYTES {
        return Err(shown());
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return Err(shown()),
        }
    }
    if out.components().count() > MAX_DEPTH {
        return Err(shown());
    }
    Ok(out)
}

/// Whether a link at `at` (inside the package) with `target` resolves inside
/// the package, reading the path alone.
fn link_stays_inside(at: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: Vec<_> = at.parent().map(|p| p.components().collect()).unwrap_or_default();
    for component in target.components() {
        match component {
            Component::Normal(_) => depth.push(component),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// Writes into `root` and keeps the budget.
struct Writer<'a> {
    root: &'a Path,
    bytes: u64,
    entries: usize,
}

impl Writer<'_> {
    fn count(&mut self) -> Result<(), ArchiveError> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(ArchiveError::TooMany);
        }
        Ok(())
    }

    /// Makes the folders above `rel`, refusing to pass through a link.
    fn parents(&self, rel: &Path) -> Result<(), ArchiveError> {
        let mut at = self.root.to_path_buf();
        if let Some(parent) = rel.parent() {
            for part in parent.components() {
                at.push(part);
                match fs::symlink_metadata(&at) {
                    Ok(meta) if meta.is_dir() => {}
                    Ok(_) => return Err(ArchiveError::Path(rel.display().to_string())),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&at)?,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }

    fn dir(&mut self, rel: &Path) -> Result<(), ArchiveError> {
        self.count()?;
        if rel.as_os_str().is_empty() {
            return Ok(());
        }
        self.parents(rel)?;
        let at = self.root.join(rel);
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.is_dir() => Ok(()),
            Ok(_) => Err(ArchiveError::Duplicate(rel.display().to_string())),
            Err(_) => Ok(fs::create_dir(&at)?),
        }
    }

    fn file(&mut self, rel: &Path, executable: bool, size_hint: u64, content: &mut dyn Read) -> Result<(), ArchiveError> {
        self.count()?;
        if rel.as_os_str().is_empty() {
            return Err(ArchiveError::Path(String::new()));
        }
        if self.bytes.saturating_add(size_hint) > MAX_TOTAL_BYTES {
            return Err(ArchiveError::TooLarge);
        }
        self.parents(rel)?;
        let at = self.root.join(rel);
        let mut file = match fs::OpenOptions::new().write(true).create_new(true).open(&at) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Err(ArchiveError::Duplicate(rel.display().to_string())),
            Err(error) => return Err(error.into()),
        };
        // The header's size is not trusted: the bytes actually copied count.
        let left = MAX_TOTAL_BYTES - self.bytes;
        let copied = io::copy(&mut content.take(left + 1), &mut file)?;
        if copied > left {
            return Err(ArchiveError::TooLarge);
        }
        self.bytes += copied;
        fs::set_permissions(&at, fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }))?;
        Ok(())
    }

    fn link(&mut self, rel: &Path, target: &Path) -> Result<(), ArchiveError> {
        self.count()?;
        if rel.as_os_str().is_empty() || !link_stays_inside(rel, target) {
            return Err(ArchiveError::Link(rel.display().to_string()));
        }
        self.parents(rel)?;
        let at = self.root.join(rel);
        if fs::symlink_metadata(&at).is_ok() {
            return Err(ArchiveError::Duplicate(rel.display().to_string()));
        }
        std::os::unix::fs::symlink(target, &at)?;
        Ok(())
    }
}

/// Unpacks a gzip-compressed tar into the empty folder `root`.
pub fn unpack_tar_gz(archive: &Path, root: &Path) -> Result<(), ArchiveError> {
    let file = fs::File::open(archive)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(io::BufReader::new(file)));
    let mut writer = Writer { root, bytes: 0, entries: 0 };
    for entry in tar.entries()? {
        let mut entry = entry?;
        let raw_path = entry.path()?.into_owned();
        let shown = raw_path.display().to_string();
        let rel = checked(&raw_path)?;
        let kind = entry.header().entry_type();
        match kind {
            tar::EntryType::Directory => writer.dir(&rel)?,
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                // macOS's tar adds `._name` entries for extended attributes:
                // metadata of the packer's disk, not the package.
                if rel.file_name().is_some_and(|name| name.to_string_lossy().starts_with("._")) {
                    continue;
                }
                let mode = entry.header().mode().unwrap_or(0o644);
                let size = entry.header().size().unwrap_or(u64::MAX);
                writer.file(&rel, mode & 0o111 != 0, size, &mut entry)?;
            }
            tar::EntryType::Symlink => {
                let target = entry.link_name()?.ok_or_else(|| ArchiveError::Link(shown.clone()))?.into_owned();
                writer.link(&rel, &target)?;
            }
            // Extended headers for the archive as a whole carry nothing a
            // package needs.
            tar::EntryType::XGlobalHeader => {}
            _ => return Err(ArchiveError::Kind(shown)),
        }
    }
    Ok(())
}

/// Copies the folder `source` into the empty folder `root`, leaving out the
/// top-level names in `skip`.
pub fn copy_folder(source: &Path, root: &Path, skip: &[&str]) -> Result<(), ArchiveError> {
    let mut writer = Writer { root, bytes: 0, entries: 0 };
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let mut children: Vec<_> = fs::read_dir(source.join(&rel))?.collect::<Result<_, _>>()?;
        children.sort_by_key(|child| child.file_name());
        for child in children {
            let name = child.file_name();
            if rel.as_os_str().is_empty() && skip.iter().any(|skip| name == *skip) {
                continue;
            }
            let child_rel = checked(&rel.join(&name))?;
            let meta = fs::symlink_metadata(child.path())?;
            let kind = meta.file_type();
            if kind.is_dir() {
                writer.dir(&child_rel)?;
                stack.push(child_rel);
            } else if kind.is_file() {
                let mut file = fs::File::open(child.path())?;
                writer.file(&child_rel, meta.permissions().mode() & 0o111 != 0, meta.len(), &mut file)?;
            } else if kind.is_symlink() {
                writer.link(&child_rel, &fs::read_link(child.path())?)?;
            } else {
                return Err(ArchiveError::Kind(child_rel.display().to_string()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_read_against_their_own_folder() {
        assert!(link_stays_inside(Path::new("node_modules/.bin/x"), Path::new("../pkg/bin.js")));
        assert!(link_stays_inside(Path::new("a"), Path::new("b/c")));
        assert!(!link_stays_inside(Path::new("a"), Path::new("../b")));
        assert!(!link_stays_inside(Path::new("x/a"), Path::new("../../b")));
        assert!(!link_stays_inside(Path::new("a"), Path::new("/etc/passwd")));
    }

    #[test]
    fn paths_must_stay_relative() {
        assert!(checked(Path::new("a/./b")).is_ok());
        assert!(checked(Path::new("../a")).is_err());
        assert!(checked(Path::new("a/../../b")).is_err());
        assert!(checked(Path::new("/a")).is_err());
    }
}
