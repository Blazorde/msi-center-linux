//! Incoming content: MIME normalisation, limits, hashing, validation.

use std::fmt;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::preview;

/// Default maximum number of unpinned clips kept per session (FIFO on last use).
pub const DEFAULT_MAX_UNPINNED: usize = 500;
/// Default maximum size of a single clip: 10 MiB. Larger content is rejected, not truncated.
pub const DEFAULT_MAX_CLIP_BYTES: usize = 10 * 1024 * 1024;
/// Default maximum number of characters of a text preview (an ellipsis is appended when cut).
pub const DEFAULT_PREVIEW_CHARS: usize = 200;
/// Maximum length of a MIME type, in bytes.
pub const MAX_MIME_LEN: usize = 127;

/// Limits applied by the [`crate::Library`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Unpinned clips beyond this count are evicted, least recently used first. Must be ≥ 1.
    pub max_unpinned: usize,
    /// Maximum content size in bytes, for text and images alike.
    pub max_clip_bytes: usize,
    /// Maximum characters in a text preview.
    pub preview_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_unpinned: DEFAULT_MAX_UNPINNED,
            max_clip_bytes: DEFAULT_MAX_CLIP_BYTES,
            preview_chars: DEFAULT_PREVIEW_CHARS,
        }
    }
}

/// Why incoming content was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RejectReason {
    #[error("content is empty")]
    Empty,
    #[error("content is {size} bytes, limit is {max}")]
    TooLarge { size: usize, max: usize },
    #[error("invalid MIME type")]
    InvalidMime,
    #[error("unsupported MIME type {0} (only text/* and image/* are stored)")]
    UnsupportedMime(String),
    #[error("text content is not valid UTF-8")]
    InvalidUtf8,
}

/// What kind of content a MIME type describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Text,
    Image,
}

/// A normalised MIME type: lowercase `type/subtype`, parameters (`; charset=…`) stripped.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mime(String);

impl Mime {
    /// Normalises and validates `raw`. Only `text/*` and `image/*` are accepted.
    ///
    /// # Errors
    ///
    /// [`RejectReason::InvalidMime`] if `raw` is not a syntactically valid MIME type,
    /// [`RejectReason::UnsupportedMime`] if it is neither text nor image.
    pub fn parse(raw: &str) -> Result<Self, RejectReason> {
        let essence = raw
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if essence.is_empty() || essence.len() > MAX_MIME_LEN {
            return Err(RejectReason::InvalidMime);
        }
        let Some((top, sub)) = essence.split_once('/') else {
            return Err(RejectReason::InvalidMime);
        };
        let is_token = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
        };
        if !is_token(top) || !is_token(sub) {
            return Err(RejectReason::InvalidMime);
        }
        if top != "text" && top != "image" {
            return Err(RejectReason::UnsupportedMime(essence));
        }
        Ok(Self(essence))
    }

    /// Wraps an already-normalised MIME type read back from storage, re-validating it.
    ///
    /// # Errors
    ///
    /// Same as [`Mime::parse`]; also fails if `stored` is not already in normal form.
    pub fn from_stored(stored: &str) -> Result<Self, RejectReason> {
        let mime = Self::parse(stored)?;
        if mime.0 == stored {
            Ok(mime)
        } else {
            Err(RejectReason::InvalidMime)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn kind(&self) -> ContentKind {
        if self.0.starts_with("text/") {
            ContentKind::Text
        } else {
            ContentKind::Image
        }
    }
}

impl fmt::Display for Mime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// SHA-256 of `mime ‖ 0x00 ‖ data`. Two clips with the same hash are the same clip (dedup).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    #[must_use]
    pub fn compute(mime: &Mime, data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(mime.as_str().as_bytes());
        hasher.update([0u8]);
        hasher.update(data);
        Self(hasher.finalize().into())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short form: enough to correlate log lines, not a content fingerprint worth leaking.
        for b in &self.0[..4] {
            write!(f, "{b:02x}")?;
        }
        f.write_str("…")
    }
}

/// Validated incoming content, ready to be stored.
#[derive(Clone, PartialEq, Eq)]
pub struct Candidate {
    pub mime: Mime,
    pub hash: ContentHash,
    pub data: Vec<u8>,
    pub preview: String,
    /// Lowercased full text used for search; `None` for images.
    pub search_text: Option<String>,
}

