//! D-Bus interface `org.blazorde.MsiCenter1` (frozen, additive changes only, see `docs/architecture/dbus-api.md`).
//!
//! Every call runs the (blocking) sysfs work on Tokio's blocking pool under one mutex: calls and the 1 Hz
//! tick are serialised. Properties are served from a snapshot refreshed after each call and each tick, so a
//! property read never waits for the hardware. Mutating methods check polkit first.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use msi_core::curve::{FanCurveParams, FanCurveSpec};
use msi_core::fan_profile::{FanProfile, ParseFanProfileError};
use msi_core::profile::Profile;
use msi_core::staged::{FloorRaise, Stage, StagedCurve};
use msi_core::table::{GeneratedCurve, RaisedPoint, RawTables, generate};
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{DBusError, OwnedVariant, Str, interface};

use crate::controller::{ControlError, Controller, CpuStagesView, Readings, SafetyEvent, Status};
use crate::ec::Fan;
use crate::gpu::GpuError;
use crate::polkit::{Action, Authorizer};

/// Well-known bus name (system bus).
pub const BUS_NAME: &str = "org.blazorde.MsiCenter1";
/// Object path.
pub const OBJECT_PATH: &str = "/org/blazorde/MsiCenter";

/// D-Bus errors, named `org.blazorde.MsiCenter1.Error.<Variant>`.
#[derive(Debug, DBusError)]
#[zbus(prefix = "org.blazorde.MsiCenter1.Error")]
pub enum MsiError {
    /// Transport error.
    #[zbus(error)]
    ZBus(zbus::Error),
    /// polkit denied the call.
    NotAuthorized(String),
    /// Bad argument (S3): unknown profile or fan, out-of-range value, invalid curve.
    InvalidArgument(String),
    /// S8: unsupported EC, nothing is written.
    ReadOnly(String),
    /// A hardware write failed (battery).
    Hardware(String),
    /// An EC write or readback failed: the EC was put back in fan mode `auto` (S7).
    SafetyFallback(String),
    /// A dGPU sysfs write failed during a profile change. Kept for compatibility: a **refused** removal (dGPU
    /// held) no longer fails `SetProfile` (the dGPU is kept in runtime suspend, `SafetyEvent("gpu_busy")`).
    GpuBusy(String),
    /// Internal failure (details in the journal).
    Internal(String),
}

impl From<ControlError> for MsiError {
    fn from(e: ControlError) -> Self {
        let msg = e.to_string();
        match e {
            ControlError::ReadOnly(_) => Self::ReadOnly(msg),
            ControlError::InvalidArgument(_) => Self::InvalidArgument(msg),
            ControlError::Hardware(_) | ControlError::Gpu(GpuError::Io { .. }) => Self::Hardware(msg),
            ControlError::SafetyFallback(_) => Self::SafetyFallback(msg),
            ControlError::Gpu(_) => Self::GpuBusy(msg),
        }
    }
}

/// Curve specification on the wire: `(y idle, y t0, y t1, y min, y max, d k, ay up, y hysteresis)`.
pub type SpecWire = (u8, u8, u8, u8, u8, f64, Vec<u8>, u8);
/// EC table on the wire: `(ay speeds, ay up, ay down)`.
pub type TableWire = (Vec<u8>, Vec<u8>, Vec<u8>);
/// Raised step on the wire: `(y step, y entry_temp, y curve_speed, y applied_speed, y floor_temp, y floor_speed)`.
pub type RaisedWire = (u8, u8, u8, u8, u8, u8);
/// Stage of a staged CPU curve on the wire: `(y up, y down, y speed)`.
pub type StageWire = (u8, u8, u8);
/// S2 floor that takes over from the stages on the wire: `(y floor_temp, y floor_speed, y curve_speed)`.
pub type FloorWire = (u8, u8, u8);
/// `CpuStages` reply: `(y idle, a(yyy) stages, a(yyy) floors, b saved, b active, b supported)`.
pub type CpuStagesWire = (u8, Vec<StageWire>, Vec<FloorWire>, bool, bool, bool);

