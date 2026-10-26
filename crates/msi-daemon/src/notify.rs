//! Minimal `sd_notify(3)` client (`READY=1`, `WATCHDOG=1`, `STOPPING=1`), safe Rust only.
//!
//! systemd passes the socket in `$NOTIFY_SOCKET`: a path, or an abstract socket name starting with `@`.
//! Without the variable (not started by systemd) every call is a no-op.

use std::ffi::OsStr;
use std::io;
use std::os::linux::net::SocketAddrExt as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::Duration;

/// Sends notifications to the service manager.
#[derive(Debug, Clone, Default)]
pub struct Notifier {
    socket: Option<Vec<u8>>,
}

impl Notifier {
    /// Notifier for `$NOTIFY_SOCKET` (no-op if unset).
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_socket(std::env::var_os("NOTIFY_SOCKET").as_deref())
    }

    /// Notifier for an explicit socket value (as found in `$NOTIFY_SOCKET`).
    #[must_use]
    pub fn from_socket(value: Option<&OsStr>) -> Self {
        Self {
            socket: value.map(|v| v.as_bytes().to_vec()).filter(|v| !v.is_empty()),
        }
    }

    /// `true` if a socket is configured.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.socket.is_some()
    }

    /// Sends `state` (e.g. `"READY=1"`).
    ///
    /// # Errors
    /// Socket errors.
    pub fn notify(&self, state: &str) -> io::Result<()> {
        let Some(socket) = &self.socket else {
            return Ok(());
        };
        let addr = match socket.strip_prefix(b"@") {
            Some(name) => SocketAddr::from_abstract_name(name)?,
            None => SocketAddr::from_pathname(OsStr::from_bytes(socket))?,
        };
        let sock = UnixDatagram::unbound()?;
        sock.send_to_addr(state.as_bytes(), &addr).map(|_| ())
    }
}

/// Watchdog period from `$WATCHDOG_USEC` (`None` if unset or invalid).
#[must_use]
pub fn watchdog_period(value: Option<&OsStr>) -> Option<Duration> {
    let usec: u64 = value?.to_str()?.parse().ok()?;
    (usec > 0).then(|| Duration::from_micros(usec))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sends_to_a_path_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let server = UnixDatagram::bind(&path).unwrap();
        let n = Notifier::from_socket(Some(path.as_os_str()));
        assert!(n.is_enabled());
        n.notify("READY=1").unwrap();
        n.notify("WATCHDOG=1").unwrap();
        let mut buf = [0u8; 64];
        let len = server.recv(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"READY=1");
        let len = server.recv(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"WATCHDOG=1");
    }

    #[test]
    fn sends_to_an_abstract_socket() {
        let name = format!("msi-daemon-test-{}", std::process::id());
        let server =
            UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
        let value = format!("@{name}");
        Notifier::from_socket(Some(OsStr::new(&value)))
            .notify("STOPPING=1")
            .unwrap();
        let mut buf = [0u8; 64];
        let len = server.recv(&mut buf).unwrap();
        assert_eq!(&buf[..len], b"STOPPING=1");
    }

    #[test]
    fn unset_socket_is_a_no_op() {
        let n = Notifier::from_socket(None);
        assert!(!n.is_enabled());
        n.notify("READY=1").unwrap();
        assert!(!Notifier::from_socket(Some(OsStr::new(""))).is_enabled());
    }

    #[test]
    fn parses_watchdog_period() {
        assert_eq!(
            watchdog_period(Some(OsStr::new("15000000"))),
            Some(Duration::from_secs(15))
        );
        assert_eq!(watchdog_period(Some(OsStr::new("0"))), None);
        assert_eq!(watchdog_period(Some(OsStr::new("x"))), None);
        assert_eq!(watchdog_period(None), None);
    }
}
