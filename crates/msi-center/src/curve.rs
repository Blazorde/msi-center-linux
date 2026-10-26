//! Fan curve view-model: the editable specification, the EC table and the S2 safety report (pure).
//!
//! Validation reuses `msi-core` (the same S3 rules as the daemon) so the user sees a French reason before
//! anything is sent; the daemon stays the authority and validates again.

use msi_core::curve::{FanCurveParams, FanCurveSpec, ParamsError, SpecError, THRESHOLD_COUNT};

use crate::format::{self, NNBSP};

/// Spec on the wire: `(y idle, y t0, y t1, y min, y max, d k, ay up, y hysteresis)`.
pub type SpecWire = (u8, u8, u8, u8, u8, f64, Vec<u8>, u8);
/// EC table on the wire: `(ay speeds, ay up, ay down)`.
pub type TableWire = (Vec<u8>, Vec<u8>, Vec<u8>);
/// Raised step on the wire: `(step, entry_temp, curve_speed, applied_speed, floor_temp, floor_speed)`.
pub type RaisedWire = (u8, u8, u8, u8, u8, u8);

/// Editable curve parameters (may be invalid while the user drags a slider).
#[derive(Debug, Clone, PartialEq)]
pub struct CurveSpec {
    /// Step 0 speed %.
    pub idle: u8,
    /// Ramp start °C.
    pub t0: u8,
    /// Ramp end °C.
    pub t1: u8,
    /// Ramp start speed %.
    pub min: u8,
    /// Ramp end speed %.
    pub max: u8,
    /// Exponent.
    pub k: f64,
    /// Up thresholds °C.
    pub up: [u8; THRESHOLD_COUNT],
    /// `up − down` °C.
    pub hysteresis: u8,
}

impl CurveSpec {
    /// From the wire form; `None` if `up` does not have 6 entries.
    #[must_use]
    pub fn from_wire(w: &SpecWire) -> Option<Self> {
        let (idle, t0, t1, min, max, k, ref up, hysteresis) = *w;
        Some(Self {
            idle,
            t0,
            t1,
            min,
            max,
            k,
            up: up.as_slice().try_into().ok()?,
            hysteresis,
        })
    }

    /// Wire form.
    #[must_use]
    pub fn to_wire(&self) -> SpecWire {
        (
            self.idle,
            self.t0,
            self.t1,
            self.min,
            self.max,
            self.k,
            self.up.to_vec(),
            self.hysteresis,
        )
    }

    /// From a `msi-core` preset.
    #[must_use]
    pub fn from_core(s: &FanCurveSpec) -> Self {
        let p = s.params();
        Self {
            idle: s.idle(),
            t0: p.t0(),
            t1: p.t1(),
            min: p.min(),
            max: p.max(),
            k: p.k(),
            up: s.up(),
            hysteresis: s.hysteresis(),
        }
    }

    /// Validates with the S3 rules of `msi-core`.
    ///
    /// # Errors
    /// A French explanation of the first violated rule.
    pub fn validate(&self) -> Result<FanCurveSpec, String> {
        let params = FanCurveParams::new(self.t0, self.t1, self.min, self.max, self.k)
            .map_err(|e| params_error_fr(&e))?;
        FanCurveSpec::new(params, self.idle, self.up, self.hysteresis).map_err(|e| spec_error_fr(&e))
    }

    /// Exponential curve speed at `temp` (for drawing), or `None` if the parameters are invalid.
    #[must_use]
    pub fn curve_speed_at(&self, temp: u8) -> Option<f64> {
        FanCurveParams::new(self.t0, self.t1, self.min, self.max, self.k)
            .ok()
            .map(|p| p.speed_at(temp))
    }

    /// Spreads the six up thresholds evenly from `t0` to `t1` (used when the user moves T0 or T1).
    #[must_use]
    pub fn with_spread_thresholds(mut self) -> Self {
        self.up = spread_thresholds(self.t0, self.t1);
        self
    }
}

/// Six thresholds evenly spread from `t0` to `t1` included, rounded to the degree. Strictly increasing as
/// soon as `t1 − t0 ≥ 5`.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the result lies between t0 and t1"
)]
pub fn spread_thresholds(t0: u8, t1: u8) -> [u8; THRESHOLD_COUNT] {
    let mut up = [0; THRESHOLD_COUNT];
    for (i, u) in (0u8..).zip(up.iter_mut()) {
        let v = f64::from(t0) + (f64::from(t1) - f64::from(t0)) * f64::from(i) / 5.0;
        *u = v.round().clamp(0.0, 255.0) as u8;
    }
    up
}

