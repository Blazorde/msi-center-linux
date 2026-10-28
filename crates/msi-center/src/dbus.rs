//! Typed async clients of `org.blazorde.MsiCenter1` (system bus) and `org.blazorde.Clipd1` (session bus).
//!
//! Everything here is runtime-agnostic async code on top of zbus; [`crate::backend`] runs it on a tokio
//! thread. The proxies are declared here (not imported from `msi-daemon`) so the app does not link the
//! daemon; `tests/dbus.rs` checks them against the real interfaces over peer-to-peer connections.

use std::collections::HashMap;

use futures_util::stream::{self, BoxStream, StreamExt as _};
use zbus::Connection;
use zbus::OwnedVariant as OwnedValue;

use crate::curve::{CurveSpec, Generated, RaisedWire, SpecWire, TableWire};
use crate::errors::{AppError, Service};
use crate::model::{self, ClipEntry, ClipWire, DaemonStatus, Fan, Scope};
use crate::stages::{CpuStagesWire, FloorWire, StageWire, StagedState};

/// Bus name of the daemon.
pub const DAEMON_NAME: &str = "org.blazorde.MsiCenter1";
/// Object path of the daemon.
pub const DAEMON_PATH: &str = "/org/blazorde/MsiCenter";
/// Bus name of clipd.
pub const CLIPD_NAME: &str = "org.blazorde.Clipd1";
/// Object path of clipd.
pub const CLIPD_PATH: &str = "/org/blazorde/Clipd";

/// Optional method that hands the fans back to the EC factory tables. Not in the frozen API yet: the
/// button is shown only if introspection lists it.
pub const RESTORE_FACTORY: &str = "RestoreFactory";

/// Proxy of `org.blazorde.MsiCenter1` (see `docs/architecture/dbus-api.md`).
#[zbus::proxy(
    interface = "org.blazorde.MsiCenter1",
    default_service = "org.blazorde.MsiCenter1",
    default_path = "/org/blazorde/MsiCenter"
)]
pub trait MsiCenter1 {
    /// `SetProfile`.
    fn set_profile(&self, profile: &str) -> zbus::Result<()>;
    /// `SetAutoProfileOnPower`.
    fn set_auto_profile_on_power(&self, enabled: bool) -> zbus::Result<()>;
    /// `SetFanProfile`.
    fn set_fan_profile(&self, fan_profile: &str) -> zbus::Result<()>;
    /// `CustomFanCurve`.
    fn custom_fan_curve(&self, fan: &str) -> zbus::Result<(SpecWire, TableWire, Vec<RaisedWire>)>;
    /// `FanCurve`.
    fn fan_curve(&self, fan: &str) -> zbus::Result<(SpecWire, TableWire, Vec<RaisedWire>, bool)>;
    /// `SetFanCurve`.
    fn set_fan_curve(&self, fan: &str, spec: &SpecWire) -> zbus::Result<(TableWire, Vec<RaisedWire>)>;
    /// `ResetFanCurve`.
    fn reset_fan_curve(&self, fan: &str) -> zbus::Result<()>;
    /// `PreviewFanCurve`.
    fn preview_fan_curve(&self, spec: &SpecWire) -> zbus::Result<(TableWire, Vec<RaisedWire>)>;
    /// `CpuStages` (ADR-0013).
    fn cpu_stages(&self) -> zbus::Result<CpuStagesWire>;
    /// `SetCpuStages` (ADR-0013): returns the S2 floors that take over from the stages.
    fn set_cpu_stages(&self, idle: u8, stages: &[StageWire]) -> zbus::Result<Vec<FloorWire>>;
    /// `ResetCpuStages` (ADR-0013).
    fn reset_cpu_stages(&self) -> zbus::Result<()>;
    /// `PreviewCpuStages` (ADR-0013).
    fn preview_cpu_stages(&self, idle: u8, stages: &[StageWire]) -> zbus::Result<Vec<FloorWire>>;
    /// `SetCoolerBoost`.
    fn set_cooler_boost(&self, enabled: bool) -> zbus::Result<()>;
    /// `SetChargeLimit`.
    fn set_charge_limit(&self, limit: u8) -> zbus::Result<()>;
    /// `GpuHolders`.
    fn gpu_holders(&self) -> zbus::Result<Vec<(u32, String)>>;
    /// `Telemetry` signal.
    #[zbus(signal)]
    fn telemetry(&self, data: HashMap<String, OwnedValue>) -> zbus::Result<()>;
    /// `SafetyEvent` signal.
    #[zbus(signal)]
    fn safety_event(&self, kind: String, value: f64, detail: String) -> zbus::Result<()>;
}

