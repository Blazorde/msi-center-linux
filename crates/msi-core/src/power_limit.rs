//! CPU package power limits (Intel RAPL PL1 / PL2) per profile (ADR-0014).
//!
//! The firmware sets PL1 = 110 W / PL2 = 160 W on this laptop: the 185H then boosts in bursts up to 97–99 °C
//! (S4 forces Cooler Boost several times an hour while the CPU is mostly idle). A profile may **lower** the
//! limits; it never raises them above what the firmware set ([`PowerLimits::capped`]), so the worst case of a
//! wrong value is a slower CPU, never a hotter one.

use std::fmt;

/// Lowest accepted PL1: below, the CPU crawls (the 185H's minimum assured power is 35 W).
pub const MIN_PL1_W: u32 = 15;

/// Highest accepted value: above any 185H setting, guards against a unit mistake (W vs µW).
pub const MAX_W: u32 = 250;

/// The limits are out of bounds (S3: rejected, never clamped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("power limits PL1 {pl1_w} W / PL2 {pl2_w} W: expected 15 ≤ PL1 ≤ PL2 ≤ 250 W")]
pub struct PowerLimitError {
    /// Rejected PL1, in W.
    pub pl1_w: u32,
    /// Rejected PL2, in W.
    pub pl2_w: u32,
}

/// PL1 (long term) and PL2 (short term) of the CPU package, in µW (the RAPL sysfs unit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PowerLimits {
    /// Long-term limit (`constraint_0`, `long_term`), µW.
    pub pl1_uw: u64,
    /// Short-term limit (`constraint_1`, `short_term`), µW.
    pub pl2_uw: u64,
}

impl PowerLimits {
    /// Validates limits given in W.
    ///
    /// # Errors
    /// [`PowerLimitError`] unless `MIN_PL1_W ≤ pl1 ≤ pl2 ≤ MAX_W`.
    pub const fn from_watts(pl1_w: u32, pl2_w: u32) -> Result<Self, PowerLimitError> {
        if pl1_w < MIN_PL1_W || pl1_w > pl2_w || pl2_w > MAX_W {
            return Err(PowerLimitError { pl1_w, pl2_w });
        }
        Ok(Self {
            pl1_uw: pl1_w as u64 * 1_000_000,
            pl2_uw: pl2_w as u64 * 1_000_000,
        })
    }

    /// What is actually written: each limit at most the firmware's, and PL1 ≤ PL2. Never above `firmware`.
    #[must_use]
    pub fn capped(self, firmware: Self) -> Self {
        let pl2_uw = self.pl2_uw.min(firmware.pl2_uw);
        Self {
            pl1_uw: self.pl1_uw.min(firmware.pl1_uw).min(pl2_uw),
            pl2_uw,
        }
    }
}

impl fmt::Display for PowerLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PL1 {} W / PL2 {} W",
            self.pl1_uw / 1_000_000,
            self.pl2_uw / 1_000_000
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const FIRMWARE: PowerLimits = PowerLimits {
        pl1_uw: 110_000_000,
        pl2_uw: 160_000_000,
    };

    #[test]
    fn validates_bounds() {
        assert_eq!(
            PowerLimits::from_watts(35, 60),
            Ok(PowerLimits {
                pl1_uw: 35_000_000,
                pl2_uw: 60_000_000
            })
        );
        assert!(PowerLimits::from_watts(15, 15).is_ok());
        assert!(PowerLimits::from_watts(250, 250).is_ok());
        for (pl1, pl2) in [(14, 60), (61, 60), (35, 251), (0, 0)] {
            assert_eq!(
                PowerLimits::from_watts(pl1, pl2),
                Err(PowerLimitError {
                    pl1_w: pl1,
                    pl2_w: pl2
                })
            );
        }
    }

    #[test]
    fn capped_never_exceeds_the_firmware() {
        let firmwares = [
            FIRMWARE,
            PowerLimits::from_watts(45, 45).unwrap(),
            PowerLimits {
                pl1_uw: 110_000_000,
                pl2_uw: 30_000_000,
            },
        ];
        for pl1 in (MIN_PL1_W..=MAX_W).step_by(5) {
            for pl2 in (pl1..=MAX_W).step_by(5) {
                let wanted = PowerLimits::from_watts(pl1, pl2).unwrap();
                for fw in firmwares {
                    let c = wanted.capped(fw);
                    assert!(c.pl1_uw <= fw.pl1_uw && c.pl2_uw <= fw.pl2_uw, "{wanted} vs {fw}");
                    assert!(c.pl1_uw <= c.pl2_uw, "{wanted} vs {fw}");
                    assert!(
                        c.pl1_uw <= wanted.pl1_uw && c.pl2_uw <= wanted.pl2_uw,
                        "{wanted} vs {fw}"
                    );
                }
            }
        }
    }

    #[test]
    fn capped_keeps_lower_limits_unchanged() {
        let wanted = PowerLimits::from_watts(35, 60).unwrap();
        assert_eq!(wanted.capped(FIRMWARE), wanted);
    }

    #[test]
    fn displays_in_watts() {
        assert_eq!(FIRMWARE.to_string(), "PL1 110 W / PL2 160 W");
    }
}
