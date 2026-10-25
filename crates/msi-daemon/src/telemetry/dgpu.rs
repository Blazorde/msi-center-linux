//! dGPU utilisation, temperature and power via NVML, gated so that telemetry never wakes the dGPU.
//!
//! Why NVML: the NVIDIA driver exposes no hwmon and its DRM fdinfo has no `drm-engine-*` keys, so NVML
//! (`libnvidia-ml.so.1`, loaded at runtime by `nvml-wrapper` through `libloading`) is the only source of
//! utilisation and temperature. Parsing `nvidia-smi` would spawn a process every second and wake the GPU just
//! the same.
//!
//! Why it never wakes the GPU: NVML calls (including `nvmlInit`) resume a runtime-suspended GPU and, polled
//! every second, keep it from ever suspending. [`DgpuMonitor`] therefore:
//! 1. only calls the backend when the sysfs state (a software attribute) is [`GpuState::Active`];
//! 2. releases the backend (NVML shutdown, `/dev/nvidia*` closed) as soon as the state is anything else, so
//!    the daemon itself never blocks a PCI removal;
//! 3. does nothing at all under [`DgpuPolicy::Off`] (Balanced profile, where polling would defeat RTD3).

use std::fmt;

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

use crate::gpu::GpuState;

/// One dGPU reading. Fields are `None` when the backend cannot provide them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DgpuReading {
    /// GPU utilisation in percent.
    pub util_pct: Option<f64>,
    /// GPU die temperature in °C.
    pub temp_c: Option<f64>,
    /// Board power draw in W.
    pub power_w: Option<f64>,
}

/// Something that can query the dGPU (NVML in production, a fake in tests).
pub trait DgpuBackend: fmt::Debug + Send {
    /// Queries the GPU. May wake it: only called when the GPU is already active.
    fn read(&mut self) -> Option<DgpuReading>;
    /// Drops every handle on the GPU (closes `/dev/nvidia*`). Must be cheap when already released.
    fn release(&mut self);
}

/// When the dGPU may be queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DgpuPolicy {
    /// Never (Balanced: polling would keep the GPU out of RTD3; Éco: it is gone).
    Off,
    /// Only while the GPU is already active (Gaming).
    WhenActive,
}

/// Applies a [`DgpuPolicy`] around a [`DgpuBackend`].
#[derive(Debug)]
pub struct DgpuMonitor<B> {
    backend: B,
    policy: DgpuPolicy,
}

impl<B: DgpuBackend> DgpuMonitor<B> {
    /// Monitor with the given backend and policy.
    pub fn new(backend: B, policy: DgpuPolicy) -> Self {
        Self { backend, policy }
    }

    /// Changes the policy; switching to [`DgpuPolicy::Off`] releases the backend immediately.
    pub fn set_policy(&mut self, policy: DgpuPolicy) {
        self.policy = policy;
        if policy == DgpuPolicy::Off {
            self.backend.release();
        }
    }

    /// Current policy.
    #[must_use]
    pub fn policy(&self) -> DgpuPolicy {
        self.policy
    }

    /// Releases the backend (call before removing the dGPU).
    pub fn release(&mut self) {
        self.backend.release();
    }

    /// Reads the dGPU if allowed for `state`; otherwise releases the backend and returns `None`.
    pub fn sample(&mut self, state: GpuState) -> Option<DgpuReading> {
        if self.policy == DgpuPolicy::WhenActive && state == GpuState::Active {
            self.backend.read()
        } else {
            self.backend.release();
            None
        }
    }

    /// The backend (tests).
    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }
}

/// NVML backend. NVML is initialised lazily on the first allowed read and shut down on [`DgpuBackend::release`].
pub struct NvmlBackend {
    pci_bus_id: String,
    nvml: Option<Nvml>,
    retry_in: u32,
}

