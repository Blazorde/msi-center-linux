//! Intel NPU busy time (`intel_vpu`, `/sys/class/accel/accel*/device/npu_busy_time_us`).
//!
//! The attribute is a software counter maintained by the driver: reading it does not resume a suspended NPU.

use std::fs;
use std::path::PathBuf;

use super::{BusySample, Clock};
use crate::fs::{FsRoot, read_u64};

/// Reader of the NPU busy-time counter.
#[derive(Debug, Clone)]
pub struct NpuReader {
    path: Option<PathBuf>,
}

impl NpuReader {
    /// Finds the first accel device exposing `npu_busy_time_us`.
    #[must_use]
    pub fn discover(root: &FsRoot) -> Self {
        let path = fs::read_dir(root.sys("class/accel")).ok().and_then(|entries| {
            let mut found: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path().join("device/npu_busy_time_us"))
                .filter(|p| p.exists())
                .collect();
            found.sort();
            found.into_iter().next()
        });
        Self { path }
    }

    /// Busy time so far, timestamped with `at`. `None` without an NPU.
    #[must_use]
    pub fn read(&self, at: Clock) -> Option<BusySample> {
        let busy_us = read_u64(self.path.as_deref()?)?;
        Some(BusySample {
            busy_ns: busy_us.saturating_mul(1000),
            at_ns: at.0,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::fs::testutil::{symlink, write};
    use crate::telemetry::busy_percent;

    #[test]
    fn reads_counter_and_computes_percent() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(
            r,
            "sys/devices/pci0000:00/0000:00:0b.0/npu_busy_time_us",
            "1000\n",
        );
        symlink(
            r,
            "sys/class/accel/accel0/device",
            "../../../devices/pci0000:00/0000:00:0b.0",
        );
        let npu = NpuReader::discover(&FsRoot::new(r));
        let a = npu.read(Clock(0)).unwrap();
        write(
            r,
            "sys/devices/pci0000:00/0000:00:0b.0/npu_busy_time_us",
            "251000\n",
        );
        let b = npu.read(Clock(1_000_000_000)).unwrap();
        assert!((busy_percent(a, b).unwrap() - 25.0).abs() < 1e-9);
    }

    #[test]
    fn absent_npu_degrades_to_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(NpuReader::discover(&FsRoot::new(dir.path())).read(Clock(1)), None);
    }
}
