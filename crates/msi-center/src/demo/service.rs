//! In-process fake `org.blazorde.MsiCenter1` and `org.blazorde.Clipd1`, served over private socket pairs.
//!
//! [`connect`] returns client connections the regular [`crate::dbus`] clients use unchanged. No message bus
//! is involved (peer-to-peer), so the demo can never reach — or D-Bus-activate — the real services.

#![allow(
    clippy::needless_pass_by_value,
    clippy::used_underscore_binding,
    reason = "D-Bus method signatures are set by the interface macro"
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use msi_core::curve::FanCurveSpec;
use msi_core::ec::FACTORY_14K2EMS1_110;
use msi_core::fan_profile::{FanCurveSource, FanProfile};
use msi_core::hybrid::CpuCurve;
use msi_core::profile::Profile;
use msi_core::staged::{FloorRaise, StagedCurve};
use msi_core::table::{GeneratedCurve, generate};
use zbus::object_server::SignalEmitter;
use zbus::{Connection, OwnedVariant as OwnedValue, interface};

use super::{DemoClip, DemoState, ProfileOutcome};
use crate::curve::{RaisedWire, SpecWire, TableWire};
use crate::dbus::{CLIPD_PATH, DAEMON_PATH};
use crate::model::{ClipWire, Fan};
use crate::stages::{FloorWire, StageWire, curve_from_wire, stages_to_wire};

/// Errors of the fake services, named like the real ones.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.blazorde.MsiCenter1.Error")]
pub enum DemoError {
    /// Transport errors.
    #[zbus(error)]
    ZBus(zbus::Error),
    /// Rejected value.
    InvalidArgument(String),
    /// No such clip.
    NotFound(String),
}

type Shared<T> = Arc<Mutex<T>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Default)]
struct DaemonInner {
    state: DemoState,
    tick: u64,
    custom: HashMap<(&'static str, &'static str), FanCurveSpec>,
    /// Staged CPU curve per profile (ADR-0013).
    staged: HashMap<&'static str, StagedCurve>,
    cpu_temp: f64,
}

/// The fake daemon.
#[derive(Debug, Clone, Default)]
pub struct DemoDaemon {
    inner: Shared<DaemonInner>,
}

fn invalid(e: impl std::fmt::Display) -> DemoError {
    DemoError::InvalidArgument(e.to_string())
}

fn spec_from_wire(w: &SpecWire) -> Result<FanCurveSpec, DemoError> {
    crate::curve::CurveSpec::from_wire(w)
        .ok_or_else(|| invalid("expected 6 up thresholds"))?
        .validate()
        .map_err(invalid)
}

fn spec_to_wire(s: &FanCurveSpec) -> SpecWire {
    crate::curve::CurveSpec::from_core(s).to_wire()
}

