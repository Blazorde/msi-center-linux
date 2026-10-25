//! The daemon's imperative core: applies profiles and curves to the EC with every safety rule, runs the 1 Hz
//! supervision tick, and owns the persisted settings. Synchronous: the D-Bus layer runs it on Tokio's
//! blocking pool under one mutex.
//!
//! | Rule | Here |
//! |---|---|
//! | S2/S3 | every table comes from `msi_core::table::generate` (or is rejected before any write) |
//! | Kernel order | `msi_core::table::write_plan`, one full table per write |
//! | S4 | [`Controller::tick`] feeds `ThermalSupervisor` (spike filter) and forces Cooler Boost |
//! | S5 | [`Controller::shutdown`] (+ panic hook and `ExecStopPost` in `main`/systemd) |
//! | S7 | [`Controller::apply_profile`] reads back tables, fan mode and shift mode; mismatch → `auto` + event |
//! | S8 | [`Controller::new`]: unknown firmware or missing curve files → read-only, no EC write at all |

use std::fmt;

use msi_core::battery::ChargeLimit;
use msi_core::curve::FanCurveSpec;
use msi_core::ec::{CONF_G2_5, FanMode, ShiftMode};
use msi_core::profile::{GpuMode, Profile};
use msi_core::supervisor::{BoostDecision, SensorReading, ThermalSupervisor};
use msi_core::table::{GeneratedCurve, StepCurve, generate, write_plan};

use crate::ec::{EcBackend, Fan};
use crate::gpu::{GpuError, GpuManager, GpuState, Holder, PowerControl};
use crate::power::PowerSupply;
use crate::state::{Settings, StateStore};
use crate::telemetry::dgpu::{DgpuBackend, DgpuPolicy};
use crate::telemetry::{Clock, Sample, Sampler};

/// Ticks between two checks that the EC still holds our tables (EC reset after resume, AC, …).
pub const DRIFT_CHECK_EVERY: u64 = 10;

/// Telemetry provider (the real [`Sampler`] or a fake in tests).
pub trait Telemetry: fmt::Debug + Send {
    /// One sample.
    fn sample(&mut self, now: Clock) -> Sample;
    /// dGPU polling policy for the active profile.
    fn set_dgpu_policy(&mut self, policy: DgpuPolicy);
    /// Closes every dGPU handle (before removing the dGPU).
    fn release_dgpu(&mut self);
}

impl<B: DgpuBackend> Telemetry for Sampler<B> {
    fn sample(&mut self, now: Clock) -> Sample {
        Sampler::sample(self, now)
    }
    fn set_dgpu_policy(&mut self, policy: DgpuPolicy) {
        Sampler::set_dgpu_policy(self, policy);
    }
    fn release_dgpu(&mut self) {
        Sampler::release_dgpu(self);
    }
}

/// Kind of a `SafetyEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// S4 forced Cooler Boost (value: hottest temperature, 0 if a sensor failed).
    BoostForced,
    /// S4 released Cooler Boost.
    BoostReleased,
    /// S7: the EC does not hold what was written → fan mode `auto`.
    ReadbackMismatch,
    /// An EC write failed → fan mode `auto`.
    WriteFailed,
    /// The EC lost our tables (resume, reset): re-applied.
    EcReset,
}

impl EventKind {
    /// Stable name on D-Bus.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BoostForced => "boost_forced",
            Self::BoostReleased => "boost_released",
            Self::ReadbackMismatch => "readback_mismatch",
            Self::WriteFailed => "write_failed",
            Self::EcReset => "ec_reset",
        }
    }
}

/// A safety-relevant event, emitted on D-Bus as `SafetyEvent`.
#[derive(Debug, Clone, PartialEq)]
pub struct SafetyEvent {
    /// Kind.
    pub kind: EventKind,
    /// Associated value (°C for boost events, 0 otherwise).
    pub value: f64,
    /// Human-readable detail (English).
    pub detail: String,
}

