//! Telemetry sampling: CPU, RAM, iGPU, dGPU, NPU, temperatures and fans.
//!
//! Sources: `docs/hardware/gpu-power.md`. Delta computations are pure functions; readers take an [`FsRoot`].
//! A missing source yields `None`, never an error or a panic. One [`Sampler::sample`] per second reads a few
//! small files (plus a cached list of DRM fdinfo files, fully rescanned every
//! [`IgpuReader::DEFAULT_RESCAN_EVERY`] samples).

pub mod cpu;
pub mod dgpu;
pub mod drm;
pub mod hwmon;
pub mod npu;

use std::fs;

use self::cpu::{CpuTimes, cpu_percent, parse_proc_stat, ram_percent};
use self::dgpu::{DgpuBackend, DgpuMonitor, DgpuPolicy};
use self::drm::{DrmSnapshot, IgpuReader, busiest_engine_percent};
use self::hwmon::{Coretemp, MsiWmiFans, ThermalSource, Thermals};
use self::npu::NpuReader;
use crate::fs::FsRoot;
use crate::gpu::{Dgpu, GpuManager, GpuState};

/// Monotonic timestamp in nanoseconds (injected so delta logic is testable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Clock(pub u64);

/// A cumulative busy-time counter at an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusySample {
    /// Busy time so far.
    pub busy_ns: u64,
    /// When it was read.
    pub at_ns: u64,
}

/// `num / den` in percent, unclamped; `None` if `den == 0`.
#[allow(
    clippy::cast_precision_loss,
    reason = "deltas between two samples are far below 2^52"
)]
pub(crate) fn percent_f(num: u64, den: u64) -> Option<f64> {
    (den != 0).then(|| num as f64 / den as f64 * 100.0)
}

/// `num / den` in percent clamped to `0..=100`; `None` if `den == 0`.
pub(crate) fn percent(num: u64, den: u64) -> Option<f64> {
    percent_f(num, den).map(|p| p.clamp(0.0, 100.0))
}

/// Busy percentage between two counter samples. `None` if no time elapsed or time went backwards.
#[must_use]
pub fn busy_percent(prev: BusySample, cur: BusySample) -> Option<f64> {
    let dt = cur.at_ns.checked_sub(prev.at_ns)?;
    percent(cur.busy_ns.saturating_sub(prev.busy_ns), dt)
}

/// One telemetry sample (the D-Bus `Telemetry` signal payload).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// CPU load, %.
    pub cpu_pct: Option<f64>,
    /// RAM in use, %.
    pub ram_pct: Option<f64>,
    /// iGPU busiest engine, %.
    pub igpu_pct: Option<f64>,
    /// dGPU utilisation, % (only when the dGPU is active and polling is allowed).
    pub dgpu_pct: Option<f64>,
    /// NPU busy, %.
    pub npu_pct: Option<f64>,
    /// Temperatures and fans.
    pub thermals: Thermals,
    /// dGPU board power, W.
    pub dgpu_power_w: Option<f64>,
    /// dGPU power state.
    pub dgpu_state: GpuState,
}

/// Stateful sampler holding the previous counters.
#[derive(Debug)]
pub struct Sampler<B> {
    root: FsRoot,
    gpu: GpuManager,
    dgpu: Option<Dgpu>,
    rediscover_in: u32,
    cpu_prev: Option<CpuTimes>,
    igpu: Option<IgpuReader>,
    igpu_prev: Option<DrmSnapshot>,
    npu: NpuReader,
    npu_prev: Option<BusySample>,
    thermal_sources: Vec<Box<dyn ThermalSource>>,
    dgpu_monitor: DgpuMonitor<B>,
}

impl<B: DgpuBackend> Sampler<B> {
    /// Samples before looking for a missing dGPU again.
    const REDISCOVER_EVERY: u32 = 5;

    /// Sampler reading under `root`, with the default thermal sources (`coretemp`, `msi_wmi_platform`).
    pub fn new(root: FsRoot, backend: B, policy: DgpuPolicy) -> Self {
        let gpu = GpuManager::new(root.clone());
        Self {
            dgpu: gpu.discover(),
            gpu,
            rediscover_in: 0,
            cpu_prev: None,
            igpu: IgpuReader::discover(&root, IgpuReader::DEFAULT_RESCAN_EVERY),
            igpu_prev: None,
            npu: NpuReader::discover(&root),
            npu_prev: None,
            thermal_sources: vec![Box::new(Coretemp::new(&root)), Box::new(MsiWmiFans::new(&root))],
            dgpu_monitor: DgpuMonitor::new(backend, policy),
            root,
        }
    }

    /// Adds a thermal source with the highest priority (e.g. `msi-ec` once available).
    pub fn prepend_thermal_source(&mut self, source: Box<dyn ThermalSource>) {
        self.thermal_sources.insert(0, source);
    }

    /// Changes the dGPU polling policy (per profile).
    pub fn set_dgpu_policy(&mut self, policy: DgpuPolicy) {
        self.dgpu_monitor.set_policy(policy);
    }

