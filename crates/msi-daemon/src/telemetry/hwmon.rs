//! Temperatures and fan speeds from hwmon, behind the [`ThermalSource`] extension point.
//!
//! hwmon numbers change between boots, so directories are found by `name` and re-discovered after a failed
//! read. Sources are queried in priority order and only fill values still missing, so a future `msi-ec`
//! source (EC temperatures/RPM, also available when the dGPU is off) is simply inserted first.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::fs::{FsRoot, read_i64, read_trimmed, read_u64};

/// Temperatures (°C) and fan speeds (RPM). `None` = unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Thermals {
    /// CPU package temperature.
    pub cpu_temp_c: Option<f64>,
    /// dGPU temperature.
    pub gpu_temp_c: Option<f64>,
    /// Fan 1 (CPU side).
    pub fan1_rpm: Option<u32>,
    /// Fan 2 (GPU side).
    pub fan2_rpm: Option<u32>,
}

impl Thermals {
    /// Fills the fields that are still `None` from `other`.
    pub fn fill_missing(&mut self, other: Thermals) {
        self.cpu_temp_c = self.cpu_temp_c.or(other.cpu_temp_c);
        self.gpu_temp_c = self.gpu_temp_c.or(other.gpu_temp_c);
        self.fan1_rpm = self.fan1_rpm.or(other.fan1_rpm);
        self.fan2_rpm = self.fan2_rpm.or(other.fan2_rpm);
    }
}

/// A provider of temperatures and/or fan speeds.
pub trait ThermalSource: fmt::Debug + Send {
    /// Reads whatever this source knows; unknown fields stay `None`. Must never panic.
    fn read(&mut self) -> Thermals;
}

/// Finds `/sys/class/hwmon/hwmonN` whose `name` is `name`.
#[must_use]
pub fn find_hwmon(root: &FsRoot, name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(root.sys("class/hwmon"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| read_trimmed(&p.join("name")).is_ok_and(|n| n == name))
        .collect();
    dirs.sort();
    dirs.into_iter().next()
}

/// Lazily located hwmon directory.
#[derive(Debug, Clone)]
struct HwmonDir {
    root: FsRoot,
    name: &'static str,
    dir: Option<PathBuf>,
}

impl HwmonDir {
    fn new(root: &FsRoot, name: &'static str) -> Self {
        Self {
            root: root.clone(),
            name,
            dir: None,
        }
    }

    /// Reads `<dir>/<file>` with `parse`; forgets the directory on failure so it is searched again next time.
    fn read<T>(&mut self, file: &str, parse: impl Fn(&Path) -> Option<T>) -> Option<T> {
        if self.dir.is_none() {
            self.dir = find_hwmon(&self.root, self.name);
        }
        let value = parse(&self.dir.as_ref()?.join(file));
        if value.is_none() {
            self.dir = None;
        }
        value
    }
}

#[allow(clippy::cast_precision_loss, reason = "millidegrees are far below 2^52")]
fn millidegrees(path: &Path) -> Option<f64> {
    read_i64(path).map(|m| m as f64 / 1000.0)
}

/// CPU package temperature from `coretemp` (`temp*_label == "Package id 0"`, else `temp1_input`).
#[derive(Debug, Clone)]
pub struct Coretemp {
    hwmon: HwmonDir,
    input: Option<String>,
}

impl Coretemp {
    /// Source reading under `root`.
    #[must_use]
    pub fn new(root: &FsRoot) -> Self {
        Self {
            hwmon: HwmonDir::new(root, "coretemp"),
            input: None,
        }
    }

    fn package_input(dir: &Path) -> String {
        fs::read_dir(dir)
            .ok()
            .and_then(|entries| {
                entries.flatten().find_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let prefix = name.strip_suffix("_label")?;
                    read_trimmed(&e.path()).ok().filter(|l| l == "Package id 0")?;
                    Some(format!("{prefix}_input"))
                })
            })
            .unwrap_or_else(|| "temp1_input".to_owned())
    }
}

impl ThermalSource for Coretemp {
    fn read(&mut self) -> Thermals {
        if self.input.is_none() {
            if self.hwmon.dir.is_none() {
                self.hwmon.dir = find_hwmon(&self.hwmon.root, self.hwmon.name);
            }
            self.input = self.hwmon.dir.as_deref().map(Self::package_input);
        }
        let Some(input) = self.input.clone() else {
            return Thermals::default();
        };
        let cpu_temp_c = self.hwmon.read(&input, millidegrees);
        if cpu_temp_c.is_none() {
            self.input = None;
        }
        Thermals {
            cpu_temp_c,
            ..Thermals::default()
        }
    }
}

