//! Owner settings persisted in `/var/lib/msi-center/state.toml` (systemd `StateDirectory=msi-center`).
//!
//! Every value is validated through `msi-core` on load (S3): an invalid entry is dropped with a warning, never
//! "fixed". A missing or unreadable file gives the defaults. Writes are atomic (temporary file + rename).
//!
//! Fan profiles (`[fan_profiles]`, version 2): the owner's choice per performance profile; absent = the
//! profile's default ([`Profile::default_fan_profile`]). A version 1 file has no `[fan_profiles]` table: every
//! profile with a custom curve then gets the `custom` fan profile, so it behaves exactly as before.
//!
//! Staged CPU curves (`[staged.<profile>]`, version 3, ADR-0013): one per performance profile, any number of
//! stages. A version 2 file has none and loads unchanged; an older daemon reading a version 3 file ignores the
//! table (the CPU then falls back to its exponential curve, which is always safe).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use msi_core::battery::ChargeLimit;
use msi_core::curve::{FanCurveParams, FanCurveSpec};
use msi_core::fan_profile::{FanCurveSource, FanProfile};
use msi_core::hybrid::CpuCurve;
use msi_core::profile::Profile;
use msi_core::staged::{Stage, StagedCurve};
use serde::{Deserialize, Serialize};

use crate::ec::Fan;

/// Default location of the state file.
pub const DEFAULT_STATE_PATH: &str = "/var/lib/msi-center/state.toml";

/// Validated owner settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Profile chosen by the owner (or by the AC switch).
    pub profile: Profile,
    /// Switch profile when AC is plugged/unplugged.
    pub auto_profile_on_power: bool,
    /// Profile used on AC when [`Settings::auto_profile_on_power`] is on.
    pub ac_profile: Profile,
    /// Profile used on battery when [`Settings::auto_profile_on_power`] is on.
    pub battery_profile: Profile,
    /// Charge limit to apply at start (`None` = leave the firmware value).
    pub charge_limit: Option<ChargeLimit>,
    /// Custom curves (fan profile `custom`) of one fan in one performance profile.
    pub curves: HashMap<(Profile, Fan), FanCurveSpec>,
    /// Fan profile chosen per performance profile, when it differs from its default.
    pub fan_profiles: HashMap<Profile, FanProfile>,
    /// Staged CPU curve (fan profile `custom`) of one performance profile. When present it replaces the
    /// exponential custom curve of the CPU fan; the GPU keeps its own.
    pub staged: HashMap<Profile, StagedCurve>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            profile: Profile::Balanced,
            auto_profile_on_power: false,
            ac_profile: Profile::Balanced,
            battery_profile: Profile::Eco,
            charge_limit: None,
            curves: HashMap::new(),
            fan_profiles: HashMap::new(),
            staged: HashMap::new(),
        }
    }
}

impl Settings {
    /// Fan profile of `profile`: the owner's choice, else its default.
    #[must_use]
    pub fn fan_profile(&self, profile: Profile) -> FanProfile {
        self.fan_profiles
            .get(&profile)
            .copied()
            .unwrap_or_else(|| profile.default_fan_profile())
    }

    /// Remembers `fan_profile` for `profile` (choosing the default forgets the override).
    pub fn set_fan_profile(&mut self, profile: Profile, fan_profile: FanProfile) {
        if fan_profile == profile.default_fan_profile() {
            self.fan_profiles.remove(&profile);
        } else {
            self.fan_profiles.insert(profile, fan_profile);
        }
    }

    /// Curve of `fan` in `profile` for its fan profile: the preset, or the custom curve. Under `auto` (the EC
    /// factory curve, nothing of ours applied) this is the preset of the default fan profile, unused.
    #[must_use]
    pub fn curve(&self, profile: Profile, fan: Fan) -> FanCurveSpec {
        match self.fan_profile(profile).source() {
            FanCurveSource::Preset(p) => p.spec(),
            FanCurveSource::Custom => self.custom_curve(profile, fan),
            FanCurveSource::Factory => profile.target().curve.spec(),
        }
    }

