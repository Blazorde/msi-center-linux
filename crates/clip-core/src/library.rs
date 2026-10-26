//! [`Library`]: every clipboard rule, on top of any [`ClipStore`].

use thiserror::Error;

use crate::content::{Candidate, Limits, Mime, RejectReason};
use crate::model::{ClipId, Timestamp};
use crate::query::{Page, Query, QueryError};
use crate::retention;
use crate::store::ClipStore;
use crate::tags::{Tag, TagError, TagSet};

#[derive(Debug, Error)]
pub enum LibraryError<E: std::error::Error + 'static> {
    #[error("content rejected: {0}")]
    Rejected(#[from] RejectReason),
    #[error("invalid tags: {0}")]
    Tag(#[from] TagError),
    #[error("invalid query: {0}")]
    Query(#[from] QueryError),
    #[error("no clip with id {0}")]
    NotFound(ClipId),
    #[error("storage error")]
    Store(#[source] E),
}

/// What [`Library::add`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddKind {
    /// New clip stored.
    Inserted,
    /// Identical content already existed (pinned or not): its `last_used` was bumped.
    Bumped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddOutcome {
    pub id: ClipId,
    pub kind: AddKind,
    /// Unpinned clips evicted to respect [`Limits::max_unpinned`].
    pub evicted: Vec<ClipId>,
}

/// What [`Library::set_tags`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetTagsOutcome {
    /// The tags now on the clip, normalised.
    pub tags: TagSet,
    /// `true` if this call pinned the clip (it was unpinned and received a non-empty tag set).
    pub pinned: bool,
}

/// Applies validation, dedup, retention and pinning rules over a [`ClipStore`].
#[derive(Debug)]
pub struct Library<S> {
    store: S,
    limits: Limits,
}

type Result<T, S> = std::result::Result<T, LibraryError<<S as ClipStore>::Error>>;

impl<S: ClipStore> Library<S> {
    /// `limits.max_unpinned` is raised to 1 if 0 (a just-added clip is never evicted).
    pub fn new(store: S, mut limits: Limits) -> Self {
        limits.max_unpinned = limits.max_unpinned.max(1);
        Self { store, limits }
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn into_store(self) -> S {
        self.store
    }

    /// Adds clipboard content. Identical content (same normalised MIME type and bytes) is bumped instead
    /// of duplicated, whether it is pinned or not.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Rejected`] if the content fails validation, [`LibraryError::Store`] on storage
    /// failure.
    pub fn add(&mut self, mime: &str, data: Vec<u8>, now: Timestamp) -> Result<AddOutcome, S> {
        let candidate = Candidate::new(mime, data, &self.limits)?;
        if let Some(id) = self
            .store
            .find_by_hash(&candidate.hash)
            .map_err(LibraryError::Store)?
        {
            self.store.touch(id, now).map_err(LibraryError::Store)?;
            return Ok(AddOutcome {
                id,
                kind: AddKind::Bumped,
                evicted: Vec::new(),
            });
        }
        let id = self.store.insert(&candidate, now).map_err(LibraryError::Store)?;
        let evicted = self.enforce_retention_protecting(Some(id))?;
        Ok(AddOutcome {
            id,
            kind: AddKind::Inserted,
            evicted,
        })
    }

    /// Evicts the least recently used unpinned clips beyond the limit. Pinned clips are never touched.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Store`] on storage failure.
    pub fn enforce_retention(&mut self) -> Result<Vec<ClipId>, S> {
        self.enforce_retention_protecting(None)
    }

    fn enforce_retention_protecting(&mut self, protect: Option<ClipId>) -> Result<Vec<ClipId>, S> {
        let count = self.store.unpinned_count().map_err(LibraryError::Store)?;
        let n = retention::excess(count, self.limits.max_unpinned);
        if n == 0 {
            return Ok(Vec::new());
        }
        let victims = self
            .store
            .oldest_unpinned(n, protect)
            .map_err(LibraryError::Store)?;
        self.store.delete(&victims).map_err(LibraryError::Store)?;
        Ok(victims)
    }

    /// # Errors
    ///
    /// [`LibraryError::Store`] on storage failure.
    pub fn list(&self, query: &Query) -> Result<Page, S> {
        self.store.list(query).map_err(LibraryError::Store)
    }

    /// # Errors
    ///
    /// [`LibraryError::NotFound`], [`LibraryError::Store`].
    pub fn get(&self, id: ClipId) -> Result<(Mime, Vec<u8>), S> {
        self.store
            .content(id)
            .map_err(LibraryError::Store)?
            .ok_or(LibraryError::NotFound(id))
    }

    /// Pins a clip: it becomes persistent and is never evicted. Idempotent.
    ///
    /// # Errors
    ///
    /// [`LibraryError::NotFound`], [`LibraryError::Store`].
    pub fn pin(&mut self, id: ClipId) -> Result<(), S> {
        if self.store.set_pinned(id, true).map_err(LibraryError::Store)? {
            Ok(())
        } else {
            Err(LibraryError::NotFound(id))
        }
    }

    /// Unpins a clip: it goes back to the session tier (erased at logout). Its tags are kept (an explicit
    /// unpin wins over the "tagged means pinned" rule of [`Self::set_tags`]). If that exceeds the limit,
    /// the oldest *other* unpinned clips are evicted. Idempotent.
    ///
    /// # Errors
    ///
    /// [`LibraryError::NotFound`], [`LibraryError::Store`].
    pub fn unpin(&mut self, id: ClipId) -> Result<Vec<ClipId>, S> {
        if !self.store.set_pinned(id, false).map_err(LibraryError::Store)? {
            return Err(LibraryError::NotFound(id));
        }
        self.enforce_retention_protecting(Some(id))
    }

    /// Replaces the tags of a clip with the normalised form of `raw`.
    ///
    /// Tagging means "keep this": a non-empty tag set on an unpinned clip **pins** it (as [`Self::pin`]
    /// does), so it survives logout. An empty tag set never unpins. [`Self::unpin`] stays explicit and
    /// keeps the tags. The clip is pinned before its tags are written, so a storage failure in between
    /// can leave it pinned with its old tags, never tagged but still erasable at logout.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Tag`] (nothing is changed), [`LibraryError::NotFound`], [`LibraryError::Store`].
    pub fn set_tags<T: AsRef<str>>(&mut self, id: ClipId, raw: &[T]) -> Result<SetTagsOutcome, S> {
        let tags = TagSet::from_raw(raw)?;
        let meta = self
            .store
            .meta(id)
            .map_err(LibraryError::Store)?
            .ok_or(LibraryError::NotFound(id))?;
        let pin = !tags.is_empty() && !meta.pinned;
        if pin {
            self.pin(id)?;
        }
        if self.store.set_tags(id, &tags).map_err(LibraryError::Store)? {
            Ok(SetTagsOutcome { tags, pinned: pin })
        } else {
            Err(LibraryError::NotFound(id))
        }
    }

    /// Deletes clips (pinned or not). Unknown ids are ignored. Returns how many were deleted.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Store`].
    pub fn delete(&mut self, ids: &[ClipId]) -> Result<u32, S> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.store.delete(ids).map_err(LibraryError::Store)
    }

    /// Deletes every unpinned clip.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Store`].
    pub fn clear_unpinned(&mut self) -> Result<u32, S> {
        self.store.delete_unpinned().map_err(LibraryError::Store)
    }

    /// # Errors
    ///
    /// [`LibraryError::Store`].
    pub fn tags(&self) -> Result<Vec<(Tag, u32)>, S> {
        self.store.tag_counts().map_err(LibraryError::Store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Scope;
    use crate::store::MemoryStore;
    use proptest::prelude::*;

    fn lib(max_unpinned: usize) -> Library<MemoryStore> {
        Library::new(
            MemoryStore::new(),
            Limits {
                max_unpinned,
                ..Limits::default()
            },
        )
    }

    fn all() -> Query {
        let none: [&str; 0] = [];
        Query::new("", &none, Scope::All, 0, 1000).unwrap()
    }

    fn ids(page: &Page) -> Vec<u64> {
        page.clips.iter().map(|c| c.id.0).collect()
    }

    #[test]
    fn add_then_get() {
        let mut l = lib(10);
        let out = l.add("text/plain", b"hello".to_vec(), Timestamp(1)).unwrap();
        assert_eq!(out.kind, AddKind::Inserted);
        let (mime, data) = l.get(out.id).unwrap();
        assert_eq!(mime.as_str(), "text/plain");
        assert_eq!(data, b"hello");
    }

    #[test]
    fn dedup_bumps_instead_of_duplicating() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(1)).unwrap();
        let b = l.add("text/plain", b"b".to_vec(), Timestamp(2)).unwrap();
        let again = l
            .add("text/plain; charset=utf-8", b"a".to_vec(), Timestamp(3))
            .unwrap();
        assert_eq!(again.id, a.id);
        assert_eq!(again.kind, AddKind::Bumped);
        let page = l.list(&all()).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(ids(&page), vec![a.id.0, b.id.0]);
        let meta = l.store().meta(a.id).unwrap().unwrap();
        assert_eq!(meta.created, Timestamp(1));
        assert_eq!(meta.last_used, Timestamp(3));
    }

    #[test]
    fn dedup_applies_to_pinned_clips() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(1)).unwrap();
        l.pin(a.id).unwrap();
        let again = l.add("text/plain", b"a".to_vec(), Timestamp(5)).unwrap();
        assert_eq!(again.id, a.id);
        let meta = l.store().meta(a.id).unwrap().unwrap();
        assert!(meta.pinned);
        assert_eq!(meta.last_used, Timestamp(5));
    }

    #[test]
    fn eviction_is_lru_and_spares_pinned() {
        let mut l = lib(3);
        let first = l.add("text/plain", b"0".to_vec(), Timestamp(0)).unwrap().id;
        l.pin(first).unwrap();
        let mut unpinned = Vec::new();
        for i in 1..=3u8 {
            unpinned.push(
                l.add("text/plain", vec![b'0' + i], Timestamp(i.into()))
                    .unwrap()
                    .id,
            );
        }
        // Bump the oldest unpinned so the second one becomes the eviction victim.
        l.add("text/plain", b"1".to_vec(), Timestamp(10)).unwrap();
        let out = l.add("text/plain", b"x".to_vec(), Timestamp(11)).unwrap();
        assert_eq!(out.evicted, vec![unpinned[1]]);
        assert_eq!(l.store().unpinned_count().unwrap(), 3);
        assert!(l.store().meta(first).unwrap().is_some());
    }

    #[test]
    fn unpin_never_evicts_the_unpinned_clip() {
        let mut l = lib(2);
        let old = l.add("text/plain", b"old".to_vec(), Timestamp(0)).unwrap().id;
        l.pin(old).unwrap();
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(1)).unwrap().id;
        let _b = l.add("text/plain", b"b".to_vec(), Timestamp(2)).unwrap().id;
        let evicted = l.unpin(old).unwrap();
        assert_eq!(evicted, vec![a]);
        assert!(!l.store().meta(old).unwrap().unwrap().pinned);
    }

    #[test]
    fn pin_unpin_unknown_is_not_found() {
        let mut l = lib(2);
        assert!(matches!(
            l.pin(ClipId(42)),
            Err(LibraryError::NotFound(ClipId(42)))
        ));
        assert!(matches!(l.unpin(ClipId(42)), Err(LibraryError::NotFound(_))));
        assert!(matches!(l.get(ClipId(42)), Err(LibraryError::NotFound(_))));
        assert!(matches!(
            l.set_tags(ClipId(42), &["a"]),
            Err(LibraryError::NotFound(_))
        ));
    }

    #[test]
    fn pinned_survive_logout_unpinned_do_not() {
        let mut l = lib(10);
        let keep = l.add("text/plain", b"keep".to_vec(), Timestamp(0)).unwrap().id;
        let lose = l.add("text/plain", b"lose".to_vec(), Timestamp(1)).unwrap().id;
        l.pin(keep).unwrap();
        let mut store = l.into_store();
        store.logout();
        let l = Library::new(store, Limits::default());
        assert!(l.get(keep).is_ok());
        assert!(matches!(l.get(lose), Err(LibraryError::NotFound(_))));
    }

    #[test]
    fn tags_are_normalised_and_counted() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(0)).unwrap().id;
        let b = l.add("text/plain", b"b".to_vec(), Timestamp(1)).unwrap().id;
        assert_eq!(
            l.set_tags(a, &[" Work", "code"]).unwrap().tags.to_strings(),
            vec!["code", "work"]
        );
        l.set_tags(b, &["WORK"]).unwrap();
        let counts: Vec<(String, u32)> = l
            .tags()
            .unwrap()
            .into_iter()
            .map(|(t, n)| (t.as_str().to_owned(), n))
            .collect();
        assert_eq!(counts, vec![("code".into(), 1), ("work".into(), 2)]);
        let q = Query::new("", &["work", "code"], Scope::All, 0, 0).unwrap();
        assert_eq!(ids(&l.list(&q).unwrap()), vec![a.0]);
        assert!(matches!(l.set_tags(a, &["x\u{1}"]), Err(LibraryError::Tag(_))));
    }

    #[test]
    fn tagging_an_unpinned_clip_pins_it() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(0)).unwrap().id;
        let out = l.set_tags(a, &["Work"]).unwrap();
        assert_eq!(out.tags.to_strings(), vec!["work"]);
        assert!(out.pinned, "the call reports that it pinned the clip");
        let meta = l.store().meta(a).unwrap().unwrap();
        assert!(meta.pinned);
        assert_eq!(meta.tags.to_strings(), vec!["work"]);
    }

    #[test]
    fn tagging_an_already_pinned_clip_reports_no_pin() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(0)).unwrap().id;
        l.pin(a).unwrap();
        let out = l.set_tags(a, &["work"]).unwrap();
        assert!(!out.pinned);
        assert!(l.store().meta(a).unwrap().unwrap().pinned);
    }

    #[test]
    fn tags_that_normalise_to_nothing_do_not_pin() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(0)).unwrap().id;
        let none: [&str; 0] = [];
        for raw in [&none[..], &["", "   "][..]] {
            let out = l.set_tags(a, raw).unwrap();
            assert!(out.tags.is_empty());
            assert!(!out.pinned);
            assert!(!l.store().meta(a).unwrap().unwrap().pinned);
        }
    }

    #[test]
    fn clearing_tags_does_not_unpin() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(0)).unwrap().id;
        l.set_tags(a, &["work"]).unwrap();
        let none: [&str; 0] = [];
        let out = l.set_tags(a, &none).unwrap();
        assert!(!out.pinned);
        let meta = l.store().meta(a).unwrap().unwrap();
        assert!(meta.pinned, "still pinned");
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn unpinning_a_tagged_clip_keeps_its_tags() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(0)).unwrap().id;
        l.set_tags(a, &["work"]).unwrap();
        l.unpin(a).unwrap();
        let meta = l.store().meta(a).unwrap().unwrap();
        assert!(!meta.pinned, "explicit unpin wins over the tag rule");
        assert_eq!(meta.tags.to_strings(), vec!["work"]);
        // Re-tagging is an explicit "keep this" again.
        assert!(l.set_tags(a, &["work"]).unwrap().pinned);
    }

    #[test]
    fn invalid_tags_neither_pin_nor_change_anything() {
        let mut l = lib(10);
        let a = l.add("text/plain", b"a".to_vec(), Timestamp(0)).unwrap().id;
        assert!(matches!(
            l.set_tags(a, &["ok", "x\u{1}"]),
            Err(LibraryError::Tag(_))
        ));
        let meta = l.store().meta(a).unwrap().unwrap();
        assert!(!meta.pinned);
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn tagged_clip_survives_logout() {
        let mut l = lib(10);
        let keep = l.add("text/plain", b"keep".to_vec(), Timestamp(0)).unwrap().id;
        let lose = l.add("text/plain", b"lose".to_vec(), Timestamp(1)).unwrap().id;
        l.set_tags(keep, &["work"]).unwrap();
        let mut store = l.into_store();
        store.logout();
        let l = Library::new(store, Limits::default());
        assert!(l.get(keep).is_ok());
        assert!(matches!(l.get(lose), Err(LibraryError::NotFound(_))));
    }

    #[test]
    fn bulk_delete_and_clear() {
        let mut lib = lib(10);
        let mut ids_added = Vec::new();
        for (i, v) in [b"a", b"b", b"c", b"d"].into_iter().enumerate() {
            let now = Timestamp(i64::try_from(i).unwrap());
            ids_added.push(lib.add("text/plain", v.to_vec(), now).unwrap().id);
        }
        let [first, second, third, last] = ids_added[..] else {
            unreachable!()
        };
        lib.pin(last).unwrap();
        assert_eq!(lib.delete(&[first, second, ClipId(999)]).unwrap(), 2);
        assert_eq!(lib.delete(&[]).unwrap(), 0);
        assert_eq!(lib.clear_unpinned().unwrap(), 1);
        assert_eq!(ids(&lib.list(&all()).unwrap()), vec![last.0]);
        assert!(lib.get(third).is_err());
    }

    #[test]
    fn rejected_content_is_not_stored() {
        let mut l = lib(10);
        assert!(matches!(
            l.add("application/x-secret", b"a".to_vec(), Timestamp(0)),
            Err(LibraryError::Rejected(RejectReason::UnsupportedMime(_)))
        ));
        assert_eq!(l.list(&all()).unwrap().total, 0);
    }

    #[test]
    fn pagination() {
        let mut l = lib(100);
        for i in 0..10u8 {
            l.add("text/plain", vec![b'a' + i], Timestamp(i.into())).unwrap();
        }
        let none: [&str; 0] = [];
        let q = Query::new("", &none, Scope::All, 3, 4).unwrap();
        let page = l.list(&q).unwrap();
        assert_eq!(page.total, 10);
        assert_eq!(ids(&page), vec![7, 6, 5, 4]);
    }

    #[derive(Debug, Clone)]
    enum Op {
        Add(u8),
        Pin(u8),
        Unpin(u8),
        Delete(u8),
        Tag(u8, u8),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => (0u8..20).prop_map(Op::Add),
            1 => (0u8..30).prop_map(Op::Pin),
            1 => (0u8..30).prop_map(Op::Unpin),
            1 => (0u8..30).prop_map(Op::Delete),
            1 => ((0u8..30), (0u8..4)).prop_map(|(i, t)| Op::Tag(i, t)),
        ]
    }

    proptest! {
        /// Invariants under any sequence of operations: unpinned count ≤ limit, no duplicate content,
        /// pinned clips are only removed by an explicit delete.
        #[test]
        fn invariants_hold(ops in prop::collection::vec(op(), 0..200), max in 1usize..6) {
            let mut l = lib(max);
            let mut pinned_alive = std::collections::BTreeSet::new();
            for (t, op) in ops.into_iter().enumerate() {
                let now = Timestamp(i64::try_from(t).unwrap());
                match op {
                    Op::Add(v) => { l.add("text/plain", vec![v], now).unwrap(); }
                    Op::Pin(i) => if l.pin(ClipId(i.into())).is_ok() { pinned_alive.insert(i); },
                    Op::Unpin(i) => if l.unpin(ClipId(i.into())).is_ok() { pinned_alive.remove(&i); },
                    Op::Delete(i) => { l.delete(&[ClipId(i.into())]).unwrap(); pinned_alive.remove(&i); }
                    // A non-empty tag set pins the clip.
                    Op::Tag(i, tag) => if l.set_tags(ClipId(i.into()), &[format!("t{tag}")]).is_ok() { pinned_alive.insert(i); },
                }
                prop_assert!(l.store().unpinned_count().unwrap() <= max);
                let page = l.list(&all()).unwrap();
                let mut hashes: Vec<_> = page.clips.iter().map(|c| c.hash).collect();
                hashes.sort();
                hashes.dedup();
                prop_assert_eq!(hashes.len(), page.clips.len());
                for i in &pinned_alive {
                    let meta = l.store().meta(ClipId((*i).into())).unwrap();
                    prop_assert!(meta.is_some_and(|m| m.pinned));
                }
            }
        }
    }
}