/// Why an operation failed. Mapped to `org.blazorde.MsiCenter1.Error.*`.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    /// S8: the EC is not supported, nothing is written.
    #[error("read-only: {0}")]
    ReadOnly(String),
    /// Invalid input (S3).
    #[error("{0}")]
    InvalidArgument(String),
    /// A non-EC hardware write failed (battery).
    #[error("{0}")]
    Hardware(String),
    /// An EC write or readback failed: the EC was put back in fan mode `auto`.
    #[error("EC put back in fan mode auto: {0}")]
    SafetyFallback(String),
    /// The dGPU step of a profile was refused (EC part applied).
    #[error("dGPU: {0}")]
    Gpu(#[from] GpuError),
}

/// Snapshot exposed as D-Bus properties.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// Selected profile.
    pub profile: Profile,
    /// AC auto-switch enabled.
    pub auto_profile_on_power: bool,
    /// AC state (`None` = unknown).
    pub on_ac: Option<bool>,
    /// EC fan mode as read (`auto`, `silent`, `advanced`, or `unknown`).
    pub fan_mode: String,
    /// Cooler Boost as read.
    pub cooler_boost: bool,
    /// Cooler Boost is forced by S4.
    pub boost_forced: bool,
    /// Charge limit (`None` = unsupported).
    pub charge_limit: Option<u8>,
    /// dGPU state.
    pub gpu_state: GpuState,
    /// `Some(reason)` when read-only (S8).
    pub read_only: Option<String>,
    /// EC firmware version (empty if unknown).
    pub fw_version: String,
}

/// What one tick measured (the D-Bus `Telemetry` payload).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Readings {
    /// System telemetry.
    pub sample: Option<Sample>,
    /// EC CPU temperature (°C).
    pub cpu_temp: Option<u8>,
    /// EC GPU temperature (°C).
    pub gpu_temp: Option<u8>,
    /// EC fan speeds (%), CPU then GPU.
    pub fan_pct: [Option<u8>; 2],
    /// Cooler Boost as read.
    pub cooler_boost: Option<bool>,
}

/// The daemon state. See the module documentation.
#[derive(Debug)]
pub struct Controller {
    ec: Box<dyn EcBackend>,
    power: PowerSupply,
    gpu: GpuManager,
    telemetry: Option<Box<dyn Telemetry>>,
    store: StateStore,
    settings: Settings,
    read_only: Option<String>,
    fw_version: String,
    applied: bool,
    supervisor: ThermalSupervisor,
    boost_forced: bool,
    user_boost: bool,
    last_ac: Option<bool>,
    ticks: u64,
    events: Vec<SafetyEvent>,
}

fn reading(temp: &Result<u8, crate::ec::EcError>, zero_is_absent: bool) -> SensorReading {
    match *temp {
        Ok(0) if zero_is_absent => SensorReading::Absent,
        Ok(t) => SensorReading::Celsius(f64::from(t)),
        Err(_) => SensorReading::Failed,
    }
}

impl Controller {
    /// Builds the controller, loads the settings and checks the firmware (S8). Returns the load warnings.
    #[must_use]
    pub fn new(
        ec: Box<dyn EcBackend>,
        power: PowerSupply,
        gpu: GpuManager,
        telemetry: Option<Box<dyn Telemetry>>,
        store: StateStore,
    ) -> (Self, Vec<String>) {
        let (settings, warnings) = store.load();
        let (fw_version, read_only) = match ec.fw_version() {
            Err(e) => (String::new(), Some(format!("msi-ec not available ({e})"))),
            Ok(fw) if !CONF_G2_5.supports_firmware(&fw) => {
                let reason = format!("EC firmware {fw} is not in the supported list");
                (fw, Some(reason))
            }
            Ok(fw) if !ec.curve_supported() => (
                fw,
                Some("msi-ec has no fan curve attributes (upstream module or firmware override)".into()),
            ),
            Ok(fw) => (fw, None),
        };
        let user_boost = read_only.is_none() && ec.cooler_boost().unwrap_or(false);
        (
            Self {
                ec,
                power,
                gpu,
                telemetry,
                store,
                settings,
                read_only,
                fw_version,
                applied: false,
                supervisor: ThermalSupervisor::new(),
                boost_forced: false,
                user_boost,
                last_ac: None,
                ticks: 0,
                events: Vec::new(),
            },
            warnings,
        )
    }

    /// Current settings.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// `true` once our curve is in force (fan mode `advanced` set and verified).
    #[must_use]
    pub fn is_applied(&self) -> bool {
        self.applied
    }