    /// Custom curve of `fan` in `profile` (fan profile `custom`): the saved one, else the preset of the
    /// profile's default fan profile (the editor's starting point).
    #[must_use]
    pub fn custom_curve(&self, profile: Profile, fan: Fan) -> FanCurveSpec {
        self.curves
            .get(&(profile, fan))
            .copied()
            .unwrap_or_else(|| profile.target().curve.spec())
    }

    /// `true` if `profile` uses the `custom` fan profile (so `fan` follows the owner's curve).
    #[must_use]
    pub fn is_custom(&self, profile: Profile, _fan: Fan) -> bool {
        self.fan_profile(profile) == FanProfile::Custom
    }

    /// Saved staged CPU curve of `profile`, in force or not (the editor's source).
    #[must_use]
    pub fn staged_curve(&self, profile: Profile) -> Option<&StagedCurve> {
        self.staged.get(&profile)
    }

    /// `true` when the CPU fan of `profile` follows the owner's staged curve: fan profile `custom` and a saved
    /// staged curve.
    #[must_use]
    pub fn is_staged(&self, profile: Profile) -> bool {
        self.fan_profile(profile) == FanProfile::Custom && self.staged.contains_key(&profile)
    }

    /// The curve that drives the live CPU target in `profile`: the staged curve when [`Self::is_staged`], else
    /// the exponential curve of its fan profile ([`Self::curve`]).
    #[must_use]
    pub fn cpu_curve(&self, profile: Profile) -> CpuCurve {
        match self.staged.get(&profile).filter(|_| self.is_staged(profile)) {
            Some(curve) => CpuCurve::Staged(curve.clone()),
            None => CpuCurve::Exponential(self.curve(profile, Fan::Cpu)),
        }
    }

