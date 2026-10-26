//! App preferences in `$XDG_CONFIG_HOME/msi-center/settings.ini` (`key=value` lines). Parsing is pure and
//! lenient: unknown keys and bad values are ignored, so an old or hand-edited file never blocks startup.

use std::io;
use std::path::{Path, PathBuf};

/// Colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// Follow GNOME.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl Theme {
    /// All themes in display order.
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// Stored value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// French label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "Système",
            Self::Light => "Clair",
            Self::Dark => "Sombre",
        }
    }

    /// Parses a stored value.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

/// Preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Colour scheme.
    pub theme: Theme,
    /// When autostarted, stay in the background (no window).
    pub start_minimised: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            start_minimised: true,
        }
    }
}

impl Settings {
    /// Parses the file content.
    #[must_use]
    pub fn parse(content: &str) -> Self {
        let mut s = Self::default();
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match (k.trim(), v.trim()) {
                ("theme", v) => s.theme = Theme::parse(v).unwrap_or(s.theme),
                ("start-minimised", "true") => s.start_minimised = true,
                ("start-minimised", "false") => s.start_minimised = false,
                _ => {}
            }
        }
        s
    }

    /// File content.
    #[must_use]
    pub fn serialize(&self) -> String {
        format!(
            "# MSI Center preferences\ntheme={}\nstart-minimised={}\n",
            self.theme.as_str(),
            self.start_minimised
        )
    }
}

/// Path of the settings file for a given XDG config directory.
#[must_use]
pub fn path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("msi-center").join("settings.ini")
}

/// Loads the settings (defaults if the file is missing or unreadable).
#[must_use]
pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path).map_or_else(|_| Settings::default(), |c| Settings::parse(&c))
}

/// Saves the settings atomically.
///
/// # Errors
/// I/O errors.
pub fn save(path: &Path, s: &Settings) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("ini.tmp");
    std::fs::write(&tmp, s.serialize())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_lenient_parsing() {
        let s = Settings {
            theme: Theme::Dark,
            start_minimised: false,
        };
        assert_eq!(Settings::parse(&s.serialize()), s);
        assert_eq!(
            Settings::parse("theme=purple\nstart-minimised=maybe\njunk\n"),
            Settings::default()
        );
        assert_eq!(Settings::parse(" theme = light ").theme, Theme::Light);
        assert_eq!(Theme::Dark.label(), "Sombre");
    }

    #[test]
    fn load_and_save() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_in(dir.path());
        assert_eq!(load(&p), Settings::default());
        let s = Settings {
            theme: Theme::Light,
            start_minimised: true,
        };
        save(&p, &s).unwrap();
        assert_eq!(load(&p), s);
    }
}