    /// Drains the pending safety events.
    pub fn take_events(&mut self) -> Vec<SafetyEvent> {
        std::mem::take(&mut self.events)
    }

    fn writable(&self) -> Result<(), ControlError> {
        match &self.read_only {
            Some(reason) => Err(ControlError::ReadOnly(reason.clone())),
            None => Ok(()),
        }
    }

    fn save(&self) {
        if let Err(e) = self.store.save(&self.settings) {
            tracing::warn!(error = %e, path = %self.store.path().display(), "cannot save settings");
        }
    }

    fn event(&mut self, kind: EventKind, value: f64, detail: String) {
        tracing::warn!(kind = kind.as_str(), value, detail = %detail, "safety event");
        self.events.push(SafetyEvent { kind, value, detail });
    }

    /// Puts the EC back in fan mode `auto` after a failure and records the event.
    fn fallback(&mut self, kind: EventKind, detail: String) -> ControlError {
        if let Err(e) = self.ec.set_fan_mode(FanMode::Auto) {
            tracing::error!(error = %e, "cannot restore fan mode auto");
        }
        self.applied = false;
        self.event(kind, 0.0, detail.clone());
        ControlError::SafetyFallback(detail)
    }

    /// Initial application at start: charge limit, then the profile for the current power source (if the AC
    /// switch is on) or the saved one.
    ///
    /// # Errors
    /// As [`Self::apply_profile`].
    pub fn start(&mut self) -> Result<(), ControlError> {
        self.last_ac = self.power.ac_online();
        self.writable()?;
        if let Some(limit) = self.settings.charge_limit {
            if let Err(e) = self.power.set_charge_limit(limit) {
                tracing::warn!(error = %e, "cannot apply the saved charge limit");
            }
        }
        if let (true, Some(ac)) = (self.settings.auto_profile_on_power, self.last_ac) {
            self.settings.profile = self.settings.profile_for_power(ac);
        }
        self.apply_profile(self.settings.profile)
    }

    /// Selects and applies a profile, then saves it.
    ///
    /// # Errors
    /// As [`Self::apply_profile`]. The profile is saved as selected even if the dGPU step is refused.
    pub fn set_profile(&mut self, profile: Profile) -> Result<(), ControlError> {
        self.writable()?;
        self.settings.profile = profile;
        self.save();
        self.apply_profile(profile)
    }

    /// Re-applies the selected profile (resume from suspend, EC reset). Idempotent: unchanged tables are not
    /// rewritten.
    ///
    /// # Errors
    /// As [`Self::apply_profile`].
    pub fn reapply(&mut self) -> Result<(), ControlError> {
        self.writable()?;
        self.apply_profile(self.settings.profile)
    }

    /// Applies `profile`: curves (ordered writes) → fan mode `advanced` → shift mode → readback (S7) → dGPU.
    ///
    /// # Errors
    /// [`ControlError::ReadOnly`] (S8), [`ControlError::SafetyFallback`] if an EC write or readback failed (the
    /// EC is back in `auto`), [`ControlError::Gpu`] if the dGPU step is refused (the EC part stays applied).
    pub fn apply_profile(&mut self, profile: Profile) -> Result<(), ControlError> {
        self.writable()?;
        let target = profile.target();
        let mut curves = Vec::with_capacity(Fan::ALL.len());
        for fan in Fan::ALL {
            let generated = generate(&self.settings.curve(profile, fan))
                .map_err(|e| ControlError::InvalidArgument(format!("{fan} curve: {e}")))?;
            curves.push((fan, generated.curve));
        }
        for (fan, curve) in &curves {
            self.write_curve(*fan, curve)?;
        }
        if let Err(e) = self.ec.set_fan_mode(FanMode::Advanced) {
            return Err(self.fallback(EventKind::WriteFailed, format!("fan mode: {e}")));
        }
        if let Err(e) = self.ec.set_shift_mode(target.shift_mode) {
            return Err(self.fallback(EventKind::WriteFailed, format!("shift mode: {e}")));
        }
        self.verify(&curves, target.shift_mode)?;
        self.applied = true;
        tracing::info!(profile = %profile, "profile applied to the EC");
        let policy = if target.gpu == GpuMode::Active {
            DgpuPolicy::WhenActive
        } else {
            DgpuPolicy::Off
        };
        if let Some(t) = self.telemetry.as_mut() {
            t.set_dgpu_policy(policy);
        }
        self.apply_gpu(target.gpu)
    }