/// Converts and validates a staged curve (S3: rejected, never corrected).
///
/// # Errors
/// [`MsiError::InvalidArgument`] with the violated rule.
pub fn staged_from_wire(idle: u8, stages: &[StageWire]) -> Result<StagedCurve, MsiError> {
    let stages = stages
        .iter()
        .map(|&(up, down, speed)| Stage { up, down, speed })
        .collect();
    StagedCurve::new(idle, stages).map_err(|e| MsiError::InvalidArgument(e.to_string()))
}

/// Wire form of the stages of a staged curve.
#[must_use]
pub fn stages_to_wire(curve: &StagedCurve) -> Vec<StageWire> {
    curve.stages().iter().map(|s| (s.up, s.down, s.speed)).collect()
}

/// Wire form of the S2 floors that take over from the stages.
#[must_use]
pub fn floors_to_wire(floors: &[FloorRaise]) -> Vec<FloorWire> {
    floors
        .iter()
        .map(|f| (f.floor.temp, f.floor.speed, f.curve_speed))
        .collect()
}

/// Wire form of a `CpuStages` view.
#[must_use]
pub fn cpu_stages_to_wire(v: &CpuStagesView) -> CpuStagesWire {
    (
        v.curve.idle(),
        stages_to_wire(&v.curve),
        floors_to_wire(&v.floors),
        v.saved,
        v.active,
        v.supported,
    )
}

/// Converts and validates a wire spec (S3: rejected, never corrected).
///
/// # Errors
/// [`MsiError::InvalidArgument`] with the violated rule.
pub fn spec_from_wire(w: &SpecWire) -> Result<FanCurveSpec, MsiError> {
    let (idle, t0, t1, min, max, k, ref up, hysteresis) = *w;
    let invalid = |e: String| MsiError::InvalidArgument(e);
    let params = FanCurveParams::new(t0, t1, min, max, k).map_err(|e| invalid(e.to_string()))?;
    let up: [u8; 6] = up
        .as_slice()
        .try_into()
        .map_err(|_| invalid(format!("{} up thresholds, expected 6", up.len())))?;
    FanCurveSpec::new(params, idle, up, hysteresis).map_err(|e| invalid(e.to_string()))
}

/// Wire form of a spec.
#[must_use]
pub fn spec_to_wire(s: &FanCurveSpec) -> SpecWire {
    let p = s.params();
    (
        s.idle(),
        p.t0(),
        p.t1(),
        p.min(),
        p.max(),
        p.k(),
        s.up().to_vec(),
        s.hysteresis(),
    )
}

/// Wire form of a generated table and its safety report.
#[must_use]
pub fn generated_to_wire(g: &GeneratedCurve) -> (TableWire, Vec<RaisedWire>) {
    tables_to_wire(&g.curve.tables(), &g.report.raised)
}

/// Wire form of EC tables and the steps the safety floors raised.
#[must_use]
pub fn tables_to_wire(t: &RawTables, raised: &[RaisedPoint]) -> (TableWire, Vec<RaisedWire>) {
    let table = (t.speeds.to_vec(), t.up.to_vec(), t.down.to_vec());
    let raised = raised
        .iter()
        .map(|p| {
            (
                u8::try_from(p.index).unwrap_or(u8::MAX),
                p.temp,
                p.curve_speed,
                p.applied_speed,
                p.floor.temp,
                p.floor.speed,
            )
        })
        .collect();
    (table, raised)
}

fn parse_fan(fan: &str) -> Result<Fan, MsiError> {
    Fan::parse(fan)
        .ok_or_else(|| MsiError::InvalidArgument(format!("unknown fan {fan:?}, expected cpu or gpu")))
}

