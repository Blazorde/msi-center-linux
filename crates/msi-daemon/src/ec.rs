//! Access to the EC through the msi-ec sysfs interface (`/sys/devices/platform/msi-ec/`).
//!
//! [`EcBackend`] is the only way the daemon talks to the EC. [`SysfsEc`] is the real implementation over an
//! [`FsRoot`]; [`MemoryEc`] is an in-memory EC that enforces the kernel's validation rules (`EINVAL` on an
//! invalid table, readback faults on demand), used by tests and for development without the hardware.
//!
//! Kernel contract (fork of msi-ec, `kernel/`): per fan directory `cpu/` and `gpu/`, `fan_curve_speeds`
//! (7 ints), `fan_curve_up_temps` and `fan_curve_down_temps` (6 ints), decimal and space-separated, full table
//! on write. The curve files only exist when msi-ec matched the configuration from the EC's own firmware
//! string: a missing file means "unsupported" and the daemon stays read-only (S8).

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use msi_core::ec::{FanMode, ShiftMode};
use msi_core::table::{RawTables, TableWrite};

use crate::fs::{FsRoot, read_trimmed, write_attr};

/// sysfs directory of msi-ec, relative to `/sys`.
pub const MSI_EC_DIR: &str = "devices/platform/msi-ec";

/// One of the two fans (and its temperature sensor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Fan {
    /// CPU fan (fan 1).
    Cpu,
    /// GPU fan (fan 2).
    Gpu,
}

impl Fan {
    /// Both fans.
    pub const ALL: [Self; 2] = [Self::Cpu, Self::Gpu];

    /// Stable name (sysfs directory, D-Bus, state file).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }

