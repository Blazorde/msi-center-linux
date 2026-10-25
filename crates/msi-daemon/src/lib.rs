//! `msi-daemon` library: hardware access layers used by the `msi-daemon` binary.
//!
//! Everything reads and writes through an injectable [`fs::FsRoot`] so it can be tested against fake
//! sysfs/procfs trees.
#![forbid(unsafe_code)]

pub mod fs;
pub mod gpu;
pub mod telemetry;