fn to_wire(g: &GeneratedCurve) -> (TableWire, Vec<RaisedWire>) {
    let table = (
        g.curve.speeds().to_vec(),
        g.curve.up().to_vec(),
        g.curve.down().to_vec(),
    );
    let raised = g
        .report
        .raised
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

fn floors_to_wire(floors: &[FloorRaise]) -> Vec<FloorWire> {
    floors
        .iter()
        .map(|f| (f.floor.temp, f.floor.speed, f.curve_speed))
        .collect()
}

fn parse_fan(fan: &str) -> Result<Fan, DemoError> {
    Fan::ALL
        .into_iter()
        .find(|f| f.as_str() == fan)
        .ok_or_else(|| invalid(format!("unknown fan {fan:?}")))
}

impl DaemonInner {
    /// Custom curve of `fan` in the selected profile (saved, else the default preset).
    fn custom_spec(&self, fan: Fan) -> FanCurveSpec {
        let key = (self.state.profile.as_str(), fan.as_str());
        self.custom
            .get(&key)
            .copied()
            .unwrap_or_else(|| self.state.profile.target().curve.spec())
    }

    /// Curve in force for `fan`, and whether the fan profile is `custom` (as the real daemon).
    fn spec(&self, fan: Fan) -> (FanCurveSpec, bool) {
        let fp = self.state.fan_profile();
        let spec = match fp.source() {
            FanCurveSource::Preset(p) => p.spec(),
            FanCurveSource::Custom => self.custom_spec(fan),
            FanCurveSource::Factory => self.state.profile.target().curve.spec(),
        };
        (spec, fp == FanProfile::Custom)
    }

    /// Staged CPU curve of the selected profile when it is in force (fan profile `custom`).
    fn staged_in_force(&self) -> Option<&StagedCurve> {
        (self.state.fan_profile() == FanProfile::Custom)
            .then(|| self.staged.get(self.state.profile.as_str()))
            .flatten()
    }

    /// The curve driving the live CPU target: the staged one when in force, else the exponential one.
    fn cpu_curve(&self) -> CpuCurve {
        self.staged_in_force().map_or_else(
            || CpuCurve::Exponential(self.spec(Fan::Cpu).0),
            |c| CpuCurve::Staged(c.clone()),
        )
    }

    /// Live CPU target % (ADR-0010, ADR-0013) at the current CPU temperature.
    fn cpu_target(&self) -> u8 {
        match self.cpu_curve() {
            CpuCurve::Exponential(spec) => {
                let p = spec.params();
                if self.cpu_temp < f64::from(p.t0()) {
                    spec.idle()
                } else {
                    p.speed_percent_celsius(self.cpu_temp)
                }
            }
            staged @ CpuCurve::Staged(_) => {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped")]
                let t = self.cpu_temp.floor().clamp(0.0, 255.0) as u8;
                staged.speed_percent(t)
            }
        }
    }

    /// Table in force: the factory table under Auto (nothing raised), else the generated / live one.
    fn table(&self, fan: Fan) -> Result<(TableWire, Vec<RaisedWire>), DemoError> {
        if self.state.fan_profile() == FanProfile::Auto {
            let t = match fan {
                Fan::Cpu => FACTORY_14K2EMS1_110.cpu,
                Fan::Gpu => FACTORY_14K2EMS1_110.gpu,
            };
            return Ok(((t.speeds.to_vec(), t.up.to_vec(), t.down.to_vec()), Vec::new()));
        }
        Ok(to_wire(&self.generated(fan)?))
    }

    fn generated(&self, fan: Fan) -> Result<GeneratedCurve, DemoError> {
        let (spec, _) = self.spec(fan);
        match fan {
            Fan::Cpu => msi_core::hybrid::hybrid_cpu_table_for(
                &self.cpu_curve(),
                &FACTORY_14K2EMS1_110.cpu,
                self.cpu_target(),
            ),
            Fan::Gpu => generate(&spec),
        }
        .map_err(invalid)
    }
}

impl DemoDaemon {
    fn inner(&self) -> MutexGuard<'_, DaemonInner> {
        lock(&self.inner)
    }

    async fn all_changed(&self, e: &SignalEmitter<'_>) {
        let _ = self.profile_changed(e).await;
        let _ = self.fan_profile_changed(e).await;
        let _ = self.default_fan_profile_changed(e).await;
        let _ = self.gpu_state_changed(e).await;
        let _ = self.fan_mode_changed(e).await;
        let _ = self.cooler_boost_changed(e).await;
        let _ = self.charge_limit_changed(e).await;
        let _ = self.auto_profile_on_power_changed(e).await;
    }

    /// One second of simulated time: telemetry, and status when the fake dGPU holder goes away.
    async fn step(&self, e: &SignalEmitter<'_>) {
        let (map, changed) = {
            let mut i = self.inner();
            i.tick += 1;
            let now = i.tick;
            let changed = i.state.tick(now);
            let s = super::sample(now, &i.state);
            i.cpu_temp = s.cpu_temp;
            let target = (i.state.fan_profile() != FanProfile::Auto).then(|| i.cpu_target());
            (super::telemetry_map(&s, &i.state, target), changed)
        };
        if changed {
            self.all_changed(e).await;
        }
        let _ = Self::telemetry(e, map).await;
    }
}

