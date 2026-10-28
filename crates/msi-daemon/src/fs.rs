//! Injectable filesystem root.
//!
//! Every sysfs/procfs access in the daemon goes through an [`FsRoot`], so tests run against a fake tree in a
//! temporary directory and never touch the real `/sys` or `/proc`.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Base directory under which `sys/` and `proc/` are resolved (`/` on the real machine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsRoot {
    base: PathBuf,
}

impl FsRoot {
    /// Root of the running system (`/`).
    #[must_use]
    pub fn real() -> Self {
        Self::new("/")
    }

    /// Root at an arbitrary directory (a fake tree in tests).
    #[must_use]
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Path of `rel` under `<base>/sys`.
    #[must_use]
    pub fn sys(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.base.join("sys").join(rel)
    }

    /// Path of `rel` under `<base>/proc`.
    #[must_use]
    pub fn proc(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.base.join("proc").join(rel)
    }

    /// Path of `rel` under `<base>/run` (tmpfs: cleared at boot).
    #[must_use]
    pub fn run(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.base.join("run").join(rel)
    }
}

/// Reads a small text attribute and trims surrounding whitespace.
pub(crate) fn read_trimmed(path: &Path) -> io::Result<String> {
    fs::read_to_string(path).map(|s| s.trim().to_owned())
}

/// Reads an unsigned integer attribute; `None` if missing or malformed.
pub(crate) fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path).ok()?.parse().ok()
}

/// Reads a signed integer attribute; `None` if missing or malformed.
pub(crate) fn read_i64(path: &Path) -> Option<i64> {
    read_trimmed(path).ok()?.parse().ok()
}

/// Writes `value` to an **existing** attribute file. Never creates files: a typo in a path fails instead of
/// silently creating a regular file.
pub(crate) fn write_attr(path: &Path, value: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(false)
        .open(path)?;
    file.write_all(value.as_bytes())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod testutil {
    //! Helpers to build fake sysfs/procfs trees.
    use std::fs;
    use std::path::Path;

    pub fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    pub fn symlink(root: &Path, rel: &str, target: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, path).unwrap();
    }

    pub fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::testutil::write;
    use super::*;

    #[test]
    fn paths_are_resolved_under_the_base() {
        let root = FsRoot::new("/tmp/x");
        assert_eq!(root.sys("bus/pci"), PathBuf::from("/tmp/x/sys/bus/pci"));
        assert_eq!(root.proc("stat"), PathBuf::from("/tmp/x/proc/stat"));
        assert_eq!(FsRoot::real().sys("class"), PathBuf::from("/sys/class"));
    }

    #[test]
    fn read_helpers_trim_and_parse() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a", " 42\n");
        write(dir.path(), "b", "-7\n");
        write(dir.path(), "c", "nope");
        assert_eq!(read_u64(&dir.path().join("a")), Some(42));
        assert_eq!(read_i64(&dir.path().join("b")), Some(-7));
        assert_eq!(read_u64(&dir.path().join("c")), None);
        assert_eq!(read_u64(&dir.path().join("missing")), None);
    }

    #[test]
    fn write_attr_replaces_existing_file_but_never_creates_one() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "control", "on\n");
        write_attr(&dir.path().join("control"), "auto").unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("control")).unwrap(), "auto");
        assert!(write_attr(&dir.path().join("absent"), "1").is_err());
        assert!(!dir.path().join("absent").exists());
    }
}
