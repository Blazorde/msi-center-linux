//! Curve library: fan curves the owner saves under a name and applies later, as many as wanted (owner request
//! 2026-11-19: « la possibilité de me faire des profils de courbes autant que je veux », ADR-0016).
//!
//! An entry is a snapshot of the curves in force in one performance profile: the CPU and GPU exponential
//! curves, plus the staged CPU curve when there is one. Applying it copies them into the custom curves of the
//! selected profile (fan profile Personnalisé). Every curve is validated like any other (S1–S3) when saved, when
//! loaded from disk and when applied; this module only validates the names.

use std::fmt;

/// Longest curve name, in characters.
pub const MAX_NAME_CHARS: usize = 40;

/// A validated curve name: trimmed, 1 to [`MAX_NAME_CHARS`] characters, no control characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CurveName(String);

/// Why a curve name was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CurveNameError {
    /// Empty once trimmed.
    #[error("curve name is empty")]
    Empty,
    /// Longer than [`MAX_NAME_CHARS`].
    #[error("curve name has {chars} characters, at most {MAX_NAME_CHARS}")]
    TooLong {
        /// Characters in the trimmed name.
        chars: usize,
    },
    /// Contains a control character (newline, tab…).
    #[error("curve name contains a control character")]
    Control,
}

impl CurveName {
    /// Validates `name` (surrounding whitespace is trimmed).
    ///
    /// # Errors
    /// [`CurveNameError`] when the trimmed name is empty, too long or contains a control character.
    pub fn new(name: &str) -> Result<Self, CurveNameError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CurveNameError::Empty);
        }
        let chars = name.chars().count();
        if chars > MAX_NAME_CHARS {
            return Err(CurveNameError::TooLong { chars });
        }
        if name.chars().any(char::is_control) {
            return Err(CurveNameError::Control);
        }
        Ok(Self(name.to_owned()))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CurveName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_trims_names() {
        assert_eq!(CurveName::new("  Nuit calme ").unwrap().as_str(), "Nuit calme");
        assert_eq!(
            CurveName::new("Gaming ❄ 80°").unwrap().to_string(),
            "Gaming ❄ 80°"
        );
        let longest = "é".repeat(MAX_NAME_CHARS);
        assert_eq!(CurveName::new(&longest).unwrap().as_str(), longest);
    }

    #[test]
    fn rejects_bad_names() {
        assert_eq!(CurveName::new(""), Err(CurveNameError::Empty));
        assert_eq!(CurveName::new("   "), Err(CurveNameError::Empty));
        assert_eq!(
            CurveName::new(&"a".repeat(MAX_NAME_CHARS + 1)),
            Err(CurveNameError::TooLong {
                chars: MAX_NAME_CHARS + 1
            })
        );
        assert_eq!(CurveName::new("a\nb"), Err(CurveNameError::Control));
        assert_eq!(CurveName::new("a\tb"), Err(CurveNameError::Control));
    }
}