    /// Profile for the given power source when the AC switch is on.
    #[must_use]
    pub fn profile_for_power(&self, on_ac: bool) -> Profile {
        if on_ac {
            self.ac_profile
        } else {
            self.battery_profile
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct CurveRecord {
    idle: u8,
    t0: u8,
    t1: u8,
    min: u8,
    max: u8,
    k: f64,
    up: Vec<u8>,
    hysteresis: u8,
}

impl CurveRecord {
    fn from_spec(spec: &FanCurveSpec) -> Self {
        let p = spec.params();
        Self {
            idle: spec.idle(),
            t0: p.t0(),
            t1: p.t1(),
            min: p.min(),
            max: p.max(),
            k: p.k(),
            up: spec.up().to_vec(),
            hysteresis: spec.hysteresis(),
        }
    }

    fn to_spec(&self) -> Result<FanCurveSpec, String> {
        let params =
            FanCurveParams::new(self.t0, self.t1, self.min, self.max, self.k).map_err(|e| e.to_string())?;
        let up: [u8; 6] = self
            .up
            .clone()
            .try_into()
            .map_err(|_| format!("{} up thresholds, expected 6", self.up.len()))?;
        FanCurveSpec::new(params, self.idle, up, self.hysteresis).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct StageRecord {
    up: u8,
    down: u8,
    speed: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StagedRecord {
    idle: u8,
    stages: Vec<StageRecord>,
}

impl StagedRecord {
    fn from_curve(curve: &StagedCurve) -> Self {
        Self {
            idle: curve.idle(),
            stages: curve
                .stages()
                .iter()
                .map(|s| StageRecord {
                    up: s.up,
                    down: s.down,
                    speed: s.speed,
                })
                .collect(),
        }
    }

    fn to_curve(&self) -> Result<StagedCurve, String> {
        let stages = self
            .stages
            .iter()
            .map(|s| Stage {
                up: s.up,
                down: s.down,
                speed: s.speed,
            })
            .collect();
        StagedCurve::new(self.idle, stages).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    version: u32,
    profile: Option<String>,
    auto_profile_on_power: Option<bool>,
    ac_profile: Option<String>,
    battery_profile: Option<String>,
    charge_limit: Option<u8>,
    #[serde(default)]
    curves: BTreeMap<String, BTreeMap<String, CurveRecord>>,
    /// Absent in version 1 files (migrated, see the module documentation).
    fan_profiles: Option<BTreeMap<String, String>>,
    /// Staged CPU curves, absent before version 3.
    #[serde(default)]
    staged: BTreeMap<String, StagedRecord>,
}

const VERSION: u32 = 3;

fn parse_profile(value: Option<&String>, key: &str, default: Profile, warnings: &mut Vec<String>) -> Profile {
    match value.map(|v| v.parse::<Profile>()) {
        None => default,
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            warnings.push(format!("{key}: {e}"));
            default
        }
    }
}

/// Parses a state file; returns the settings and a warning per dropped entry.
#[must_use]
pub fn parse(text: &str) -> (Settings, Vec<String>) {
    let mut warnings = Vec::new();
    let file: StateFile = match toml::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            warnings.push(format!("unreadable state file, using defaults: {e}"));
            return (Settings::default(), warnings);
        }
    };
    let d = Settings::default();
    let mut s = Settings {
        profile: parse_profile(file.profile.as_ref(), "profile", d.profile, &mut warnings),
        auto_profile_on_power: file.auto_profile_on_power.unwrap_or(d.auto_profile_on_power),
        ac_profile: parse_profile(
            file.ac_profile.as_ref(),
            "ac_profile",
            d.ac_profile,
            &mut warnings,
        ),
        battery_profile: parse_profile(
            file.battery_profile.as_ref(),
            "battery_profile",
            d.battery_profile,
            &mut warnings,
        ),
        charge_limit: None,
        curves: HashMap::new(),
        fan_profiles: HashMap::new(),
        staged: HashMap::new(),
    };
    if let Some(v) = file.charge_limit {
        match ChargeLimit::new(v) {
            Ok(l) => s.charge_limit = Some(l),
            Err(e) => warnings.push(format!("charge_limit: {e}")),
        }
    }
    for (profile_name, fans) in &file.curves {
        let Ok(profile) = profile_name.parse::<Profile>() else {
            warnings.push(format!("curves.{profile_name}: unknown profile"));
            continue;
        };
        for (fan_name, record) in fans {
            let Some(fan) = Fan::parse(fan_name) else {
                warnings.push(format!("curves.{profile_name}.{fan_name}: unknown fan"));
                continue;
            };
            match record.to_spec() {
                Ok(spec) => {
                    s.curves.insert((profile, fan), spec);
                }
                Err(e) => warnings.push(format!("curves.{profile_name}.{fan_name}: {e}")),
            }
        }
    }
    for (profile_name, record) in &file.staged {
        let Ok(profile) = profile_name.parse::<Profile>() else {
            warnings.push(format!("staged.{profile_name}: unknown profile"));
            continue;
        };
        match record.to_curve() {
            Ok(curve) => {
                s.staged.insert(profile, curve);
            }
            Err(e) => warnings.push(format!("staged.{profile_name}: {e}")),
        }
    }
    if let Some(map) = &file.fan_profiles {
        for (profile_name, value) in map {
            let Ok(profile) = profile_name.parse::<Profile>() else {
                warnings.push(format!("fan_profiles.{profile_name}: unknown profile"));
                continue;
            };
            match value.parse::<FanProfile>() {
                Ok(fp) => s.set_fan_profile(profile, fp),
                Err(e) => warnings.push(format!("fan_profiles.{profile_name}: {e}")),
            }
        }
    } else {
        // Version 1: a custom curve overrode the profile's preset → keep it in force as `custom`.
        let migrated: Vec<Profile> = s.curves.keys().map(|&(p, _)| p).collect();
        for p in migrated {
            s.set_fan_profile(p, FanProfile::Custom);
        }
    }
    (s, warnings)
}

/// Serialises settings (deterministic order).
///
/// # Errors
/// Serialisation errors (should not happen).
pub fn render(s: &Settings) -> Result<String, toml::ser::Error> {
    let mut curves: BTreeMap<String, BTreeMap<String, CurveRecord>> = BTreeMap::new();
    for ((profile, fan), spec) in &s.curves {
        curves
            .entry(profile.as_str().to_owned())
            .or_default()
            .insert(fan.as_str().to_owned(), CurveRecord::from_spec(spec));
    }
    let fan_profiles = s
        .fan_profiles
        .iter()
        .map(|(p, fp)| (p.as_str().to_owned(), fp.as_str().to_owned()))
        .collect();
    let file = StateFile {
        version: VERSION,
        profile: Some(s.profile.as_str().to_owned()),
        auto_profile_on_power: Some(s.auto_profile_on_power),
        ac_profile: Some(s.ac_profile.as_str().to_owned()),
        battery_profile: Some(s.battery_profile.as_str().to_owned()),
        charge_limit: s.charge_limit.map(ChargeLimit::get),
        curves,
        fan_profiles: Some(fan_profiles),
        staged: s
            .staged
            .iter()
            .map(|(p, c)| (p.as_str().to_owned(), StagedRecord::from_curve(c)))
            .collect(),
    };
    toml::to_string(&file)
}

/// The state file on disk.
#[derive(Debug, Clone)]
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    /// Store at `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path of the file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the settings; a missing file gives the defaults without warning.
    #[must_use]
    pub fn load(&self) -> (Settings, Vec<String>) {
        match fs::read_to_string(&self.path) {
            Ok(text) => parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Settings::default(), Vec::new()),
            Err(e) => (
                Settings::default(),
                vec![format!("{}: {e}, using defaults", self.path.display())],
            ),
        }
    }

    /// Saves atomically (`<file>.tmp` then rename).
    ///
    /// # Errors
    /// I/O or serialisation errors.
    pub fn save(&self, settings: &Settings) -> io::Result<()> {
        let text = render(settings).map_err(io::Error::other)?;
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use msi_core::curve::CurvePreset;

    fn custom() -> FanCurveSpec {
        FanCurveSpec::new(
            FanCurveParams::new(45, 80, 30, 100, 2.5).unwrap(),
            0,
            [45, 52, 60, 67, 73, 80],
            3,
        )
        .unwrap()
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (s, w) = StateStore::new(dir.path().join("state.toml")).load();
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 0);
        assert_eq!(s.curve(Profile::Eco, Fan::Cpu), CurvePreset::Silent.spec());
    }

    #[test]
    fn round_trips_every_setting() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("sub/state.toml"));
        let mut s = Settings {
            profile: Profile::Gaming,
            auto_profile_on_power: true,
            ac_profile: Profile::Gaming,
            battery_profile: Profile::Balanced,
            charge_limit: Some(ChargeLimit::new(80).unwrap()),
            curves: HashMap::new(),
            fan_profiles: HashMap::new(),
            staged: HashMap::new(),
        };
        s.curves.insert((Profile::Eco, Fan::Gpu), custom());
        s.set_fan_profile(Profile::Eco, FanProfile::Custom);
        s.set_fan_profile(Profile::Gaming, FanProfile::Auto);
        store.save(&s).unwrap();
        let (loaded, w) = store.load();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(loaded, s);
        assert!(loaded.is_custom(Profile::Eco, Fan::Gpu));
        assert!(!loaded.is_custom(Profile::Balanced, Fan::Gpu));
        assert_eq!(loaded.fan_profile(Profile::Gaming), FanProfile::Auto);
        assert_eq!(loaded.curve(Profile::Eco, Fan::Gpu), custom());
        assert_eq!(loaded.curve(Profile::Eco, Fan::Cpu), CurvePreset::Silent.spec());
        assert!(!dir.path().join("sub/state.toml.tmp").exists());
    }

