//! Login autostart through an XDG autostart entry (`~/.config/autostart/org.blazorde.MsiCenter.desktop`).
//!
//! The content and the parsing are pure; [`enable`] / [`disable`] / [`read`] are thin I/O wrappers.

use std::io;
use std::path::{Path, PathBuf};

/// File name of the entry in `$XDG_CONFIG_HOME/autostart/`.
pub const FILE_NAME: &str = "org.blazorde.MsiCenter.desktop";

/// Command-line flag: start without a window.
pub const BACKGROUND_FLAG: &str = "--background";

/// Content of the autostart entry. `exec` is the program (a name in `PATH` or an absolute path).
/// `entry("msi-center", true)` is byte for byte `data/org.blazorde.MsiCenter-autostart.desktop`.
#[must_use]
pub fn entry(exec: &str, background: bool) -> String {
    let args = if background {
        format!(" {BACKGROUND_FLAG}")
    } else {
        String::new()
    };
    let exec = quote_exec(exec);
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=MSI Center\n\
         Comment=Démarre MSI Center en arrière-plan à l'ouverture de session\n\
         Exec={exec}{args}\n\
         Icon=org.blazorde.MsiCenter\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n\
         X-GNOME-Autostart-Delay=2\n"
    )
}

/// Quotes a program path for `Exec=` if it contains characters reserved by the desktop entry spec.
fn quote_exec(exec: &str) -> String {
    const RESERVED: &[char] = &[
        ' ', '\t', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
    ];
    if exec.contains(RESERVED) {
        let escaped = exec
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$");
        format!("\"{escaped}\"")
    } else {
        exec.to_owned()
    }
}

fn values<'a>(content: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    content.lines().filter_map(move |l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then_some(v.trim())
    })
}

/// Whether an existing entry is active: GNOME (or the user) may have disabled it with `Hidden=true` or
/// `X-GNOME-Autostart-enabled=false` instead of deleting it.
#[must_use]
pub fn is_active(content: &str) -> bool {
    let hidden = values(content, "Hidden").any(|v| v.eq_ignore_ascii_case("true"));
    let disabled = values(content, "X-GNOME-Autostart-enabled").any(|v| v.eq_ignore_ascii_case("false"));
    !hidden && !disabled
}

/// Whether the entry starts the app without a window.
#[must_use]
pub fn starts_in_background(content: &str) -> bool {
    values(content, "Exec").any(|v| v.split_whitespace().any(|a| a == BACKGROUND_FLAG))
}

/// Path of the entry for a given XDG config directory.
#[must_use]
pub fn path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("autostart").join(FILE_NAME)
}

/// State read from disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    /// The entry exists and is active.
    pub enabled: bool,
    /// The entry starts the app without a window.
    pub background: bool,
}

/// Reads the entry at `path`. A missing file means disabled.
///
/// # Errors
/// I/O errors other than "not found".
pub fn read(path: &Path) -> io::Result<State> {
    match std::fs::read_to_string(path) {
        Ok(c) => Ok(State {
            enabled: is_active(&c),
            background: starts_in_background(&c),
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(State {
            enabled: false,
            background: true,
        }),
        Err(e) => Err(e),
    }
}

/// Writes the entry (creating `autostart/`), atomically through a temporary file.
///
/// # Errors
/// I/O errors.
pub fn enable(path: &Path, exec: &str, background: bool) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("desktop.tmp");
    std::fs::write(&tmp, entry(exec, background))?;
    std::fs::rename(&tmp, path)
}

/// Removes the entry (no error if it is already gone).
///
/// # Errors
/// I/O errors other than "not found".
pub fn disable(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn background_entry_is_the_shipped_file() {
        assert_eq!(
            entry("msi-center", true),
            include_str!("../data/org.blazorde.MsiCenter-autostart.desktop")
        );
    }

    #[test]
    fn entry_content() {
        let fg = entry("/home/me/.local/bin/msi-center", false);
        assert!(fg.contains("\nExec=/home/me/.local/bin/msi-center\n"));
        assert!(fg.contains("X-GNOME-Autostart-enabled=true"));
        assert!(!starts_in_background(&fg));
        assert!(starts_in_background(&entry("msi-center", true)));
        assert!(is_active(&fg));
        let spaced = entry("/opt/my apps/msi-center", true);
        assert!(spaced.contains("Exec=\"/opt/my apps/msi-center\" --background\n"));
        assert!(starts_in_background(&spaced));
    }

    #[test]
    fn detects_entries_disabled_by_gnome() {
        let e = entry("msi-center", true);
        assert!(!is_active(&format!("{e}Hidden=true\n")));
        assert!(!is_active(&e.replace(
            "X-GNOME-Autostart-enabled=true",
            "X-GNOME-Autostart-enabled=false"
        )));
        assert!(is_active(&format!("{e}Hidden=false\n")));
    }

    #[test]
    fn enable_read_disable() {
        let dir = tempfile::tempdir().unwrap();
        let p = path_in(dir.path());
        assert!(p.ends_with("autostart/org.blazorde.MsiCenter.desktop"));
        assert!(!read(&p).unwrap().enabled);
        enable(&p, "msi-center", true).unwrap();
        assert_eq!(
            read(&p).unwrap(),
            State {
                enabled: true,
                background: true
            }
        );
        enable(&p, "msi-center", false).unwrap();
        assert!(!read(&p).unwrap().background);
        disable(&p).unwrap();
        disable(&p).unwrap();
        assert!(!read(&p).unwrap().enabled);
    }
}
