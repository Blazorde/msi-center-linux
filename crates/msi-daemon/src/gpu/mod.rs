//! NVIDIA dGPU power management over sysfs: discovery, state, holders, runtime PM, PCI remove/rescan.
//!
//! Findings behind this design: `docs/hardware/gpu-power.md`. Every operation takes its paths from an
//! [`FsRoot`]; write operations are only ever exercised against fake trees in tests.

mod holders;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub use holders::{Holder, HolderScan};

use crate::fs::{FsRoot, read_trimmed, write_attr};

/// PCI vendor id of NVIDIA.
pub const NVIDIA_VENDOR: &str = "0x10de";
/// PCI vendor id of Intel.
pub const INTEL_VENDOR: &str = "0x8086";

const PCI_DEVICES: &str = "bus/pci/devices";

/// Power state of the dGPU, as exposed on D-Bus (`GpuState` property).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuState {
    /// Powered (D0), possibly idle.
    Active,
    /// Runtime-suspended (RTD3, D3hot/D3cold), or transitioning to/from it.
    Suspended,
    /// Not on the PCI bus (removed, Éco profile).
    Removed,
}

impl GpuState {
    /// Lower-case name used on D-Bus.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Removed => "removed",
        }
    }
}

impl fmt::Display for GpuState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classifies sysfs attributes of a present device. Transitional states count as [`GpuState::Suspended`] so
/// that nothing pokes a device that is going to sleep or waking up.
fn classify(runtime_status: Option<&str>, power_state: Option<&str>) -> GpuState {
    if power_state == Some("D3cold") {
        return GpuState::Suspended;
    }
    match runtime_status {
        Some("suspended" | "suspending" | "resuming") => GpuState::Suspended,
        // "active", "unsupported" (runtime PM disabled: always powered) or attribute missing.
        _ => GpuState::Active,
    }
}

/// `power/control` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerControl {
    /// Runtime PM enabled: the device may suspend when idle (Balanced).
    Auto,
    /// Runtime PM disabled: always powered (Gaming).
    On,
}

impl PowerControl {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
        }
    }
}

/// One PCI function (`dddd:bb:dd.f`) and its sysfs directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PciFunction {
    /// PCI address, e.g. `0000:01:00.0`.
    pub address: String,
    /// sysfs directory (`<root>/sys/bus/pci/devices/<address>`).
    pub path: PathBuf,
}

/// A discovered NVIDIA dGPU with its companion functions (HDMI audio, USB-C…) and DRM nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dgpu {
    /// The display function.
    pub gpu: PciFunction,
    /// Other functions of the same PCI device (same `dddd:bb:dd`).
    pub companions: Vec<PciFunction>,
    /// DRM nodes of the GPU, e.g. `/dev/dri/card0`, `/dev/dri/renderD129`.
    pub drm_nodes: Vec<String>,
}

impl Dgpu {
    /// Current power state, read from `power/runtime_status` and `power_state`. These are software attributes:
    /// reading them never wakes the device.
    #[must_use]
    pub fn state(&self) -> GpuState {
        if !self.gpu.path.exists() {
            return GpuState::Removed;
        }
        let status = read_trimmed(&self.gpu.path.join("power/runtime_status")).ok();
        let power = read_trimmed(&self.gpu.path.join("power_state")).ok();
        classify(status.as_deref(), power.as_deref())
    }

    /// Companions first, then the GPU: the order used for unbind/remove.
    fn functions(&self) -> impl Iterator<Item = &PciFunction> {
        self.companions.iter().chain(std::iter::once(&self.gpu))
    }

    /// Connectors of the dGPU that currently drive a display (`drm/cardN/cardN-*/status == connected`).
    #[must_use]
    pub fn connected_outputs(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(cards) = fs::read_dir(self.gpu.path.join("drm")) else {
            return out;
        };
        for card in cards
            .flatten()
            .filter(|c| is_card(&c.file_name().to_string_lossy()))
        {
            let Ok(connectors) = fs::read_dir(card.path()) else {
                continue;
            };
            for conn in connectors.flatten() {
                let name = conn.file_name().to_string_lossy().into_owned();
                if name.contains('-')
                    && read_trimmed(&conn.path().join("status")).is_ok_and(|s| s == "connected")
                {
                    out.push(name);
                }
            }
        }
        out.sort();
        out
    }
}