    #[test]
    fn s3_invalid_entries_are_dropped_with_a_warning() {
        let (s, w) = parse(
            r#"
            profile = "turbo"
            charge_limit = 20
            auto_profile_on_power = true
            [curves.eco.cpu]
            idle = 0
            t0 = 50
            t1 = 40
            min = 30
            max = 100
            k = 2.0
            up = [50, 57, 64, 70, 76, 82]
            hysteresis = 4
            [curves.eco.fan3]
            idle = 0
            t0 = 50
            t1 = 82
            min = 30
            max = 100
            k = 2.0
            up = [50, 57, 64, 70, 76, 82]
            hysteresis = 4
            "#,
        );
        assert_eq!(s.profile, Profile::Balanced);
        assert_eq!(s.charge_limit, None);
        assert!(s.auto_profile_on_power);
        assert_eq!(s.curves.len(), 0);
        assert_eq!(w.len(), 4, "{w:?}");
    }

    #[test]
    fn fan_profiles_default_per_profile_and_overrides_are_remembered() {
        let mut s = Settings::default();
        assert_eq!(s.fan_profile(Profile::Eco), FanProfile::Silent);
        assert_eq!(s.fan_profile(Profile::Balanced), FanProfile::Balanced);
        assert_eq!(s.fan_profile(Profile::Gaming), FanProfile::Turbo);
        s.set_fan_profile(Profile::Eco, FanProfile::Turbo);
        assert_eq!(s.fan_profile(Profile::Eco), FanProfile::Turbo);
        assert_eq!(s.curve(Profile::Eco, Fan::Cpu), CurvePreset::Aggressive.spec());
        // The other profiles keep their own choice.
        assert_eq!(s.fan_profile(Profile::Gaming), FanProfile::Turbo);
        // Choosing the default forgets the override.
        s.set_fan_profile(Profile::Eco, FanProfile::Silent);
        assert!(s.fan_profiles.is_empty());
    }

