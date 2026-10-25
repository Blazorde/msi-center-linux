//! Storage locations (ADR-0005).
//!
//! - Session (unpinned) DB: `$XDG_RUNTIME_DIR/clipd/session.db`. `XDG_RUNTIME_DIR` is mandatory: there
//!   is deliberately no fallback to a disk location, so unpinned clips can never reach the disk.
//! - Pinned DB: `$XDG_DATA_HOME/clipd/clips.db`, `XDG_DATA_HOME` defaulting to `$HOME/.local/share`.
//!   Relative values are ignored, as the XDG Base Directory spec requires.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use thiserror::Error;

pub const SESSION_DB: &str = "session.db";
pub const PINNED_DB: &str = "clips.db";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathError {
    #[error("XDG_RUNTIME_DIR is not set or not absolute; refusing to store clips anywhere else")]
    NoRuntimeDir,
    #[error("neither XDG_DATA_HOME nor HOME is set to an absolute path")]
    NoDataDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Directory of the session DB (tmpfs).
    pub session_dir: PathBuf,
    /// Directory of the pinned DB (disk).
    pub data_dir: PathBuf,
}

fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|p| p.is_absolute())
}

impl Paths {
    /// Explicit locations (tests, or callers that know better).
    pub fn new(session_dir: impl Into<PathBuf>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            session_dir: session_dir.into(),
            data_dir: data_dir.into(),
        }
    }

    /// Resolves locations from environment variables read through `var`.
    ///
    /// # Errors
    ///
    /// [`PathError`] if a required variable is missing or relative.
    pub fn from_env(var: impl Fn(&str) -> Option<OsString>) -> Result<Self, PathError> {
        let runtime = absolute(var("XDG_RUNTIME_DIR")).ok_or(PathError::NoRuntimeDir)?;
        let data = absolute(var("XDG_DATA_HOME"))
            .or_else(|| absolute(var("HOME")).map(|h| h.join(".local/share")))
            .ok_or(PathError::NoDataDir)?;
        Ok(Self::new(runtime.join("clipd"), data.join("clipd")))
    }

    #[must_use]
    pub fn session_db(&self) -> PathBuf {
        self.session_dir.join(SESSION_DB)
    }

    #[must_use]
    pub fn pinned_db(&self) -> PathBuf {
        self.data_dir.join(PINNED_DB)
    }

    #[must_use]
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn uses_xdg_dirs() {
        let p = Paths::from_env(env(&[
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("XDG_DATA_HOME", "/data"),
            ("HOME", "/home/u"),
        ]))
        .unwrap();
        assert_eq!(p.session_db(), PathBuf::from("/run/user/1000/clipd/session.db"));
        assert_eq!(p.pinned_db(), PathBuf::from("/data/clipd/clips.db"));
    }

    #[test]
    fn data_home_defaults_to_home_local_share() {
        let p = Paths::from_env(env(&[
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("XDG_DATA_HOME", "relative/ignored"),
            ("HOME", "/home/u"),
        ]))
        .unwrap();
        assert_eq!(
            p.pinned_db(),
            PathBuf::from("/home/u/.local/share/clipd/clips.db")
        );
    }

    #[test]
    fn runtime_dir_is_mandatory() {
        assert_eq!(
            Paths::from_env(env(&[("HOME", "/home/u")])),
            Err(PathError::NoRuntimeDir)
        );
        assert_eq!(
            Paths::from_env(env(&[("XDG_RUNTIME_DIR", "run"), ("HOME", "/home/u")])),
            Err(PathError::NoRuntimeDir)
        );
    }

    #[test]
    fn data_dir_is_mandatory() {
        assert_eq!(
            Paths::from_env(env(&[("XDG_RUNTIME_DIR", "/run/user/1000")])),
            Err(PathError::NoDataDir)
        );
    }
}
