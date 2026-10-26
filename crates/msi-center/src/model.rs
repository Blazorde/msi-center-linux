//! View-models decoded from D-Bus payloads, and their French labels (pure).

use std::collections::HashMap;

use zbus::{OwnedVariant as OwnedValue, Variant as Value};

use crate::format;

/// One `Telemetry` sample. Every field is optional: the daemon omits unknown keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Telemetry {
    /// CPU load %.
    pub cpu_pct: Option<f64>,
    /// RAM use %.
    pub ram_pct: Option<f64>,
    /// iGPU (Arc) load %.
    pub igpu_pct: Option<f64>,
    /// dGPU (RTX 4070) load %.
    pub dgpu_pct: Option<f64>,
    /// NPU load %.
    pub npu_pct: Option<f64>,
    /// CPU temperature °C (EC).
    pub cpu_temp: Option<f64>,
    /// GPU temperature °C (EC).
    pub gpu_temp: Option<f64>,
    /// Package power W.
    pub power_w: Option<f64>,
    /// CPU fan rpm.
    pub fan1_rpm: Option<u32>,
    /// GPU fan rpm.
    pub fan2_rpm: Option<u32>,
    /// CPU fan duty %.
    pub fan1_pct: Option<u8>,
    /// GPU fan duty %.
    pub fan2_pct: Option<u8>,
    /// dGPU power state.
    pub dgpu_state: Option<String>,
    /// Cooler Boost on.
    pub cooler_boost: Option<bool>,
    /// Live CPU fan target % (ADR-0010), present only while the daemon drives the CPU fan live.
    pub cpu_fan_target: Option<u8>,
}

fn get<'a, T>(m: &'a HashMap<String, OwnedValue>, key: &str) -> Option<T>
where
    T: TryFrom<&'a Value<'a>>,
{
    m.get(key).and_then(|v| T::try_from(&**v).ok())
}

fn get_str(m: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    m.get(key)
        .and_then(|v| <&str>::try_from(&**v).ok())
        .map(str::to_owned)
}

impl Telemetry {
    /// Decodes the `a{sv}` payload. Keys with an unexpected type are treated as absent.
    #[must_use]
    pub fn from_map(m: &HashMap<String, OwnedValue>) -> Self {
        Self {
            cpu_pct: get(m, "cpu_pct"),
            ram_pct: get(m, "ram_pct"),
            igpu_pct: get(m, "igpu_pct"),
            dgpu_pct: get(m, "dgpu_pct"),
            npu_pct: get(m, "npu_pct"),
            cpu_temp: get(m, "cpu_temp"),
            gpu_temp: get(m, "gpu_temp"),
            power_w: get(m, "power_w"),
            fan1_rpm: get(m, "fan1_rpm"),
            fan2_rpm: get(m, "fan2_rpm"),
            fan1_pct: get(m, "fan1_pct"),
            fan2_pct: get(m, "fan2_pct"),
            dgpu_state: get_str(m, "dgpu_state"),
            cooler_boost: get(m, "cooler_boost"),
            cpu_fan_target: get(m, "cpu_fan_target_pct"),
        }
    }

    /// Temperature followed by `fan`.
    #[must_use]
    pub const fn temp_of(&self, fan: Fan) -> Option<f64> {
        match fan {
            Fan::Cpu => self.cpu_temp,
            Fan::Gpu => self.gpu_temp,
        }
    }
}

/// The daemon's properties, read with `GetAll`.
#[allow(clippy::struct_excessive_bools, reason = "mirrors the D-Bus properties")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DaemonStatus {
    /// Selected profile name.
    pub profile: String,
    /// Automatic profile on AC plug/unplug.
    pub auto_profile_on_power: bool,
    /// On AC power.
    pub on_ac_power: bool,
    /// EC fan mode.
    pub fan_mode: String,
    /// Cooler Boost (EC).
    pub cooler_boost: bool,
    /// Cooler Boost forced by S4.
    pub boost_forced: bool,
    /// Charge limit %, 0 if unsupported.
    pub charge_limit: u8,
    /// dGPU state.
    pub gpu_state: String,
    /// S8 read-only.
    pub read_only: bool,
    /// Why read-only.
    pub read_only_reason: String,
    /// EC firmware version.
    pub firmware_version: String,
}