    /// Closes every dGPU handle held by telemetry. Must be called before removing the dGPU.
    pub fn release_dgpu(&mut self) {
        self.dgpu_monitor.release();
    }

    /// Access to the dGPU monitor (tests, diagnostics).
    #[must_use]
    pub fn dgpu_monitor(&self) -> &DgpuMonitor<B> {
        &self.dgpu_monitor
    }

    fn dgpu_state(&mut self) -> GpuState {
        if self.dgpu.is_none() {
            if self.rediscover_in == 0 {
                self.dgpu = self.gpu.discover();
                self.rediscover_in = Self::REDISCOVER_EVERY;
            }
            self.rediscover_in -= 1;
        }
        let state = self.dgpu.as_ref().map_or(GpuState::Removed, Dgpu::state);
        if state == GpuState::Removed {
            self.dgpu = None;
        }
        state
    }

    /// Takes one sample at `now`. Delta-based fields are `None` on the first call.
    pub fn sample(&mut self, now: Clock) -> Sample {
        let cpu = fs::read_to_string(self.root.proc("stat"))
            .ok()
            .and_then(|t| parse_proc_stat(&t));
        let cpu_pct = self.cpu_prev.zip(cpu).and_then(|(a, b)| cpu_percent(a, b));
        self.cpu_prev = cpu;

        let ram_pct = fs::read_to_string(self.root.proc("meminfo"))
            .ok()
            .and_then(|t| ram_percent(&t));

        let igpu = self.igpu.as_mut().map(|r| r.snapshot(now));
        let igpu_pct = self
            .igpu_prev
            .as_ref()
            .zip(igpu.as_ref())
            .and_then(|(a, b)| busiest_engine_percent(a, b));
        self.igpu_prev = igpu;

        let npu = self.npu.read(now);
        let npu_pct = self.npu_prev.zip(npu).and_then(|(a, b)| busy_percent(a, b));
        self.npu_prev = npu;

        let dgpu_state = self.dgpu_state();
        let reading = self.dgpu_monitor.sample(dgpu_state).unwrap_or_default();

        let mut thermals = Thermals::default();
        for source in &mut self.thermal_sources {
            thermals.fill_missing(source.read());
        }
        thermals.gpu_temp_c = thermals.gpu_temp_c.or(reading.temp_c);

        Sample {
            cpu_pct,
            ram_pct,
            igpu_pct,
            dgpu_pct: reading.util_pct,
            npu_pct,
            thermals,
            dgpu_power_w: reading.power_w,
            dgpu_state,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use super::dgpu::DgpuReading;
    use super::*;
    use crate::fs::testutil::{symlink, write};

    #[derive(Debug, Default)]
    struct Fake {
        reads: u32,
        open: bool,
    }

    impl DgpuBackend for Fake {
        fn read(&mut self) -> Option<DgpuReading> {
            self.reads += 1;
            self.open = true;
            Some(DgpuReading {
                util_pct: Some(12.0),
                temp_c: Some(45.0),
                power_w: Some(1.4),
            })
        }
        fn release(&mut self) {
            self.open = false;
        }
    }

    #[derive(Debug)]
    struct FixedEc;

    impl ThermalSource for FixedEc {
        fn read(&mut self) -> Thermals {
            Thermals {
                gpu_temp_c: Some(50.0),
                fan1_rpm: Some(3000),
                ..Thermals::default()
            }
        }
    }

    fn fake_machine(r: &Path, stat: &str, npu_us: &str, render_ns: u64, runtime_status: &str) {
        write(r, "proc/stat", stat);
        write(r, "proc/meminfo", "MemTotal: 1000 kB\nMemAvailable: 750 kB\n");
        write(r, "sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
        write(r, "sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n");
        write(r, "sys/bus/pci/devices/0000:01:00.0/vendor", "0x10de\n");
        write(r, "sys/bus/pci/devices/0000:01:00.0/class", "0x030000\n");
        write(
            r,
            "sys/bus/pci/devices/0000:01:00.0/power/runtime_status",
            runtime_status,
        );
        write(r, "sys/devices/npu/npu_busy_time_us", npu_us);
        if !r.join("sys/class/accel/accel0/device").exists() {
            symlink(r, "sys/class/accel/accel0/device", "../../../devices/npu");
            symlink(r, "proc/10/fd/3", "/dev/dri/renderD128");
        }
        write(
            r,
            "proc/10/fdinfo/3",
            &format!("drm-client-id:\t1\ndrm-pdev:\t0000:00:02.0\ndrm-engine-render:\t{render_ns} ns\n"),
        );
        write(r, "sys/class/hwmon/hwmon7/name", "coretemp\n");
        write(r, "sys/class/hwmon/hwmon7/temp1_label", "Package id 0\n");
        write(r, "sys/class/hwmon/hwmon7/temp1_input", "56000\n");
        write(r, "sys/class/hwmon/hwmon4/name", "msi_wmi_platform\n");
        write(r, "sys/class/hwmon/hwmon4/fan1_input", "2232\n");
        write(r, "sys/class/hwmon/hwmon4/fan2_input", "2211\n");
    }

    #[test]
    fn full_sample_from_fake_tree() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fake_machine(r, "cpu  100 0 0 900 0 0 0 0\n", "0\n", 0, "active\n");
        let mut s = Sampler::new(FsRoot::new(r), Fake::default(), DgpuPolicy::WhenActive);

        let first = s.sample(Clock(0));
        assert_eq!((first.cpu_pct, first.igpu_pct, first.npu_pct), (None, None, None));
        assert_eq!(first.ram_pct, Some(25.0));

        fake_machine(
            r,
            "cpu  150 0 0 1050 0 0 0 0\n",
            "100000\n",
            500_000_000,
            "active\n",
        );
        let second = s.sample(Clock(1_000_000_000));
        assert_eq!(second.cpu_pct, Some(25.0));
        assert_eq!(second.igpu_pct, Some(50.0));
        assert_eq!(second.npu_pct, Some(10.0));
        assert_eq!(second.dgpu_state, GpuState::Active);
        assert_eq!(second.dgpu_pct, Some(12.0));
        assert_eq!(second.dgpu_power_w, Some(1.4));
        assert_eq!(
            second.thermals,
            Thermals {
                cpu_temp_c: Some(56.0),
                gpu_temp_c: Some(45.0),
                fan1_rpm: Some(2232),
                fan2_rpm: Some(2211)
            }
        );
    }

    #[test]
    fn suspended_or_removed_dgpu_is_never_queried() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fake_machine(r, "cpu  1 0 0 1 0 0 0 0\n", "0\n", 0, "suspended\n");
        let mut s = Sampler::new(FsRoot::new(r), Fake::default(), DgpuPolicy::WhenActive);
        let sample = s.sample(Clock(0));
        assert_eq!(sample.dgpu_state, GpuState::Suspended);
        assert_eq!((sample.dgpu_pct, sample.thermals.gpu_temp_c), (None, None));

        fs::remove_dir_all(r.join("sys/bus/pci/devices/0000:01:00.0")).unwrap();
        assert_eq!(s.sample(Clock(1)).dgpu_state, GpuState::Removed);
        assert_eq!(s.sample(Clock(2)).dgpu_state, GpuState::Removed);
        assert_eq!(s.dgpu_monitor().backend().reads, 0);
        assert!(!s.dgpu_monitor().backend().open);
    }

    #[test]
    fn removed_dgpu_is_rediscovered_after_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fake_machine(r, "cpu  1 0 0 1 0 0 0 0\n", "0\n", 0, "active\n");
        let gpu = r.join("sys/bus/pci/devices/0000:01:00.0");
        let parked = r.join("parked");
        fs::rename(&gpu, &parked).unwrap();
        let mut s = Sampler::new(FsRoot::new(r), Fake::default(), DgpuPolicy::WhenActive);
        assert_eq!(s.sample(Clock(0)).dgpu_state, GpuState::Removed);
        fs::rename(&parked, &gpu).unwrap();
        let states: Vec<GpuState> = (1..=6).map(|t| s.sample(Clock(t)).dgpu_state).collect();
        assert_eq!(states.last(), Some(&GpuState::Active));
    }

