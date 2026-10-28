//! CPU package power limits through the RAPL powercap interface (`/sys/class/powercap`, ADR-0014).
//!
//! Two zones set the package limits on this laptop, and the CPU obeys the lower of the two: the MSR one
//! (`intel-rapl:0`) and the MMIO one (`intel-rapl-mmio:0`). Both are written, each read back. A zone counts
//! when its `name` is `package-0` and its constraints 0 and 1 are `long_term` (PL1) and `short_term` (PL2).
//!
//! The firmware's limits are saved in `/run/msi-center/rapl-firmware` the first time they are read: a daemon
//! restart must not mistake our own lowered limits for the firmware's. `/run` is a tmpfs, so a reboot (where
//! the firmware sets its limits again) starts over. [`Rapl::restore`] writes them back and removes the file.
//! The crash paths (panic hook, `ExecStopPost`) leave the lowered limits: a slower CPU, never a hotter one.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use msi_core::power_limit::PowerLimits;

use crate::fs::{FsRoot, read_trimmed, read_u64, write_attr};

/// Package zones, in write order.
pub const ZONES: [&str; 2] = ["intel-rapl-mmio:0", "intel-rapl:0"];

/// Firmware limits saved across daemon restarts, under `/run`.
pub const FIRMWARE_FILE: &str = "msi-center/rapl-firmware";

/// RAPL package zones under an [`FsRoot`].
#[derive(Debug, Clone)]
pub struct Rapl {
    root: FsRoot,
}

impl Rapl {
    /// RAPL under `root`.
    #[must_use]
    pub fn new(root: FsRoot) -> Self {
        Self { root }
    }

    /// Package zones present (name and directory).
    fn zones(&self) -> Vec<(&'static str, PathBuf)> {
        ZONES
            .into_iter()
            .map(|z| (z, self.root.sys("class/powercap").join(z)))
            .filter(|(_, d)| {
                read_trimmed(&d.join("name")).is_ok_and(|n| n == "package-0")
                    && read_trimmed(&d.join("constraint_0_name")).is_ok_and(|n| n == "long_term")
                    && read_trimmed(&d.join("constraint_1_name")).is_ok_and(|n| n == "short_term")
            })
            .collect()
    }

    /// Limits in force: the lowest PL1 and PL2 of the package zones. `None` without a readable zone.
    #[must_use]
    pub fn current(&self) -> Option<PowerLimits> {
        self.zones()
            .iter()
            .filter_map(|(_, d)| read_zone(d))
            .reduce(lowest)
    }

    /// The firmware's limits per zone: the saved ones if any, else the current ones (then saved).
    ///
    /// # Errors
    /// `NotFound` without a readable package zone, or the error saving them.
    pub fn firmware(&self) -> io::Result<Vec<(&'static str, PowerLimits)>> {
        let file = self.root.run(FIRMWARE_FILE);
        let saved = fs::read_to_string(&file)
            .map(|s| parse_saved(&s))
            .unwrap_or_default();
        let mut out = Vec::new();
        let mut fresh = false;
        for (zone, dir) in self.zones() {
            if let Some(&(_, l)) = saved.iter().find(|(z, _)| z == zone) {
                out.push((zone, l));
            } else if let Some(l) = read_zone(&dir) {
                out.push((zone, l));
                fresh = true;
            }
        }
        if out.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no RAPL package zone"));
        }
        if fresh {
            let mut text = String::new();
            for (zone, l) in &out {
                let _ = writeln!(text, "{zone} {} {}", l.pl1_uw, l.pl2_uw);
            }
            if let Some(dir) = file.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&file, text)?;
        }
        Ok(out)
    }

    /// Applies `wanted` capped to each zone's firmware limits (`None` = the firmware's), read back. Succeeds
    /// when at least one zone took the limits (the CPU obeys the lowest); the others are logged.
    ///
    /// # Errors
    /// `NotFound` without a package zone, or the error of the first zone when no zone took the limits.
    pub fn apply(&self, wanted: Option<PowerLimits>) -> io::Result<PowerLimits> {
        let mut first_err = None;
        let mut applied: Option<PowerLimits> = None;
        for (zone, fw) in self.firmware()? {
            let target = wanted.map_or(fw, |w| w.capped(fw));
            match write_zone(&self.root.sys("class/powercap").join(zone), target) {
                Ok(()) => applied = Some(applied.map_or(target, |a| lowest(a, target))),
                Err(e) => {
                    tracing::warn!(zone, error = %e, "RAPL zone not written");
                    first_err.get_or_insert(e);
                }
            }
        }
        match (applied, first_err) {
            (Some(l), _) => Ok(l),
            (None, Some(e)) => Err(e),
            (None, None) => Err(io::Error::new(io::ErrorKind::NotFound, "no RAPL package zone")),
        }
    }

    /// Writes the firmware's limits back, then forgets them (normal exit).
    ///
    /// # Errors
    /// As [`Self::apply`]; the saved file is kept on error.
    pub fn restore(&self) -> io::Result<PowerLimits> {
        let limits = self.apply(None)?;
        match fs::remove_file(self.root.run(FIRMWARE_FILE)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(limits),
        }
    }
}

fn lowest(a: PowerLimits, b: PowerLimits) -> PowerLimits {
    PowerLimits {
        pl1_uw: a.pl1_uw.min(b.pl1_uw),
        pl2_uw: a.pl2_uw.min(b.pl2_uw),
    }
}

