//! Pure logic for MSI EC control. No I/O: everything here must be testable without the hardware.
//!
//! Safety rules this crate enforces are specified in `docs/hardware/fan-safety.md`:
//!
//! | Rule | Where |
//! |---|---|
//! | S1 monotonic tables | [`table::CurveTable::new`] |
//! | S2 speed floors | [`table::sample`] (raises and reports), [`table::CurveTable::new`] (rejects) |
//! | S3 reject out-of-bounds inputs | [`curve::FanCurveParams::new`], [`table::CurveLayout::new`], [`table::CurveTable::new`] |
//! | S4 Cooler Boost supervisor | [`supervisor::ThermalSupervisor`] (applied by `msi-daemon`) |
//! | S8 known EC firmware only | [`ec::EcConfig::supports_firmware`] |

pub mod curve;
pub mod ec;
pub mod profile;
pub mod supervisor;
pub mod table;
