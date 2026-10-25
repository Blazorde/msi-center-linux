//! Pure logic for MSI EC control. No I/O: everything here must be testable without the hardware.
//!
//! Safety rules this crate enforces are specified in `docs/hardware/fan-safety.md`.

pub mod curve;