impl DaemonStatus {
    /// Decodes the `GetAll` reply. Missing properties keep their default.
    #[must_use]
    pub fn from_map(m: &HashMap<String, OwnedValue>) -> Self {
        Self {
            profile: get_str(m, "Profile").unwrap_or_default(),
            auto_profile_on_power: get(m, "AutoProfileOnPower").unwrap_or_default(),
            on_ac_power: get(m, "OnAcPower").unwrap_or_default(),
            fan_mode: get_str(m, "FanMode").unwrap_or_default(),
            cooler_boost: get(m, "CoolerBoost").unwrap_or_default(),
            boost_forced: get(m, "BoostForced").unwrap_or_default(),
            charge_limit: get(m, "ChargeLimit").unwrap_or_default(),
            gpu_state: get_str(m, "GpuState").unwrap_or_default(),
            read_only: get(m, "ReadOnly").unwrap_or_default(),
            read_only_reason: get_str(m, "ReadOnlyReason").unwrap_or_default(),
            firmware_version: get_str(m, "FirmwareVersion").unwrap_or_default(),
        }
    }
}

/// A fan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fan {
    /// CPU fan (fan 1).
    Cpu,
    /// GPU fan (fan 2).
    Gpu,
}

impl Fan {
    /// Both fans.
    pub const ALL: [Self; 2] = [Self::Cpu, Self::Gpu];

    /// D-Bus name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }

    /// French label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cpu => "Ventilateur CPU",
            Self::Gpu => "Ventilateur GPU",
        }
    }
}

/// French label of a profile name, with its curve preset.
#[must_use]
pub fn profile_label(p: &str) -> &'static str {
    match p {
        "eco" => "Éco",
        "balanced" => "Équilibré",
        "gaming" => "Gaming",
        _ => "Inconnu",
    }
}

/// Name of the parameterised application action selecting a profile: `app.profile::eco`.
pub const PROFILE_ACTION: &str = "profile";

/// Name of the parameterless application action selecting `p` (`profile-eco`, …). These are also the
/// desktop-file action ids: a D-Bus-activatable app receives them as `ActivateAction` without a parameter.
#[must_use]
pub fn profile_action_name(p: msi_core::profile::Profile) -> String {
    format!("{PROFILE_ACTION}-{}", p.as_str())
}

/// The profile selected by activating the application action `action` with the string `target`:
/// `("profile", Some("eco"))` or `("profile-eco", None)`. A leading `app.` is accepted. Anything else is
/// `None` (unknown action, unknown profile, missing or extra parameter).
#[must_use]
pub fn parse_profile_action(action: &str, target: Option<&str>) -> Option<msi_core::profile::Profile> {
    let action = action.strip_prefix("app.").unwrap_or(action);
    let name = match (action, target) {
        (PROFILE_ACTION, Some(t)) => t.trim(),
        (a, None) => a.strip_prefix(PROFILE_ACTION)?.strip_prefix('-')?,
        _ => return None,
    };
    name.parse().ok()
}

/// One short line telling what a profile is for (hero switch).
#[must_use]
pub const fn profile_tagline(p: msi_core::profile::Profile) -> &'static str {
    use msi_core::profile::Profile;
    match p {
        Profile::Eco => "Silence et autonomie, RTX éteinte",
        Profile::Balanced => "Le quotidien, RTX en veille",
        Profile::Gaming => "Pleine puissance, RTX active",
    }
}

/// Symbolic icon of a profile.
#[must_use]
pub const fn profile_icon(p: msi_core::profile::Profile) -> &'static str {
    use msi_core::profile::Profile;
    match p {
        Profile::Eco => "power-profile-power-saver-symbolic",
        Profile::Balanced => "power-profile-balanced-symbolic",
        Profile::Gaming => "power-profile-performance-symbolic",
    }
}

