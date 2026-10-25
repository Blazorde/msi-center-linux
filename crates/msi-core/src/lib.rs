//! Pure logic for MSI EC control. No I/O: everything here must be testable without the hardware.
//!
//! Safety rules this crate enforces are specified in `docs/hardware/fan-safety.md`:
//!
//! | Rule | Where |
//! |---|---|
//! | S1 monotonic tables | [`table::StepCurve::new`], [`table::RawTables::kernel_check`] |
//! | S2 speed floors (ascending step) | [`table::generate`] (raises and reports), [`table::StepCurve::new`] (rejects) |
//! | S3 reject out-of-bounds inputs | [`curve::FanCurveParams::new`], [`curve::FanCurveSpec::new`], [`table::StepCurve::new`], [`battery::ChargeLimit::new`] |
//! | S4 Cooler Boost supervisor + spike filter | [`supervisor::ThermalSupervisor`] (applied by `msi-daemon`) |
//! | Ordered table writes (kernel `down < up`) | [`table::write_plan`] |
//! | S8 known EC firmware only | [`ec::EcConfig::supports_firmware`] |

pub mod battery;
pub mod curve;
pub mod ec;
pub mod profile;
pub mod supervisor;
pub mod table;
