//! Tag normalisation.
//!
//! Rules: trim, lowercase, collapse inner whitespace to one space, at most [`MAX_TAG_CHARS`] characters,
//! no control characters. A [`TagSet`] is deduplicated and sorted, holds at most [`MAX_TAGS_PER_CLIP`]
//! tags, and silently skips blank entries (so a UI can split `"a, , b"` naively).

use std::collections::BTreeSet;
use std::fmt;

use thiserror::Error;

/// Maximum length of a tag, in characters (after normalisation).
pub const MAX_TAG_CHARS: usize = 32;
/// Maximum number of tags on one clip.
pub const MAX_TAGS_PER_CLIP: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TagError {
    #[error("tag is empty")]
    Empty,
    #[error("tag is longer than {MAX_TAG_CHARS} characters")]
    TooLong,
    #[error("tag contains control characters")]
    ControlChar,
    #[error("more than {MAX_TAGS_PER_CLIP} tags")]
    TooMany,
}

/// A normalised tag.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag(String);

impl Tag {
    /// Normalises `raw`.
    ///
    /// # Errors
    ///
    /// [`TagError::Empty`] if nothing is left after trimming, [`TagError::TooLong`],
    /// [`TagError::ControlChar`].
    pub fn parse(raw: &str) -> Result<Self, TagError> {
        if raw.chars().any(|c| c.is_control() && !c.is_whitespace()) {
            return Err(TagError::ControlChar);
        }
        let normalised = raw
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if normalised.is_empty() {
            return Err(TagError::Empty);
        }
        if normalised.chars().count() > MAX_TAG_CHARS {
            return Err(TagError::TooLong);
        }
        Ok(Self(normalised))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A sorted, deduplicated set of tags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TagSet(BTreeSet<Tag>);

impl TagSet {
    /// Normalises a list of raw tags, skipping blank ones.
    ///
    /// # Errors
    ///
    /// The first invalid tag's error, or [`TagError::TooMany`] if more than [`MAX_TAGS_PER_CLIP`]
    /// distinct tags remain.
    pub fn from_raw<S: AsRef<str>>(raw: &[S]) -> Result<Self, TagError> {
        let mut set = BTreeSet::new();
        for r in raw {
            match Tag::parse(r.as_ref()) {
                Ok(tag) => {
                    set.insert(tag);
                }
                Err(TagError::Empty) => {}
                Err(e) => return Err(e),
            }
        }
        if set.len() > MAX_TAGS_PER_CLIP {
            return Err(TagError::TooMany);
        }
        Ok(Self(set))
    }

    /// Builds a set from tags already normalised (e.g. read back from storage), without the count limit.
    pub fn from_tags(tags: impl IntoIterator<Item = Tag>) -> Self {
        Self(tags.into_iter().collect())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Tag> {
        self.0.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn contains(&self, tag: &Tag) -> bool {
        self.0.contains(tag)
    }

    /// True if every tag of `other` is in `self`.
    #[must_use]
    pub fn is_superset(&self, other: &Self) -> bool {
        self.0.is_superset(&other.0)
    }

    #[must_use]
    pub fn to_strings(&self) -> Vec<String> {
        self.0.iter().map(|t| t.0.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn tag_is_trimmed_lowercased_and_collapsed() {
        assert_eq!(
            Tag::parse("  Mots  de\tPasse ").unwrap().as_str(),
            "mots de passe"
        );
        assert_eq!(Tag::parse("ÉTÉ").unwrap().as_str(), "été");
    }

    #[test]
    fn tag_errors() {
        assert_eq!(Tag::parse("   "), Err(TagError::Empty));
        assert_eq!(Tag::parse("a\u{7}b"), Err(TagError::ControlChar));
        assert!(Tag::parse(&"é".repeat(MAX_TAG_CHARS)).is_ok());
        assert_eq!(Tag::parse(&"é".repeat(MAX_TAG_CHARS + 1)), Err(TagError::TooLong));
    }

    #[test]
    fn tagset_dedups_sorts_and_skips_blanks() {
        let set = TagSet::from_raw(&["Work", "  ", "work ", "code", ""]).unwrap();
        assert_eq!(set.to_strings(), vec!["code", "work"]);
    }

    #[test]
    fn tagset_limit() {
        let raw: Vec<String> = (0..=MAX_TAGS_PER_CLIP).map(|i| format!("t{i}")).collect();
        assert_eq!(TagSet::from_raw(&raw), Err(TagError::TooMany));
        assert!(TagSet::from_raw(&raw[..MAX_TAGS_PER_CLIP]).is_ok());
        // Duplicates do not count towards the limit.
        let dup: Vec<&str> = std::iter::repeat_n("same", 100).collect();
        assert_eq!(TagSet::from_raw(&dup).unwrap().len(), 1);
    }

    #[test]
    fn tagset_propagates_errors() {
        assert_eq!(TagSet::from_raw(&["ok", "a\u{0}"]), Err(TagError::ControlChar));
    }

    proptest! {
        #[test]
        fn normalisation_is_idempotent(raw in "\\PC{0,40}") {
            if let Ok(tag) = Tag::parse(&raw) {
                let again = Tag::parse(tag.as_str()).unwrap();
                prop_assert_eq!(&again, &tag);
                prop_assert!(tag.as_str().chars().count() <= MAX_TAG_CHARS);
                prop_assert_eq!(tag.as_str().trim(), tag.as_str());
            }
        }
    }
}
