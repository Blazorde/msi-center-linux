//! Battery charge limit (`charge_control_end_threshold`).

use std::ops::RangeInclusive;

/// Accepted charge limits, in %. Below 50 % the battery would be uselessly small; the EC field is 0..=100.
pub const CHARGE_LIMIT_RANGE: RangeInclusive<u8> = 50..=100;

/// The charge limit is outside [`CHARGE_LIMIT_RANGE`] (S3: rejected, never clamped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("charge limit {value} % is outside 50..=100 %")]
pub struct ChargeLimitError {
    /// Rejected value.
    pub value: u8,
}

/// A validated charge limit (%).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChargeLimit(u8);

impl ChargeLimit {
    /// No limit (charge to 100 %).
    pub const FULL: Self = Self(100);

    /// Validates a charge limit.
    ///
    /// # Errors
    ///
    /// [`ChargeLimitError`] if `value` is outside [`CHARGE_LIMIT_RANGE`].
    pub fn new(value: u8) -> Result<Self, ChargeLimitError> {
        if CHARGE_LIMIT_RANGE.contains(&value) {
            Ok(Self(value))
        } else {
            Err(ChargeLimitError { value })
        }
    }

    /// Value in %.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_accepts_50_to_100_only() {
        assert_eq!(ChargeLimit::new(50).map(ChargeLimit::get), Ok(50));
        assert_eq!(ChargeLimit::new(100), Ok(ChargeLimit::FULL));
        for bad in [0, 49, 101, 255] {
            assert_eq!(ChargeLimit::new(bad), Err(ChargeLimitError { value: bad }));
        }
    }
}