/// `Telemetry` payload. Keys are omitted when the value is unknown.
#[must_use]
pub fn telemetry_map(r: &Readings) -> HashMap<String, OwnedVariant> {
    let mut m: HashMap<String, OwnedVariant> = HashMap::new();
    let mut put = |k: &str, v: OwnedVariant| {
        m.insert(k.to_owned(), v);
    };
    if let Some(s) = &r.sample {
        for (key, value) in [
            ("cpu_pct", s.cpu_pct),
            ("ram_pct", s.ram_pct),
            ("igpu_pct", s.igpu_pct),
            ("dgpu_pct", s.dgpu_pct),
            ("npu_pct", s.npu_pct),
            ("power_w", s.dgpu_power_w),
        ] {
            if let Some(v) = value {
                put(key, v.into());
            }
        }
        if let Some(rpm) = s.thermals.fan1_rpm {
            put("fan1_rpm", rpm.into());
        }
        if let Some(rpm) = s.thermals.fan2_rpm {
            put("fan2_rpm", rpm.into());
        }
        put("dgpu_state", Str::from(s.dgpu_state.as_str()).into());
        if r.cpu_temp.is_none() {
            if let Some(t) = s.thermals.cpu_temp_c {
                put("cpu_temp", t.into());
            }
        }
        if r.gpu_temp.is_none() {
            if let Some(t) = s.thermals.gpu_temp_c {
                put("gpu_temp", t.into());
            }
        }
    }
    // EC temperatures (what the fans follow) win over hwmon/NVML ones.
    if let Some(t) = r.cpu_temp {
        put("cpu_temp", f64::from(t).into());
    }
    if let Some(t) = r.gpu_temp {
        put("gpu_temp", f64::from(t).into());
    }
    if let Some(p) = r.fan_pct[0] {
        put("fan1_pct", p.into());
    }
    if let Some(p) = r.fan_pct[1] {
        put("fan2_pct", p.into());
    }
    if let Some(b) = r.cooler_boost {
        put("cooler_boost", b.into());
    }
    // ADR-0010: live CPU fan target in force (speed of CPU steps 0–3), only while the CPU is driven live.
    if let Some(p) = r.cpu_fan_target {
        put("cpu_fan_target_pct", p.into());
    }
    m
}

/// Controller shared between the D-Bus interface and the tick loop.
pub type SharedController = Arc<Mutex<Controller>>;

fn lock(c: &Mutex<Controller>) -> MutexGuard<'_, Controller> {
    c.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The `org.blazorde.MsiCenter1` object.
#[derive(Debug)]
pub struct MsiCenter {
    ctl: SharedController,
    status: Mutex<Status>,
    auth: Arc<dyn Authorizer>,
}

impl MsiCenter {
    /// Interface over `ctl`, authorizing with `auth`.
    #[must_use]
    pub fn new(ctl: SharedController, auth: Arc<dyn Authorizer>) -> Self {
        let status = lock(&ctl).status();
        Self {
            ctl,
            status: Mutex::new(status),
            auth,
        }
    }