/// Proxy of `org.blazorde.Clipd1`.
#[zbus::proxy(
    interface = "org.blazorde.Clipd1",
    default_service = "org.blazorde.Clipd1",
    default_path = "/org/blazorde/Clipd"
)]
pub trait Clipd1 {
    /// `Add` (used by the tests; the extension captures the clipboard).
    fn add(&self, mime: &str, data: &[u8]) -> zbus::Result<u64>;
    /// `List`.
    fn list(
        &self,
        query: &str,
        tags: &[&str],
        scope: &str,
        offset: u32,
        limit: u32,
    ) -> zbus::Result<(u32, Vec<ClipWire>)>;
    /// `Get`.
    fn get(&self, id: u64) -> zbus::Result<(String, Vec<u8>)>;
    /// `Pin`.
    fn pin(&self, id: u64) -> zbus::Result<()>;
    /// `Unpin`.
    fn unpin(&self, id: u64) -> zbus::Result<()>;
    /// `Delete`.
    fn delete(&self, ids: &[u64]) -> zbus::Result<u32>;
    /// `ClearUnpinned`.
    fn clear_unpinned(&self) -> zbus::Result<u32>;
    /// `SetTags`.
    fn set_tags(&self, id: u64, tags: &[&str]) -> zbus::Result<Vec<String>>;
    /// `Tags`.
    fn tags(&self) -> zbus::Result<Vec<(String, u32)>>;
    /// `Changed` signal.
    #[zbus(signal)]
    fn changed(&self) -> zbus::Result<()>;
}

/// Result alias of this module.
pub type Result<T> = std::result::Result<T, AppError>;

fn de(e: &zbus::Error) -> AppError {
    AppError::from_zbus(e, Service::Daemon)
}

fn ce(e: &zbus::Error) -> AppError {
    AppError::from_zbus(e, Service::Clipd)
}

fn bad_reply(what: &str) -> AppError {
    AppError::Internal(format!("malformed {what} reply"))
}

/// A fan's curve as stored by the daemon.
#[derive(Debug, Clone, PartialEq)]
pub struct FanCurveState {
    /// Parameters.
    pub spec: CurveSpec,
    /// Table and S2 report.
    pub generated: Generated,
    /// Custom (`true`) or the profile's preset.
    pub custom: bool,
}

/// Something the daemon pushed.
#[derive(Debug, Clone, PartialEq)]
pub enum DaemonEvent {
    /// A telemetry sample (~1 Hz).
    Telemetry(model::Telemetry),
    /// A safety event.
    Safety(model::SafetyEvent),
    /// Properties changed; the full status was re-read.
    Status(DaemonStatus),
}

/// Client of the daemon. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Daemon {
    proxy: MsiCenter1Proxy<'static>,
    props: zbus::fdo::PropertiesProxy<'static>,
    conn: Connection,
}

impl Daemon {
    /// Builds the client on `conn` (no call is made).
    ///
    /// # Errors
    /// Proxy construction errors.
    pub async fn new(conn: &Connection) -> Result<Self> {
        let proxy = MsiCenter1Proxy::builder(conn)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .map_err(|e| de(&e))?;
        let props = zbus::fdo::PropertiesProxy::builder(conn)
            .destination(DAEMON_NAME)
            .path(DAEMON_PATH)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .map_err(|e| de(&e))?;
        Ok(Self {
            proxy,
            props,
            conn: conn.clone(),
        })
    }

