//! Storage abstraction and in-memory reference implementation.
//!
//! A [`ClipStore`] is dumb storage over two tiers (unpinned/session and pinned/persistent): it does not
//! validate, deduplicate or evict on its own. Those rules live in [`crate::Library`]. Implementations
//! must keep ids unique across both tiers and stable when a clip changes tier, and must order lists by
//! [`crate::retention::recency_desc`] and evictions oldest first.

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;

use crate::content::{Candidate, ContentHash, Mime};
use crate::model::{ClipId, ClipMeta, Timestamp};
use crate::query::{Page, Query};
use crate::retention;
use crate::tags::{Tag, TagSet};

/// Every method fails only with [`ClipStore::Error`], on storage failure; "not found" is reported in the
/// `Ok` value, never as an error.
#[allow(clippy::missing_errors_doc)] // Documented once above for every method.
pub trait ClipStore {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Id of the clip (in either tier) with this content hash.
    fn find_by_hash(&self, hash: &ContentHash) -> Result<Option<ClipId>, Self::Error>;
    /// Stores a new **unpinned** clip with `created = last_used = now`, no tags. The hash must not exist.
    fn insert(&mut self, clip: &Candidate, now: Timestamp) -> Result<ClipId, Self::Error>;
    /// Sets `last_used = now`. Returns `false` if the clip does not exist.
    fn touch(&mut self, id: ClipId, now: Timestamp) -> Result<bool, Self::Error>;
    fn meta(&self, id: ClipId) -> Result<Option<ClipMeta>, Self::Error>;
    fn content(&self, id: ClipId) -> Result<Option<(Mime, Vec<u8>)>, Self::Error>;
    fn list(&self, query: &Query) -> Result<Page, Self::Error>;
    /// Moves a clip (with its tags) to the pinned or unpinned tier, atomically. Idempotent.
    /// Returns `false` if the clip does not exist.
    fn set_pinned(&mut self, id: ClipId, pinned: bool) -> Result<bool, Self::Error>;
    /// Replaces the tags of a clip. Returns `false` if the clip does not exist.
    fn set_tags(&mut self, id: ClipId, tags: &TagSet) -> Result<bool, Self::Error>;
    /// Deletes clips in either tier; unknown ids are ignored. Returns how many were deleted.
    fn delete(&mut self, ids: &[ClipId]) -> Result<u32, Self::Error>;
    /// Deletes every unpinned clip. Returns how many were deleted.
    fn delete_unpinned(&mut self) -> Result<u32, Self::Error>;
    /// Every tag in use with its clip count, sorted by tag.
    fn tag_counts(&self) -> Result<Vec<(Tag, u32)>, Self::Error>;
    fn unpinned_count(&self) -> Result<usize, Self::Error>;
    /// The `n` least recently used unpinned clips, oldest first, excluding `protect`.
    fn oldest_unpinned(&self, n: usize, protect: Option<ClipId>) -> Result<Vec<ClipId>, Self::Error>;
}

#[derive(Debug, Clone)]
struct Entry {
    meta: ClipMeta,
    data: Vec<u8>,
    search_text: Option<String>,
}

/// In-memory [`ClipStore`]: reference model for tests and property tests.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    clips: BTreeMap<ClipId, Entry>,
    next_id: u64,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Simulates a logout: every unpinned clip disappears.
    pub fn logout(&mut self) {
        self.clips.retain(|_, e| e.meta.pinned);
    }
}

impl ClipStore for MemoryStore {
    type Error = Infallible;

    fn find_by_hash(&self, hash: &ContentHash) -> Result<Option<ClipId>, Infallible> {
        Ok(self
            .clips
            .values()
            .find(|e| &e.meta.hash == hash)
            .map(|e| e.meta.id))
    }

    fn insert(&mut self, clip: &Candidate, now: Timestamp) -> Result<ClipId, Infallible> {
        self.next_id += 1;
        let id = ClipId(self.next_id);
        let meta = ClipMeta {
            id,
            mime: clip.mime.clone(),
            hash: clip.hash,
            size: clip.size(),
            preview: clip.preview.clone(),
            created: now,
            last_used: now,
            pinned: false,
            tags: TagSet::default(),
        };
        self.clips.insert(
            id,
            Entry {
                meta,
                data: clip.data.clone(),
                search_text: clip.search_text.clone(),
            },
        );
        Ok(id)
    }

    fn touch(&mut self, id: ClipId, now: Timestamp) -> Result<bool, Infallible> {
        Ok(self.clips.get_mut(&id).map(|e| e.meta.last_used = now).is_some())
    }

    fn meta(&self, id: ClipId) -> Result<Option<ClipMeta>, Infallible> {
        Ok(self.clips.get(&id).map(|e| e.meta.clone()))
    }

    fn content(&self, id: ClipId) -> Result<Option<(Mime, Vec<u8>)>, Infallible> {
        Ok(self.clips.get(&id).map(|e| (e.meta.mime.clone(), e.data.clone())))
    }

    fn list(&self, query: &Query) -> Result<Page, Infallible> {
        let mut matching: Vec<&ClipMeta> = self
            .clips
            .values()
            .filter(|e| query.matches(&e.meta, e.search_text.as_deref()))
            .map(|e| &e.meta)
            .collect();
        matching.sort_by(|a, b| retention::recency_desc((a.last_used, a.id), (b.last_used, b.id)));
        let total = u32::try_from(matching.len()).unwrap_or(u32::MAX);
        let offset = usize::try_from(query.offset).unwrap_or(usize::MAX);
        let limit = usize::try_from(query.limit).unwrap_or(usize::MAX);
        let clips = matching.into_iter().skip(offset).take(limit).cloned().collect();
        Ok(Page { total, clips })
    }

    fn set_pinned(&mut self, id: ClipId, pinned: bool) -> Result<bool, Infallible> {
        Ok(self.clips.get_mut(&id).map(|e| e.meta.pinned = pinned).is_some())
    }

    fn set_tags(&mut self, id: ClipId, tags: &TagSet) -> Result<bool, Infallible> {
        Ok(self
            .clips
            .get_mut(&id)
            .map(|e| e.meta.tags = tags.clone())
            .is_some())
    }

    fn delete(&mut self, ids: &[ClipId]) -> Result<u32, Infallible> {
        let mut n = 0;
        for id in ids {
            if self.clips.remove(id).is_some() {
                n += 1;
            }
        }
        Ok(n)
    }

    fn delete_unpinned(&mut self) -> Result<u32, Infallible> {
        let before = self.clips.len();
        self.logout();
        Ok(u32::try_from(before - self.clips.len()).unwrap_or(u32::MAX))
    }

    fn tag_counts(&self) -> Result<Vec<(Tag, u32)>, Infallible> {
        let mut counts: HashMap<&Tag, u32> = HashMap::new();
        for e in self.clips.values() {
            for t in e.meta.tags.iter() {
                *counts.entry(t).or_default() += 1;
            }
        }
        let mut v: Vec<(Tag, u32)> = counts.into_iter().map(|(t, n)| (t.clone(), n)).collect();
        v.sort();
        Ok(v)
    }

    fn unpinned_count(&self) -> Result<usize, Infallible> {
        Ok(self.clips.values().filter(|e| !e.meta.pinned).count())
    }

    fn oldest_unpinned(&self, n: usize, protect: Option<ClipId>) -> Result<Vec<ClipId>, Infallible> {
        let unpinned = self
            .clips
            .values()
            .filter(|e| !e.meta.pinned)
            .map(|e| (e.meta.last_used, e.meta.id))
            .collect();
        Ok(retention::pick_evictions(unpinned, n, protect))
    }
}