/// French description of what a profile does (from `msi_core::profile::Profile::target`).
#[must_use]
pub fn profile_description(p: msi_core::profile::Profile) -> String {
    use msi_core::curve::CurvePreset;
    use msi_core::ec::ShiftMode;
    use msi_core::profile::GpuMode;
    let t = p.target();
    let curve = match t.curve {
        CurvePreset::Silent => "courbe Silencieuse (arrêt sous 50 °C)",
        CurvePreset::Progressive => "courbe Progressive (dès 42 °C)",
        CurvePreset::Aggressive => "courbe Agressive (jamais à l'arrêt)",
    };
    let shift = match t.shift_mode {
        ShiftMode::Eco => "processeur économe",
        ShiftMode::Comfort => "processeur équilibré",
        ShiftMode::Turbo => "processeur turbo",
    };
    let gpu = match t.gpu {
        GpuMode::Removed => "RTX 4070 retirée (0 W)",
        GpuMode::RuntimeSuspend => "RTX 4070 en veille automatique",
        GpuMode::Active => "RTX 4070 active",
    };
    format!("{curve} · {shift} · {gpu}")
}

/// French label of a dGPU state.
#[must_use]
pub fn gpu_state_label(s: &str) -> &'static str {
    match s {
        "active" => "Active",
        "suspended" => "En veille (RTD3)",
        "removed" => "Retirée du bus (0 W)",
        _ => "Inconnu",
    }
}

/// French label of the EC fan mode.
#[must_use]
pub fn fan_mode_label(s: &str) -> &'static str {
    match s {
        "advanced" => "Courbes MSI Center",
        "auto" => "Courbe d'usine (EC)",
        _ => "Inconnu",
    }
}

/// French label of the power source.
#[must_use]
pub const fn power_source_label(on_ac: bool) -> &'static str {
    if on_ac { "Secteur" } else { "Batterie" }
}

/// A `SafetyEvent` signal.
#[derive(Debug, Clone, PartialEq)]
pub struct SafetyEvent {
    /// `boost_forced`, `boost_released`, `readback_mismatch`, `write_failed`, `ec_reset`, …
    pub kind: String,
    /// Hottest °C for boost events.
    pub value: f64,
    /// English detail.
    pub detail: String,
}

/// How serious an event is, shown as text and icon (never by colour alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Informational.
    Info,
    /// Needs attention.
    Warning,
}

impl SafetyEvent {
    /// French one-line description.
    #[must_use]
    pub fn describe(&self) -> String {
        let t = if self.value > 0.0 {
            format::temp(Some(self.value))
        } else {
            "capteur illisible".to_owned()
        };
        match self.kind.as_str() {
            "boost_forced" => format!("Surchauffe ({t}) : Cooler Boost forcé par la sécurité."),
            "boost_released" => format!("Température revenue à la normale ({t}) : Cooler Boost relâché."),
            "readback_mismatch" => {
                "Relecture EC incohérente : retour à la courbe d'usine par sécurité.".to_owned()
            }
            "write_failed" => "Écriture EC en échec : retour à la courbe d'usine par sécurité.".to_owned(),
            "ec_reset" => "L'EC avait perdu les courbes : elles ont été ré-appliquées.".to_owned(),
            "gpu_busy" => {
                "La RTX 4070 est utilisée par des applications : elle reste en veille au lieu d'être retirée."
                    .to_owned()
            }
            "fan_stalled" => {
                format!("Ventilateur arrêté alors qu'il devrait tourner ({t}) : Cooler Boost activé.")
            }
            other => format!("Événement de sécurité « {other} »."),
        }
    }

    /// Severity of the event.
    #[must_use]
    pub fn severity(&self) -> Severity {
        match self.kind.as_str() {
            "boost_released" | "ec_reset" | "gpu_busy" => Severity::Info,
            _ => Severity::Warning,
        }
    }
}

/// Temperature level, shown next to the value as a word (not only as a colour).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempLevel {
    /// Below 75 °C.
    Normal,
    /// 75–89 °C.
    Hot,
    /// 90 °C and above.
    Critical,
}