    /// The raw proxy.
    #[must_use]
    pub const fn proxy(&self) -> &MsiCenter1Proxy<'static> {
        &self.proxy
    }

    /// All properties (activates the daemon if needed).
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn status(&self) -> Result<DaemonStatus> {
        let iface = zbus::names::InterfaceName::from_static_str_unchecked(DAEMON_NAME);
        let all = self
            .props
            .get_all(iface)
            .await
            .map_err(|e| de(&zbus::Error::from(e)))?;
        Ok(DaemonStatus::from_map(&all))
    }

    /// `SetProfile`.
    ///
    /// # Errors
    /// Mapped D-Bus errors (`GpuBusy` means the EC part was applied).
    pub async fn set_profile(&self, profile: &str) -> Result<()> {
        self.proxy.set_profile(profile).await.map_err(|e| de(&e))
    }

    /// `SetAutoProfileOnPower`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn set_auto_profile_on_power(&self, enabled: bool) -> Result<()> {
        self.proxy
            .set_auto_profile_on_power(enabled)
            .await
            .map_err(|e| de(&e))
    }

    /// `FanCurve`.
    ///
    /// # Errors
    /// Mapped D-Bus errors, or a malformed reply.
    pub async fn fan_curve(&self, fan: Fan) -> Result<FanCurveState> {
        let (spec, table, raised, custom) = self.proxy.fan_curve(fan.as_str()).await.map_err(|e| de(&e))?;
        Ok(FanCurveState {
            spec: CurveSpec::from_wire(&spec).ok_or_else(|| bad_reply("FanCurve"))?,
            generated: Generated::from_wire(&table, &raised).ok_or_else(|| bad_reply("FanCurve"))?,
            custom,
        })
    }

    /// `SetFanProfile`: the fan profile of the selected profile (remembered per profile).
    ///
    /// # Errors
    /// Mapped D-Bus errors (`GpuBusy` means the EC part was applied).
    pub async fn set_fan_profile(&self, fan_profile: msi_core::fan_profile::FanProfile) -> Result<()> {
        self.proxy
            .set_fan_profile(fan_profile.as_str())
            .await
            .map_err(|e| de(&e))
    }

    /// `CustomFanCurve`: the custom curve of `fan` (the Personnalisé editor), in force or not, with its step
    /// table and S2 report.
    ///
    /// # Errors
    /// Mapped D-Bus errors, or a malformed reply.
    pub async fn custom_fan_curve(&self, fan: Fan) -> Result<(CurveSpec, Generated)> {
        let (spec, table, raised) = self
            .proxy
            .custom_fan_curve(fan.as_str())
            .await
            .map_err(|e| de(&e))?;
        Ok((
            CurveSpec::from_wire(&spec).ok_or_else(|| bad_reply("CustomFanCurve"))?,
            Generated::from_wire(&table, &raised).ok_or_else(|| bad_reply("CustomFanCurve"))?,
        ))
    }

    /// `SetFanCurve` (switches the selected profile to the custom fan profile).
    ///
    /// # Errors
    /// Mapped D-Bus errors, or a malformed reply.
    pub async fn set_fan_curve(&self, fan: Fan, spec: &CurveSpec) -> Result<Generated> {
        let (table, raised) = self
            .proxy
            .set_fan_curve(fan.as_str(), &spec.to_wire())
            .await
            .map_err(|e| de(&e))?;
        Generated::from_wire(&table, &raised).ok_or_else(|| bad_reply("SetFanCurve"))
    }

    /// `ResetFanCurve`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn reset_fan_curve(&self, fan: Fan) -> Result<()> {
        self.proxy.reset_fan_curve(fan.as_str()).await.map_err(|e| de(&e))
    }

    /// `PreviewFanCurve` (writes nothing).
    ///
    /// # Errors
    /// Mapped D-Bus errors, or a malformed reply.
    pub async fn preview_fan_curve(&self, spec: &CurveSpec) -> Result<Generated> {
        let (table, raised) = self
            .proxy
            .preview_fan_curve(&spec.to_wire())
            .await
            .map_err(|e| de(&e))?;
        Generated::from_wire(&table, &raised).ok_or_else(|| bad_reply("PreviewFanCurve"))
    }

    /// `CpuStages`: the staged CPU curve of the selected profile (ADR-0013). Daemons older than ADR-0013
    /// answer `UnknownMethod`, which [`AppError::from_zbus`] maps to `Unavailable`: callers that only want to
    /// know whether staged curves are offered treat any error as "not offered".
    ///
    /// # Errors
    /// Mapped D-Bus errors, or a malformed reply.
    pub async fn cpu_stages(&self) -> Result<StagedState> {
        let wire = self.proxy.cpu_stages().await.map_err(|e| de(&e))?;
        StagedState::from_wire(&wire).ok_or_else(|| bad_reply("CpuStages"))
    }

    /// `SetCpuStages` (switches the selected profile to the custom fan profile). Returns the number of S2
    /// floors that take over from the stages.
    ///
    /// # Errors
    /// Mapped D-Bus errors (`InvalidArgument` names the violated rule).
    pub async fn set_cpu_stages(&self, idle: u8, stages: &[StageWire]) -> Result<usize> {
        self.proxy
            .set_cpu_stages(idle, stages)
            .await
            .map(|floors| floors.len())
            .map_err(|e| de(&e))
    }

    /// `ResetCpuStages`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn reset_cpu_stages(&self) -> Result<()> {
        self.proxy.reset_cpu_stages().await.map_err(|e| de(&e))
    }

    /// `SetCoolerBoost`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn set_cooler_boost(&self, enabled: bool) -> Result<()> {
        self.proxy.set_cooler_boost(enabled).await.map_err(|e| de(&e))
    }

    /// `SetChargeLimit`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn set_charge_limit(&self, limit: u8) -> Result<()> {
        self.proxy.set_charge_limit(limit).await.map_err(|e| de(&e))
    }

    /// `GpuHolders`: `(pid, name)` of processes keeping the dGPU awake.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn gpu_holders(&self) -> Result<Vec<(u32, String)>> {
        self.proxy.gpu_holders().await.map_err(|e| de(&e))
    }

    /// Whether the daemon's interface has `method` (introspection). `false` on any error.
    pub async fn has_method(&self, method: &str) -> bool {
        let Ok(p) = zbus::fdo::IntrospectableProxy::builder(&self.conn)
            .destination(DAEMON_NAME)
            .path(DAEMON_PATH)
            .build()
            .await
        else {
            return false;
        };
        p.introspect()
            .await
            .is_ok_and(|xml| interface_has_method(&xml, DAEMON_NAME, method))
    }

    /// Calls the optional [`RESTORE_FACTORY`] method (no arguments).
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn restore_factory(&self) -> Result<()> {
        self.proxy
            .inner()
            .call_method(RESTORE_FACTORY, &())
            .await
            .map(|_| ())
            .map_err(|e| de(&e))
    }

    /// Streams telemetry, safety events and status updates until the connection closes.
    ///
    /// # Errors
    /// Failing to subscribe.
    pub async fn events(&self) -> Result<BoxStream<'static, DaemonEvent>> {
        let telemetry = self
            .proxy
            .receive_telemetry()
            .await
            .map_err(|e| de(&e))?
            .filter_map(|s| async move {
                s.args()
                    .ok()
                    .map(|a| DaemonEvent::Telemetry(model::Telemetry::from_map(a.data())))
            })
            .boxed();
        let safety = self
            .proxy
            .receive_safety_event()
            .await
            .map_err(|e| de(&e))?
            .filter_map(|s| async move {
                s.args().ok().map(|a| {
                    DaemonEvent::Safety(model::SafetyEvent {
                        kind: a.kind().clone(),
                        value: *a.value(),
                        detail: a.detail().clone(),
                    })
                })
            })
            .boxed();
        let me = self.clone();
        let status = self
            .props
            .receive_properties_changed()
            .await
            .map_err(|e| de(&e))?
            .filter_map(move |_| {
                let me = me.clone();
                async move { me.status().await.ok().map(DaemonEvent::Status) }
            })
            .boxed();
        Ok(stream::select_all([telemetry, safety, status]).boxed())
    }
}