    /// Parses `cpu` / `gpu`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

impl fmt::Display for Fan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parses a `fan_mode` value.
#[must_use]
pub fn parse_fan_mode(s: &str) -> Option<FanMode> {
    [FanMode::Auto, FanMode::Silent, FanMode::Advanced]
        .into_iter()
        .find(|m| m.as_str() == s)
}

/// Parses a `shift_mode` value.
#[must_use]
pub fn parse_shift_mode(s: &str) -> Option<ShiftMode> {
    [ShiftMode::Eco, ShiftMode::Comfort, ShiftMode::Turbo]
        .into_iter()
        .find(|m| m.as_str() == s)
}

/// EC access failure.
#[derive(Debug, thiserror::Error)]
pub enum EcError {
    /// An attribute could not be read or written (`EINVAL` = rejected by the kernel, `EIO` = EC error or
    /// readback mismatch in the kernel).
    #[error("{}: {source}", path.display())]
    Io {
        /// Attribute.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// An attribute holds something unexpected.
    #[error("{}: unexpected value {value:?}", path.display())]
    Parse {
        /// Attribute.
        path: PathBuf,
        /// Value read.
        value: String,
    },
}

/// Operations the daemon needs from the EC. Implementations must never panic.
pub trait EcBackend: fmt::Debug + Send {
    /// EC firmware version (`fw_version`).
    ///
    /// # Errors
    /// [`EcError`] if msi-ec is not loaded or the attribute cannot be read.
    fn fw_version(&self) -> Result<String, EcError>;
    /// `true` if every fan curve attribute exists (msi-ec fork, configuration matched from the firmware).
    fn curve_supported(&self) -> bool;
    /// Current fan mode.
    ///
    /// # Errors
    /// [`EcError`] on read failure or unknown value.
    fn fan_mode(&self) -> Result<FanMode, EcError>;
    /// Sets the fan mode.
    ///
    /// # Errors
    /// [`EcError`] on write failure.
    fn set_fan_mode(&mut self, mode: FanMode) -> Result<(), EcError>;
    /// Current shift mode.
    ///
    /// # Errors
    /// [`EcError`] on read failure or unknown value.
    fn shift_mode(&self) -> Result<ShiftMode, EcError>;
    /// Sets the shift mode.
    ///
    /// # Errors
    /// [`EcError`] on write failure.
    fn set_shift_mode(&mut self, mode: ShiftMode) -> Result<(), EcError>;
    /// Cooler Boost state.
    ///
    /// # Errors
    /// [`EcError`] on read failure or unknown value.
    fn cooler_boost(&self) -> Result<bool, EcError>;
    /// Sets Cooler Boost.
    ///
    /// # Errors
    /// [`EcError`] on write failure.
    fn set_cooler_boost(&mut self, on: bool) -> Result<(), EcError>;
    /// The three curve tables of `fan` as the EC holds them.
    ///
    /// # Errors
    /// [`EcError`] on read failure or malformed table.
    fn read_tables(&self, fan: Fan) -> Result<RawTables, EcError>;
    /// Writes one full table of `fan`.
    ///
    /// # Errors
    /// [`EcError`]: `EINVAL` if the kernel rejects the table, `EIO` on EC failure.
    fn write_table(&mut self, fan: Fan, write: &TableWrite) -> Result<(), EcError>;
    /// Real-time temperature (°C) of the sensor driving `fan`.
    ///
    /// # Errors
    /// [`EcError`] on read failure.
    fn temperature(&self, fan: Fan) -> Result<u8, EcError>;
    /// Real-time speed (%) of `fan`.
    ///
    /// # Errors
    /// [`EcError`] on read failure.
    fn fan_speed(&self, fan: Fan) -> Result<u8, EcError>;
}

/// Formats a table for sysfs: decimal, space-separated.
#[must_use]
pub fn format_table(values: &[u8]) -> String {
    values.iter().map(u8::to_string).collect::<Vec<_>>().join(" ")
}

/// Parses exactly `N` whitespace-separated decimal bytes.
#[must_use]
pub fn parse_table<const N: usize>(text: &str) -> Option<[u8; N]> {
    let values: Vec<u8> = text
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    values.try_into().ok()
}

const CURVE_FILES: [&str; 3] = ["fan_curve_speeds", "fan_curve_up_temps", "fan_curve_down_temps"];

/// The real EC, through msi-ec sysfs attributes under an [`FsRoot`].
#[derive(Debug, Clone)]
pub struct SysfsEc {
    dir: PathBuf,
}

impl SysfsEc {
    /// msi-ec under `root` (`<root>/sys/devices/platform/msi-ec`).
    #[must_use]
    pub fn new(root: &FsRoot) -> Self {
        Self {
            dir: root.sys(MSI_EC_DIR),
        }
    }

    /// Path of the `fan_mode` attribute (also used by the panic hook to restore `auto`, S5).
    #[must_use]
    pub fn fan_mode_path(&self) -> PathBuf {
        self.dir.join("fan_mode")
    }

    fn read(&self, rel: &str) -> Result<(PathBuf, String), EcError> {
        let path = self.dir.join(rel);
        match read_trimmed(&path) {
            Ok(v) => Ok((path, v)),
            Err(source) => Err(EcError::Io { path, source }),
        }
    }

    fn write(&self, rel: &str, value: &str) -> Result<(), EcError> {
        let path = self.dir.join(rel);
        write_attr(&path, value).map_err(|source| EcError::Io { path, source })
    }

    fn read_parsed<T>(&self, rel: &str, parse: impl Fn(&str) -> Option<T>) -> Result<T, EcError> {
        let (path, value) = self.read(rel)?;
        parse(&value).ok_or(EcError::Parse { path, value })
    }

    fn table<const N: usize>(&self, fan: Fan, file: &str) -> Result<[u8; N], EcError> {
        self.read_parsed(&format!("{fan}/{file}"), parse_table::<N>)
    }
}

fn parse_u8(s: &str) -> Option<u8> {
    s.parse().ok()
}

impl EcBackend for SysfsEc {
    fn fw_version(&self) -> Result<String, EcError> {
        self.read("fw_version").map(|(_, v)| v)
    }