    fn snapshot(&self) -> Status {
        self.status.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Runs `f` on the blocking pool with the controller locked; returns its result, the pending events and
    /// the new status.
    async fn run<T, F>(&self, f: F) -> Result<(T, Vec<SafetyEvent>, Status), MsiError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Controller) -> T + Send + 'static,
    {
        let ctl = Arc::clone(&self.ctl);
        tokio::task::spawn_blocking(move || {
            let mut c = lock(&ctl);
            let out = f(&mut c);
            (out, c.take_events(), c.status())
        })
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "blocking task failed");
            MsiError::Internal("task failed".into())
        })
    }

    async fn authorize(&self, hdr: &Header<'_>, action: Action) -> Result<(), MsiError> {
        let sender = hdr.sender().map(zbus::names::UniqueName::as_str);
        match self.auth.check(sender, action).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(MsiError::NotAuthorized(format!(
                "not authorized: {}",
                action.id()
            ))),
            Err(e) => {
                tracing::error!(error = %e, "authorization check failed");
                Err(MsiError::NotAuthorized(format!(
                    "authorization check failed: {e}"
                )))
            }
        }
    }

    /// Emits `SafetyEvent`s, stores the new snapshot and emits `PropertiesChanged` for what changed.
    pub async fn publish(&self, emitter: &SignalEmitter<'_>, events: Vec<SafetyEvent>, status: Status) {
        for e in events {
            if let Err(err) = Self::safety_event(emitter, e.kind.as_str(), e.value, &e.detail).await {
                tracing::warn!(error = %err, "cannot emit SafetyEvent");
            }
        }
        let old = std::mem::replace(
            &mut *self.status.lock().unwrap_or_else(PoisonError::into_inner),
            status.clone(),
        );
        self.emit_changes(emitter, &old, &status).await;
    }

    async fn emit_changes(&self, e: &SignalEmitter<'_>, old: &Status, new: &Status) {
        let mut res = Vec::new();
        if old.profile != new.profile {
            res.push(self.profile_changed(e).await);
        }
        if old.fan_profile != new.fan_profile {
            res.push(self.fan_profile_changed(e).await);
        }
        if old.default_fan_profile != new.default_fan_profile {
            res.push(self.default_fan_profile_changed(e).await);
        }
        if old.auto_profile_on_power != new.auto_profile_on_power {
            res.push(self.auto_profile_on_power_changed(e).await);
        }
        if old.on_ac != new.on_ac {
            res.push(self.on_ac_power_changed(e).await);
        }
        if old.fan_mode != new.fan_mode {
            res.push(self.fan_mode_changed(e).await);
        }
        if old.cooler_boost != new.cooler_boost {
            res.push(self.cooler_boost_changed(e).await);
        }
        if old.boost_forced != new.boost_forced {
            res.push(self.boost_forced_changed(e).await);
        }
        if old.charge_limit != new.charge_limit {
            res.push(self.charge_limit_changed(e).await);
        }
        if old.gpu_state != new.gpu_state {
            res.push(self.gpu_state_changed(e).await);
        }
        if old.read_only != new.read_only {
            res.push(self.read_only_changed(e).await);
            res.push(self.read_only_reason_changed(e).await);
        }
        for r in res {
            if let Err(err) = r {
                tracing::warn!(error = %err, "cannot emit PropertiesChanged");
            }
        }
    }

    /// One supervision step from the tick loop: runs [`Controller::tick`], emits `Telemetry`, events and
    /// property changes.
    ///
    /// # Errors
    /// [`MsiError::Internal`] if the blocking task failed (the caller must then stop pinging the watchdog).
    pub async fn tick(
        &self,
        emitter: &SignalEmitter<'_>,
        now: crate::telemetry::Clock,
    ) -> Result<(), MsiError> {
        let (readings, events, status) = self.run(move |c| c.tick(now)).await?;
        if let Err(e) = Self::telemetry(emitter, telemetry_map(&readings)).await {
            tracing::debug!(error = %e, "cannot emit Telemetry");
        }
        self.publish(emitter, events, status).await;
        Ok(())
    }

    /// Re-applies the selected profile (resume from suspend).
    pub async fn resume(&self, emitter: &SignalEmitter<'_>) {
        match self.run(Controller::reapply).await {
            Ok((res, events, status)) => {
                if let Err(e) = res {
                    tracing::warn!(error = %e, "re-apply after resume incomplete");
                }
                self.publish(emitter, events, status).await;
            }
            Err(e) => tracing::error!(error = %e, "re-apply after resume failed"),
        }
    }
}

#[interface(name = "org.blazorde.MsiCenter1")]
impl MsiCenter {
    /// Selects and applies a profile (`eco`, `balanced`, `gaming`). The EC part (curves, fan mode, shift mode)
    /// is applied before the dGPU step. If Éco cannot remove the dGPU (held by processes…) the call still
    /// succeeds: the dGPU is kept in runtime suspend and `SafetyEvent("gpu_busy", 0, reason)` is emitted.
    async fn set_profile(
        &self,
        profile: String,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), MsiError> {
        let p: Profile = profile
            .parse()
            .map_err(|e: msi_core::profile::ParseProfileError| MsiError::InvalidArgument(e.to_string()))?;
        self.authorize(&hdr, Action::SetProfile).await?;
        let (res, events, status) = self.run(move |c| c.set_profile(p)).await?;
        self.publish(&emitter, events, status).await;
        res.map_err(Into::into)
    }

    /// Enables or disables the automatic profile on AC plug/unplug (AC: `balanced`, battery: `eco` by default).
    async fn set_auto_profile_on_power(
        &self,
        enabled: bool,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), MsiError> {
        self.authorize(&hdr, Action::SetProfile).await?;
        let (res, events, status) = self.run(move |c| c.set_auto_profile_on_power(enabled)).await?;
        self.publish(&emitter, events, status).await;
        res.map_err(Into::into)
    }

