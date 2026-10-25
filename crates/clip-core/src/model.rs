//! Clip identifiers, timestamps and metadata.

use std::fmt;

use crate::content::{ContentHash, Mime};
use crate::tags::TagSet;

/// Stable identifier of a clip. It does not change when a clip is pinned or unpinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClipId(pub u64);

impl fmt::Display for ClipId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Milliseconds since the Unix epoch. The caller supplies it: this crate never reads the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(pub i64);

/// Everything about a clip except its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipMeta {
    pub id: ClipId,
    pub mime: Mime,
    pub hash: ContentHash,
    /// Content size in bytes.
    pub size: u64,
    /// Short single-line text shown in lists. Empty for images.
    pub preview: String,
    pub created: Timestamp,
    /// Last time the clip was copied (re-copying identical content bumps it).
    pub last_used: Timestamp,
    /// Pinned clips are persistent and never evicted.
    pub pinned: bool,
    pub tags: TagSet,
}
