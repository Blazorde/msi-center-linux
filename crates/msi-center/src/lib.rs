//! `msi-center` library: everything that can be tested without a display — D-Bus clients, view-models,
//! formatting, chart geometry, autostart and preferences. The GTK shell lives in the binary (`src/main.rs`,
//! `src/ui/`).
#![forbid(unsafe_code)]

pub mod autostart;
pub mod backend;
pub mod chart;
pub mod curve;
pub mod dbus;
pub mod errors;
pub mod format;
pub mod gpu_env;
pub mod model;
pub mod settings;

/// Application id (also the D-Bus name of the app and the desktop file id).
pub const APP_ID: &str = "org.blazorde.MsiCenter";