    /// Selects the fan profile (`silent`, `auto`, `balanced`, `turbo`, `custom`) of the selected profile,
    /// remembered for that profile, and applies it. `auto` = EC factory curve in fan mode `auto`.
    async fn set_fan_profile(
        &self,
        fan_profile: String,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), MsiError> {
        let fp: FanProfile = fan_profile
            .parse()
            .map_err(|e: ParseFanProfileError| MsiError::InvalidArgument(e.to_string()))?;
        self.authorize(&hdr, Action::SetFanCurve).await?;
        let (res, events, status) = self.run(move |c| c.set_fan_profile(fp)).await?;
        self.publish(&emitter, events, status).await;
        res.map_err(Into::into)
    }

    /// Curve of `fan` (`cpu`, `gpu`) in the selected profile: spec, resulting EC table, steps raised by the
    /// safety floors, and whether the fan profile is `custom`. For the CPU under hybrid control (ADR-0010) the
    /// table is the live vector in force: factory thresholds, steps 0–3 at the current target, hot steps
    /// floored. Under the `auto` fan profile: the factory table, nothing raised.
    #[zbus(out_args("spec", "table", "raised", "custom"))]
    async fn fan_curve(&self, fan: String) -> Result<(SpecWire, TableWire, Vec<RaisedWire>, bool), MsiError> {
        let fan = parse_fan(&fan)?;
        let (res, _, _) = self.run(move |c| c.fan_curve(fan)).await?;
        let view = res?;
        let (table, raised) = tables_to_wire(&view.table, &view.raised);
        Ok((spec_to_wire(&view.spec), table, raised, view.custom))
    }

    /// Custom curve of `fan` in the selected profile (the `custom` fan profile's curve, in force or not): the
    /// saved spec, else the preset of the profile's default fan profile; with its step table and S2 report.
    #[zbus(out_args("spec", "table", "raised"))]
    async fn custom_fan_curve(
        &self,
        fan: String,
    ) -> Result<(SpecWire, TableWire, Vec<RaisedWire>), MsiError> {
        let fan = parse_fan(&fan)?;
        let (res, _, _) = self.run(move |c| c.custom_fan_curve(fan)).await?;
        let (spec, generated) = res?;
        let (table, raised) = generated_to_wire(&generated);
        Ok((spec_to_wire(&spec), table, raised))
    }