fn read_zone(dir: &Path) -> Option<PowerLimits> {
    Some(PowerLimits {
        pl1_uw: read_u64(&dir.join("constraint_0_power_limit_uw"))?,
        pl2_uw: read_u64(&dir.join("constraint_1_power_limit_uw"))?,
    })
}

fn parse_saved(text: &str) -> Vec<(String, PowerLimits)> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let zone = it.next()?.to_owned();
            let pl1_uw = it.next()?.parse().ok()?;
            let pl2_uw = it.next()?.parse().ok()?;
            Some((zone, PowerLimits { pl1_uw, pl2_uw }))
        })
        .collect()
}

/// Writes PL1 and PL2 so that PL1 ≤ PL2 holds after each write, then reads both back.
fn write_zone(dir: &Path, target: PowerLimits) -> io::Result<()> {
    let pl1 = dir.join("constraint_0_power_limit_uw");
    let pl2 = dir.join("constraint_1_power_limit_uw");
    let raising = read_u64(&pl2).is_some_and(|cur| target.pl2_uw >= cur);
    let order = if raising {
        [(&pl2, target.pl2_uw), (&pl1, target.pl1_uw)]
    } else {
        [(&pl1, target.pl1_uw), (&pl2, target.pl2_uw)]
    };
    for (path, value) in order {
        write_attr(path, &value.to_string())?;
    }
    match (read_u64(&pl1), read_u64(&pl2)) {
        (Some(a), Some(b)) if a == target.pl1_uw && b == target.pl2_uw => Ok(()),
        got => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "readback {got:?}, expected ({}, {}) (locked?)",
                target.pl1_uw, target.pl2_uw
            ),
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::fs::testutil::{read, write};

    const BALANCED: PowerLimits = PowerLimits {
        pl1_uw: 35_000_000,
        pl2_uw: 60_000_000,
    };

    fn zone(r: &Path, z: &str, pl1: u64, pl2: u64) {
        let d = format!("sys/class/powercap/{z}");
        write(r, &format!("{d}/name"), "package-0\n");
        write(r, &format!("{d}/constraint_0_name"), "long_term\n");
        write(r, &format!("{d}/constraint_1_name"), "short_term\n");
        write(
            r,
            &format!("{d}/constraint_0_power_limit_uw"),
            &format!("{pl1}\n"),
        );
        write(
            r,
            &format!("{d}/constraint_1_power_limit_uw"),
            &format!("{pl2}\n"),
        );
    }

    fn limits(r: &Path, z: &str) -> (String, String) {
        let d = format!("sys/class/powercap/{z}");
        (
            read(r, &format!("{d}/constraint_0_power_limit_uw")),
            read(r, &format!("{d}/constraint_1_power_limit_uw")),
        )
    }

    #[test]
    fn lowers_both_zones_and_restores_the_firmware_limits() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        zone(r, "intel-rapl:0", 110_000_000, 160_000_000);
        zone(r, "intel-rapl-mmio:0", 110_000_000, 160_000_000);
        let rapl = Rapl::new(FsRoot::new(r));
        assert_eq!(rapl.apply(Some(BALANCED)).unwrap(), BALANCED);
        for z in ZONES {
            assert_eq!(limits(r, z), ("35000000".into(), "60000000".into()));
        }
        assert_eq!(rapl.current(), Some(BALANCED));
        // A daemon restart must keep the firmware's limits, not our lowered ones.
        let restarted = Rapl::new(FsRoot::new(r));
        assert_eq!(restarted.firmware().unwrap()[0].1.pl1_uw, 110_000_000);
        assert_eq!(restarted.restore().unwrap().pl2_uw, 160_000_000);
        for z in ZONES {
            assert_eq!(limits(r, z), ("110000000".into(), "160000000".into()));
        }
        assert!(!r.join("run").join(FIRMWARE_FILE).exists());
    }

    #[test]
    fn never_raises_above_the_firmware() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        zone(r, "intel-rapl:0", 28_000_000, 45_000_000);
        let rapl = Rapl::new(FsRoot::new(r));
        let expected = PowerLimits {
            pl1_uw: 28_000_000,
            pl2_uw: 45_000_000,
        };
        assert_eq!(rapl.apply(Some(BALANCED)).unwrap(), expected);
        assert_eq!(limits(r, "intel-rapl:0"), ("28000000".into(), "45000000".into()));
    }

    #[test]
    fn ignores_non_package_zones_and_fails_without_any() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let rapl = Rapl::new(FsRoot::new(r));
        assert_eq!(
            rapl.apply(Some(BALANCED)).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        zone(r, "intel-rapl:0", 110_000_000, 160_000_000);
        write(r, "sys/class/powercap/intel-rapl:0/name", "psys\n");
        assert_eq!(rapl.current(), None);
        assert!(rapl.apply(None).is_err());
    }

    #[test]
    fn a_zone_that_does_not_take_the_limits_is_tolerated_if_another_does() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        zone(r, "intel-rapl:0", 110_000_000, 160_000_000);
        zone(r, "intel-rapl-mmio:0", 110_000_000, 160_000_000);
        let rapl = Rapl::new(FsRoot::new(r));
        rapl.firmware().unwrap();
        // A missing attribute stands for a locked MSR zone: the write fails.
        fs::remove_file(r.join("sys/class/powercap/intel-rapl:0/constraint_0_power_limit_uw")).unwrap();
        assert_eq!(rapl.apply(Some(BALANCED)).unwrap(), BALANCED);
        assert_eq!(
            limits(r, "intel-rapl-mmio:0"),
            ("35000000".into(), "60000000".into())
        );
    }
}