#[interface(name = "org.blazorde.MsiCenter1")]
impl DemoDaemon {
    async fn set_profile(
        &self,
        profile: String,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<(), DemoError> {
        let p: Profile = profile.parse().map_err(invalid)?;
        // A short, visible latency, like polkit + EC writes on the real machine.
        tokio::time::sleep(Duration::from_millis(450)).await;
        let outcome = {
            let mut i = self.inner();
            let now = i.tick;
            i.state.set_profile(p, now)
        };
        self.all_changed(&e).await;
        if let ProfileOutcome::GpuBusy(detail) = outcome {
            let _ = Self::safety_event(&e, "gpu_busy", 0.0, &detail).await;
        }
        Ok(())
    }

    async fn set_auto_profile_on_power(
        &self,
        enabled: bool,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<(), DemoError> {
        self.inner().state.auto_profile_on_power = enabled;
        let _ = self.auto_profile_on_power_changed(&e).await;
        Ok(())
    }

    async fn set_fan_profile(
        &self,
        fan_profile: String,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<(), DemoError> {
        let fp: FanProfile = fan_profile.parse().map_err(invalid)?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.inner().state.set_fan_profile(fp);
        let _ = self.fan_profile_changed(&e).await;
        let _ = self.fan_mode_changed(&e).await;
        Ok(())
    }

    #[zbus(out_args("spec", "table", "raised"))]
    fn custom_fan_curve(&self, fan: String) -> Result<(SpecWire, TableWire, Vec<RaisedWire>), DemoError> {
        let fan = parse_fan(&fan)?;
        let spec = self.inner().custom_spec(fan);
        let (table, raised) = to_wire(&generate(&spec).map_err(invalid)?);
        Ok((spec_to_wire(&spec), table, raised))
    }

    #[zbus(out_args("spec", "table", "raised", "custom"))]
    fn fan_curve(&self, fan: String) -> Result<(SpecWire, TableWire, Vec<RaisedWire>, bool), DemoError> {
        let fan = parse_fan(&fan)?;
        let i = self.inner();
        let (spec, custom) = i.spec(fan);
        let (table, raised) = i.table(fan)?;
        Ok((spec_to_wire(&spec), table, raised, custom))
    }

    #[zbus(out_args("table", "raised"))]
    async fn set_fan_curve(
        &self,
        fan: String,
        spec: SpecWire,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<(TableWire, Vec<RaisedWire>), DemoError> {
        let fan = parse_fan(&fan)?;
        let spec = spec_from_wire(&spec)?;
        let out = {
            let mut i = self.inner();
            let key = (i.state.profile.as_str(), fan.as_str());
            i.custom.insert(key, spec);
            if fan == Fan::Cpu {
                // Last edit wins: the exponential curve replaces the staged one.
                let profile = i.state.profile.as_str();
                i.staged.remove(profile);
            }
            i.state.set_fan_profile(FanProfile::Custom);
            to_wire(&i.generated(fan)?)
        };
        let _ = self.fan_profile_changed(&e).await;
        let _ = self.fan_mode_changed(&e).await;
        Ok(out)
    }

    async fn reset_fan_curve(
        &self,
        fan: String,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<(), DemoError> {
        let fan = parse_fan(&fan)?;
        {
            let mut i = self.inner();
            let profile = i.state.profile.as_str();
            i.custom.remove(&(profile, fan.as_str()));
            if fan == Fan::Cpu {
                i.staged.remove(profile);
            }
            let any_left = i.staged.contains_key(profile)
                || Fan::ALL
                    .iter()
                    .any(|f| i.custom.contains_key(&(profile, f.as_str())));
            if !any_left && i.state.fan_profile() == FanProfile::Custom {
                let default = i.state.profile.default_fan_profile();
                i.state.set_fan_profile(default);
            }
        }
        let _ = self.fan_profile_changed(&e).await;
        let _ = self.fan_mode_changed(&e).await;
        Ok(())
    }

    #[zbus(out_args("idle", "stages", "floors", "saved", "active", "supported"))]
    #[allow(
        clippy::type_complexity,
        reason = "zbus splits out arguments only from an inline tuple"
    )]
    fn cpu_stages(&self) -> Result<(u8, Vec<StageWire>, Vec<FloorWire>, bool, bool, bool), DemoError> {
        let i = self.inner();
        let saved = i.staged.get(i.state.profile.as_str()).cloned();
        let curve = if let Some(c) = &saved {
            c.clone()
        } else {
            let generated = generate(&i.custom_spec(Fan::Cpu)).map_err(invalid)?;
            StagedCurve::from_step_curve(&generated.curve).map_err(invalid)?
        };
        let active = saved.is_some() && i.state.fan_profile() == FanProfile::Custom;
        Ok((
            curve.idle(),
            stages_to_wire(&curve),
            floors_to_wire(&curve.raised_floors()),
            saved.is_some(),
            active,
            true,
        ))
    }

    #[zbus(out_args("floors"))]
    async fn set_cpu_stages(
        &self,
        idle: u8,
        stages: Vec<StageWire>,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<Vec<FloorWire>, DemoError> {
        let curve = curve_from_wire(idle, &stages).map_err(invalid)?;
        let floors = floors_to_wire(&curve.raised_floors());
        {
            let mut i = self.inner();
            let profile = i.state.profile.as_str();
            i.staged.insert(profile, curve);
            i.state.set_fan_profile(FanProfile::Custom);
        }
        let _ = self.fan_profile_changed(&e).await;
        let _ = self.fan_mode_changed(&e).await;
        Ok(floors)
    }

    async fn reset_cpu_stages(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> Result<(), DemoError> {
        {
            let mut i = self.inner();
            let profile = i.state.profile.as_str();
            if i.staged.remove(profile).is_none() {
                return Ok(());
            }
            let any_left = Fan::ALL
                .iter()
                .any(|f| i.custom.contains_key(&(profile, f.as_str())));
            if !any_left && i.state.fan_profile() == FanProfile::Custom {
                let default = i.state.profile.default_fan_profile();
                i.state.set_fan_profile(default);
            }
        }
        let _ = self.fan_profile_changed(&e).await;
        let _ = self.fan_mode_changed(&e).await;
        Ok(())
    }

    #[zbus(out_args("floors"))]
    #[allow(clippy::unused_self, reason = "D-Bus method")]
    fn preview_cpu_stages(&self, idle: u8, stages: Vec<StageWire>) -> Result<Vec<FloorWire>, DemoError> {
        let curve = curve_from_wire(idle, &stages).map_err(invalid)?;
        Ok(floors_to_wire(&curve.raised_floors()))
    }

    #[zbus(out_args("table", "raised"))]
    #[allow(clippy::unused_self, clippy::needless_pass_by_value, reason = "D-Bus method")]
    fn preview_fan_curve(&self, spec: SpecWire) -> Result<(TableWire, Vec<RaisedWire>), DemoError> {
        Ok(to_wire(&generate(&spec_from_wire(&spec)?).map_err(invalid)?))
    }

    async fn set_cooler_boost(
        &self,
        enabled: bool,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<(), DemoError> {
        self.inner().state.cooler_boost = enabled;
        let _ = self.cooler_boost_changed(&e).await;
        Ok(())
    }

    #[zbus(property)]
    fn fan_profile(&self) -> String {
        self.inner().state.fan_profile().as_str().to_owned()
    }

    #[zbus(property)]
    fn default_fan_profile(&self) -> String {
        self.inner()
            .state
            .profile
            .default_fan_profile()
            .as_str()
            .to_owned()
    }

    async fn restore_factory(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> Result<(), DemoError> {
        self.inner().state.fan_mode = "auto";
        let _ = self.fan_mode_changed(&e).await;
        Ok(())
    }

    async fn set_charge_limit(
        &self,
        limit: u8,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<(), DemoError> {
        if !msi_core::battery::CHARGE_LIMIT_RANGE.contains(&limit) {
            return Err(invalid(format!("charge limit {limit} % is outside 50..=100 %")));
        }
        self.inner().state.charge_limit = limit;
        let _ = self.charge_limit_changed(&e).await;
        Ok(())
    }

    #[zbus(out_args("holders"))]
    fn gpu_holders(&self) -> Vec<(u32, String)> {
        self.inner().state.holders()
    }

    #[zbus(signal)]
    async fn telemetry(e: &SignalEmitter<'_>, data: HashMap<String, OwnedValue>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn safety_event(e: &SignalEmitter<'_>, kind: &str, value: f64, detail: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn profile(&self) -> String {
        self.inner().state.profile.as_str().to_owned()
    }

    #[zbus(property)]
    fn auto_profile_on_power(&self) -> bool {
        self.inner().state.auto_profile_on_power
    }

    #[zbus(property)]
    fn on_ac_power(&self) -> bool {
        self.inner().state.on_ac
    }

    #[zbus(property)]
    fn fan_mode(&self) -> String {
        self.inner().state.fan_mode.to_owned()
    }

    #[zbus(property)]
    fn cooler_boost(&self) -> bool {
        self.inner().state.cooler_boost
    }

    #[zbus(property)]
    #[allow(clippy::unused_self, reason = "D-Bus property")]
    fn boost_forced(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn charge_limit(&self) -> u8 {
        self.inner().state.charge_limit
    }

    #[zbus(property)]
    fn gpu_state(&self) -> String {
        self.inner().state.gpu_state.to_owned()
    }

    #[zbus(property)]
    #[allow(clippy::unused_self, reason = "D-Bus property")]
    fn read_only(&self) -> bool {
        false
    }

    #[zbus(property)]
    #[allow(clippy::unused_self, reason = "D-Bus property")]
    fn read_only_reason(&self) -> String {
        String::new()
    }

    #[zbus(property)]
    #[allow(clippy::unused_self, reason = "D-Bus property")]
    fn firmware_version(&self) -> String {
        FACTORY_14K2EMS1_110.firmware.to_owned()
    }
}

/// The fake clipd.
#[derive(Debug, Clone)]
pub struct DemoClipd {
    clips: Shared<Vec<DemoClip>>,
}

impl DemoClipd {
    fn new(now_ms: i64) -> Self {
        Self {
            clips: Arc::new(Mutex::new(super::clips(now_ms))),
        }
    }

    fn with_clip<T>(&self, id: u64, f: impl FnOnce(&mut DemoClip) -> T) -> Result<T, DemoError> {
        let mut clips = lock(&self.clips);
        let c = clips
            .iter_mut()
            .find(|c| c.0.0 == id)
            .ok_or_else(|| DemoError::NotFound(format!("no clip {id}")))?;
        Ok(f(c))
    }
}

#[interface(name = "org.blazorde.Clipd1")]
impl DemoClipd {
    #[allow(clippy::unused_self, reason = "D-Bus method")]
    fn add(&self, _mime: &str, _data: Vec<u8>) -> Result<u64, DemoError> {
        Err(DemoError::InvalidArgument("read-only demo".into()))
    }

    fn list(
        &self,
        query: &str,
        tags: Vec<String>,
        scope: &str,
        offset: u32,
        limit: u32,
    ) -> (u32, Vec<ClipWire>) {
        let q = query.to_lowercase();
        let all: Vec<ClipWire> = lock(&self.clips)
            .iter()
            .map(|c| c.0.clone())
            .filter(|w| q.is_empty() || w.2.to_lowercase().contains(&q))
            .filter(|w| tags.iter().all(|t| w.7.contains(t)))
            .filter(|w| match scope {
                "pinned" => w.6,
                "unpinned" => !w.6,
                _ => true,
            })
            .collect();
        let total = u32::try_from(all.len()).unwrap_or(u32::MAX);
        let page = all
            .into_iter()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .collect();
        (total, page)
    }

    fn get(&self, id: u64) -> Result<(String, Vec<u8>), DemoError> {
        self.with_clip(id, |c| (c.1.clone(), c.2.clone()))
    }

    async fn pin(&self, id: u64, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> Result<(), DemoError> {
        self.with_clip(id, |c| c.0.6 = true)?;
        let _ = Self::changed(&e).await;
        Ok(())
    }

    async fn unpin(&self, id: u64, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> Result<(), DemoError> {
        self.with_clip(id, |c| c.0.6 = false)?;
        let _ = Self::changed(&e).await;
        Ok(())
    }

    async fn delete(&self, ids: Vec<u64>, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> u32 {
        let n = {
            let mut clips = lock(&self.clips);
            let before = clips.len();
            clips.retain(|c| !ids.contains(&c.0.0));
            u32::try_from(before - clips.len()).unwrap_or(0)
        };
        let _ = Self::changed(&e).await;
        n
    }

    async fn clear_unpinned(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> u32 {
        let n = {
            let mut clips = lock(&self.clips);
            let before = clips.len();
            clips.retain(|c| c.0.6);
            u32::try_from(before - clips.len()).unwrap_or(0)
        };
        let _ = Self::changed(&e).await;
        n
    }

    async fn set_tags(
        &self,
        id: u64,
        tags: Vec<String>,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<Vec<String>, DemoError> {
        let mut norm: Vec<String> = tags
            .iter()
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        norm.sort();
        norm.dedup();
        let out = norm.clone();
        self.with_clip(id, move |c| {
            if !norm.is_empty() {
                c.0.6 = true;
            }
            c.0.7 = norm;
        })?;
        let _ = Self::changed(&e).await;
        Ok(out)
    }

    fn tags(&self) -> Vec<(String, u32)> {
        super::tag_counts(&lock(&self.clips))
    }

    #[zbus(signal)]
    async fn changed(e: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Server ends (keep them alive) and client ends of the two fake services.
#[derive(Debug)]
pub struct DemoLinks {
    /// Server side of the daemon link.
    pub daemon_server: Connection,
    /// Client side of the daemon link.
    pub daemon: Connection,
    /// Server side of the clipd link.
    pub clipd_server: Connection,
    /// Client side of the clipd link.
    pub clipd: Connection,
}

async fn pair<I>(path: &str, iface: I) -> zbus::Result<(Connection, Connection)>
where
    I: zbus::object_server::Interface,
{
    let (a, b) = std::os::unix::net::UnixStream::pair()?;
    let server = zbus::connection::Builder::unix_stream(a)
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at(path, iface)
        .build();
    let client = zbus::connection::Builder::unix_stream(b).p2p().build();
    tokio::try_join!(server, client)
}

/// Starts both fake services (must run on the tokio runtime) and the 1 Hz telemetry of the daemon.
///
/// # Errors
/// Socket pair or connection setup failures.
pub async fn connect(now_ms: i64) -> zbus::Result<DemoLinks> {
    let demo = DemoDaemon::default();
    // Screenshots of another fan profile (`scripts/app-shots.sh`, page `fans-auto`).
    if let Some(fp) = std::env::var(super::FAN_PROFILE_ENV)
        .ok()
        .and_then(|v| v.parse::<FanProfile>().ok())
    {
        demo.inner().state.set_fan_profile(fp);
    }
    let (daemon_server, daemon) = pair(DAEMON_PATH, demo).await?;
    let (clipd_server, clipd) = pair(CLIPD_PATH, DemoClipd::new(now_ms)).await?;
    let iface = daemon_server
        .object_server()
        .interface::<_, DemoDaemon>(DAEMON_PATH)
        .await?;
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(1));
        loop {
            every.tick().await;
            let d = iface.get().await.clone();
            d.step(iface.signal_emitter()).await;
        }
    });
    Ok(DemoLinks {
        daemon_server,
        daemon,
        clipd_server,
        clipd,
    })
}