impl TempLevel {
    /// Level of a temperature.
    #[must_use]
    pub fn of(c: f64) -> Self {
        if c >= 90.0 {
            Self::Critical
        } else if c >= 75.0 {
            Self::Hot
        } else {
            Self::Normal
        }
    }

    /// French word.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Hot => "chaud",
            Self::Critical => "critique",
        }
    }

    /// CSS class.
    #[must_use]
    pub const fn css_class(self) -> &'static str {
        match self {
            Self::Normal => "level-normal",
            Self::Hot => "level-hot",
            Self::Critical => "level-critical",
        }
    }
}

/// One entry of `Clipd1.List`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipEntry {
    /// Clip id.
    pub id: u64,
    /// Normalised MIME type.
    pub mime: String,
    /// One-line preview (empty for images).
    pub preview: String,
    /// Size in bytes.
    pub size: u64,
    /// Creation time, ms since the epoch.
    pub created_ms: i64,
    /// Last use, ms since the epoch.
    pub last_used_ms: i64,
    /// Pinned (persistent).
    pub pinned: bool,
    /// Sorted tags.
    pub tags: Vec<String>,
}

/// Wire form of a `List` entry: `(tsstxxbas)`.
pub type ClipWire = (u64, String, String, u64, i64, i64, bool, Vec<String>);

impl From<ClipWire> for ClipEntry {
    fn from(w: ClipWire) -> Self {
        let (id, mime, preview, size, created_ms, last_used_ms, pinned, tags) = w;
        Self {
            id,
            mime,
            preview,
            size,
            created_ms,
            last_used_ms,
            pinned,
            tags,
        }
    }
}

impl ClipEntry {
    /// Whether the clip is an image.
    #[must_use]
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    /// Short French description of the kind: `Texte`, `Image PNG`.
    #[must_use]
    pub fn kind_label(&self) -> String {
        if let Some(sub) = self.mime.strip_prefix("image/") {
            format!("Image {}", sub.to_uppercase())
        } else {
            "Texte".to_owned()
        }
    }
}

/// Clip list scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// Every clip.
    #[default]
    All,
    /// Pinned only.
    Pinned,
    /// Session history only.
    Unpinned,
}

impl Scope {
    /// D-Bus value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Pinned => "pinned",
            Self::Unpinned => "unpinned",
        }
    }
}

