//! The daemon's imperative core: applies profiles and curves to the EC with every safety rule, runs the 1 Hz
//! supervision tick, and owns the persisted settings. Synchronous: the D-Bus layer runs it on Tokio's
//! blocking pool under one mutex.
//!
//! | Rule | Here |
//! |---|---|
//! | S2/S3 | every table comes from `msi_core::table::generate` (or is rejected before any write) |
//! | Kernel order | `msi_core::table::write_plan`, one full table per write |
//! | S4 | [`Controller::tick`] feeds `ThermalSupervisor` (spike filter) and forces Cooler Boost |
//! | S5 | [`Controller::shutdown`]: factory tables then `auto` (+ panic hook and `msi-daemon --restore-factory` in `ExecStopPost`) |
//! | S7 | [`Controller::apply_profile`] reads back tables, fan mode and shift mode; mismatch → factory + `auto` + event |
//! | S8 | [`Controller::new`]: unknown firmware or missing curve files → read-only, no EC write at all |
//! | S9 | [`Controller::tick`] feeds one `StallDetector` per fan (0 % at a filtered ≥ 70 °C for 30 s) → factory + `auto`, read-only until restart |
//! | ADR-0010 | CPU fan driven live: factory thresholds kept, [`Controller::tick`] writes only `cpu/fan_curve_speeds` (from `msi_core::hybrid`) when the vector changes, read back (S7) |
//!
//! # Hybrid CPU control (ADR-0010)
//!
//! The EC ignores written CPU thresholds, so when the factory tables of the firmware are known the CPU table
//! keeps them and only its speeds are ours: [`msi_core::hybrid::hybrid_cpu_table`] of the live target that
//! [`msi_core::hybrid::CpuFanPilot`] computes from the filtered CPU temperature. Every tick updates the
//! filter and the target; the speeds are written only while our curve is in force (`applied`: profile
//! applied and verified, not handed back, not tripped, EC still in `advanced` with the factory thresholds)
//! and only when the vector changes, then read back (S7: mismatch → factory + `auto` + event). The drift
//! check compares the EC with the last vector written. Firmware without known factory tables: the CPU table
//! is generated like the GPU one (previous behaviour).
//!
//! # Fan profiles
//!
//! Each performance profile applies its fan profile ([`Settings::fan_profile`]): Silencieux, Équilibré, Turbo
//! and Personnalisé write our curves as above; **Auto** writes the factory tables of both fans (when known)
//! and fan mode `auto`, verifies them (S7) and stops the hybrid CPU loop. Unlike [`Controller::restore_factory`]
//! it is a chosen, persisted mode: re-applied at start, after resume and by the drift check. S4 keeps
//! supervising; S9 only watches our own tables (the firmware drives the fans under Auto).

use std::fmt;

use msi_core::battery::ChargeLimit;
use msi_core::curve::FanCurveSpec;
use msi_core::ec::{CONF_G2_5, FactoryTables, FanMode, ShiftMode};
use msi_core::fan_profile::FanProfile;
use msi_core::hybrid::{CpuFanPilot, TempFilter, hybrid_cpu_table};
use msi_core::profile::{GpuMode, Profile};
use msi_core::stall::{FanSample, STALL_DURATION_MS, STALL_TEMP, StallDetector};
use msi_core::supervisor::{BoostDecision, SensorReading, ThermalSupervisor};
use msi_core::table::{GeneratedCurve, RaisedPoint, RawTables, StepCurve, TableWrite, generate, write_plan};