    fn curve_supported(&self) -> bool {
        Fan::ALL.iter().all(|fan| {
            CURVE_FILES
                .iter()
                .all(|file| self.dir.join(fan.as_str()).join(file).is_file())
        })
    }

    fn fan_mode(&self) -> Result<FanMode, EcError> {
        self.read_parsed("fan_mode", parse_fan_mode)
    }

    fn set_fan_mode(&mut self, mode: FanMode) -> Result<(), EcError> {
        self.write("fan_mode", mode.as_str())
    }

    fn shift_mode(&self) -> Result<ShiftMode, EcError> {
        self.read_parsed("shift_mode", parse_shift_mode)
    }

    fn set_shift_mode(&mut self, mode: ShiftMode) -> Result<(), EcError> {
        self.write("shift_mode", mode.as_str())
    }

    fn cooler_boost(&self) -> Result<bool, EcError> {
        self.read_parsed("cooler_boost", |v| match v {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        })
    }

    fn set_cooler_boost(&mut self, on: bool) -> Result<(), EcError> {
        self.write("cooler_boost", if on { "on" } else { "off" })
    }

    fn read_tables(&self, fan: Fan) -> Result<RawTables, EcError> {
        Ok(RawTables {
            speeds: self.table(fan, CURVE_FILES[0])?,
            up: self.table(fan, CURVE_FILES[1])?,
            down: self.table(fan, CURVE_FILES[2])?,
        })
    }

    fn write_table(&mut self, fan: Fan, write: &TableWrite) -> Result<(), EcError> {
        let (file, text) = match write {
            TableWrite::Speeds(s) => (CURVE_FILES[0], format_table(s)),
            TableWrite::Up(u) => (CURVE_FILES[1], format_table(u)),
            TableWrite::Down(d) => (CURVE_FILES[2], format_table(d)),
        };
        self.write(&format!("{fan}/{file}"), &text)
    }

    fn temperature(&self, fan: Fan) -> Result<u8, EcError> {
        self.read_parsed(&format!("{fan}/realtime_temperature"), parse_u8)
    }

