//! Pure logic for the clipboard manager. No I/O.
//!
//! - [`model`]: clip identifiers, timestamps, metadata.
//! - [`content`]: MIME normalisation, size limits, validation of incoming content ([`Candidate`]).
//! - [`tags`]: tag normalisation ([`Tag`], [`TagSet`]).
//! - [`preview`]: safe text previews.
//! - [`query`]: search/filter/pagination model.
//! - [`retention`]: ordering and eviction rules.
//! - [`store`]: the [`ClipStore`] trait implemented by `clipd`'s storage, and an in-memory reference
//!   implementation ([`MemoryStore`]).
//! - [`library`]: [`Library`], which applies every rule (dedup, limits, retention, pinning, tagged clips
//!   pinned automatically) on top of any [`ClipStore`].
//!
//! Storage split (session tmpfs vs pinned SQLite) is described in `docs/adr/0005-clipboard-storage-split.md`.

#![forbid(unsafe_code)]

pub mod content;
pub mod library;
pub mod model;
pub mod preview;
pub mod query;
pub mod retention;
pub mod store;
pub mod tags;

pub use content::{Candidate, ContentHash, ContentKind, Limits, Mime, RejectReason};
pub use library::{AddKind, AddOutcome, Library, LibraryError, SetTagsOutcome};
pub use model::{ClipId, ClipMeta, Timestamp};
pub use query::{Page, Query, QueryError, Scope};
pub use store::{ClipStore, MemoryStore};
pub use tags::{Tag, TagError, TagSet};