fn params_error_fr(e: &ParamsError) -> String {
    match e {
        ParamsError::TempOutOfRange { value } => {
            format!("La température {value}{NNBSP}°C sort de la plage 20–100{NNBSP}°C.")
        }
        ParamsError::TempOrder { t0, t1 } => {
            format!("T0 ({t0}{NNBSP}°C) doit être strictement inférieur à T1 ({t1}{NNBSP}°C).")
        }
        ParamsError::SpeedOutOfRange { value } => {
            format!("La vitesse {value}{NNBSP}% dépasse 100{NNBSP}%.")
        }
        ParamsError::SpeedOrder { min, max } => {
            format!("La vitesse min ({min}{NNBSP}%) ne doit pas dépasser la vitesse max ({max}{NNBSP}%).")
        }
        ParamsError::ExponentOutOfRange { value } => format!(
            "L'exposant k = {} sort de la plage 1,0–4,0.",
            format::decimal(*value, 1)
        ),
    }
}

fn spec_error_fr(e: &SpecError) -> String {
    match e {
        SpecError::IdleAboveMin { idle, min } => format!(
            "La vitesse au repos ({idle}{NNBSP}%) ne doit pas dépasser la vitesse min ({min}{NNBSP}%)."
        ),
        SpecError::ThresholdOutOfRange { index, value } => format!(
            "Le seuil {} ({value}{NNBSP}°C) sort de la plage 20–100{NNBSP}°C.",
            index + 1
        ),
        SpecError::ThresholdsNotIncreasing { index } => format!(
            "Les seuils doivent être strictement croissants (seuil {}) : écartez T0 et T1 d'au moins 5{NNBSP}°C.",
            index + 1
        ),
        SpecError::HysteresisOutOfRange { value } => {
            format!("L'hystérésis {value}{NNBSP}°C sort de la plage 2–10{NNBSP}°C.")
        }
        SpecError::DownOutOfRange { up, hysteresis } => format!(
            "Premier seuil ({up}{NNBSP}°C) moins l'hystérésis ({hysteresis}{NNBSP}°C) : sous 20{NNBSP}°C."
        ),
    }
}

/// The 7-step EC table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Speed % of steps 0..=6.
    pub speeds: [u8; 7],
    /// Up threshold °C to enter steps 1..=6.
    pub up: [u8; 6],
    /// Down threshold °C below which steps 1..=6 are left.
    pub down: [u8; 6],
}

impl Table {
    /// From the wire form; `None` on wrong lengths.
    #[must_use]
    pub fn from_wire(w: &TableWire) -> Option<Self> {
        Some(Self {
            speeds: w.0.as_slice().try_into().ok()?,
            up: w.1.as_slice().try_into().ok()?,
            down: w.2.as_slice().try_into().ok()?,
        })
    }

    /// Ascending step at `temp` (number of up thresholds ≤ `temp`), the conservative reading used by S2.
    #[must_use]
    pub fn ascending_step(&self, temp: f64) -> usize {
        self.up.iter().filter(|&&u| f64::from(u) <= temp).count()
    }
}

/// A step raised by an S2 floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raised {
    /// Step index.
    pub step: u8,
    /// Entry temperature (0 for step 0).
    pub entry_temp: u8,
    /// Speed the curve asked for.
    pub curve_speed: u8,
    /// Speed actually written.
    pub applied_speed: u8,
    /// Floor temperature.
    pub floor_temp: u8,
    /// Floor speed.
    pub floor_speed: u8,
}

impl Raised {
    /// From the wire form.
    #[must_use]
    pub const fn from_wire(w: RaisedWire) -> Self {
        let (step, entry_temp, curve_speed, applied_speed, floor_temp, floor_speed) = w;
        Self {
            step,
            entry_temp,
            curve_speed,
            applied_speed,
            floor_temp,
            floor_speed,
        }
    }

    /// French sentence for the safety report.
    #[must_use]
    pub fn describe(&self) -> String {
        let from = if self.step == 0 || self.entry_temp == 0 {
            "sous le premier seuil".to_owned()
        } else {
            format!("dès {}{NNBSP}°C", self.entry_temp)
        };
        format!(
            "Palier {} ({from}) : la courbe donne {}{NNBSP}%, relevé à {}{NNBSP}% par le plancher de sécurité {}{NNBSP}% dès {}{NNBSP}°C.",
            self.step, self.curve_speed, self.applied_speed, self.floor_speed, self.floor_temp
        )
    }
}

/// A generated table and its S2 report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generated {
    /// Table written (or to be written) to the EC.
    pub table: Table,
    /// Steps raised by S2.
    pub raised: Vec<Raised>,
}

impl Generated {
    /// From the `(table, raised)` reply of `PreviewFanCurve` / `SetFanCurve`.
    #[must_use]
    pub fn from_wire(table: &TableWire, raised: &[RaisedWire]) -> Option<Self> {
        Some(Self {
            table: Table::from_wire(table)?,
            raised: raised.iter().copied().map(Raised::from_wire).collect(),
        })
    }