/// Whether the introspection `xml` declares `method` in `interface`. A tiny scan, not an XML parser: the
/// daemon's introspection is generated by zbus and well-formed.
#[must_use]
pub fn interface_has_method(xml: &str, interface: &str, method: &str) -> bool {
    let open = format!("<interface name=\"{interface}\">");
    let Some(start) = xml.find(&open) else {
        return false;
    };
    let body = &xml[start + open.len()..];
    let body = body.find("</interface>").map_or(body, |end| &body[..end]);
    body.contains(&format!("<method name=\"{method}\">"))
        || body.contains(&format!("<method name=\"{method}\"/>"))
}

/// Client of clipd. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Clips {
    proxy: Clipd1Proxy<'static>,
}

/// One page of clips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipPage {
    /// Number of matches before pagination.
    pub total: u32,
    /// Clips of the page, most recently used first.
    pub clips: Vec<ClipEntry>,
}

impl Clips {
    /// Builds the client on `conn`.
    ///
    /// # Errors
    /// Proxy construction errors.
    pub async fn new(conn: &Connection) -> Result<Self> {
        let proxy = Clipd1Proxy::builder(conn)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .await
            .map_err(|e| ce(&e))?;
        Ok(Self { proxy })
    }

    /// The raw proxy.
    #[must_use]
    pub const fn proxy(&self) -> &Clipd1Proxy<'static> {
        &self.proxy
    }

    /// `List`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn list(
        &self,
        query: &str,
        tags: &[String],
        scope: Scope,
        offset: u32,
        limit: u32,
    ) -> Result<ClipPage> {
        let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
        let (total, clips) = self
            .proxy
            .list(query, &tags, scope.as_str(), offset, limit)
            .await
            .map_err(|e| ce(&e))?;
        Ok(ClipPage {
            total,
            clips: clips.into_iter().map(ClipEntry::from).collect(),
        })
    }

    /// `Get`: `(mime, data)`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn get(&self, id: u64) -> Result<(String, Vec<u8>)> {
        self.proxy.get(id).await.map_err(|e| ce(&e))
    }

    /// `Pin` or `Unpin`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn set_pinned(&self, id: u64, pinned: bool) -> Result<()> {
        if pinned {
            self.proxy.pin(id).await
        } else {
            self.proxy.unpin(id).await
        }
        .map_err(|e| ce(&e))
    }

    /// `Delete`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn delete(&self, ids: &[u64]) -> Result<u32> {
        self.proxy.delete(ids).await.map_err(|e| ce(&e))
    }

    /// `ClearUnpinned`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn clear_unpinned(&self) -> Result<u32> {
        self.proxy.clear_unpinned().await.map_err(|e| ce(&e))
    }

    /// `SetTags`: returns the normalised tags. Non-empty tags pin the clip.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn set_tags(&self, id: u64, tags: &[String]) -> Result<Vec<String>> {
        let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
        self.proxy.set_tags(id, &tags).await.map_err(|e| ce(&e))
    }

    /// `Tags`: `(tag, count)`.
    ///
    /// # Errors
    /// Mapped D-Bus errors.
    pub async fn tags(&self) -> Result<Vec<(String, u32)>> {
        self.proxy.tags().await.map_err(|e| ce(&e))
    }

    /// Streams `Changed` signals.
    ///
    /// # Errors
    /// Failing to subscribe.
    pub async fn changes(&self) -> Result<BoxStream<'static, ()>> {
        Ok(self
            .proxy
            .receive_changed()
            .await
            .map_err(|e| ce(&e))?
            .map(|_| ())
            .boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_methods_in_the_right_interface() {
        let xml = r#"<node>
 <interface name="org.freedesktop.DBus.Peer"><method name="Ping"/></interface>
 <interface name="org.blazorde.MsiCenter1">
  <method name="SetProfile"><arg name="profile" type="s" direction="in"/></method>
  <method name="RestoreFactory">
  </method>
 </interface>
</node>"#;
        assert!(interface_has_method(xml, DAEMON_NAME, "RestoreFactory"));
        assert!(interface_has_method(xml, DAEMON_NAME, "SetProfile"));
        assert!(!interface_has_method(xml, DAEMON_NAME, "Ping"));
        assert!(!interface_has_method(xml, DAEMON_NAME, "Restore"));
        assert!(!interface_has_method("<node/>", DAEMON_NAME, "SetProfile"));
    }
}
