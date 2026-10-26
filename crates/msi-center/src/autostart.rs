//! Login autostart through an XDG autostart entry (`~/.config/autostart/org.blazorde.MsiCenter.desktop`).
//!
//! The Arch package also ships a system-wide entry (`/etc/xdg/autostart/`, any `$XDG_CONFIG_DIRS`). Per the XDG
//! autostart spec a user entry with the same name replaces it, so turning autostart off while a system entry
//! exists writes a user entry with `Hidden=true` instead of deleting the user file.
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

/// Paths of the system-wide entries, most important first, from the value of `$XDG_CONFIG_DIRS`
/// (`/etc/xdg` when unset or empty). Relative directories are ignored, as the spec requires.
#[must_use]
pub fn system_paths(config_dirs: Option<&str>) -> Vec<PathBuf> {
    let dirs = config_dirs.filter(|d| !d.is_empty()).unwrap_or("/etc/xdg");
    dirs.split(':')
        .map(Path::new)
        .filter(|d| d.is_absolute())
        .map(path_in)
        .collect()
}

/// Content of a user entry that hides a system-wide one (`Hidden=true`).
#[must_use]
pub fn hidden_entry(exec: &str, background: bool) -> String {
    format!("{}Hidden=true\n", entry(exec, background))
}

/// State read from disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    /// The entry exists and is active.
    pub enabled: bool,
    /// The entry starts the app without a window.
    pub background: bool,
}

fn read_opt(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(c) => Ok(Some(c)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Reads the effective entry: the user entry at `path` if it exists, else the first existing system-wide
/// entry of `system`. No entry at all means disabled.
///
/// # Errors
/// I/O errors other than "not found".
pub fn read(path: &Path, system: &[PathBuf]) -> io::Result<State> {
    let mut content = read_opt(path)?;
    for p in system {
        if content.is_some() {
            break;
        }
        content = read_opt(p)?;
    }
    Ok(content.map_or(
        State {
            enabled: false,
            background: true,
        },
        |c| State {
            enabled: is_active(&c),
            background: starts_in_background(&c),
        },
    ))
}

fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("desktop.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// Writes the user entry (creating `autostart/`), atomically through a temporary file.
///
/// # Errors
/// I/O errors.
pub fn enable(path: &Path, exec: &str, background: bool) -> io::Result<()> {
    write_atomic(path, &entry(exec, background))
}

/// Turns autostart off: removes the user entry (no error if it is already gone) or, when one of the
/// `system` entries exists, replaces it with a `Hidden=true` user entry so the system one is not run.
///
/// # Errors
/// I/O errors other than "not found".
pub fn disable(path: &Path, system: &[PathBuf], exec: &str, background: bool) -> io::Result<()> {
    if system.iter().any(|p| p.is_file()) {
        return write_atomic(path, &hidden_entry(exec, background));
    }
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
        assert!(!read(&p, &[]).unwrap().enabled);
        enable(&p, "msi-center", true).unwrap();
        assert_eq!(
            read(&p, &[]).unwrap(),
            State {
                enabled: true,
                background: true
            }
        );
        enable(&p, "msi-center", false).unwrap();
        assert!(!read(&p, &[]).unwrap().background);
        disable(&p, &[], "msi-center", false).unwrap();
        disable(&p, &[], "msi-center", false).unwrap();
        assert!(!p.exists());
        assert!(!read(&p, &[]).unwrap().enabled);
    }

    #[test]
    fn system_paths_follow_xdg_config_dirs() {
        assert_eq!(
            system_paths(None),
            vec![PathBuf::from("/etc/xdg/autostart/org.blazorde.MsiCenter.desktop")]
        );
        assert_eq!(system_paths(Some("")), system_paths(None));
        assert_eq!(
            system_paths(Some("/a:rel:/b")),
            vec![path_in(Path::new("/a")), path_in(Path::new("/b"))]
        );
    }

    #[test]
    fn system_entry_is_shadowed_by_a_hidden_user_entry() {
        let dir = tempfile::tempdir().unwrap();
        let user = path_in(&dir.path().join("user"));
        let system = vec![
            path_in(&dir.path().join("missing")),
            path_in(&dir.path().join("sys")),
        ];
        // Packaged entry only: autostart is on, in the background.
        write_atomic(&system[1], &entry("msi-center", true)).unwrap();
        assert_eq!(
            read(&user, &system).unwrap(),
            State {
                enabled: true,
                background: true
            }
        );
        // Turning it off keeps a user entry that hides the system one.
        disable(&user, &system, "/usr/bin/msi-center", true).unwrap();
        let content = std::fs::read_to_string(&user).unwrap();
        assert!(content.ends_with("Hidden=true\n"));
        assert!(!read(&user, &system).unwrap().enabled);
        assert!(read(&user, &system).unwrap().background);
        // Back on: the user entry is active again.
        enable(&user, "/usr/bin/msi-center", false).unwrap();
        assert_eq!(
            read(&user, &system).unwrap(),
            State {
                enabled: true,
                background: false
            }
        );
    }
}
