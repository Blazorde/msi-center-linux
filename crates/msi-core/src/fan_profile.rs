//! Fan profiles: what drives the fans, chosen per performance [`Profile`] (`docs/hardware/fan-safety.md`,
//! section « Profils de ventilation »).
//!
//! | Fan profile | Id | Curve |
//! |---|---|---|
//! | Silencieux | `silent` | [`CurvePreset::Silent`] |
//! | Auto | `auto` | EC factory tables, fan mode `auto` (the firmware drives the fans) |
//! | Équilibré | `balanced` | [`CurvePreset::Progressive`] |
//! | Turbo | `turbo` | [`CurvePreset::Aggressive`] |
//! | Personnalisé | `custom` | the owner's [`crate::curve::FanCurveSpec`] per fan (S1–S3 on input, S2 raises reported by [`crate::table::generate`]) |
//!
//! Each performance profile has a default fan profile ([`Profile::default_fan_profile`]): Éco → Silencieux,
//! Équilibré → Équilibré, Gaming → Turbo. Cooler Boost stays a separate toggle.

use std::fmt;
use std::str::FromStr;

use crate::curve::CurvePreset;
use crate::profile::Profile;

/// Fan profile. Parsed from / displayed as `silent`, `auto`, `balanced`, `turbo`, `custom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FanProfile {
    /// Silencieux: off at idle, late and gentle rise.
    Silent,
    /// Auto: the EC factory curve, driven by the firmware.
    Auto,
    /// Équilibré: gentle start, 100 % from 85 °C.
    Balanced,
    /// Turbo: rises early and hard, 100 % from 74 °C.
    Turbo,
    /// Personnalisé: the owner's curve per fan.
    Custom,
}

/// Where the tables of a fan profile come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FanCurveSource {
    /// EC factory tables in fan mode `auto` (no curve of ours, no hybrid CPU control).
    Factory,
    /// A documented preset.
    Preset(CurvePreset),
    /// The owner's curve of each fan.
    Custom,
}

impl FanProfile {
    /// All fan profiles, in display order.
    pub const ALL: [Self; 5] = [
        Self::Silent,
        Self::Auto,
        Self::Balanced,
        Self::Turbo,
        Self::Custom,
    ];

    /// Stable id (D-Bus, CLI, state file, `GApplication` actions).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Auto => "auto",
            Self::Balanced => "balanced",
            Self::Turbo => "turbo",
            Self::Custom => "custom",
        }
    }

    /// French label shown to the owner.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Silent => "Silencieux",
            Self::Auto => "Auto",
            Self::Balanced => "Équilibré",
            Self::Turbo => "Turbo",
            Self::Custom => "Personnalisé",
        }
    }

    /// One-line French description (narrow no-break spaces before `%` and `°C`).
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Silent => "Arrêté au repos, montée lente et tardive",
            Self::Auto => "Courbe d'usine MSI gérée par le contrôleur",
            Self::Balanced => "Démarrage doux, 100\u{202f}% dès 85\u{202f}°C",
            Self::Turbo => "Réagit tôt et fort, 100\u{202f}% dès 74\u{202f}°C",
            Self::Custom => "Votre courbe, planchers de sécurité inclus",
        }
    }

    /// Where the tables come from.
    #[must_use]
    pub const fn source(self) -> FanCurveSource {
        match self {
            Self::Silent => FanCurveSource::Preset(CurvePreset::Silent),
            Self::Auto => FanCurveSource::Factory,
            Self::Balanced => FanCurveSource::Preset(CurvePreset::Progressive),
            Self::Turbo => FanCurveSource::Preset(CurvePreset::Aggressive),
            Self::Custom => FanCurveSource::Custom,
        }
    }

    /// The preset of this fan profile, `None` for Auto and Custom.
    #[must_use]
    pub const fn preset(self) -> Option<CurvePreset> {
        match self.source() {
            FanCurveSource::Preset(p) => Some(p),
            FanCurveSource::Factory | FanCurveSource::Custom => None,
        }
    }
}

impl Profile {
    /// Fan profile used by this performance profile unless the owner picked another one.
    #[must_use]
    pub const fn default_fan_profile(self) -> FanProfile {
        match self {
            Self::Eco => FanProfile::Silent,
            Self::Balanced => FanProfile::Balanced,
            Self::Gaming => FanProfile::Turbo,
        }
    }
}

impl fmt::Display for FanProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The string is not a fan profile id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown fan profile {input:?}, expected silent, auto, balanced, turbo or custom")]
pub struct ParseFanProfileError {
    /// The rejected input.
    pub input: String,
}

impl FromStr for FanProfile {
    type Err = ParseFanProfileError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.as_str() == s)
            .ok_or_else(|| ParseFanProfileError { input: s.to_owned() })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::table::generate;

    #[test]
    fn ids_round_trip_and_are_stable() {
        let ids: Vec<&str> = FanProfile::ALL.iter().map(|p| p.as_str()).collect();
        assert_eq!(ids, ["silent", "auto", "balanced", "turbo", "custom"]);
        for p in FanProfile::ALL {
            assert_eq!(p.to_string().parse::<FanProfile>(), Ok(p));
        }
    }

    #[test]
    fn rejects_unknown_ids() {
        for bad in ["", "Silent", " auto", "eco", "aggressive", "progressive", "max"] {
            assert_eq!(
                bad.parse::<FanProfile>(),
                Err(ParseFanProfileError {
                    input: bad.to_owned()
                })
            );
        }
    }

    #[test]
    fn french_labels() {
        let labels: Vec<&str> = FanProfile::ALL.iter().map(|p| p.label()).collect();
        assert_eq!(
            labels,
            ["Silencieux", "Auto", "Équilibré", "Turbo", "Personnalisé"]
        );
        for p in FanProfile::ALL {
            assert_ne!(p.description(), "", "{p}");
        }
    }

    #[test]
    fn sources_map_to_the_documented_presets() {
        assert_eq!(FanProfile::Silent.preset(), Some(CurvePreset::Silent));
        assert_eq!(FanProfile::Balanced.preset(), Some(CurvePreset::Progressive));
        // Turbo is the Aggressive curve (owner decision 2026-10-26), not Cooler Boost.
        assert_eq!(FanProfile::Turbo.preset(), Some(CurvePreset::Aggressive));
        assert_eq!(FanProfile::Auto.source(), FanCurveSource::Factory);
        assert_eq!(FanProfile::Custom.source(), FanCurveSource::Custom);
        assert_eq!(FanProfile::Auto.preset(), None);
        assert_eq!(FanProfile::Custom.preset(), None);
    }

    #[test]
    fn defaults_per_profile_match_the_profile_presets() {
        assert_eq!(Profile::Eco.default_fan_profile(), FanProfile::Silent);
        assert_eq!(Profile::Balanced.default_fan_profile(), FanProfile::Balanced);
        assert_eq!(Profile::Gaming.default_fan_profile(), FanProfile::Turbo);
        for p in Profile::ALL {
            assert_eq!(p.default_fan_profile().preset(), Some(p.target().curve), "{p}");
        }
    }

    #[test]
    fn every_preset_fan_profile_generates_a_safe_table() {
        for fp in FanProfile::ALL {
            if let Some(preset) = fp.preset() {
                let g = generate(&preset.spec()).unwrap();
                assert_eq!(g.curve.speeds()[6], 100, "{fp}");
            }
        }
    }
}