/// Fan speeds from `msi_wmi_platform` (`fan1_input`, `fan2_input`).
#[derive(Debug, Clone)]
pub struct MsiWmiFans {
    hwmon: HwmonDir,
}

impl MsiWmiFans {
    /// Source reading under `root`.
    #[must_use]
    pub fn new(root: &FsRoot) -> Self {
        Self {
            hwmon: HwmonDir::new(root, "msi_wmi_platform"),
        }
    }
}

fn rpm(path: &Path) -> Option<u32> {
    read_u64(path).and_then(|v| u32::try_from(v).ok())
}

impl ThermalSource for MsiWmiFans {
    fn read(&mut self) -> Thermals {
        let fan1_rpm = self.hwmon.read("fan1_input", rpm);
        let fan2_rpm = self.hwmon.read("fan2_input", rpm);
        Thermals {
            fan1_rpm,
            fan2_rpm,
            ..Thermals::default()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::fs::testutil::write;

    fn fake(r: &Path) {
        write(r, "sys/class/hwmon/hwmon1/name", "acpitz\n");
        write(r, "sys/class/hwmon/hwmon1/temp1_input", "58000\n");
        write(r, "sys/class/hwmon/hwmon4/name", "msi_wmi_platform\n");
        write(r, "sys/class/hwmon/hwmon4/fan1_input", "2232\n");
        write(r, "sys/class/hwmon/hwmon4/fan2_input", "2211\n");
        write(r, "sys/class/hwmon/hwmon7/name", "coretemp\n");
        write(r, "sys/class/hwmon/hwmon7/temp1_label", "Core 0\n");
        write(r, "sys/class/hwmon/hwmon7/temp1_input", "40000\n");
        write(r, "sys/class/hwmon/hwmon7/temp2_label", "Package id 0\n");
        write(r, "sys/class/hwmon/hwmon7/temp2_input", "56500\n");
    }

    #[test]
    fn finds_hwmon_by_name() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path());
        let root = FsRoot::new(dir.path());
        assert_eq!(
            find_hwmon(&root, "coretemp"),
            Some(dir.path().join("sys/class/hwmon/hwmon7"))
        );
        assert_eq!(find_hwmon(&root, "msi_ec"), None);
    }

    #[test]
    fn coretemp_prefers_package_sensor() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path());
        let t = Coretemp::new(&FsRoot::new(dir.path())).read();
        assert_eq!(
            t,
            Thermals {
                cpu_temp_c: Some(56.5),
                ..Thermals::default()
            }
        );
    }

    #[test]
    fn fans_from_msi_wmi_platform() {
        let dir = tempfile::tempdir().unwrap();
        fake(dir.path());
        let t = MsiWmiFans::new(&FsRoot::new(dir.path())).read();
        assert_eq!((t.fan1_rpm, t.fan2_rpm), (Some(2232), Some(2211)));
    }

    #[test]
    fn sources_survive_hwmon_renumbering_and_absence() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        let root = FsRoot::new(r);
        let mut fans = MsiWmiFans::new(&root);
        let mut cpu = Coretemp::new(&root);
        assert_eq!(fans.read(), Thermals::default());
        assert_eq!(cpu.read(), Thermals::default());
        fake(r);
        assert_eq!(fans.read().fan1_rpm, Some(2232));
        // Driver reloaded: hwmon4 becomes hwmon9.
        fs::rename(r.join("sys/class/hwmon/hwmon4"), r.join("sys/class/hwmon/hwmon9")).unwrap();
        assert_eq!(fans.read().fan1_rpm, None);
        assert_eq!(fans.read().fan1_rpm, Some(2232));
        assert_eq!(cpu.read().cpu_temp_c, Some(56.5));
    }

    #[test]
    fn fill_missing_keeps_priority() {
        let mut t = Thermals {
            cpu_temp_c: Some(50.0),
            ..Thermals::default()
        };
        t.fill_missing(Thermals {
            cpu_temp_c: Some(99.0),
            fan1_rpm: Some(1),
            ..Thermals::default()
        });
        assert_eq!(
            t,
            Thermals {
                cpu_temp_c: Some(50.0),
                fan1_rpm: Some(1),
                ..Thermals::default()
            }
        );
    }
}