    /// Writes one fan's tables in kernel-safe order, then reads them back (S7).
    fn write_curve(&mut self, fan: Fan, curve: &StepCurve) -> Result<(), ControlError> {
        let current = match self.ec.read_tables(fan) {
            Ok(t) => t,
            Err(e) => return Err(self.fallback(EventKind::WriteFailed, format!("{fan} tables: {e}"))),
        };
        for write in write_plan(&current, curve) {
            if let Err(e) = self.ec.write_table(fan, &write) {
                return Err(self.fallback(EventKind::WriteFailed, format!("{fan} table write: {e}")));
            }
        }
        match self.ec.read_tables(fan) {
            Ok(t) if t == curve.tables() => Ok(()),
            Ok(t) => Err(self.fallback(
                EventKind::ReadbackMismatch,
                format!("{fan} tables read back {t:?}, expected {:?}", curve.tables()),
            )),
            Err(e) => Err(self.fallback(EventKind::ReadbackMismatch, format!("{fan} readback: {e}"))),
        }
    }

    /// S7: everything written is still what the EC holds.
    fn verify(&mut self, curves: &[(Fan, StepCurve)], shift: ShiftMode) -> Result<(), ControlError> {
        if let Some(detail) = self.drift(curves, Some(shift)) {
            return Err(self.fallback(EventKind::ReadbackMismatch, detail));
        }
        Ok(())
    }

    /// First difference between the EC and what we applied, if any.
    fn drift(&self, curves: &[(Fan, StepCurve)], shift: Option<ShiftMode>) -> Option<String> {
        for (fan, curve) in curves {
            match self.ec.read_tables(*fan) {
                Ok(t) if t == curve.tables() => {}
                Ok(t) => return Some(format!("{fan} tables are {t:?}")),
                Err(e) => return Some(format!("{fan} tables: {e}")),
            }
        }
        match self.ec.fan_mode() {
            Ok(FanMode::Advanced) => {}
            Ok(m) => return Some(format!("fan mode is {}", m.as_str())),
            Err(e) => return Some(format!("fan mode: {e}")),
        }
        if let Some(shift) = shift {
            match self.ec.shift_mode() {
                Ok(m) if m == shift => {}
                Ok(m) => return Some(format!("shift mode is {}", m.as_str())),
                Err(e) => return Some(format!("shift mode: {e}")),
            }
        }
        None
    }

    fn expected_curves(&self) -> Option<Vec<(Fan, StepCurve)>> {
        Fan::ALL
            .iter()
            .map(|&fan| {
                generate(&self.settings.curve(self.settings.profile, fan))
                    .ok()
                    .map(|g| (fan, g.curve))
            })
            .collect()
    }

    /// dGPU step. Never kills anything: removal is refused while the dGPU is held (ADR-0008).
    fn apply_gpu(&mut self, mode: GpuMode) -> Result<(), ControlError> {
        let control = match mode {
            GpuMode::Removed => {
                if let Some(t) = self.telemetry.as_mut() {
                    t.release_dgpu();
                }
                return self.gpu.remove().map_err(ControlError::Gpu);
            }
            GpuMode::RuntimeSuspend => PowerControl::Auto,
            GpuMode::Active => PowerControl::On,
        };
        if self.gpu.discover().is_none() {
            self.gpu.rescan()?;
        }
        if let Some(dgpu) = self.gpu.discover() {
            self.gpu.set_runtime_pm(&dgpu, control)?;
        }
        Ok(())
    }

    /// Enables or disables the AC auto-switch; enabling applies the profile of the current source.
    ///
    /// # Errors
    /// As [`Self::apply_profile`] when enabling switches the profile.
    pub fn set_auto_profile_on_power(&mut self, enabled: bool) -> Result<(), ControlError> {
        self.writable()?;
        self.settings.auto_profile_on_power = enabled;
        self.save();
        match (enabled, self.power.ac_online()) {
            (true, Some(ac)) => {
                let profile = self.settings.profile_for_power(ac);
                if profile == self.settings.profile && self.applied {
                    Ok(())
                } else {
                    self.set_profile(profile)
                }
            }
            _ => Ok(()),
        }
    }