use crate::ec::{EcBackend, Fan, restore_factory};
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
    /// S9: a fan stayed stopped above its spin temperature → factory tables + `auto`, read-only until restart
    /// (value: °C of that fan's sensor).
    FanStalled,
    /// Éco could not remove the dGPU (held, display connected…): kept in runtime suspend instead. Not a
    /// failure of `SetProfile` (detail: the reason, with the holders).
    GpuBusy,
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
            Self::FanStalled => "fan_stalled",
            Self::GpuBusy => "gpu_busy",
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
    /// Fan profile of the selected profile.
    pub fan_profile: FanProfile,
    /// Default fan profile of the selected profile.
    pub default_fan_profile: FanProfile,
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
    /// `Some(reason)` when read-only: S8 (unsupported EC) or S9 (fan stall, until restart).
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
    /// Fan rpm (hwmon), CPU then GPU.
    pub fan_rpm: [Option<u32>; 2],
    /// Cooler Boost as read.
    pub cooler_boost: Option<bool>,
    /// Live CPU fan target (%) in force in the EC (speed of CPU steps 0–3, ADR-0010); `None` when the CPU is
    /// not driven live (curve not applied, handed back, legacy firmware).
    pub cpu_fan_target: Option<u8>,
}

/// Curve of one fan as `FanCurve` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct FanCurveView {
    /// Curve of the fan profile in force (under Auto: the default fan profile's preset, not applied).
    pub spec: FanCurveSpec,
    /// Table in force: generated table, live CPU vector (ADR-0010), or factory table under Auto.
    pub table: RawTables,
    /// Steps raised by the safety floors (empty under Auto).
    pub raised: Vec<RaisedPoint>,
    /// `true` when the fan profile is `custom`.
    pub custom: bool,
}