impl fmt::Debug for Candidate {
    // Never print clip content (it may be a password).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Candidate")
            .field("mime", &self.mime)
            .field("hash", &self.hash)
            .field("size", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl Candidate {
    /// Validates raw content against `limits`.
    ///
    /// # Errors
    ///
    /// See [`RejectReason`]: empty, too large, bad MIME type, or text that is not UTF-8.
    pub fn new(raw_mime: &str, data: Vec<u8>, limits: &Limits) -> Result<Self, RejectReason> {
        if data.is_empty() {
            return Err(RejectReason::Empty);
        }
        if data.len() > limits.max_clip_bytes {
            return Err(RejectReason::TooLarge {
                size: data.len(),
                max: limits.max_clip_bytes,
            });
        }
        let mime = Mime::parse(raw_mime)?;
        let (preview, search_text) = match mime.kind() {
            ContentKind::Text => {
                let text = std::str::from_utf8(&data).map_err(|_| RejectReason::InvalidUtf8)?;
                (
                    preview::text_preview(text, limits.preview_chars),
                    Some(text.to_lowercase()),
                )
            }
            ContentKind::Image => (String::new(), None),
        };
        let hash = ContentHash::compute(&mime, &data);
        Ok(Self {
            mime,
            hash,
            data,
            preview,
            search_text,
        })
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.data.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits::default()
    }

    #[test]
    fn mime_is_normalised() {
        let m = Mime::parse("  Text/Plain; charset=UTF-8 ").unwrap();
        assert_eq!(m.as_str(), "text/plain");
        assert_eq!(m.kind(), ContentKind::Text);
        assert_eq!(Mime::parse("image/PNG").unwrap().kind(), ContentKind::Image);
    }

    #[test]
    fn mime_rejects_garbage_and_unsupported() {
        assert_eq!(Mime::parse(""), Err(RejectReason::InvalidMime));
        assert_eq!(Mime::parse("text"), Err(RejectReason::InvalidMime));
        assert_eq!(Mime::parse("text/"), Err(RejectReason::InvalidMime));
        assert_eq!(Mime::parse("te xt/plain"), Err(RejectReason::InvalidMime));
        assert_eq!(
            Mime::parse(&format!("text/{}", "a".repeat(200))),
            Err(RejectReason::InvalidMime)
        );
        assert_eq!(
            Mime::parse("application/pdf"),
            Err(RejectReason::UnsupportedMime("application/pdf".into()))
        );
    }

    #[test]
    fn from_stored_requires_normal_form() {
        assert!(Mime::from_stored("text/plain").is_ok());
        assert_eq!(Mime::from_stored("Text/Plain"), Err(RejectReason::InvalidMime));
    }

    #[test]
    fn empty_content_rejected() {
        assert_eq!(
            Candidate::new("text/plain", vec![], &limits()),
            Err(RejectReason::Empty)
        );
    }

    #[test]
    fn size_limit_is_inclusive() {
        let max = DEFAULT_MAX_CLIP_BYTES;
        assert!(Candidate::new("image/png", vec![0; max], &limits()).is_ok());
        assert_eq!(
            Candidate::new("image/png", vec![0; max + 1], &limits()),
            Err(RejectReason::TooLarge { size: max + 1, max })
        );
    }

    #[test]
    fn text_must_be_utf8() {
        assert_eq!(
            Candidate::new("text/plain", vec![0xff, 0xfe], &limits()),
            Err(RejectReason::InvalidUtf8)
        );
    }

    #[test]
    fn image_has_no_preview_nor_search_text() {
        let c = Candidate::new("image/png", vec![1, 2, 3], &limits()).unwrap();
        assert_eq!(c.preview, "");
        assert!(c.search_text.is_none());
        assert_eq!(c.size(), 3);
    }

    #[test]
    fn text_search_text_is_lowercased() {
        let c = Candidate::new("text/plain", "Héllo ÉTÉ".into(), &limits()).unwrap();
        assert_eq!(c.search_text.as_deref(), Some("héllo été"));
    }

    #[test]
    fn hash_depends_on_mime_and_data_only() {
        let a = Candidate::new("text/plain", b"x".to_vec(), &limits()).unwrap();
        let b = Candidate::new("TEXT/plain;charset=utf-8", b"x".to_vec(), &limits()).unwrap();
        let c = Candidate::new("text/html", b"x".to_vec(), &limits()).unwrap();
        let d = Candidate::new("text/plain", b"y".to_vec(), &limits()).unwrap();
        assert_eq!(a.hash, b.hash);
        assert_ne!(a.hash, c.hash);
        assert_ne!(a.hash, d.hash);
    }

    #[test]
    fn debug_never_prints_content() {
        let c = Candidate::new("text/plain", b"hunter2".to_vec(), &limits()).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("hunter2"));
        // The hash is shortened to 4 bytes + ellipsis.
        assert_eq!(format!("{:?}", c.hash).chars().count(), 9);
    }
}
