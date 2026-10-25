//! Ordering and eviction rules.
//!
//! Clips are ordered by `last_used` (re-copying bumps it), ties broken by id. Listing shows the most
//! recently used first; eviction removes the least recently used **unpinned** clips first ("FIFO" on
//! use order). Pinned clips are never evicted.

use std::cmp::Ordering;

use crate::model::{ClipId, Timestamp};

/// Ordering used for listing: most recently used first, then highest id first.
#[must_use]
pub fn recency_desc(a: (Timestamp, ClipId), b: (Timestamp, ClipId)) -> Ordering {
    b.cmp(&a)
}

/// How many unpinned clips must be evicted when `unpinned_count` are stored and `max` are allowed.
#[must_use]
pub fn excess(unpinned_count: usize, max: usize) -> usize {
    unpinned_count.saturating_sub(max)
}

/// Picks the `n` clips to evict among unpinned `(last_used, id)` pairs, oldest first, never picking
/// `protect`.
#[must_use]
pub fn pick_evictions(
    mut unpinned: Vec<(Timestamp, ClipId)>,
    n: usize,
    protect: Option<ClipId>,
) -> Vec<ClipId> {
    unpinned.retain(|(_, id)| Some(*id) != protect);
    unpinned.sort_unstable();
    unpinned.into_iter().take(n).map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excess_counts() {
        assert_eq!(excess(500, 500), 0);
        assert_eq!(excess(501, 500), 1);
        assert_eq!(excess(3, 500), 0);
    }

    #[test]
    fn evicts_oldest_first_with_id_tiebreak() {
        let v = vec![
            (Timestamp(30), ClipId(1)),
            (Timestamp(10), ClipId(3)),
            (Timestamp(10), ClipId(2)),
            (Timestamp(20), ClipId(4)),
        ];
        assert_eq!(pick_evictions(v.clone(), 2, None), vec![ClipId(2), ClipId(3)]);
        assert_eq!(pick_evictions(v, 2, Some(ClipId(2))), vec![ClipId(3), ClipId(4)]);
    }

    #[test]
    fn recency_orders_newest_first() {
        let mut v = [
            (Timestamp(1), ClipId(1)),
            (Timestamp(2), ClipId(2)),
            (Timestamp(2), ClipId(3)),
        ];
        v.sort_by(|a, b| recency_desc(*a, *b));
        assert_eq!(v.iter().map(|x| x.1.0).collect::<Vec<_>>(), vec![3, 2, 1]);
    }
}