    fn fan_speed(&self, fan: Fan) -> Result<u8, EcError> {
        self.read_parsed(&format!("{fan}/realtime_fan_speed"), parse_u8)
    }
}

/// Writes `auto` to a `fan_mode` attribute without any other state: used by the panic hook and at exit (S5),
/// where no lock may be taken.
///
/// # Errors
/// The I/O error if the attribute cannot be written.
pub fn restore_auto_at(fan_mode: &Path) -> io::Result<()> {
    write_attr(fan_mode, FanMode::Auto.as_str())
}

/// Factory CPU tables of firmware `14K2EMS1.110`.
pub const FACTORY_CPU: RawTables = RawTables {
    speeds: [0, 36, 40, 50, 62, 80, 96],
    up: [57, 62, 70, 77, 83, 90],
    down: [53, 57, 65, 73, 80, 87],
};

/// Factory GPU tables of firmware `14K2EMS1.110`.
pub const FACTORY_GPU: RawTables = RawTables {
    speeds: [0, 36, 42, 50, 62, 80, 96],
    up: [50, 60, 65, 70, 78, 83],
    down: [45, 55, 63, 67, 75, 80],
};

/// State of a [`MemoryEc`], shared with the test that created it.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryState {
    /// `fw_version`.
    pub fw_version: String,
    /// Whether the curve attributes exist.
    pub curve_supported: bool,
    /// Fan mode.
    pub fan_mode: FanMode,
    /// Shift mode.
    pub shift_mode: ShiftMode,
    /// Cooler Boost.
    pub cooler_boost: bool,
    /// CPU tables.
    pub cpu: RawTables,
    /// GPU tables.
    pub gpu: RawTables,
    /// CPU temperature (°C); `None` = read error.
    pub cpu_temp: Option<u8>,
    /// GPU temperature (°C); `None` = read error.
    pub gpu_temp: Option<u8>,
    /// Every accepted table write, in order.
    pub writes: Vec<(Fan, TableWrite)>,
    /// Every fan mode written, in order.
    pub fan_mode_writes: Vec<FanMode>,
    /// Fault: table writes fail with `EIO` (nothing changes).
    pub fail_table_writes: bool,
    /// Fault: the readback of this fan's speeds is off by one (EC not applying the table).
    pub corrupt_readback: Option<Fan>,
}

impl Default for MemoryState {
    fn default() -> Self {
        Self {
            fw_version: "14K2EMS1.110".into(),
            curve_supported: true,
            fan_mode: FanMode::Auto,
            shift_mode: ShiftMode::Comfort,
            cooler_boost: false,
            cpu: FACTORY_CPU,
            gpu: FACTORY_GPU,
            cpu_temp: Some(50),
            gpu_temp: Some(45),
            writes: Vec::new(),
            fan_mode_writes: Vec::new(),
            fail_table_writes: false,
            corrupt_readback: None,
        }
    }
}

/// In-memory EC enforcing the kernel's table validation ([`RawTables::kernel_check`] on every write, as the
/// msi-ec fork does). Clones share the same state.
#[derive(Debug, Clone, Default)]
pub struct MemoryEc {
    state: Arc<Mutex<MemoryState>>,
}

fn einval(what: &str) -> EcError {
    EcError::Io {
        path: PathBuf::from(what),
        source: io::Error::from_raw_os_error(22),
    }
}

fn eio(what: &str) -> EcError {
    EcError::Io {
        path: PathBuf::from(what),
        source: io::Error::from_raw_os_error(5),
    }
}

impl MemoryEc {
    /// EC with the given state.
    #[must_use]
    pub fn new(state: MemoryState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Locks the shared state (tests inspect and change it).
    pub fn state(&self) -> MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl EcBackend for MemoryEc {
    fn fw_version(&self) -> Result<String, EcError> {
        Ok(self.state().fw_version.clone())
    }

    fn curve_supported(&self) -> bool {
        self.state().curve_supported
    }

    fn fan_mode(&self) -> Result<FanMode, EcError> {
        Ok(self.state().fan_mode)
    }

    fn set_fan_mode(&mut self, mode: FanMode) -> Result<(), EcError> {
        let mut s = self.state();
        s.fan_mode = mode;
        s.fan_mode_writes.push(mode);
        Ok(())
    }

    fn shift_mode(&self) -> Result<ShiftMode, EcError> {
        Ok(self.state().shift_mode)
    }

    fn set_shift_mode(&mut self, mode: ShiftMode) -> Result<(), EcError> {
        self.state().shift_mode = mode;
        Ok(())
    }

    fn cooler_boost(&self) -> Result<bool, EcError> {
        Ok(self.state().cooler_boost)
    }

    fn set_cooler_boost(&mut self, on: bool) -> Result<(), EcError> {
        self.state().cooler_boost = on;
        Ok(())
    }

    fn read_tables(&self, fan: Fan) -> Result<RawTables, EcError> {
        let s = self.state();
        if !s.curve_supported {
            return Err(eio("fan_curve_speeds"));
        }
        let mut t = match fan {
            Fan::Cpu => s.cpu,
            Fan::Gpu => s.gpu,
        };
        if s.corrupt_readback == Some(fan) {
            t.speeds[1] = t.speeds[1].wrapping_add(1);
        }
        Ok(t)
    }

    fn write_table(&mut self, fan: Fan, write: &TableWrite) -> Result<(), EcError> {
        let mut s = self.state();
        if !s.curve_supported {
            return Err(eio("fan_curve_speeds"));
        }
        if s.fail_table_writes {
            return Err(eio("fan_curve"));
        }
        let mut next = match fan {
            Fan::Cpu => s.cpu,
            Fan::Gpu => s.gpu,
        };
        next.apply(write);
        if next.kernel_check().is_err() {
            return Err(einval("fan_curve"));
        }
        match fan {
            Fan::Cpu => s.cpu = next,
            Fan::Gpu => s.gpu = next,
        }
        s.writes.push((fan, *write));
        Ok(())
    }

    fn temperature(&self, fan: Fan) -> Result<u8, EcError> {
        let s = self.state();
        match fan {
            Fan::Cpu => s.cpu_temp,
            Fan::Gpu => s.gpu_temp,
        }
        .ok_or_else(|| eio("realtime_temperature"))
    }

    fn fan_speed(&self, fan: Fan) -> Result<u8, EcError> {
        let temp = self.temperature(fan)?;
        let t = self.read_tables(fan)?;
        Ok(t.min_speed_at(temp))
    }
}

/// Writes a fake msi-ec tree (factory tables, `auto`, `comfort`) under `root` (tests and development).
///
/// # Errors
/// I/O errors while creating the files.
pub fn write_fake_tree(root: &Path, fw: &str, with_curve: bool) -> io::Result<()> {
    let dir = root.join("sys").join(MSI_EC_DIR);
    let put = |rel: &str, content: &str| -> io::Result<()> {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)
    };
    put("fw_version", &format!("{fw}\n"))?;
    put("fan_mode", "auto\n")?;
    put("shift_mode", "comfort\n")?;
    put("cooler_boost", "off\n")?;
    for (fan, t, temp) in [(Fan::Cpu, FACTORY_CPU, "52"), (Fan::Gpu, FACTORY_GPU, "47")] {
        put(&format!("{fan}/realtime_temperature"), &format!("{temp}\n"))?;
        put(&format!("{fan}/realtime_fan_speed"), "0\n")?;
        if with_curve {
            put(
                &format!("{fan}/fan_curve_speeds"),
                &format!("{}\n", format_table(&t.speeds)),
            )?;
            put(
                &format!("{fan}/fan_curve_up_temps"),
                &format!("{}\n", format_table(&t.up)),
            )?;
            put(
                &format!("{fan}/fan_curve_down_temps"),
                &format!("{}\n", format_table(&t.down)),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::fs::testutil::read;

    #[test]
    fn table_format_round_trips() {
        assert_eq!(format_table(&[0, 36, 100]), "0 36 100");
        assert_eq!(parse_table::<3>("0 36 100\n"), Some([0, 36, 100]));
        assert_eq!(parse_table::<3>("0 36"), None);
        assert_eq!(parse_table::<3>("0 36 100 4"), None);
        assert_eq!(parse_table::<2>("0 x"), None);
        assert_eq!(parse_table::<2>("0 256"), None);
    }

    #[test]
    fn sysfs_reads_and_writes_the_contract_files() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tree(dir.path(), "14K2EMS1.110", true).unwrap();
        let mut ec = SysfsEc::new(&FsRoot::new(dir.path()));
        assert_eq!(ec.fw_version().unwrap(), "14K2EMS1.110");
        assert!(ec.curve_supported());
        assert_eq!(ec.fan_mode().unwrap(), FanMode::Auto);
        assert_eq!(ec.shift_mode().unwrap(), ShiftMode::Comfort);
        assert!(!ec.cooler_boost().unwrap());
        assert_eq!(ec.read_tables(Fan::Cpu).unwrap(), FACTORY_CPU);
        assert_eq!(ec.read_tables(Fan::Gpu).unwrap(), FACTORY_GPU);
        assert_eq!(ec.temperature(Fan::Cpu).unwrap(), 52);
        assert_eq!(ec.fan_speed(Fan::Gpu).unwrap(), 0);

        ec.set_fan_mode(FanMode::Advanced).unwrap();
        ec.set_shift_mode(ShiftMode::Eco).unwrap();
        ec.set_cooler_boost(true).unwrap();
        ec.write_table(Fan::Gpu, &TableWrite::Up([40, 47, 54, 61, 68, 74]))
            .unwrap();
        let base = "sys/devices/platform/msi-ec";
        assert_eq!(read(dir.path(), &format!("{base}/fan_mode")), "advanced");
        assert_eq!(read(dir.path(), &format!("{base}/shift_mode")), "eco");
        assert_eq!(read(dir.path(), &format!("{base}/cooler_boost")), "on");
        assert_eq!(
            read(dir.path(), &format!("{base}/gpu/fan_curve_up_temps")),
            "40 47 54 61 68 74"
        );
        restore_auto_at(&ec.fan_mode_path()).unwrap();
        assert_eq!(ec.fan_mode().unwrap(), FanMode::Auto);
    }

    #[test]
    fn s8_missing_curve_files_mean_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tree(dir.path(), "14K2EMS1.109", false).unwrap();
        let ec = SysfsEc::new(&FsRoot::new(dir.path()));
        assert!(!ec.curve_supported());
        assert!(ec.read_tables(Fan::Cpu).is_err());
        let empty = tempfile::tempdir().unwrap();
        let ec = SysfsEc::new(&FsRoot::new(empty.path()));
        assert!(ec.fw_version().is_err());
        assert!(!ec.curve_supported());
    }

    #[test]
    fn unknown_values_are_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_tree(dir.path(), "x", true).unwrap();
        fs::write(
            dir.path().join("sys").join(MSI_EC_DIR).join("fan_mode"),
            "weird\n",
        )
        .unwrap();
        let ec = SysfsEc::new(&FsRoot::new(dir.path()));
        assert!(matches!(ec.fan_mode(), Err(EcError::Parse { .. })));
    }

    #[test]
    fn writes_never_create_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut ec = SysfsEc::new(&FsRoot::new(dir.path()));
        assert!(ec.set_fan_mode(FanMode::Advanced).is_err());
        assert!(!dir.path().join("sys").exists());
    }

