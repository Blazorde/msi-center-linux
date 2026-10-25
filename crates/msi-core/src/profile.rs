//! User profiles and the target state each one maps to.

use std::fmt;
use std::str::FromStr;

use crate::curve::CurvePreset;
use crate::ec::ShiftMode;

/// User-facing profile. Parsed from / displayed as `eco`, `balanced`, `gaming`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Profile {
    /// Battery first: quiet, dGPU off.
    Eco,
    /// Default.
    Balanced,
    /// Performance first.
    Gaming,
}

/// dGPU power state wanted by a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuMode {
    /// Removed from the PCI bus (0 W).
    Removed,
    /// On the bus, runtime PM allowed to suspend it (RTD3).
    RuntimeSuspend,
    /// Kept active.
    Active,
}

/// State the daemon applies for a profile, in order: curve → EC shift mode → GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProfileTarget {
    /// Fan curve preset.
    pub curve: CurvePreset,
    /// EC shift mode.
    pub shift_mode: ShiftMode,
    /// dGPU power state.
    pub gpu: GpuMode,
}

impl Profile {
    /// All profiles.
    pub const ALL: [Self; 3] = [Self::Eco, Self::Balanced, Self::Gaming];

    /// Stable name (D-Bus, CLI, config).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Eco => "eco",
            Self::Balanced => "balanced",
            Self::Gaming => "gaming",
        }
    }

    /// Target state of the profile.
    #[must_use]
    pub const fn target(self) -> ProfileTarget {
        match self {
            Self::Eco => ProfileTarget {
                curve: CurvePreset::Silent,
                shift_mode: ShiftMode::Eco,
                gpu: GpuMode::Removed,
            },
            Self::Balanced => ProfileTarget {
                curve: CurvePreset::Progressive,
                shift_mode: ShiftMode::Comfort,
                gpu: GpuMode::RuntimeSuspend,
            },
            Self::Gaming => ProfileTarget {
                curve: CurvePreset::Aggressive,
                shift_mode: ShiftMode::Turbo,
                gpu: GpuMode::Active,
            },
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The string is not one of `eco`, `balanced`, `gaming`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown profile {input:?}, expected eco, balanced or gaming")]
pub struct ParseProfileError {
    /// The rejected input.
    pub input: String,
}

impl FromStr for Profile {
    type Err = ParseProfileError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| ParseProfileError { input: s.to_owned() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_round_trip() {
        for p in Profile::ALL {
            assert_eq!(p.to_string().parse::<Profile>(), Ok(p));
        }
        assert_eq!("eco".parse(), Ok(Profile::Eco));
        assert_eq!("balanced".parse(), Ok(Profile::Balanced));
        assert_eq!("gaming".parse(), Ok(Profile::Gaming));
    }

    #[test]
    fn rejects_unknown_names() {
        for bad in ["", "Eco", " eco", "turbo", "silent"] {
            assert_eq!(
                bad.parse::<Profile>(),
                Err(ParseProfileError {
                    input: bad.to_owned()
                })
            );
        }
    }

    #[test]
    fn targets_match_the_fan_safety_presets() {
        assert_eq!(
            Profile::Eco.target(),
            ProfileTarget {
                curve: CurvePreset::Silent,
                shift_mode: ShiftMode::Eco,
                gpu: GpuMode::Removed
            }
        );
        assert_eq!(
            Profile::Balanced.target(),
            ProfileTarget {
                curve: CurvePreset::Progressive,
                shift_mode: ShiftMode::Comfort,
                gpu: GpuMode::RuntimeSuspend
            }
        );
        assert_eq!(
            Profile::Gaming.target(),
            ProfileTarget {
                curve: CurvePreset::Aggressive,
                shift_mode: ShiftMode::Turbo,
                gpu: GpuMode::Active
            }
        );
    }

    #[test]
    fn every_profile_curve_samples_to_a_safe_table() {
        let layout = crate::table::CurveLayout::default();
        for p in Profile::ALL {
            assert!(
                crate::table::sample(&p.target().curve.params(), &layout).is_ok(),
                "{p}"
            );
        }
    }
}