    #[test]
    fn prepended_source_wins_and_policy_off_releases() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        fake_machine(r, "cpu  1 0 0 1 0 0 0 0\n", "0\n", 0, "active\n");
        let mut s = Sampler::new(FsRoot::new(r), Fake::default(), DgpuPolicy::WhenActive);
        s.prepend_thermal_source(Box::new(FixedEc));
        let sample = s.sample(Clock(0));
        assert_eq!(sample.thermals.gpu_temp_c, Some(50.0));
        assert_eq!(sample.thermals.fan1_rpm, Some(3000));
        assert_eq!(sample.thermals.fan2_rpm, Some(2211));
        assert!(s.dgpu_monitor().backend().open);
        s.set_dgpu_policy(DgpuPolicy::Off);
        assert!(!s.dgpu_monitor().backend().open);
        s.sample(Clock(1));
        s.release_dgpu();
        assert_eq!(s.dgpu_monitor().backend().reads, 1);
    }

    #[test]
    fn empty_root_degrades_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Sampler::new(FsRoot::new(dir.path()), Fake::default(), DgpuPolicy::WhenActive);
        for t in 0..3 {
            let sample = s.sample(Clock(t));
            assert_eq!(sample.cpu_pct, None);
            assert_eq!(sample.ram_pct, None);
            assert_eq!(sample.igpu_pct, None);
            assert_eq!(sample.npu_pct, None);
            assert_eq!(sample.dgpu_state, GpuState::Removed);
            assert_eq!(sample.thermals, Thermals::default());
        }
    }

    #[test]
    fn busy_percent_handles_time_going_backwards() {
        let a = BusySample {
            busy_ns: 0,
            at_ns: 10,
        };
        let b = BusySample {
            busy_ns: 5,
            at_ns: 20,
        };
        assert_eq!(busy_percent(a, b), Some(50.0));
        assert_eq!(busy_percent(b, a), None);
        assert_eq!(busy_percent(a, a), None);
    }
}