/// Parses a comma-separated tag list typed by the user (trimmed, empty entries dropped). Normalisation is
/// the service's job (`SetTags` returns the normalised tags).
#[must_use]
pub fn parse_tag_input(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn map(entries: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).unwrap()))
            .collect()
    }

    #[test]
    fn decodes_telemetry_and_ignores_bad_types() {
        let m = map(vec![
            ("cpu_temp", Value::F64(61.5)),
            ("fan1_rpm", Value::U32(2070)),
            ("fan1_pct", Value::U8(30)),
            ("cpu_fan_target_pct", Value::U8(37)),
            ("dgpu_state", Value::from("removed")),
            ("cooler_boost", Value::Bool(false)),
            ("gpu_temp", Value::from("hot")),
        ]);
        let t = Telemetry::from_map(&m);
        assert_eq!(t.cpu_temp, Some(61.5));
        assert_eq!(t.fan1_rpm, Some(2070));
        assert_eq!(t.fan1_pct, Some(30));
        assert_eq!(t.cpu_fan_target, Some(37));
        assert_eq!(t.dgpu_state.as_deref(), Some("removed"));
        assert_eq!(t.cooler_boost, Some(false));
        assert_eq!(t.gpu_temp, None);
        assert_eq!(t.temp_of(Fan::Cpu), Some(61.5));
        assert_eq!(t.npu_pct, None);
    }

    #[test]
    fn decodes_status() {
        let m = map(vec![
            ("Profile", Value::from("eco")),
            ("OnAcPower", Value::Bool(true)),
            ("ChargeLimit", Value::U8(80)),
            ("ReadOnly", Value::Bool(true)),
            ("ReadOnlyReason", Value::from("unknown firmware")),
        ]);
        let s = DaemonStatus::from_map(&m);
        assert_eq!(s.profile, "eco");
        assert!(s.on_ac_power);
        assert_eq!(s.charge_limit, 80);
        assert!(s.read_only);
        assert_eq!(s.read_only_reason, "unknown firmware");
        assert_eq!(s.gpu_state, "");
    }

    #[test]
    fn labels_are_french() {
        assert_eq!(profile_label("eco"), "Éco");
        assert_eq!(profile_label("x"), "Inconnu");
        assert_eq!(gpu_state_label("removed"), "Retirée du bus (0 W)");
        assert_eq!(fan_mode_label("auto"), "Courbe d'usine (EC)");
        assert_eq!(power_source_label(false), "Batterie");
        assert!(profile_description(msi_core::profile::Profile::Eco).contains("RTX 4070 retirée"));
    }

    #[test]
    fn describes_safety_events() {
        let e = |kind: &str, value| SafetyEvent {
            kind: kind.into(),
            value,
            detail: String::new(),
        };
        assert_eq!(
            e("boost_forced", 96.0).describe().replace(format::NNBSP, " "),
            "Surchauffe (96 °C) : Cooler Boost forcé par la sécurité."
        );
        assert!(e("boost_forced", 0.0).describe().contains("capteur illisible"));
        assert_eq!(e("boost_released", 80.0).severity(), Severity::Info);
        assert_eq!(e("write_failed", 0.0).severity(), Severity::Warning);
        assert!(e("weird", 0.0).describe().contains("weird"));
    }

    #[test]
    fn profile_actions() {
        use msi_core::profile::Profile;
        for p in Profile::ALL {
            let name = profile_action_name(p);
            assert_eq!(parse_profile_action(&name, None), Some(p));
            assert_eq!(parse_profile_action(&format!("app.{name}"), None), Some(p));
            assert_eq!(parse_profile_action(PROFILE_ACTION, Some(p.as_str())), Some(p));
            assert_ne!(profile_tagline(p), "");
            assert!(profile_icon(p).ends_with("-symbolic"));
        }
        assert_eq!(
            parse_profile_action("app.profile", Some(" gaming ")),
            Some(Profile::Gaming)
        );
        for (a, t) in [
            ("profile", None),
            ("profile", Some("turbo")),
            ("profile-eco", Some("eco")),
            ("profile-", None),
            ("profileeco", None),
            ("quit", None),
            ("page", Some("eco")),
        ] {
            assert_eq!(parse_profile_action(a, t), None, "{a} {t:?}");
        }
    }

    #[test]
    fn desktop_file_lists_the_profile_actions() {
        use msi_core::profile::Profile;
        let desktop = include_str!("../data/org.blazorde.MsiCenter.desktop");
        let names: Vec<String> = Profile::ALL.into_iter().map(profile_action_name).collect();
        assert!(desktop.contains(&format!("\nActions={};\n", names.join(";"))));
        for (p, n) in Profile::ALL.into_iter().zip(&names) {
            let section = format!("[Desktop Action {n}]\nName={}\n", profile_label(p.as_str()));
            assert!(desktop.contains(&section), "{section}");
            assert!(desktop.contains(&format!("gapplication action org.blazorde.MsiCenter {n}\n")));
        }
    }

    #[test]
    fn temperature_levels() {
        assert_eq!(TempLevel::of(60.0), TempLevel::Normal);
        assert_eq!(TempLevel::of(75.0), TempLevel::Hot);
        assert_eq!(TempLevel::of(95.0), TempLevel::Critical);
        assert_eq!(TempLevel::Critical.label(), "critique");
    }

    #[test]
    fn clip_entries() {
        let c = ClipEntry::from((7, "image/png".to_owned(), String::new(), 10, 1, 2, false, vec![]));
        assert!(c.is_image());
        assert_eq!(c.kind_label(), "Image PNG");
        assert_eq!(Scope::Unpinned.as_str(), "unpinned");
        assert_eq!(parse_tag_input(" work, ,Rust  ,"), vec!["work", "Rust"]);
    }
}