    #[test]
    fn memory_ec_enforces_kernel_rules() {
        let mut ec = MemoryEc::default();
        // Lowering up thresholds below the current down thresholds: EINVAL, nothing changes.
        let err = ec
            .write_table(Fan::Cpu, &TableWrite::Up([40, 47, 54, 61, 68, 74]))
            .unwrap_err();
        assert!(err.to_string().contains("os error 22"), "{err}");
        // Last speed below 80 %: EINVAL.
        assert!(
            ec.write_table(Fan::Cpu, &TableWrite::Speeds([0, 30, 33, 43, 57, 70, 79]))
                .is_err()
        );
        // Decreasing speeds: EINVAL.
        assert!(
            ec.write_table(Fan::Cpu, &TableWrite::Speeds([0, 30, 20, 43, 57, 70, 100]))
                .is_err()
        );
        assert_eq!(ec.read_tables(Fan::Cpu).unwrap(), FACTORY_CPU);
        assert_eq!(ec.state().writes.len(), 0);
        ec.write_table(Fan::Cpu, &TableWrite::Speeds([0, 30, 33, 43, 57, 76, 100]))
            .unwrap();
        assert_eq!(ec.state().writes.len(), 1);
    }

    #[test]
    fn names_round_trip() {
        for f in Fan::ALL {
            assert_eq!(Fan::parse(f.as_str()), Some(f));
        }
        assert_eq!(Fan::parse("CPU"), None);
        for m in [FanMode::Auto, FanMode::Silent, FanMode::Advanced] {
            assert_eq!(parse_fan_mode(m.as_str()), Some(m));
        }
        for m in [ShiftMode::Eco, ShiftMode::Comfort, ShiftMode::Turbo] {
            assert_eq!(parse_shift_mode(m.as_str()), Some(m));
        }
    }
}
