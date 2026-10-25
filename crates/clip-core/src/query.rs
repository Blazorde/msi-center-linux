//! Search, filter and pagination model.
//!
//! - `text`: case-insensitive substring match on the full text of text clips (images never match a
//!   non-empty text). Matching is done on Unicode-lowercased strings (`str::to_lowercase`).
//! - `tags`: a clip matches if it carries **all** the given tags (AND).
//! - `scope`: all clips, only pinned, or only unpinned.
//! - Results are ordered most recently used first (see [`crate::retention::recency_desc`]).

use thiserror::Error;

use crate::model::ClipMeta;
use crate::tags::{TagError, TagSet};

/// Default page size when the caller passes `0`.
pub const DEFAULT_PAGE_SIZE: u32 = 50;
/// Largest page a caller may request; larger values are clamped.
pub const MAX_PAGE_SIZE: u32 = 1000;

/// Which tier(s) to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    Pinned,
    Unpinned,
}

impl Scope {
    /// Parses `"all"`, `"pinned"`, `"unpinned"` (or `""`, meaning all).
    ///
    /// # Errors
    ///
    /// [`QueryError::InvalidScope`] for anything else.
    pub fn parse(raw: &str) -> Result<Self, QueryError> {
        match raw {
            "" | "all" => Ok(Self::All),
            "pinned" => Ok(Self::Pinned),
            "unpinned" => Ok(Self::Unpinned),
            other => Err(QueryError::InvalidScope(other.to_owned())),
        }
    }

    /// `Some(pinned)` filter value, `None` for all.
    #[must_use]
    pub fn pinned_filter(self) -> Option<bool> {
        match self {
            Self::All => None,
            Self::Pinned => Some(true),
            Self::Unpinned => Some(false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QueryError {
    #[error("invalid tag filter: {0}")]
    Tag(#[from] TagError),
    #[error("invalid scope {0:?} (expected all, pinned or unpinned)")]
    InvalidScope(String),
}

/// A validated query.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    /// Lowercased needle; `None` when the search text is blank.
    pub needle: Option<String>,
    pub tags: TagSet,
    pub scope: Scope,
    pub offset: u32,
    pub limit: u32,
}

impl Query {
    /// Builds a query from raw caller input. `limit == 0` means [`DEFAULT_PAGE_SIZE`]; larger than
    /// [`MAX_PAGE_SIZE`] is clamped.
    ///
    /// # Errors
    ///
    /// [`QueryError::Tag`] if a tag in the filter is invalid.
    pub fn new<S: AsRef<str>>(
        text: &str,
        tags: &[S],
        scope: Scope,
        offset: u32,
        limit: u32,
    ) -> Result<Self, QueryError> {
        let trimmed = text.trim();
        let needle = (!trimmed.is_empty()).then(|| trimmed.to_lowercase());
        let limit = match limit {
            0 => DEFAULT_PAGE_SIZE,
            l => l.min(MAX_PAGE_SIZE),
        };
        Ok(Self {
            needle,
            tags: TagSet::from_raw(tags)?,
            scope,
            offset,
            limit,
        })
    }

    /// Whether `meta` (with its lowercased search text, if any) matches the filters. Ignores pagination.
    #[must_use]
    pub fn matches(&self, meta: &ClipMeta, search_text: Option<&str>) -> bool {
        if self
            .scope
            .pinned_filter()
            .is_some_and(|pinned| meta.pinned != pinned)
        {
            return false;
        }
        if let Some(needle) = &self.needle {
            match search_text {
                Some(hay) if hay.contains(needle.as_str()) => {}
                _ => return false,
            }
        }
        meta.tags.is_superset(&self.tags)
    }
}

/// One page of results.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    /// Number of clips matching the filters, regardless of pagination.
    pub total: u32,
    pub clips: Vec<ClipMeta>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{ContentHash, Mime};
    use crate::model::{ClipId, Timestamp};

    fn meta(pinned: bool, tags: &[&str]) -> ClipMeta {
        let mime = Mime::parse("text/plain").unwrap();
        ClipMeta {
            id: ClipId(1),
            hash: ContentHash::compute(&mime, b"x"),
            mime,
            size: 1,
            preview: String::new(),
            created: Timestamp(0),
            last_used: Timestamp(0),
            pinned,
            tags: TagSet::from_raw(tags).unwrap(),
        }
    }

    #[test]
    fn scope_parsing() {
        assert_eq!(Scope::parse("").unwrap(), Scope::All);
        assert_eq!(Scope::parse("pinned").unwrap(), Scope::Pinned);
        assert_eq!(Scope::parse("unpinned").unwrap(), Scope::Unpinned);
        assert!(Scope::parse("PINNED").is_err());
    }

    #[test]
    fn limit_defaults_and_clamps() {
        let none: [&str; 0] = [];
        assert_eq!(
            Query::new("", &none, Scope::All, 0, 0).unwrap().limit,
            DEFAULT_PAGE_SIZE
        );
        assert_eq!(Query::new("", &none, Scope::All, 0, 5).unwrap().limit, 5);
        assert_eq!(
            Query::new("", &none, Scope::All, 0, u32::MAX).unwrap().limit,
            MAX_PAGE_SIZE
        );
    }

    #[test]
    fn needle_is_trimmed_and_lowercased() {
        let none: [&str; 0] = [];
        assert_eq!(
            Query::new("  ÉTÉ ", &none, Scope::All, 0, 0)
                .unwrap()
                .needle
                .as_deref(),
            Some("été")
        );
        assert_eq!(Query::new("   ", &none, Scope::All, 0, 0).unwrap().needle, None);
    }

    #[test]
    fn text_match_is_case_insensitive_substring() {
        let none: [&str; 0] = [];
        let q = Query::new("WoRl", &none, Scope::All, 0, 0).unwrap();
        assert!(q.matches(&meta(false, &[]), Some("hello world")));
        assert!(!q.matches(&meta(false, &[]), Some("hello")));
        // Images (no search text) never match a needle.
        assert!(!q.matches(&meta(false, &[]), None));
    }

    #[test]
    fn tag_filter_is_and() {
        let q = Query::new("", &["a", "B"], Scope::All, 0, 0).unwrap();
        assert!(q.matches(&meta(false, &["a", "b", "c"]), None));
        assert!(!q.matches(&meta(false, &["a"]), None));
    }

    #[test]
    fn scope_filter() {
        let none: [&str; 0] = [];
        let pinned = Query::new("", &none, Scope::Pinned, 0, 0).unwrap();
        let unpinned = Query::new("", &none, Scope::Unpinned, 0, 0).unwrap();
        assert!(pinned.matches(&meta(true, &[]), None));
        assert!(!pinned.matches(&meta(false, &[]), None));
        assert!(unpinned.matches(&meta(false, &[]), None));
        assert!(!unpinned.matches(&meta(true, &[]), None));
    }

    #[test]
    fn invalid_tag_in_filter_is_an_error() {
        assert!(matches!(
            Query::new("", &["x".repeat(100)], Scope::All, 0, 0),
            Err(QueryError::Tag(TagError::TooLong))
        ));
    }
}
