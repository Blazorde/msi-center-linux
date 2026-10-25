//! Battery charge limit and AC adapter state (`/sys/class/power_supply`).
//!
//! The AC state is **polled** by the daemon's 1 Hz loop (one small sysfs read, ~µs) instead of listening to
//! udev: no new dependency (libudev needs FFI), no extra task, and a latency of ≤ 1 s is fine for switching
//! profiles (ADR-0009).

use std::fs;
use std::io;
use std::path::PathBuf;

use msi_core::battery::ChargeLimit;

use crate::fs::{FsRoot, read_trimmed, read_u64, write_attr};

/// Battery attribute holding the charge limit.
pub const CHARGE_LIMIT_ATTR: &str = "charge_control_end_threshold";

/// Power supplies under an [`FsRoot`]. Supplies are found by `type` (`Battery`, `Mains`), preferring the
/// names of this laptop (`BAT1`, `ADP1`).
#[derive(Debug, Clone)]
pub struct PowerSupply {
    root: FsRoot,
}

impl PowerSupply {
    /// Power supplies under `root`.
    #[must_use]
    pub fn new(root: FsRoot) -> Self {
        Self { root }
    }

    fn find(&self, preferred: &str, kind: &str, attr: &str) -> Option<PathBuf> {
        let class = self.root.sys("class/power_supply");
        let preferred = class.join(preferred);
        if preferred.join(attr).exists() {
            return Some(preferred);
        }
        let mut dirs: Vec<PathBuf> = fs::read_dir(&class)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| read_trimmed(&p.join("type")).is_ok_and(|t| t == kind) && p.join(attr).exists())
            .collect();
        dirs.sort();
        dirs.into_iter().next()
    }

    /// `true` on AC, `false` on battery, `None` if no adapter is found.
    #[must_use]
    pub fn ac_online(&self) -> Option<bool> {
        let dir = self.find("ADP1", "Mains", "online")?;
        read_u64(&dir.join("online")).map(|v| v != 0)
    }

    /// Current charge limit (%), `None` if unsupported.
    #[must_use]
    pub fn charge_limit(&self) -> Option<u8> {
        let dir = self.find("BAT1", "Battery", CHARGE_LIMIT_ATTR)?;
        read_u64(&dir.join(CHARGE_LIMIT_ATTR)).and_then(|v| u8::try_from(v).ok())
    }

    /// Sets the charge limit and reads it back.
    ///
    /// # Errors
    /// `NotFound` if no battery exposes the attribute, the write error, or `InvalidData` if the readback differs.
    pub fn set_charge_limit(&self, limit: ChargeLimit) -> io::Result<()> {
        let dir = self
            .find("BAT1", "Battery", CHARGE_LIMIT_ATTR)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no battery charge limit attribute"))?;
        let path = dir.join(CHARGE_LIMIT_ATTR);
        write_attr(&path, &limit.get().to_string())?;
        match read_u64(&path) {
            Some(v) if v == u64::from(limit.get()) => Ok(()),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("charge limit readback {other:?}, expected {}", limit.get()),
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::fs::testutil::{read, write};

    #[test]
    fn reads_ac_and_charge_limit() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let p = PowerSupply::new(FsRoot::new(r));
        assert_eq!(p.ac_online(), None);
        assert_eq!(p.charge_limit(), None);
        write(r, "sys/class/power_supply/ADP1/online", "1\n");
        write(
            r,
            "sys/class/power_supply/BAT1/charge_control_end_threshold",
            "80\n",
        );
        assert_eq!(p.ac_online(), Some(true));
        assert_eq!(p.charge_limit(), Some(80));
        write(r, "sys/class/power_supply/ADP1/online", "0\n");
        assert_eq!(p.ac_online(), Some(false));
    }

    #[test]
    fn finds_supplies_by_type() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(r, "sys/class/power_supply/AC0/type", "Mains\n");
        write(r, "sys/class/power_supply/AC0/online", "1\n");
        write(r, "sys/class/power_supply/BAT0/type", "Battery\n");
        write(
            r,
            "sys/class/power_supply/BAT0/charge_control_end_threshold",
            "100\n",
        );
        let p = PowerSupply::new(FsRoot::new(r));
        assert_eq!(p.ac_online(), Some(true));
        p.set_charge_limit(ChargeLimit::new(60).unwrap()).unwrap();
        assert_eq!(
            read(r, "sys/class/power_supply/BAT0/charge_control_end_threshold"),
            "60"
        );
    }

    #[test]
    fn set_charge_limit_without_battery_fails() {
        let dir = tempfile::tempdir().unwrap();
        let p = PowerSupply::new(FsRoot::new(dir.path()));
        let err = p.set_charge_limit(ChargeLimit::FULL).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
