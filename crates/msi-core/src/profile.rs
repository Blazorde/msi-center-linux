//! User profiles and the target state each one maps to.

use std::fmt;
use std::str::FromStr;

use crate::curve::CurvePreset;
use crate::ec::ShiftMode;
use crate::power_limit::PowerLimits;

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

/// CPU power limits of Éco (ADR-0014): quiet and cool, on battery first.
pub const ECO_CPU_POWER: PowerLimits = preset(20, 35);

/// CPU power limits of Équilibré (ADR-0014): the 185H's base power with short bursts, no 160 W spikes.
pub const BALANCED_CPU_POWER: PowerLimits = preset(35, 60);

/// Compile-time checked preset (an invalid one fails the build).
const fn preset(pl1_w: u32, pl2_w: u32) -> PowerLimits {
    match PowerLimits::from_watts(pl1_w, pl2_w) {
        Ok(l) => l,
        Err(_) => panic!("invalid CPU power preset"),
    }
}

/// State the daemon applies for a profile, in order: curve → EC shift mode → CPU power limits → GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProfileTarget {
    /// Fan curve preset.
    pub curve: CurvePreset,
    /// EC shift mode.
    pub shift_mode: ShiftMode,
    /// dGPU power state.
    pub gpu: GpuMode,
    /// CPU package power limits, capped to the firmware's; `None` = the firmware's own (ADR-0014).
    pub cpu_power: Option<PowerLimits>,
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
                cpu_power: Some(ECO_CPU_POWER),
            },
            Self::Balanced => ProfileTarget {
                curve: CurvePreset::Progressive,
                shift_mode: ShiftMode::Comfort,
                gpu: GpuMode::RuntimeSuspend,
                cpu_power: Some(BALANCED_CPU_POWER),
            },
            Self::Gaming => ProfileTarget {
                curve: CurvePreset::Aggressive,
                shift_mode: ShiftMode::Turbo,
                gpu: GpuMode::Active,
                cpu_power: None,
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
#[allow(clippy::unwrap_used)]
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
                gpu: GpuMode::Removed,
                cpu_power: Some(ECO_CPU_POWER)
            }
        );
        assert_eq!(
            Profile::Balanced.target(),
            ProfileTarget {
                curve: CurvePreset::Progressive,
                shift_mode: ShiftMode::Comfort,
                gpu: GpuMode::RuntimeSuspend,
                cpu_power: Some(BALANCED_CPU_POWER)
            }
        );
        assert_eq!(
            Profile::Gaming.target(),
            ProfileTarget {
                curve: CurvePreset::Aggressive,
                shift_mode: ShiftMode::Turbo,
                gpu: GpuMode::Active,
                cpu_power: None
            }
        );
    }

    #[test]
    fn cpu_power_presets_lower_the_firmware_limits() {
        let firmware = PowerLimits::from_watts(110, 160).unwrap();
        assert_eq!(ECO_CPU_POWER, PowerLimits::from_watts(20, 35).unwrap());
        assert_eq!(BALANCED_CPU_POWER, PowerLimits::from_watts(35, 60).unwrap());
        for p in Profile::ALL {
            if let Some(l) = p.target().cpu_power {
                assert_eq!(l.capped(firmware), l, "{p}");
            }
        }
    }

    #[test]
    fn every_profile_curve_generates_a_safe_table() {
        for p in Profile::ALL {
            let generated = crate::table::generate(&p.target().curve.spec()).unwrap();
            // Only Eco (Silent, T1 = 88 °C) needs S2: its step entered at 85 °C is raised to 100 %.
            let raised: Vec<(usize, u8)> = generated
                .report
                .raised
                .iter()
                .map(|r| (r.index, r.floor.temp))
                .collect();
            let expected: &[(usize, u8)] = if p == Profile::Eco { &[(6, 85)] } else { &[] };
            assert_eq!(raised, expected, "{p}");
        }
    }
}