    /// Curve spec, generated table and report of `fan` in the selected profile, and whether it is custom.
    ///
    /// # Errors
    /// [`ControlError::InvalidArgument`] if the stored spec does not generate (cannot happen: validated on load).
    pub fn fan_curve(&self, fan: Fan) -> Result<(FanCurveSpec, GeneratedCurve, bool), ControlError> {
        let profile = self.settings.profile;
        let spec = self.settings.curve(profile, fan);
        let generated = generate(&spec).map_err(|e| ControlError::InvalidArgument(e.to_string()))?;
        Ok((spec, generated, self.settings.is_custom(profile, fan)))
    }

    /// Sets a custom curve for `fan` in the selected profile, saves it and applies the profile.
    ///
    /// # Errors
    /// [`ControlError::ReadOnly`], or as [`Self::apply_profile`].
    pub fn set_fan_curve(&mut self, fan: Fan, spec: FanCurveSpec) -> Result<GeneratedCurve, ControlError> {
        self.writable()?;
        let generated = generate(&spec).map_err(|e| ControlError::InvalidArgument(e.to_string()))?;
        self.settings.curves.insert((self.settings.profile, fan), spec);
        self.save();
        self.apply_profile(self.settings.profile)?;
        Ok(generated)
    }

    /// Removes the custom curve of `fan` in the selected profile (back to the preset) and applies it.
    ///
    /// # Errors
    /// As [`Self::set_fan_curve`].
    pub fn reset_fan_curve(&mut self, fan: Fan) -> Result<(), ControlError> {
        self.writable()?;
        self.settings.curves.remove(&(self.settings.profile, fan));
        self.save();
        self.apply_profile(self.settings.profile)
    }

    /// Sets the owner's Cooler Boost preference. Turning it off is refused while S4 forces it.
    ///
    /// # Errors
    /// [`ControlError::ReadOnly`], [`ControlError::InvalidArgument`] while forced, or
    /// [`ControlError::SafetyFallback`] if the write fails.
    pub fn set_cooler_boost(&mut self, on: bool) -> Result<(), ControlError> {
        self.writable()?;
        if !on && self.boost_forced {
            return Err(ControlError::InvalidArgument(
                "Cooler Boost is forced by the thermal supervisor (S4) until CPU and GPU are below 85 °C"
                    .into(),
            ));
        }
        self.user_boost = on;
        if let Err(e) = self.ec.set_cooler_boost(on) {
            return Err(self.fallback(EventKind::WriteFailed, format!("cooler boost: {e}")));
        }
        Ok(())
    }

    /// Sets and saves the battery charge limit (50–100 %).
    ///
    /// # Errors
    /// [`ControlError::InvalidArgument`] out of range (S3), [`ControlError::ReadOnly`], or
    /// [`ControlError::Hardware`] if the write or its readback fails.
    pub fn set_charge_limit(&mut self, value: u8) -> Result<(), ControlError> {
        let limit = ChargeLimit::new(value).map_err(|e| ControlError::InvalidArgument(e.to_string()))?;
        self.writable()?;
        self.power
            .set_charge_limit(limit)
            .map_err(|e| ControlError::Hardware(format!("charge limit: {e}")))?;
        self.settings.charge_limit = Some(limit);
        self.save();
        Ok(())
    }

    /// Processes holding the dGPU (empty if it is not on the bus).
    #[must_use]
    pub fn gpu_holders(&self) -> Vec<Holder> {
        self.gpu
            .discover()
            .map(|d| self.gpu.holders(&d).holders)
            .unwrap_or_default()
    }

