//! Client proxy of `org.blazorde.MsiCenter1`, used by `msictl` and tested against the real interface.

use std::collections::HashMap;

use zbus::OwnedVariant;

pub use crate::service::{RaisedWire, SpecWire, TableWire};

/// Proxy of `org.blazorde.MsiCenter1` (generates `MsiCenter1Proxy`).
#[zbus::proxy(
    interface = "org.blazorde.MsiCenter1",
    default_service = "org.blazorde.MsiCenter1",
    default_path = "/org/blazorde/MsiCenter"
)]
pub trait MsiCenter1 {
    /// See the interface documentation.
    fn set_profile(&self, profile: &str) -> zbus::Result<()>;
    /// See the interface documentation.
    fn set_auto_profile_on_power(&self, enabled: bool) -> zbus::Result<()>;
    /// See the interface documentation.
    fn fan_curve(&self, fan: &str) -> zbus::Result<(SpecWire, TableWire, Vec<RaisedWire>, bool)>;
    /// See the interface documentation.
    fn set_fan_curve(&self, fan: &str, spec: &SpecWire) -> zbus::Result<(TableWire, Vec<RaisedWire>)>;
    /// See the interface documentation.
    fn reset_fan_curve(&self, fan: &str) -> zbus::Result<()>;
    /// See the interface documentation.
    fn preview_fan_curve(&self, spec: &SpecWire) -> zbus::Result<(TableWire, Vec<RaisedWire>)>;
    /// See the interface documentation.
    fn set_cooler_boost(&self, enabled: bool) -> zbus::Result<()>;
    /// See the interface documentation.
    fn restore_factory(&self) -> zbus::Result<()>;
    /// See the interface documentation.
    fn set_charge_limit(&self, limit: u8) -> zbus::Result<()>;
    /// See the interface documentation.
    fn gpu_holders(&self) -> zbus::Result<Vec<(u32, String)>>;
    /// See the interface documentation.
    #[zbus(signal)]
    fn telemetry(&self, data: HashMap<String, OwnedVariant>) -> zbus::Result<()>;
    /// See the interface documentation.
    #[zbus(signal)]
    fn safety_event(&self, kind: String, value: f64, detail: String) -> zbus::Result<()>;
    /// See the interface documentation.
    #[zbus(property)]
    fn profile(&self) -> zbus::Result<String>;
    /// See the interface documentation.
    #[zbus(property)]
    fn auto_profile_on_power(&self) -> zbus::Result<bool>;
    /// See the interface documentation.
    #[zbus(property)]
    fn on_ac_power(&self) -> zbus::Result<bool>;
    /// See the interface documentation.
    #[zbus(property)]
    fn fan_mode(&self) -> zbus::Result<String>;
    /// See the interface documentation.
    #[zbus(property)]
    fn cooler_boost(&self) -> zbus::Result<bool>;
    /// See the interface documentation.
    #[zbus(property)]
    fn boost_forced(&self) -> zbus::Result<bool>;
    /// See the interface documentation.
    #[zbus(property)]
    fn charge_limit(&self) -> zbus::Result<u8>;
    /// See the interface documentation.
    #[zbus(property)]
    fn gpu_state(&self) -> zbus::Result<String>;
    /// See the interface documentation.
    #[zbus(property)]
    fn read_only(&self) -> zbus::Result<bool>;
    /// See the interface documentation.
    #[zbus(property)]
    fn read_only_reason(&self) -> zbus::Result<String>;
    /// See the interface documentation.
    #[zbus(property)]
    fn firmware_version(&self) -> zbus::Result<String>;
}