fn is_card(name: &str) -> bool {
    name.strip_prefix("card")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Errors of GPU power operations.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// No NVIDIA dGPU on the bus.
    #[error("no NVIDIA dGPU found on the PCI bus")]
    NotFound,
    /// Processes still hold the device: removing it would crash them (or the compositor).
    #[error("dGPU is in use by {}", format_holders(.0))]
    Busy(Vec<Holder>),
    /// Some processes could not be inspected, so the device cannot be proven free.
    #[error("cannot inspect {0} process(es) (not root?): refusing to remove the dGPU")]
    IncompleteScan(usize),
    /// The dGPU drives a connected display.
    #[error("dGPU drives connected output(s): {}", .0.join(", "))]
    DisplayConnected(Vec<String>),
    /// The dGPU is the boot VGA device: removing it would blank the console.
    #[error("dGPU is the boot VGA device")]
    BootVga,
    /// A sysfs write failed.
    #[error("writing {}: {source}", path.display())]
    Io {
        /// Attribute written.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
}

fn format_holders(holders: &[Holder]) -> String {
    holders
        .iter()
        .map(|h| format!("{} ({})", h.name, h.pid))
        .collect::<Vec<_>>()
        .join(", ")
}

fn write(path: PathBuf, value: &str) -> Result<(), GpuError> {
    write_attr(&path, value).map_err(|source| GpuError::Io { path, source })
}

/// Entry point of GPU power management.
#[derive(Debug, Clone)]
pub struct GpuManager {
    root: FsRoot,
}

impl GpuManager {
    /// Manager operating under `root`.
    #[must_use]
    pub fn new(root: FsRoot) -> Self {
        Self { root }
    }

    /// Finds the first NVIDIA display-class PCI function, its companions and DRM nodes.
    #[must_use]
    pub fn discover(&self) -> Option<Dgpu> {
        let gpu = find_display_function(&self.root, NVIDIA_VENDOR)?;
        let slot = slot_prefix(&gpu.address)?;
        let mut companions: Vec<PciFunction> = pci_functions(&self.root)
            .into_iter()
            .filter(|f| f.address != gpu.address && f.address.starts_with(slot))
            .collect();
        companions.sort_by(|a, b| a.address.cmp(&b.address));
        let drm_nodes = drm_nodes(&gpu.path);
        Some(Dgpu {
            gpu,
            companions,
            drm_nodes,
        })
    }

    /// State of the dGPU; [`GpuState::Removed`] when it is not on the bus.
    #[must_use]
    pub fn state(&self) -> GpuState {
        self.discover().map_or(GpuState::Removed, |d| d.state())
    }

    /// Processes holding the dGPU (NVIDIA character devices or its DRM nodes).
    #[must_use]
    pub fn holders(&self, dgpu: &Dgpu) -> HolderScan {
        holders::scan(&self.root, &dgpu.drm_nodes)
    }

    /// Writes `power/control` on every function of the dGPU (all functions must allow runtime PM for the
    /// link to reach D3cold).
    ///
    /// # Errors
    /// [`GpuError::Io`] if an attribute cannot be written.
    pub fn set_runtime_pm(&self, dgpu: &Dgpu, control: PowerControl) -> Result<(), GpuError> {
        for function in dgpu.functions() {
            write(function.path.join("power/control"), control.as_str())?;
        }
        Ok(())
    }

    /// Checks every precondition of [`Self::remove`] without changing anything.
    ///
    /// # Errors
    /// The first failed precondition: [`GpuError::BootVga`], [`GpuError::DisplayConnected`],
    /// [`GpuError::IncompleteScan`] or [`GpuError::Busy`] (with every holder).
    pub fn check_removable(&self, dgpu: &Dgpu) -> Result<(), GpuError> {
        if read_trimmed(&dgpu.gpu.path.join("boot_vga")).is_ok_and(|v| v == "1") {
            return Err(GpuError::BootVga);
        }
        let outputs = dgpu.connected_outputs();
        if !outputs.is_empty() {
            return Err(GpuError::DisplayConnected(outputs));
        }
        let scan = self.holders(dgpu);
        if scan.unreadable > 0 {
            return Err(GpuError::IncompleteScan(scan.unreadable));
        }
        if !scan.holders.is_empty() {
            return Err(GpuError::Busy(scan.holders));
        }
        Ok(())
    }

    /// Removes the dGPU from the PCI bus (Éco). Idempotent: succeeds if it is already gone.
    ///
    /// Never kills anything. The caller must first stop `nvidia-powerd`/`nvidia-persistenced` and release its
    /// own NVML handle, otherwise they show up as holders. Drivers are unbound, then each function (companions
    /// first) is removed. Modules stay loaded so that [`Self::rescan`] rebinds the driver automatically.
    ///
    /// # Errors
    /// Any [`Self::check_removable`] error, or [`GpuError::Io`] if a sysfs write fails.
    pub fn remove(&self) -> Result<(), GpuError> {
        let Some(dgpu) = self.discover() else {
            return Ok(());
        };
        self.check_removable(&dgpu)?;
        for function in dgpu.functions() {
            let driver = function.path.join("driver");
            if driver.exists() {
                write(driver.join("unbind"), &function.address)?;
            }
        }
        for function in dgpu.functions() {
            write(function.path.join("remove"), "1")?;
        }
        Ok(())
    }

    /// Rescans the PCI bus so that a removed dGPU reappears (leaving Éco).
    ///
    /// # Errors
    /// [`GpuError::Io`] if `/sys/bus/pci/rescan` cannot be written.
    pub fn rescan(&self) -> Result<(), GpuError> {
        write(self.root.sys("bus/pci/rescan"), "1")
    }
}

/// All PCI functions listed under `<root>/sys/bus/pci/devices`.
fn pci_functions(root: &FsRoot) -> Vec<PciFunction> {
    let dir = root.sys(PCI_DEVICES);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .map(|address| PciFunction {
            path: dir.join(&address),
            address,
        })
        .collect()
}

/// First (lowest address) display-class (`0x03xxxx`) function of `vendor`.
pub(crate) fn find_display_function(root: &FsRoot, vendor: &str) -> Option<PciFunction> {
    let mut candidates: Vec<PciFunction> = pci_functions(root)
        .into_iter()
        .filter(|f| {
            read_trimmed(&f.path.join("vendor")).is_ok_and(|v| v == vendor)
                && read_trimmed(&f.path.join("class")).is_ok_and(|c| c.starts_with("0x03"))
        })
        .collect();
    candidates.sort_by(|a, b| a.address.cmp(&b.address));
    candidates.into_iter().next()
}

/// `0000:01:00.0` → `0000:01:00.`
fn slot_prefix(address: &str) -> Option<&str> {
    address.rfind('.').map(|dot| &address[..=dot])
}

fn drm_nodes(device: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(device.join("drm")) else {
        return Vec::new();
    };
    let mut nodes: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_card(n) || n.starts_with("renderD"))
        .map(|n| format!("/dev/dri/{n}"))
        .collect();
    nodes.sort();
    nodes
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::fs::testutil::{read, symlink, write};

    /// Fake tree mirroring the A1VGG: iGPU 00:02.0, root port 00:01.0, dGPU 01:00.0 + audio 01:00.1.
    fn fake_machine(r: &Path) {
        let dev = "sys/devices/pci0000:00";
        for (addr, parent, vendor, class) in [
            ("0000:00:02.0", "", "0x8086", "0x030000"),
            ("0000:00:01.0", "", "0x8086", "0x060400"),
            ("0000:01:00.0", "0000:00:01.0/", "0x10de", "0x030000"),
            ("0000:01:00.1", "0000:00:01.0/", "0x10de", "0x040300"),
        ] {
            let path = format!("{dev}/{parent}{addr}");
            write(r, &format!("{path}/vendor"), &format!("{vendor}\n"));
            write(r, &format!("{path}/class"), &format!("{class}\n"));
            write(r, &format!("{path}/power/control"), "on\n");
            write(r, &format!("{path}/power/runtime_status"), "active\n");
            write(r, &format!("{path}/power_state"), "D0\n");
            write(r, &format!("{path}/remove"), "");
            symlink(
                r,
                &format!("sys/bus/pci/devices/{addr}"),
                &format!("../../../devices/pci0000:00/{parent}{addr}"),
            );
        }
        let gpu = format!("{dev}/0000:00:01.0/0000:01:00.0");
        write(r, &format!("{gpu}/boot_vga"), "0\n");
        write(
            r,
            &format!("{gpu}/drm/card0/card0-HDMI-A-1/status"),
            "disconnected\n",
        );
        write(
            r,
            &format!("{gpu}/drm/card0/card0-eDP-2/status"),
            "disconnected\n",
        );
        write(r, &format!("{gpu}/drm/renderD129/dev"), "226:129\n");
        fs::create_dir_all(r.join(format!("{gpu}/drm/controlD64"))).unwrap();
        write(r, "sys/bus/pci/drivers/nvidia/unbind", "");
        write(r, "sys/bus/pci/drivers/snd_hda_intel/unbind", "");
        symlink(r, &format!("{gpu}/driver"), "../../../../bus/pci/drivers/nvidia");
        symlink(
            r,
            &format!("{dev}/0000:00:01.0/0000:01:00.1/driver"),
            "../../../../bus/pci/drivers/snd_hda_intel",
        );
        write(r, "sys/bus/pci/rescan", "");
        fs::create_dir_all(r.join("proc")).unwrap();
    }

    const GPU: &str = "sys/bus/pci/devices/0000:01:00.0";

    fn manager(r: &Path) -> GpuManager {
        GpuManager::new(FsRoot::new(r))
    }

    #[test]
    fn classify_maps_runtime_pm_attributes() {
        assert_eq!(classify(Some("active"), Some("D0")), GpuState::Active);
        assert_eq!(classify(Some("unsupported"), None), GpuState::Active);
        assert_eq!(classify(None, None), GpuState::Active);
        assert_eq!(classify(Some("suspended"), Some("D3cold")), GpuState::Suspended);
        assert_eq!(classify(Some("suspending"), Some("D0")), GpuState::Suspended);
        assert_eq!(classify(Some("resuming"), Some("D3hot")), GpuState::Suspended);
        assert_eq!(classify(Some("active"), Some("D3cold")), GpuState::Suspended);
    }

    #[test]
    fn state_names_match_dbus_api() {
        assert_eq!(GpuState::Active.to_string(), "active");
        assert_eq!(GpuState::Suspended.as_str(), "suspended");
        assert_eq!(GpuState::Removed.as_str(), "removed");
    }

    #[test]
    fn discovers_nvidia_display_function_companions_and_drm_nodes() {
        let dir = tempfile::tempdir().unwrap();
        fake_machine(dir.path());
        let dgpu = manager(dir.path()).discover().unwrap();
        assert_eq!(dgpu.gpu.address, "0000:01:00.0");
        assert_eq!(
            dgpu.companions
                .iter()
                .map(|f| f.address.as_str())
                .collect::<Vec<_>>(),
            ["0000:01:00.1"]
        );
        assert_eq!(dgpu.drm_nodes, ["/dev/dri/card0", "/dev/dri/renderD129"]);
        assert_eq!(dgpu.state(), GpuState::Active);
    }

    #[test]
    fn missing_dgpu_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        fake_machine(dir.path());
        fs::remove_file(dir.path().join(GPU)).unwrap();
        fs::remove_file(dir.path().join("sys/bus/pci/devices/0000:01:00.1")).unwrap();
        let m = manager(dir.path());
        assert!(m.discover().is_none());
        assert_eq!(m.state(), GpuState::Removed);
        // Already removed: remove is a no-op.
        m.remove().unwrap();
    }

    #[test]
    fn state_follows_runtime_status() {
        let dir = tempfile::tempdir().unwrap();
        fake_machine(dir.path());
        write(dir.path(), &format!("{GPU}/power/runtime_status"), "suspended\n");
        write(dir.path(), &format!("{GPU}/power_state"), "D3cold\n");
        assert_eq!(manager(dir.path()).state(), GpuState::Suspended);
    }

    #[test]
    fn set_runtime_pm_writes_every_function() {
        let dir = tempfile::tempdir().unwrap();
        fake_machine(dir.path());
        let m = manager(dir.path());
        let dgpu = m.discover().unwrap();
        m.set_runtime_pm(&dgpu, PowerControl::Auto).unwrap();
        assert_eq!(read(dir.path(), &format!("{GPU}/power/control")), "auto");
        assert_eq!(
            read(dir.path(), "sys/bus/pci/devices/0000:01:00.1/power/control"),
            "auto"
        );
        assert_eq!(
            read(dir.path(), "sys/bus/pci/devices/0000:00:02.0/power/control"),
            "on\n"
        );
        m.set_runtime_pm(&dgpu, PowerControl::On).unwrap();
        assert_eq!(read(dir.path(), &format!("{GPU}/power/control")), "on");
    }

    #[test]
    fn remove_refuses_with_holders_and_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fake_machine(r);
        write(r, "proc/3876/comm", "gnome-shell\n");
        symlink(r, "proc/3876/fd/14", "/dev/nvidia0");
        write(r, "proc/4255/comm", "Xwayland\n");
        symlink(r, "proc/4255/fd/3", "/dev/dri/card0");

        let err = manager(r).remove().unwrap_err();
        match &err {
            GpuError::Busy(holders) => assert_eq!(
                holders,
                &[
                    Holder {
                        pid: 3876,
                        name: "gnome-shell".into()
                    },
                    Holder {
                        pid: 4255,
                        name: "Xwayland".into()
                    }
                ]
            ),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            err.to_string(),
            "dGPU is in use by gnome-shell (3876), Xwayland (4255)"
        );
        assert_eq!(read(r, &format!("{GPU}/remove")), "");
        assert_eq!(read(r, "sys/bus/pci/drivers/nvidia/unbind"), "");
    }

    #[test]
    fn remove_refuses_when_a_display_is_connected() {
        let dir = tempfile::tempdir().unwrap();
        fake_machine(dir.path());
        write(
            dir.path(),
            &format!("{GPU}/drm/card0/card0-HDMI-A-1/status"),
            "connected\n",
        );
        let err = manager(dir.path()).remove().unwrap_err();
        assert!(
            matches!(&err, GpuError::DisplayConnected(o) if o == &["card0-HDMI-A-1"]),
            "{err:?}"
        );
        assert_eq!(read(dir.path(), &format!("{GPU}/remove")), "");
    }

    #[test]
    fn remove_refuses_boot_vga() {
        let dir = tempfile::tempdir().unwrap();
        fake_machine(dir.path());
        write(dir.path(), &format!("{GPU}/boot_vga"), "1\n");
        assert!(matches!(manager(dir.path()).remove(), Err(GpuError::BootVga)));
    }

    #[test]
    fn remove_unbinds_then_removes_every_function() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fake_machine(r);
        write(r, "proc/6164/comm", "ghostty\n");
        symlink(r, "proc/6164/fd/9", "/dev/dri/renderD128");

        manager(r).remove().unwrap();
        assert_eq!(read(r, "sys/bus/pci/drivers/nvidia/unbind"), "0000:01:00.0");
        assert_eq!(
            read(r, "sys/bus/pci/drivers/snd_hda_intel/unbind"),
            "0000:01:00.1"
        );
        assert_eq!(read(r, &format!("{GPU}/remove")), "1");
        assert_eq!(read(r, "sys/bus/pci/devices/0000:01:00.1/remove"), "1");
        assert_eq!(read(r, "sys/bus/pci/devices/0000:00:02.0/remove"), "");
    }

    #[test]
    fn rescan_writes_bus_rescan() {
        let dir = tempfile::tempdir().unwrap();
        fake_machine(dir.path());
        manager(dir.path()).rescan().unwrap();
        assert_eq!(read(dir.path(), "sys/bus/pci/rescan"), "1");
    }

    #[test]
    fn write_errors_carry_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let err = manager(dir.path()).rescan().unwrap_err();
        assert!(err.to_string().contains("sys/bus/pci/rescan"), "{err}");
    }

    #[test]
    fn helpers() {
        assert_eq!(slot_prefix("0000:01:00.0"), Some("0000:01:00."));
        assert!(is_card("card0") && is_card("card12"));
        assert!(!is_card("card") && !is_card("card0-HDMI-A-1") && !is_card("controlD64"));
    }
}