impl fmt::Debug for NvmlBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NvmlBackend")
            .field("pci_bus_id", &self.pci_bus_id)
            .field("initialised", &self.nvml.is_some())
            .finish_non_exhaustive()
    }
}

impl NvmlBackend {
    /// Reads made before retrying a failed `nvmlInit` (library missing, driver not loaded).
    const INIT_RETRY: u32 = 30;

    /// Backend for the GPU at `pci_bus_id` (e.g. `0000:01:00.0`).
    #[must_use]
    pub fn new(pci_bus_id: impl Into<String>) -> Self {
        Self {
            pci_bus_id: pci_bus_id.into(),
            nvml: None,
            retry_in: 0,
        }
    }

    fn nvml(&mut self) -> Option<&Nvml> {
        if self.nvml.is_none() {
            if self.retry_in > 0 {
                self.retry_in -= 1;
                return None;
            }
            self.nvml = Nvml::init().ok();
            if self.nvml.is_none() {
                self.retry_in = Self::INIT_RETRY;
            }
        }
        self.nvml.as_ref()
    }
}

impl DgpuBackend for NvmlBackend {
    fn read(&mut self) -> Option<DgpuReading> {
        let bus_id = self.pci_bus_id.clone();
        let nvml = self.nvml()?;
        let Ok(device) = nvml.device_by_pci_bus_id(bus_id.as_str()) else {
            return Some(DgpuReading::default());
        };
        Some(DgpuReading {
            util_pct: device.utilization_rates().ok().map(|u| f64::from(u.gpu)),
            temp_c: device.temperature(TemperatureSensor::Gpu).ok().map(f64::from),
            power_w: device.power_usage().ok().map(|mw| f64::from(mw) / 1000.0),
        })
    }

    fn release(&mut self) {
        // Dropping `Nvml` calls nvmlShutdown, which closes the device files.
        self.nvml = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Fake {
        reads: u32,
        releases: u32,
        open: bool,
    }

    impl DgpuBackend for Fake {
        fn read(&mut self) -> Option<DgpuReading> {
            self.reads += 1;
            self.open = true;
            Some(DgpuReading {
                util_pct: Some(42.0),
                ..DgpuReading::default()
            })
        }
        fn release(&mut self) {
            self.releases += 1;
            self.open = false;
        }
    }

    #[test]
    fn queries_only_when_active_and_allowed() {
        let mut m = DgpuMonitor::new(Fake::default(), DgpuPolicy::WhenActive);
        assert_eq!(m.sample(GpuState::Active).unwrap().util_pct, Some(42.0));
        assert!(m.backend().open);
        for state in [GpuState::Suspended, GpuState::Removed] {
            assert_eq!(m.sample(state), None);
            assert!(!m.backend().open, "handle must be released when {state}");
        }
        assert_eq!(m.backend().reads, 1);
    }

    #[test]
    fn off_policy_never_touches_the_gpu() {
        let mut m = DgpuMonitor::new(Fake::default(), DgpuPolicy::Off);
        for state in [GpuState::Active, GpuState::Suspended, GpuState::Removed] {
            assert_eq!(m.sample(state), None);
        }
        assert_eq!(m.backend().reads, 0);
        assert!(!m.backend().open);
    }

    #[test]
    fn switching_policy_off_releases_immediately() {
        let mut m = DgpuMonitor::new(Fake::default(), DgpuPolicy::WhenActive);
        m.sample(GpuState::Active);
        m.set_policy(DgpuPolicy::Off);
        assert_eq!(m.policy(), DgpuPolicy::Off);
        assert!(!m.backend().open);
        m.set_policy(DgpuPolicy::WhenActive);
        m.sample(GpuState::Active);
        m.release();
        assert!(!m.backend().open);
    }

    #[test]
    fn nvml_backend_debug_does_not_initialise() {
        let b = NvmlBackend::new("0000:01:00.0");
        assert!(format!("{b:?}").contains("initialised: false"));
    }
}