    /// Sets the custom curve of `fan` in the selected profile and switches it to the `custom` fan profile;
    /// saves and applies it. Returns the EC table and the steps raised by the S2 floors.
    #[zbus(out_args("table", "raised"))]
    async fn set_fan_curve(
        &self,
        fan: String,
        spec: SpecWire,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(TableWire, Vec<RaisedWire>), MsiError> {
        let fan = parse_fan(&fan)?;
        let spec = spec_from_wire(&spec)?;
        self.authorize(&hdr, Action::SetFanCurve).await?;
        let (res, events, status) = self.run(move |c| c.set_fan_curve(fan, spec)).await?;
        self.publish(&emitter, events, status).await;
        Ok(generated_to_wire(&res?))
    }

    /// Drops the custom curve of `fan` in the selected profile and applies it; with no custom curve left, the
    /// profile goes back to its default fan profile.
    async fn reset_fan_curve(
        &self,
        fan: String,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), MsiError> {
        let fan = parse_fan(&fan)?;
        self.authorize(&hdr, Action::SetFanCurve).await?;
        let (res, events, status) = self.run(move |c| c.reset_fan_curve(fan)).await?;
        self.publish(&emitter, events, status).await;
        res.map_err(Into::into)
    }

    /// Computes the EC table of a spec without applying anything (UI preview). No authorization needed.
    #[zbus(out_args("table", "raised"))]
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_value,
        reason = "D-Bus method: signature set by the interface macro"
    )]
    fn preview_fan_curve(&self, spec: SpecWire) -> Result<(TableWire, Vec<RaisedWire>), MsiError> {
        let spec = spec_from_wire(&spec)?;
        let generated = generate(&spec).map_err(|e| MsiError::Internal(e.to_string()))?;
        Ok(generated_to_wire(&generated))
    }

    /// Staged CPU curve of the selected profile (any number of stages, each with a rise temperature, a
    /// fall-back temperature and a speed): the saved one, else the 6 stages of the preset the custom curve
    /// starts from. `floors` lists the S2 floors that take over from the stages (never silent); `saved` = the
    /// owner saved this curve; `active` = the CPU fan follows it now; `supported` = the firmware allows live CPU
    /// control, so a staged curve can be applied.
    #[zbus(out_args("idle", "stages", "floors", "saved", "active", "supported"))]
    #[allow(
        clippy::type_complexity,
        reason = "zbus splits the out arguments only from an inline tuple, not from a type alias"
    )]
    async fn cpu_stages(&self) -> Result<(u8, Vec<StageWire>, Vec<FloorWire>, bool, bool, bool), MsiError> {
        let (res, _, _) = self.run(|c| c.cpu_stages()).await?;
        Ok(cpu_stages_to_wire(&res?))
    }

    /// Sets the staged CPU curve of the selected profile, switches it to the `custom` fan profile, saves and
    /// applies it. The GPU keeps its own 7-step curve. Out-of-bounds input (S3) is rejected before anything is
    /// changed. Returns the S2 floors that take over from the stages.
    #[zbus(out_args("floors"))]
    async fn set_cpu_stages(
        &self,
        idle: u8,
        stages: Vec<StageWire>,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<Vec<FloorWire>, MsiError> {
        let curve = staged_from_wire(idle, &stages)?;
        self.authorize(&hdr, Action::SetFanCurve).await?;
        let (res, events, status) = self.run(move |c| c.set_cpu_stages(curve)).await?;
        self.publish(&emitter, events, status).await;
        Ok(floors_to_wire(&res?.floors))
    }

    /// Drops the staged CPU curve of the selected profile and applies it: the CPU follows its exponential curve
    /// again. With no custom curve left, the profile goes back to its default fan profile.
    async fn reset_cpu_stages(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), MsiError> {
        self.authorize(&hdr, Action::SetFanCurve).await?;
        let (res, events, status) = self.run(Controller::reset_cpu_stages).await?;
        self.publish(&emitter, events, status).await;
        res.map_err(Into::into)
    }

    /// Validates a staged CPU curve and returns its S2 floors without applying anything (UI preview). No
    /// authorization needed.
    #[zbus(out_args("floors"))]
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_value,
        reason = "D-Bus method: signature set by the interface macro"
    )]
    fn preview_cpu_stages(&self, idle: u8, stages: Vec<StageWire>) -> Result<Vec<FloorWire>, MsiError> {
        let curve = staged_from_wire(idle, &stages)?;
        Ok(floors_to_wire(&curve.raised_floors()))
    }

    /// Owner's Cooler Boost choice. Turning it off fails with `InvalidArgument` while S4 forces it.
    async fn set_cooler_boost(
        &self,
        enabled: bool,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), MsiError> {
        self.authorize(&hdr, Action::SetFanCurve).await?;
        let (res, events, status) = self.run(move |c| c.set_cooler_boost(enabled)).await?;
        self.publish(&emitter, events, status).await;
        res.map_err(Into::into)
    }

    /// Hands the fans back to the firmware: factory tables, then fan mode `auto`. Lasts until a profile or curve
    /// is applied again (`SetProfile`, `SetFanCurve`, …); resume and the AC switch leave it alone.
    async fn restore_factory(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), MsiError> {
        self.authorize(&hdr, Action::SetFanCurve).await?;
        let (res, events, status) = self.run(Controller::restore_factory).await?;
        self.publish(&emitter, events, status).await;
        res.map_err(Into::into)
    }

    /// Battery charge limit, 50–100 %. Saved and re-applied at start.
    async fn set_charge_limit(
        &self,
        limit: u8,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), MsiError> {
        if !msi_core::battery::CHARGE_LIMIT_RANGE.contains(&limit) {
            return Err(MsiError::InvalidArgument(format!(
                "charge limit {limit} % is outside 50..=100 %"
            )));
        }
        self.authorize(&hdr, Action::SetBattery).await?;
        let (res, events, status) = self.run(move |c| c.set_charge_limit(limit)).await?;
        self.publish(&emitter, events, status).await;
        res.map_err(Into::into)
    }

    /// Processes holding the dGPU open `(pid, name)`: they block Éco's dGPU removal (never killed).
    #[zbus(out_args("holders"))]
    async fn gpu_holders(&self) -> Result<Vec<(u32, String)>, MsiError> {
        let (holders, _, _) = self.run(|c| c.gpu_holders()).await?;
        Ok(holders.into_iter().map(|h| (h.pid, h.name)).collect())
    }

    /// ~1 Hz measurements. Keys (omitted when unknown): `cpu_pct`, `ram_pct`, `igpu_pct`, `dgpu_pct`,
    /// `npu_pct`, `cpu_temp`, `gpu_temp`, `power_w` (d); `fan1_rpm`, `fan2_rpm` (u); `fan1_pct`, `fan2_pct` (y);
    /// `dgpu_state` (s); `cooler_boost` (b).
    #[zbus(signal)]
    async fn telemetry(emitter: &SignalEmitter<'_>, data: HashMap<String, OwnedVariant>) -> zbus::Result<()>;

    /// Safety event: `kind` (`boost_forced`, `boost_released`, `readback_mismatch`, `write_failed`,
    /// `ec_reset`, `fan_stalled`, `gpu_busy`), `value` (°C for boost and stall events, else 0) and an English
    /// `detail`.
    #[zbus(signal)]
    async fn safety_event(
        emitter: &SignalEmitter<'_>,
        kind: &str,
        value: f64,
        detail: &str,
    ) -> zbus::Result<()>;

    /// Selected profile.
    #[zbus(property)]
    fn profile(&self) -> String {
        self.snapshot().profile.as_str().to_owned()
    }

    /// Fan profile of the selected profile: `silent`, `auto`, `balanced`, `turbo` or `custom`.
    #[zbus(property)]
    fn fan_profile(&self) -> String {
        self.snapshot().fan_profile.as_str().to_owned()
    }

    /// Default fan profile of the selected profile (`eco` → `silent`, `balanced` → `balanced`, `gaming` →
    /// `turbo`).
    #[zbus(property)]
    fn default_fan_profile(&self) -> String {
        self.snapshot().default_fan_profile.as_str().to_owned()
    }

    /// Automatic profile on AC plug/unplug.
    #[zbus(property)]
    fn auto_profile_on_power(&self) -> bool {
        self.snapshot().auto_profile_on_power
    }

    /// On AC power (`false` also when unknown).
    #[zbus(property)]
    fn on_ac_power(&self) -> bool {
        self.snapshot().on_ac.unwrap_or(false)
    }

    /// EC fan mode as read: `advanced` = our curves are in force; `auto` = firmware curve (start, fallback,
    /// read-only); `unknown` if unreadable.
    #[zbus(property)]
    fn fan_mode(&self) -> String {
        self.snapshot().fan_mode
    }

    /// Cooler Boost as read from the EC.
    #[zbus(property)]
    fn cooler_boost(&self) -> bool {
        self.snapshot().cooler_boost
    }

    /// Cooler Boost forced by the thermal supervisor (S4).
    #[zbus(property)]
    fn boost_forced(&self) -> bool {
        self.snapshot().boost_forced
    }

    /// Battery charge limit (%), 0 if unsupported.
    #[zbus(property)]
    fn charge_limit(&self) -> u8 {
        self.snapshot().charge_limit.unwrap_or(0)
    }

    /// dGPU state: `active`, `suspended` or `removed`.
    #[zbus(property)]
    fn gpu_state(&self) -> String {
        self.snapshot().gpu_state.as_str().to_owned()
    }

    /// `true` when nothing is written: S8 (EC not supported) or S9 (fan stall, until the daemon restarts).
    #[zbus(property)]
    fn read_only(&self) -> bool {
        self.snapshot().read_only.is_some()
    }

    /// Why the daemon is read-only (empty otherwise).
    #[zbus(property)]
    fn read_only_reason(&self) -> String {
        self.snapshot().read_only.unwrap_or_default()
    }

    /// EC firmware version (empty if msi-ec is not loaded).
    #[zbus(property)]
    fn firmware_version(&self) -> String {
        self.snapshot().fw_version
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::telemetry::Sample;
    use crate::telemetry::hwmon::Thermals;
    use msi_core::curve::CurvePreset;

    #[test]
    fn spec_round_trips_and_invalid_specs_are_rejected() {
        let spec = CurvePreset::Silent.spec();
        assert_eq!(spec_from_wire(&spec_to_wire(&spec)).unwrap(), spec);
        let mut w = spec_to_wire(&spec);
        w.6.pop();
        assert!(matches!(spec_from_wire(&w), Err(MsiError::InvalidArgument(_))));
        let mut w = spec_to_wire(&spec);
        w.5 = 9.0;
        assert!(matches!(spec_from_wire(&w), Err(MsiError::InvalidArgument(_))));
        let mut w = spec_to_wire(&spec);
        w.7 = 1;
        assert!(matches!(spec_from_wire(&w), Err(MsiError::InvalidArgument(_))));
    }

    #[test]
    fn staged_curve_round_trips_and_invalid_curves_are_rejected() {
        let wire = vec![(50, 46, 30), (60, 56, 45), (70, 66, 70)];
        let curve = staged_from_wire(0, &wire).unwrap();
        assert_eq!(stages_to_wire(&curve), wire);
        assert_eq!(curve.idle(), 0);
        for (idle, stages) in [
            (0, vec![]),
            (0, vec![(50, 50, 30)]),
            (0, vec![(50, 49, 30)]),
            (0, vec![(60, 56, 45), (50, 46, 30)]),
            (0, vec![(50, 46, 40), (60, 56, 30)]),
            (0, vec![(50, 46, 101)]),
            (31, vec![(50, 46, 30)]),
            (0, vec![(19, 15, 30)]),
        ] {
            assert!(
                matches!(staged_from_wire(idle, &stages), Err(MsiError::InvalidArgument(_))),
                "{idle} {stages:?}"
            );
        }
        let too_many: Vec<StageWire> = vec![(50, 46, 30); 80];
        assert!(matches!(
            staged_from_wire(0, &too_many),
            Err(MsiError::InvalidArgument(_))
        ));
    }

    #[test]
    fn floors_wire_is_floor_temp_floor_speed_curve_speed() {
        let quiet = staged_from_wire(0, &[(40, 36, 20), (90, 80, 100)]).unwrap();
        assert_eq!(
            floors_to_wire(&quiet.raised_floors()),
            [(60, 30, 20), (75, 50, 20), (85, 100, 20)]
        );
    }

    #[test]
    fn telemetry_prefers_ec_temperatures_and_omits_unknowns() {
        let r = Readings {
            sample: Some(Sample {
                cpu_pct: Some(12.5),
                ram_pct: None,
                igpu_pct: None,
                dgpu_pct: None,
                npu_pct: Some(0.0),
                thermals: Thermals {
                    cpu_temp_c: Some(40.0),
                    gpu_temp_c: Some(41.0),
                    fan1_rpm: Some(2200),
                    fan2_rpm: None,
                },
                dgpu_power_w: None,
                dgpu_state: crate::gpu::GpuState::Removed,
            }),
            cpu_temp: Some(55),
            gpu_temp: None,
            fan_pct: [Some(33), None],
            fan_rpm: [Some(2200), None],
            cooler_boost: Some(false),
            cpu_fan_target: Some(37),
        };
        let m = telemetry_map(&r);
        let mut keys: Vec<&str> = m.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "cooler_boost",
                "cpu_fan_target_pct",
                "cpu_pct",
                "cpu_temp",
                "dgpu_state",
                "fan1_pct",
                "fan1_rpm",
                "gpu_temp",
                "npu_pct"
            ]
        );
        assert!((f64::try_from(&*m["cpu_temp"]).unwrap() - 55.0).abs() < 1e-9);
        assert!((f64::try_from(&*m["gpu_temp"]).unwrap() - 41.0).abs() < 1e-9);
        assert_eq!(u32::try_from(&*m["fan1_rpm"]).unwrap(), 2200);
        assert_eq!(u8::try_from(&*m["cpu_fan_target_pct"]).unwrap(), 37);
    }
}
