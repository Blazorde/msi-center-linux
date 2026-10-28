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
//! | Hybrid CPU fan control (ADR-0010): spike filter, live target, safe vector | [`hybrid::TempFilter`], [`hybrid::CpuFanPilot`], [`hybrid::hybrid_cpu_table`] |
//! | Staged CPU curve, any number of stages with a fall-back temperature each (ADR-0013); S1–S3 on input, S2 as a reported floor | [`staged::StagedCurve`], [`staged::StagedPilot`], [`hybrid::CpuCurve`], [`hybrid::hybrid_cpu_table_for`] |
//! | S9 fan stall detector | [`stall::StallDetector`] (applied by `msi-daemon`) |
//! | Fan profiles (Silencieux, Auto, Équilibré, Turbo, Personnalisé) and their curve source | [`fan_profile::FanProfile`] |
//! | CPU power limits per profile, never above the firmware's (ADR-0014) | [`power_limit::PowerLimits::capped`], [`profile::ProfileTarget::cpu_power`] |
//! | Factory tables (hand control back to the EC) | [`ec::EcConfig::factory_tables`], [`table::write_plan_raw`] |

pub mod battery;
pub mod curve;
pub mod ec;
pub mod fan_profile;
pub mod hybrid;
pub mod power_limit;
pub mod profile;
pub mod staged;
pub mod stall;
pub mod supervisor;
pub mod table;
