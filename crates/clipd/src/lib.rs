//! `clipd`: user service storing clipboard history, exposed as `org.blazorde.Clipd1` on the session bus.
//!
//! Rules live in `clip-core`; this crate only does I/O: SQLite storage ([`store`]), locations
//! ([`paths`]) and D-Bus ([`service`]). See `docs/adr/0005-clipboard-storage-split.md`.

#![forbid(unsafe_code)]

pub mod migrations;
pub mod paths;
pub mod service;
pub mod store;

pub use paths::Paths;
pub use service::{BUS_NAME, Clipd, ClipdError, OBJECT_PATH};
pub use store::{SqliteStore, StoreError};