    #[test]
    fn custom_fan_profile_uses_the_saved_curve_or_the_default_preset() {
        let mut s = Settings::default();
        s.curves.insert((Profile::Gaming, Fan::Gpu), custom());
        // Saved but not selected: Turbo stays in force.
        assert_eq!(s.curve(Profile::Gaming, Fan::Gpu), CurvePreset::Aggressive.spec());
        assert!(!s.is_custom(Profile::Gaming, Fan::Gpu));
        s.set_fan_profile(Profile::Gaming, FanProfile::Custom);
        assert_eq!(s.curve(Profile::Gaming, Fan::Gpu), custom());
        assert_eq!(s.curve(Profile::Gaming, Fan::Cpu), CurvePreset::Aggressive.spec());
        assert!(s.is_custom(Profile::Gaming, Fan::Cpu));
        assert_eq!(s.custom_curve(Profile::Eco, Fan::Cpu), CurvePreset::Silent.spec());
    }

    #[test]
    fn version_1_custom_curves_migrate_to_the_custom_fan_profile() {
        let (s, w) = parse(
            r#"
            version = 1
            profile = "eco"
            [curves.eco.gpu]
            idle = 0
            t0 = 45
            t1 = 80
            min = 30
            max = 100
            k = 2.5
            up = [45, 52, 60, 67, 73, 80]
            hysteresis = 3
            "#,
        );
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.fan_profile(Profile::Eco), FanProfile::Custom);
        assert_eq!(s.curve(Profile::Eco, Fan::Gpu), custom());
        // The CPU of Éco had no custom curve: still its preset, as before.
        assert_eq!(s.curve(Profile::Eco, Fan::Cpu), CurvePreset::Silent.spec());
        assert_eq!(s.fan_profile(Profile::Gaming), FanProfile::Turbo);
        // Saved again as version 2, the choice is explicit and survives.
        let (again, w) = parse(&render(&s).unwrap());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(again, s);
    }

    #[test]
    fn version_2_without_overrides_does_not_migrate_saved_curves() {
        let mut s = Settings::default();
        s.curves.insert((Profile::Eco, Fan::Cpu), custom());
        let text = render(&s).unwrap();
        assert!(text.contains("fan_profiles"), "{text}");
        let (loaded, w) = parse(&text);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(loaded.fan_profile(Profile::Eco), FanProfile::Silent);
        assert_eq!(loaded, s);
    }

    #[test]
    fn invalid_fan_profiles_are_dropped_with_a_warning() {
        let (s, w) = parse(
            r#"
            [fan_profiles]
            eco = "max"
            turbo = "auto"
            gaming = "auto"
            "#,
        );
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(s.fan_profile(Profile::Eco), FanProfile::Silent);
        assert_eq!(s.fan_profile(Profile::Gaming), FanProfile::Auto);
    }

    fn staged() -> StagedCurve {
        StagedCurve::new(
            0,
            vec![
                Stage {
                    up: 50,
                    down: 46,
                    speed: 30,
                },
                Stage {
                    up: 58,
                    down: 52,
                    speed: 38,
                },
                Stage {
                    up: 66,
                    down: 60,
                    speed: 55,
                },
                Stage {
                    up: 74,
                    down: 68,
                    speed: 75,
                },
                Stage {
                    up: 80,
                    down: 72,
                    speed: 90,
                },
                Stage {
                    up: 85,
                    down: 78,
                    speed: 100,
                },
                Stage {
                    up: 90,
                    down: 82,
                    speed: 100,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn staged_curves_round_trip_with_any_number_of_stages() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("state.toml"));
        let mut s = Settings::default();
        s.staged.insert(Profile::Gaming, staged());
        // The widest curve: one stage per degree.
        let widest = StagedCurve::new(
            0,
            (22..=100u8)
                .map(|t| Stage {
                    up: t,
                    down: t - 2,
                    speed: 100,
                })
                .collect(),
        )
        .unwrap();
        s.staged.insert(Profile::Eco, widest.clone());
        s.set_fan_profile(Profile::Gaming, FanProfile::Custom);
        store.save(&s).unwrap();
        let text = std::fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("version = 3"), "{text}");
        let (loaded, w) = store.load();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(loaded, s);
        assert_eq!(loaded.staged_curve(Profile::Eco), Some(&widest));
        assert_eq!(
            loaded.staged_curve(Profile::Gaming).map(|c| c.stages().len()),
            Some(7)
        );
    }

    #[test]
    fn staged_curve_drives_the_cpu_only_under_the_custom_fan_profile() {
        let mut s = Settings::default();
        s.staged.insert(Profile::Gaming, staged());
        // Saved but not selected: Turbo's exponential curve stays in force.
        assert!(!s.is_staged(Profile::Gaming));
        assert_eq!(
            s.cpu_curve(Profile::Gaming),
            CpuCurve::Exponential(CurvePreset::Aggressive.spec())
        );
        s.set_fan_profile(Profile::Gaming, FanProfile::Custom);
        assert!(s.is_staged(Profile::Gaming));
        assert_eq!(s.cpu_curve(Profile::Gaming), CpuCurve::Staged(staged()));
        // The GPU does not follow it: it keeps its own exponential curve.
        assert_eq!(s.curve(Profile::Gaming, Fan::Gpu), CurvePreset::Aggressive.spec());
        // Another profile is untouched.
        assert!(!s.is_staged(Profile::Eco));
        assert_eq!(
            s.cpu_curve(Profile::Eco),
            CpuCurve::Exponential(CurvePreset::Silent.spec())
        );
    }

    #[test]
    fn s3_invalid_staged_curves_are_dropped_with_a_warning() {
        let (s, w) = parse(
            r"
            [staged.eco]
            idle = 0
            stages = [{ up = 60, down = 50, speed = 40 }, { up = 55, down = 52, speed = 50 }]
            [staged.gaming]
            idle = 50
            stages = [{ up = 60, down = 50, speed = 40 }]
            [staged.turbo]
            idle = 0
            stages = [{ up = 60, down = 50, speed = 40 }]
            [staged.balanced]
            idle = 0
            stages = []
            ",
        );
        assert!(s.staged.is_empty());
        assert_eq!(w.len(), 4, "{w:?}");
        assert!(w.iter().any(|m| m.contains("strictly increasing")), "{w:?}");
        assert!(w.iter().any(|m| m.contains("idle speed")), "{w:?}");
        assert!(w.iter().any(|m| m.contains("unknown profile")), "{w:?}");
        assert!(w.iter().any(|m| m.contains("at least one stage")), "{w:?}");
    }

    #[test]
    fn version_2_files_load_without_staged_curves() {
        let (s, w) = parse(
            r#"
            version = 2
            profile = "gaming"
            [fan_profiles]
            gaming = "custom"
            "#,
        );
        assert!(w.is_empty(), "{w:?}");
        assert!(s.staged.is_empty());
        assert_eq!(s.fan_profile(Profile::Gaming), FanProfile::Custom);
        assert!(!s.is_staged(Profile::Gaming));
        assert_eq!(
            s.cpu_curve(Profile::Gaming),
            CpuCurve::Exponential(CurvePreset::Aggressive.spec())
        );
    }

    #[test]
    fn garbage_gives_defaults() {
        let (s, w) = parse("profile = [");
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn profile_for_power_source() {
        let s = Settings::default();
        assert_eq!(s.profile_for_power(true), Profile::Balanced);
        assert_eq!(s.profile_for_power(false), Profile::Eco);
    }
}