    /// Property snapshot.
    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            profile: self.settings.profile,
            auto_profile_on_power: self.settings.auto_profile_on_power,
            on_ac: self.last_ac.or_else(|| self.power.ac_online()),
            fan_mode: self
                .ec
                .fan_mode()
                .map_or_else(|_| "unknown".to_owned(), |m| m.as_str().to_owned()),
            cooler_boost: self.ec.cooler_boost().unwrap_or(false),
            boost_forced: self.boost_forced,
            charge_limit: self.power.charge_limit(),
            gpu_state: self.gpu.state(),
            read_only: self.read_only.clone(),
            fw_version: self.fw_version.clone(),
        }
    }

    /// One supervision step (~1 Hz): S4, AC auto-switch, EC drift check, telemetry.
    pub fn tick(&mut self, now: Clock) -> Readings {
        self.ticks += 1;
        let cpu_temp = self.ec.temperature(Fan::Cpu);
        let gpu_temp = self.ec.temperature(Fan::Gpu);
        let readings = Readings {
            sample: None,
            cpu_temp: cpu_temp.as_ref().ok().copied(),
            gpu_temp: gpu_temp.as_ref().ok().copied().filter(|&t| t > 0),
            fan_pct: [self.ec.fan_speed(Fan::Cpu).ok(), self.ec.fan_speed(Fan::Gpu).ok()],
            cooler_boost: self.ec.cooler_boost().ok(),
        };
        if self.read_only.is_none() {
            // The EC reports 0 °C for the GPU when the dGPU is off: absent, not failed.
            let decision = self
                .supervisor
                .update(reading(&cpu_temp, false), reading(&gpu_temp, true));
            self.enforce_boost(decision, &readings);
            self.auto_switch();
            if self.applied && self.ticks % DRIFT_CHECK_EVERY == 0 {
                self.check_drift();
            }
        }
        let sample = self.telemetry.as_mut().map(|t| t.sample(now));
        Readings { sample, ..readings }
    }

    fn enforce_boost(&mut self, decision: BoostDecision, r: &Readings) {
        let hottest = f64::from(r.cpu_temp.unwrap_or(0).max(r.gpu_temp.unwrap_or(0)));
        match (decision, self.boost_forced) {
            (BoostDecision::Forced, false) => {
                if let Err(e) = self.ec.set_cooler_boost(true) {
                    tracing::error!(error = %e, "cannot force Cooler Boost");
                    return;
                }
                self.boost_forced = true;
                let detail = match (r.cpu_temp, r.gpu_temp) {
                    (None, _) => "CPU temperature unreadable".to_owned(),
                    (Some(c), g) => format!(
                        "CPU {c} °C, GPU {}",
                        g.map_or_else(|| "absent".to_owned(), |g| format!("{g} °C"))
                    ),
                };
                self.event(EventKind::BoostForced, hottest, detail);
            }
            (BoostDecision::Forced, true) => {
                if r.cooler_boost == Some(false) {
                    if let Err(e) = self.ec.set_cooler_boost(true) {
                        tracing::error!(error = %e, "cannot re-force Cooler Boost");
                    }
                }
            }
            (BoostDecision::Released, true) => {
                if let Err(e) = self.ec.set_cooler_boost(self.user_boost) {
                    tracing::error!(error = %e, "cannot release Cooler Boost");
                    return;
                }
                self.boost_forced = false;
                self.event(EventKind::BoostReleased, hottest, "below 85 °C".into());
            }
            (BoostDecision::Released, false) => {}
        }
    }

    fn auto_switch(&mut self) {
        let ac = self.power.ac_online();
        let previous = std::mem::replace(&mut self.last_ac, ac);
        if !self.settings.auto_profile_on_power {
            return;
        }
        if let (Some(before), Some(now)) = (previous, ac) {
            if before == now {
                return;
            }
            let profile = self.settings.profile_for_power(now);
            tracing::info!(on_ac = now, profile = %profile, "power source changed");
            if let Err(e) = self.set_profile(profile) {
                tracing::warn!(error = %e, "automatic profile switch incomplete");
            }
        }
    }

    fn check_drift(&mut self) {
        let Some(curves) = self.expected_curves() else {
            return;
        };
        let shift = self.settings.profile.target().shift_mode;
        if let Some(detail) = self.drift(&curves, Some(shift)) {
            self.event(EventKind::EcReset, 0.0, format!("{detail}: re-applying"));
            if let Err(e) = self.reapply() {
                tracing::warn!(error = %e, "re-apply after EC reset incomplete");
            }
        }
    }

    /// S5: puts the EC back in fan mode `auto` (normal exit, signal). Cooler Boost is left on if S4 forces it.
    pub fn shutdown(&mut self) {
        if self.read_only.is_some() {
            return;
        }
        match self.ec.set_fan_mode(FanMode::Auto) {
            Ok(()) => tracing::info!("EC fan mode restored to auto"),
            Err(e) => tracing::error!(error = %e, "cannot restore fan mode auto"),
        }
        self.applied = false;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