    /// Whether `step` was raised by S2.
    #[must_use]
    pub fn is_raised(&self, step: usize) -> bool {
        self.raised.iter().any(|r| usize::from(r.step) == step)
    }
}

/// Slider description of one parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamRange {
    /// Lowest value.
    pub min: f64,
    /// Highest value.
    pub max: f64,
    /// Step.
    pub step: f64,
}

/// Which parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Param {
    /// `idle`.
    Idle,
    /// `t0`.
    T0,
    /// `t1`.
    T1,
    /// `min`.
    Min,
    /// `max`.
    Max,
    /// `k`.
    K,
    /// `hysteresis`.
    Hysteresis,
}

impl Param {
    /// All parameters in display order.
    pub const ALL: [Self; 7] = [
        Self::Idle,
        Self::T0,
        Self::T1,
        Self::Min,
        Self::Max,
        Self::K,
        Self::Hysteresis,
    ];

    /// French title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Idle => "Vitesse au repos",
            Self::T0 => "Début de la rampe (T0)",
            Self::T1 => "Pleine vitesse à (T1)",
            Self::Min => "Vitesse de départ",
            Self::Max => "Vitesse maximale",
            Self::K => "Progressivité (k)",
            Self::Hysteresis => "Hystérésis",
        }
    }

    /// French explanation.
    #[must_use]
    pub const fn subtitle(self) -> &'static str {
        match self {
            Self::Idle => "Sous le premier seuil (0 = ventilateur arrêté)",
            Self::T0 => "Température où le ventilateur démarre",
            Self::T1 => "Température où il atteint la vitesse max",
            Self::Min => "Vitesse au premier palier",
            Self::Max => "Vitesse au dernier palier",
            Self::K => "Plus k est grand, plus c'est doux au début et fort à la fin",
            Self::Hysteresis => "Écart montée / descente, évite le « pompage »",
        }
    }

    /// Slider range (S3 bounds).
    #[must_use]
    pub const fn range(self) -> ParamRange {
        match self {
            Self::Idle | Self::Min | Self::Max => ParamRange {
                min: 0.0,
                max: 100.0,
                step: 1.0,
            },
            Self::T0 | Self::T1 => ParamRange {
                min: 20.0,
                max: 100.0,
                step: 1.0,
            },
            Self::K => ParamRange {
                min: 1.0,
                max: 4.0,
                step: 0.1,
            },
            Self::Hysteresis => ParamRange {
                min: 2.0,
                max: 10.0,
                step: 1.0,
            },
        }
    }

    /// Value of this parameter in `spec`.
    #[must_use]
    pub fn get(self, s: &CurveSpec) -> f64 {
        match self {
            Self::Idle => f64::from(s.idle),
            Self::T0 => f64::from(s.t0),
            Self::T1 => f64::from(s.t1),
            Self::Min => f64::from(s.min),
            Self::Max => f64::from(s.max),
            Self::K => s.k,
            Self::Hysteresis => f64::from(s.hysteresis),
        }
    }

    /// Returns `spec` with this parameter set to `v` (rounded to the slider step). Moving T0 or T1 spreads
    /// the thresholds again.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255"
    )]
    pub fn set(self, s: &CurveSpec, v: f64) -> CurveSpec {
        let byte = v.round().clamp(0.0, 255.0) as u8;
        let mut out = s.clone();
        match self {
            Self::Idle => out.idle = byte,
            Self::T0 => out.t0 = byte,
            Self::T1 => out.t1 = byte,
            Self::Min => out.min = byte,
            Self::Max => out.max = byte,
            Self::K => out.k = (v * 10.0).round() / 10.0,
            Self::Hysteresis => out.hysteresis = byte,
        }
        if matches!(self, Self::T0 | Self::T1) {
            out = out.with_spread_thresholds();
        }
        out
    }

    /// Formatted value.
    #[must_use]
    pub fn format(self, v: f64) -> String {
        match self {
            Self::K => format::decimal(v, 1),
            Self::T0 | Self::T1 | Self::Hysteresis => format!("{}{NNBSP}°C", format::round_u64(v)),
            Self::Idle | Self::Min | Self::Max => format!("{}{NNBSP}%", format::round_u64(v)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use msi_core::curve::CurvePreset;
    use msi_core::table::generate;

    use super::*;

    fn silent() -> CurveSpec {
        CurveSpec::from_core(&CurvePreset::Silent.spec())
    }

    #[test]
    fn wire_round_trip() {
        let s = silent();
        assert_eq!(CurveSpec::from_wire(&s.to_wire()), Some(s.clone()));
        let mut bad = s.to_wire();
        bad.6.pop();
        assert_eq!(CurveSpec::from_wire(&bad), None);
    }

    #[test]
    fn presets_validate() {
        for p in CurvePreset::ALL {
            assert!(CurveSpec::from_core(&p.spec()).validate().is_ok(), "{p:?}");
        }
    }

    #[test]
    fn invalid_specs_get_french_reasons() {
        let s = silent();
        let e = |s: CurveSpec| s.validate().unwrap_err();
        assert!(e(CurveSpec { t0: 90, ..s.clone() }).starts_with("T0 (90"));
        assert!(
            e(CurveSpec {
                min: 100,
                max: 50,
                ..s.clone()
            })
            .contains("vitesse min")
        );
        assert!(e(CurveSpec { k: 5.0, ..s.clone() }).contains("5,0"));
        assert!(
            e(CurveSpec {
                idle: 40,
                ..s.clone()
            })
            .contains("au repos")
        );
        assert!(
            e(CurveSpec {
                hysteresis: 12,
                ..s.clone()
            })
            .contains("hystérésis 12")
        );
        assert!(
            e(CurveSpec {
                up: [50, 50, 64, 70, 76, 82],
                ..s.clone()
            })
            .contains("croissants")
        );
        assert!(
            e(CurveSpec {
                up: [21, 57, 64, 70, 76, 82],
                ..s
            })
            .contains("sous 20")
        );
    }

    #[test]
    fn client_validation_agrees_with_the_core_on_a_grid() {
        let base = silent();
        for t0 in (20..=100).step_by(7) {
            for t1 in (20..=100).step_by(9) {
                for h in [1, 2, 6, 10, 11] {
                    let s = CurveSpec {
                        t0,
                        t1,
                        hysteresis: h,
                        ..base.clone()
                    }
                    .with_spread_thresholds();
                    let core = FanCurveParams::new(s.t0, s.t1, s.min, s.max, s.k)
                        .ok()
                        .and_then(|p| FanCurveSpec::new(p, s.idle, s.up, s.hysteresis).ok());
                    assert_eq!(s.validate().is_ok(), core.is_some(), "{s:?}");
                }
            }
        }
    }

    #[test]
    fn spreads_thresholds() {
        assert_eq!(spread_thresholds(42, 79), [42, 49, 57, 64, 72, 79]);
        assert_eq!(spread_thresholds(50, 55), [50, 51, 52, 53, 54, 55]);
        let s = Param::T1.set(&silent(), 90.0);
        assert_eq!(s.up, spread_thresholds(50, 90));
        assert!(s.validate().is_ok());
    }

    #[test]
    fn params_round_and_format() {
        let s = Param::K.set(&silent(), 2.345);
        assert!((s.k - 2.3).abs() < 1e-9);
        assert_eq!(Param::K.format(2.3), "2,3");
        assert_eq!(Param::T0.format(50.0).replace(NNBSP, " "), "50 °C");
        assert_eq!(Param::Idle.format(0.0).replace(NNBSP, " "), "0 %");
        for p in Param::ALL {
            let r = p.range();
            assert!(r.min < r.max && r.step > 0.0);
            assert!(!p.title().is_empty() && !p.subtitle().is_empty());
        }
        assert!((Param::Hysteresis.get(&silent()) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn table_and_report() {
        let g = generate(&CurvePreset::Silent.spec()).unwrap();
        let t = Table::from_wire(&(
            g.curve.speeds().to_vec(),
            g.curve.up().to_vec(),
            g.curve.down().to_vec(),
        ))
        .unwrap();
        assert_eq!(t.ascending_step(40.0), 0);
        assert_eq!(t.ascending_step(50.0), 1);
        assert_eq!(t.ascending_step(99.0), 6);
        assert_eq!(Table::from_wire(&(vec![1], vec![], vec![])), None);

        let generated = Generated::from_wire(
            &(t.speeds.to_vec(), t.up.to_vec(), t.down.to_vec()),
            &[(2, 61, 25, 30, 60, 30)],
        )
        .unwrap();
        assert!(generated.is_raised(2) && !generated.is_raised(1));
        assert_eq!(
            generated.raised[0].describe().replace(NNBSP, " "),
            "Palier 2 (dès 61 °C) : la courbe donne 25 %, relevé à 30 % par le plancher de sécurité 30 % dès 60 °C."
        );
        assert!(
            Raised::from_wire((0, 0, 0, 30, 60, 30))
                .describe()
                .contains("sous le premier seuil")
        );
    }

    #[test]
    fn curve_speed_for_drawing() {
        let s = silent();
        assert_eq!(s.curve_speed_at(40), Some(30.0));
        assert_eq!(s.curve_speed_at(90), Some(100.0));
        assert_eq!(CurveSpec { t0: 90, ..s }.curve_speed_at(50), None);
    }
}