/// The daemon state. See the module documentation.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags: curve applied, Auto applied, S4 forcing, owner's boost choice, control handed back"
)]
pub struct Controller {
    ec: Box<dyn EcBackend>,
    power: PowerSupply,
    gpu: GpuManager,
    telemetry: Option<Box<dyn Telemetry>>,
    store: StateStore,
    settings: Settings,
    read_only: Option<String>,
    tripped: Option<String>,
    factory: Option<FactoryTables>,
    handed_back: bool,
    fw_version: String,
    applied: bool,
    auto_applied: bool,
    stall: [StallDetector; 2],
    cpu_filter: TempFilter,
    cpu_pilot: CpuFanPilot,
    cpu_live: Option<GeneratedCurve>,
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
        let factory = CONF_G2_5.factory_tables(&fw_version);
        (
            Self {
                ec,
                power,
                gpu,
                telemetry,
                store,
                settings,
                read_only,
                tripped: None,
                factory,
                handed_back: false,
                fw_version,
                applied: false,
                auto_applied: false,
                stall: [StallDetector::new(); 2],
                cpu_filter: TempFilter::new(),
                cpu_pilot: CpuFanPilot::new(),
                cpu_live: None,
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

    /// `true` once the Auto fan profile is in force (factory tables and fan mode `auto` set and verified).
    #[must_use]
    pub fn is_auto_applied(&self) -> bool {
        self.auto_applied
    }

    /// Drains the pending safety events.
    pub fn take_events(&mut self) -> Vec<SafetyEvent> {
        std::mem::take(&mut self.events)
    }

    fn writable(&self) -> Result<(), ControlError> {
        match self.read_only.as_ref().or(self.tripped.as_ref()) {
            Some(reason) => Err(ControlError::ReadOnly(reason.clone())),
            None => Ok(()),
        }
    }

    /// `true` after [`Self::restore_factory`]: the firmware drives the fans until a profile or curve is applied
    /// again (resume, AC switch and drift check leave it alone).
    #[must_use]
    pub fn is_handed_back(&self) -> bool {
        self.handed_back
    }

    /// Factory tables (when known for this firmware) then fan mode `auto`. Never fails silently: errors are
    /// logged and returned.
    fn hand_back(&mut self) -> Result<(), String> {
        self.applied = false;
        self.auto_applied = false;
        self.cpu_live = None;
        for d in &mut self.stall {
            d.reset();
        }
        if self.factory.is_none() {
            tracing::warn!(fw = %self.fw_version, "factory tables unknown for this firmware: only fan mode auto restored");
        }
        restore_factory(self.ec.as_mut(), self.factory.as_ref()).map_err(|e| {
            tracing::error!(error = %e, "factory restore incomplete");
            e.to_string()
        })
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

    /// Puts the EC back on its factory tables in fan mode `auto` after a failure (S7) and records the event.
    fn fallback(&mut self, kind: EventKind, detail: String) -> ControlError {
        let _ = self.hand_back();
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
        self.handed_back = false;
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
        if self.handed_back {
            return Ok(());
        }
        self.apply_profile(self.settings.profile)
    }

    /// Hands the fans back to the firmware (D-Bus `RestoreFactory`): factory tables then fan mode `auto`. Stays
    /// so until a profile or curve is applied again. Allowed after S9 (it is what S9 did).
    ///
    /// # Errors
    /// [`ControlError::ReadOnly`] under S8 (nothing is ever written), [`ControlError::SafetyFallback`] if a step
    /// failed (fan mode `auto` is always attempted).
    pub fn restore_factory(&mut self) -> Result<(), ControlError> {
        if let Some(reason) = &self.read_only {
            return Err(ControlError::ReadOnly(reason.clone()));
        }
        self.handed_back = true;
        let res = self.hand_back();
        tracing::info!("control handed back to the EC (factory tables, fan mode auto)");
        res.map_err(ControlError::SafetyFallback)
    }

    /// Applies `profile`: curves (ordered writes) → fan mode `advanced` → shift mode → readback (S7) → dGPU.
    ///
    /// # Errors
    /// [`ControlError::ReadOnly`] (S8), [`ControlError::SafetyFallback`] if an EC write or readback failed (the
    /// EC is back on its factory tables in `auto`), [`ControlError::Gpu`] if a dGPU sysfs write fails (the EC
    /// part stays applied). A **refused** dGPU removal (Éco, dGPU held) is not an error: the dGPU is kept in
    /// runtime suspend and a `gpu_busy` event is recorded.
    pub fn apply_profile(&mut self, profile: Profile) -> Result<(), ControlError> {
        self.writable()?;
        if self.settings.fan_profile(profile) == FanProfile::Auto {
            return self.apply_auto(profile);
        }
        let target = profile.target();
        let mut curves = Vec::with_capacity(Fan::ALL.len());
        let mut cpu_live = None;
        for fan in Fan::ALL {
            let generated = self
                .curve_for(profile, fan)
                .map_err(|e| ControlError::InvalidArgument(format!("{fan} curve: {e}")))?;
            if fan == Fan::Cpu && self.hybrid_base().is_some() {
                cpu_live = Some(generated.clone());
            }
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
        let expected: Vec<(Fan, RawTables)> = curves.iter().map(|(f, c)| (*f, c.tables())).collect();
        self.verify(&expected, FanMode::Advanced, target.shift_mode)?;
        self.applied = true;
        self.auto_applied = false;
        self.cpu_live = cpu_live;
        for d in &mut self.stall {
            d.reset();
        }
        tracing::info!(profile = %profile, fan_profile = %self.settings.fan_profile(profile), "profile applied to the EC");
        self.apply_gpu_step(target.gpu)
    }

    /// Fan profile Auto of `profile`: factory tables (ordered, read back) → fan mode `auto` → shift mode →
    /// readback (S7) → dGPU. The hybrid CPU loop and S9 stay off (the firmware drives the fans); S4 stays on.
    fn apply_auto(&mut self, profile: Profile) -> Result<(), ControlError> {
        let target = profile.target();
        if let Err(e) = self.hand_back() {
            return Err(self.fallback(EventKind::WriteFailed, format!("factory tables: {e}")));
        }
        if let Err(e) = self.ec.set_shift_mode(target.shift_mode) {
            return Err(self.fallback(EventKind::WriteFailed, format!("shift mode: {e}")));
        }
        let expected = self.factory_expected();
        self.verify(&expected, FanMode::Auto, target.shift_mode)?;
        self.auto_applied = true;
        tracing::info!(profile = %profile, "profile applied, fans on the EC factory curve (fan profile auto)");
        self.apply_gpu_step(target.gpu)
    }

    /// Factory tables the EC must hold under Auto (empty when unknown for this firmware: only `auto` is set).
    fn factory_expected(&self) -> Vec<(Fan, RawTables)> {
        self.factory
            .map(|f| vec![(Fan::Cpu, f.cpu), (Fan::Gpu, f.gpu)])
            .unwrap_or_default()
    }

    /// dGPU polling policy, then the dGPU power step of a profile.
    fn apply_gpu_step(&mut self, gpu: GpuMode) -> Result<(), ControlError> {
        let policy = if gpu == GpuMode::Active {
            DgpuPolicy::WhenActive
        } else {
            DgpuPolicy::Off
        };
        if let Some(t) = self.telemetry.as_mut() {
            t.set_dgpu_policy(policy);
        }
        self.apply_gpu(gpu)
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
    fn verify(
        &mut self,
        tables: &[(Fan, RawTables)],
        mode: FanMode,
        shift: ShiftMode,
    ) -> Result<(), ControlError> {
        if let Some(detail) = self.drift(tables, mode, Some(shift)) {
            return Err(self.fallback(EventKind::ReadbackMismatch, detail));
        }
        Ok(())
    }

    /// First difference between the EC and what we applied, if any.
    fn drift(&self, tables: &[(Fan, RawTables)], mode: FanMode, shift: Option<ShiftMode>) -> Option<String> {
        for (fan, want) in tables {
            match self.ec.read_tables(*fan) {
                Ok(t) if t == *want => {}
                Ok(t) => return Some(format!("{fan} tables are {t:?}")),
                Err(e) => return Some(format!("{fan} tables: {e}")),
            }
        }
        match self.ec.fan_mode() {
            Ok(m) if m == mode => {}
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

    /// What the EC must hold now: under Auto the factory tables in `auto`; otherwise, in `advanced`, the
    /// generated GPU table and for the CPU the last live vector written (hybrid) or the generated table (legacy
    /// firmware).
    fn expected_tables(&self) -> Option<(Vec<(Fan, RawTables)>, FanMode)> {
        if self.auto_applied {
            return Some((self.factory_expected(), FanMode::Auto));
        }
        let tables = Fan::ALL
            .iter()
            .map(|&fan| match (&self.cpu_live, fan) {
                (Some(live), Fan::Cpu) => Some((fan, live.curve.tables())),
                (None, Fan::Cpu) if self.hybrid_base().is_some() => None,
                _ => generate(&self.settings.curve(self.settings.profile, fan))
                    .ok()
                    .map(|g| (fan, g.curve.tables())),
            })
            .collect::<Option<Vec<_>>>()?;
        Some((tables, FanMode::Advanced))
    }

    /// Factory CPU tables whose thresholds the hybrid CPU control keeps (ADR-0010); `None` when unknown for
    /// this firmware (the CPU table is then generated like the GPU one).
    fn hybrid_base(&self) -> Option<RawTables> {
        self.factory.map(|f| f.cpu)
    }

    /// Live CPU target for `spec`: the pilot's last output kept within the curve's range (the profile may
    /// have changed since), or the curve's start speed before any output.
    fn cpu_target(&self, spec: &FanCurveSpec) -> u8 {
        self.cpu_pilot.output().map_or_else(
            || CpuFanPilot::initial(spec),
            |o| o.clamp(spec.idle(), spec.params().max()),
        )
    }

    /// Table of `fan` in `profile`: hybrid live vector for the CPU (ADR-0010), generated table otherwise.
    fn curve_for(&self, profile: Profile, fan: Fan) -> Result<GeneratedCurve, msi_core::table::TableError> {
        let spec = self.settings.curve(profile, fan);
        match (fan, self.hybrid_base()) {
            (Fan::Cpu, Some(base)) => hybrid_cpu_table(&spec, &base, self.cpu_target(&spec)),
            _ => generate(&spec),
        }
    }

    /// dGPU step. Never kills anything: removal is refused while the dGPU is held (ADR-0008).
    fn apply_gpu(&mut self, mode: GpuMode) -> Result<(), ControlError> {
        let control = match mode {
            GpuMode::Removed => {
                if let Some(t) = self.telemetry.as_mut() {
                    t.release_dgpu();
                }
                match self.gpu.remove() {
                    Ok(()) => return Ok(()),
                    Err(e @ GpuError::Io { .. }) => return Err(ControlError::Gpu(e)),
                    Err(refused) => {
                        // Never kill anything (ADR-0008): fall back to runtime suspend (D3cold when idle).
                        self.event(
                            EventKind::GpuBusy,
                            0.0,
                            format!("{refused}; dGPU kept in runtime suspend"),
                        );
                        PowerControl::Auto
                    }
                }
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
        if enabled {
            self.handed_back = false;
        }
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

    /// Curve of `fan` in the selected profile, as `FanCurve` reports it. For the CPU under hybrid control
    /// (ADR-0010) the table is the **live vector**: factory thresholds, steps 0–3 at the current target, hot
    /// steps floored; the report lists the steps raised by the hot floors and S2. Under the Auto fan profile:
    /// the factory table (when known), nothing raised.
    ///
    /// # Errors
    /// [`ControlError::InvalidArgument`] if the stored spec does not generate (cannot happen: validated on load).
    pub fn fan_curve(&self, fan: Fan) -> Result<FanCurveView, ControlError> {
        let profile = self.settings.profile;
        let spec = self.settings.curve(profile, fan);
        let custom = self.settings.is_custom(profile, fan);
        if self.settings.fan_profile(profile) == FanProfile::Auto {
            let factory = self.factory.map(|f| if fan == Fan::Cpu { f.cpu } else { f.gpu });
            if let Some(table) = factory.or_else(|| self.ec.read_tables(fan).ok()) {
                return Ok(FanCurveView {
                    spec,
                    table,
                    raised: Vec::new(),
                    custom,
                });
            }
        }
        let generated = match (&self.cpu_live, fan) {
            (Some(live), Fan::Cpu) => live.clone(),
            _ => self
                .curve_for(profile, fan)
                .map_err(|e| ControlError::InvalidArgument(e.to_string()))?,
        };
        Ok(FanCurveView {
            spec,
            table: generated.curve.tables(),
            raised: generated.report.raised,
            custom,
        })
    }

    /// Custom curve of `fan` in the selected profile (the editor of the `custom` fan profile), whether or not it
    /// is in force, with its generated step table and S2 report (like `PreviewFanCurve`).
    ///
    /// # Errors
    /// [`ControlError::InvalidArgument`] if the stored spec does not generate (cannot happen: validated on load).
    pub fn custom_fan_curve(&self, fan: Fan) -> Result<(FanCurveSpec, GeneratedCurve), ControlError> {
        let spec = self.settings.custom_curve(self.settings.profile, fan);
        let generated = generate(&spec).map_err(|e| ControlError::InvalidArgument(e.to_string()))?;
        Ok((spec, generated))
    }

    /// Selects `fan_profile` for the selected performance profile (remembered for that profile), saves it and
    /// applies the profile.
    ///
    /// # Errors
    /// [`ControlError::ReadOnly`], or as [`Self::apply_profile`].
    pub fn set_fan_profile(&mut self, fan_profile: FanProfile) -> Result<(), ControlError> {
        self.writable()?;
        self.handed_back = false;
        let profile = self.settings.profile;
        self.settings.set_fan_profile(profile, fan_profile);
        self.save();
        self.apply_profile(profile)
    }

    /// Sets the custom curve of `fan` in the selected profile, switches that profile to the `custom` fan
    /// profile, saves and applies it. Returns the table now in force (for the CPU under hybrid control: the
    /// live vector, as [`Self::fan_curve`]). An invalid spec is rejected before anything is changed.
    ///
    /// # Errors
    /// [`ControlError::ReadOnly`], [`ControlError::InvalidArgument`], or as [`Self::apply_profile`].
    pub fn set_fan_curve(&mut self, fan: Fan, spec: FanCurveSpec) -> Result<GeneratedCurve, ControlError> {
        self.writable()?;
        let generated = generate(&spec).map_err(|e| ControlError::InvalidArgument(e.to_string()))?;
        self.handed_back = false;
        let profile = self.settings.profile;
        self.settings.curves.insert((profile, fan), spec);
        self.settings.set_fan_profile(profile, FanProfile::Custom);
        self.save();
        self.apply_profile(profile)?;
        if fan == Fan::Cpu && self.hybrid_base().is_some() {
            if let Some(live) = &self.cpu_live {
                return Ok(live.clone());
            }
        }
        Ok(generated)
    }

    /// Removes the custom curve of `fan` in the selected profile and applies the profile. When no custom curve
    /// is left and the profile was on `custom`, it goes back to its default fan profile (the preset).
    ///
    /// # Errors
    /// As [`Self::set_fan_curve`].
    pub fn reset_fan_curve(&mut self, fan: Fan) -> Result<(), ControlError> {
        self.writable()?;
        self.handed_back = false;
        let profile = self.settings.profile;
        self.settings.curves.remove(&(profile, fan));
        let any_left = Fan::ALL
            .iter()
            .any(|&f| self.settings.curves.contains_key(&(profile, f)));
        if !any_left && self.settings.fan_profile(profile) == FanProfile::Custom {
            self.settings
                .set_fan_profile(profile, profile.default_fan_profile());
        }
        self.save();
        self.apply_profile(profile)
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
            fan_profile: self.settings.fan_profile(self.settings.profile),
            default_fan_profile: self.settings.profile.default_fan_profile(),
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
            read_only: self.read_only.clone().or_else(|| self.tripped.clone()),
            fw_version: self.fw_version.clone(),
        }
    }

    /// One supervision step (~1 Hz): S4, S9, AC auto-switch, EC drift check, telemetry.
    pub fn tick(&mut self, now: Clock) -> Readings {
        self.ticks += 1;
        let sample = self.telemetry.as_mut().map(|t| t.sample(now));
        let cpu_temp = self.ec.temperature(Fan::Cpu);
        let gpu_temp = self.ec.temperature(Fan::Gpu);
        let now_ms = now.0 / 1_000_000;
        let filtered = self
            .cpu_filter
            .update(cpu_temp.as_ref().ok().copied().filter(|&t| t > 0));
        let spec = self.settings.curve(self.settings.profile, Fan::Cpu);
        let target = self.cpu_pilot.update(&spec, filtered, now_ms);
        let mut readings = Readings {
            fan_rpm: sample
                .as_ref()
                .map_or([None; 2], |s| [s.thermals.fan1_rpm, s.thermals.fan2_rpm]),
            sample,
            cpu_temp: cpu_temp.as_ref().ok().copied(),
            gpu_temp: gpu_temp.as_ref().ok().copied().filter(|&t| t > 0),
            fan_pct: [self.ec.fan_speed(Fan::Cpu).ok(), self.ec.fan_speed(Fan::Gpu).ok()],
            cooler_boost: self.ec.cooler_boost().ok(),
            cpu_fan_target: None,
        };
        if self.read_only.is_none() {
            // The EC reports 0 °C for the GPU when the dGPU is off: absent, not failed.
            let decision = self
                .supervisor
                .update(reading(&cpu_temp, false), reading(&gpu_temp, true));
            self.enforce_boost(decision, &readings);
            if self.applied {
                self.check_stall(now, &readings);
            }
            self.drive_cpu(&spec, target);
            self.auto_switch();
            if (self.applied || self.auto_applied) && self.ticks % DRIFT_CHECK_EVERY == 0 {
                self.check_drift();
            }
        }
        readings.cpu_fan_target = self
            .cpu_live
            .as_ref()
            .filter(|_| self.applied)
            .map(|g| g.curve.speeds()[0]);
        readings
    }

    /// ADR-0010: writes the CPU speed vector for `target` when it differs from the one in force. Only while our
    /// curve is applied and the EC is still in `advanced` with the factory thresholds (otherwise the drift
    /// check re-applies); only `cpu/fan_curve_speeds` is written, then read back (S7).
    fn drive_cpu(&mut self, spec: &FanCurveSpec, target: u8) {
        let (Some(base), Some(live)) = (self.hybrid_base(), self.cpu_live.as_ref()) else {
            return;
        };
        if !self.applied || self.handed_back || self.tripped.is_some() {
            return;
        }
        let next = match hybrid_cpu_table(spec, &base, target) {
            Ok(g) => g,
            Err(e) => {
                tracing::error!(error = %e, target, "hybrid CPU table rejected: not written");
                return;
            }
        };
        if next.curve == live.curve {
            return;
        }
        if !matches!(self.ec.fan_mode(), Ok(FanMode::Advanced)) {
            return;
        }
        let current = match self.ec.read_tables(Fan::Cpu) {
            Ok(t) => t,
            Err(e) => {
                let _ = self.fallback(EventKind::ReadbackMismatch, format!("cpu tables: {e}"));
                return;
            }
        };
        if current.up != base.up || current.down != base.down {
            return;
        }
        let plan = write_plan(&current, &next.curve);
        if plan.iter().any(|w| !matches!(w, TableWrite::Speeds(_))) {
            return;
        }
        for write in &plan {
            if let Err(e) = self.ec.write_table(Fan::Cpu, write) {
                let _ = self.fallback(EventKind::WriteFailed, format!("cpu speeds write: {e}"));
                return;
            }
        }
        match self.ec.read_tables(Fan::Cpu) {
            Ok(t) if t == next.curve.tables() => {
                tracing::debug!(target, speeds = ?t.speeds, "CPU fan speeds updated");
                self.cpu_live = Some(next);
            }
            Ok(t) => {
                let detail = format!("cpu tables read back {t:?}, expected {:?}", next.curve.tables());
                let _ = self.fallback(EventKind::ReadbackMismatch, detail);
            }
            Err(e) => {
                let _ = self.fallback(EventKind::ReadbackMismatch, format!("cpu readback: {e}"));
            }
        }
    }

    /// S9: a fan stopped at a filtered ≥ 70 °C for 30 s → factory tables + `auto`, read-only until restart.
    fn check_stall(&mut self, now: Clock, r: &Readings) {
        let now_ms = now.0 / 1_000_000;
        for (i, fan) in Fan::ALL.into_iter().enumerate() {
            let sample = FanSample {
                temp: [r.cpu_temp, r.gpu_temp][i],
                speed_pct: r.fan_pct[i],
                rpm: r.fan_rpm[i],
            };
            if self.stall[i].update(now_ms, sample) {
                self.trip_stall(fan, sample);
                return;
            }
        }
    }

    fn trip_stall(&mut self, fan: Fan, s: FanSample) {
        let temp = s.temp.unwrap_or(0);
        let rpm = s.rpm.map_or_else(String::new, |r| format!(", {r} rpm"));
        let detail = format!(
            "{fan} fan at {} %{rpm} for {} s at {temp} °C (≥ {STALL_TEMP} °C)",
            s.speed_pct.unwrap_or(0),
            STALL_DURATION_MS / 1000,
        );
        let restored = match self.hand_back() {
            Ok(()) => "factory curve restored, fan mode auto".to_owned(),
            Err(e) => format!("factory restore incomplete ({e})"),
        };
        self.tripped = Some(format!(
            "S9 fan stall: {detail}; {restored}. Check the fan, then restart msi-daemon to take control again"
        ));
        self.event(EventKind::FanStalled, f64::from(temp), detail);
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
        // Control handed back to the EC (RestoreFactory, S9): the firmware keeps it.
        if !self.settings.auto_profile_on_power || self.handed_back || self.tripped.is_some() {
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
        let Some((tables, mode)) = self.expected_tables() else {
            return;
        };
        let shift = self.settings.profile.target().shift_mode;
        if let Some(detail) = self.drift(&tables, mode, Some(shift)) {
            self.event(EventKind::EcReset, 0.0, format!("{detail}: re-applying"));
            if let Err(e) = self.reapply() {
                tracing::warn!(error = %e, "re-apply after EC reset incomplete");
            }
        }
    }

    /// S5: puts the EC back on its factory tables in fan mode `auto` (normal exit, signal). Cooler Boost is left
    /// on if S4 forces it. Nothing is written under S8.
    pub fn shutdown(&mut self) {
        if self.read_only.is_some() {
            return;
        }
        if self.hand_back().is_ok() {
            tracing::info!("EC back on its factory tables, fan mode auto");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
