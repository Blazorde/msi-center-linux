//! Exponential fan curve model (ADR-0006, `docs/hardware/fan-safety.md`).
//!
//! ```text
//!           ⎧ min                                       if T ≤ T0
//! speed(T) =⎨ min + (max − min) · ((T − T0)/(T1 − T0))^k  if T0 < T < T1
//!           ⎩ max                                       if T ≥ T1
//! ```

use std::ops::RangeInclusive;

/// Accepted range for `T0` / `T1`, in °C.
pub const PARAM_TEMP_RANGE: RangeInclusive<u8> = 20..=100;
/// Accepted range for the exponent `k`.
pub const EXPONENT_RANGE: RangeInclusive<f64> = 1.0..=4.0;
/// Maximum fan speed, in %.
pub const MAX_SPEED: u8 = 100;

/// Why a set of curve parameters was rejected (rule S3: rejected, never corrected).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParamsError {
    /// `T0` or `T1` is outside [`PARAM_TEMP_RANGE`].
    #[error("temperature {value} °C is outside 20..=100 °C")]
    TempOutOfRange {
        /// Offending temperature.
        value: u8,
    },
    /// `T0` must be strictly lower than `T1`.
    #[error("T0 ({t0} °C) must be strictly lower than T1 ({t1} °C)")]
    TempOrder {
        /// Start of the ramp.
        t0: u8,
        /// End of the ramp.
        t1: u8,
    },
    /// A speed is above 100 %.
    #[error("speed {value} % is above 100 %")]
    SpeedOutOfRange {
        /// Offending speed.
        value: u8,
    },
    /// `min` must not exceed `max`.
    #[error("min speed ({min} %) must not exceed max speed ({max} %)")]
    SpeedOrder {
        /// Lower speed.
        min: u8,
        /// Upper speed.
        max: u8,
    },
    /// `k` is not finite or outside [`EXPONENT_RANGE`].
    #[error("exponent k = {value} is outside 1.0..=4.0")]
    ExponentOutOfRange {
        /// Offending exponent.
        value: f64,
    },
}

/// Validated parameters of the exponential curve. Can only be built through [`FanCurveParams::new`],
/// so an instance is always within bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FanCurveParams {
    t0: u8,
    t1: u8,
    min: u8,
    max: u8,
    k: f64,
}

impl FanCurveParams {
    /// Validates and builds curve parameters.
    ///
    /// # Errors
    ///
    /// Returns a [`ParamsError`] if any value is out of bounds (rule S3).
    pub fn new(t0: u8, t1: u8, min: u8, max: u8, k: f64) -> Result<Self, ParamsError> {
        for value in [t0, t1] {
            if !PARAM_TEMP_RANGE.contains(&value) {
                return Err(ParamsError::TempOutOfRange { value });
            }
        }
        if t0 >= t1 {
            return Err(ParamsError::TempOrder { t0, t1 });
        }
        for value in [min, max] {
            if value > MAX_SPEED {
                return Err(ParamsError::SpeedOutOfRange { value });
            }
        }
        if min > max {
            return Err(ParamsError::SpeedOrder { min, max });
        }
        if !k.is_finite() || !EXPONENT_RANGE.contains(&k) {
            return Err(ParamsError::ExponentOutOfRange { value: k });
        }
        Ok(Self { t0, t1, min, max, k })
    }

    /// Temperature (°C) at and below which the fan runs at `min`.
    #[must_use]
    pub const fn t0(&self) -> u8 {
        self.t0
    }

    /// Temperature (°C) at and above which the fan runs at `max`.
    #[must_use]
    pub const fn t1(&self) -> u8 {
        self.t1
    }

    /// Lowest speed (%).
    #[must_use]
    pub const fn min(&self) -> u8 {
        self.min
    }

    /// Highest speed (%).
    #[must_use]
    pub const fn max(&self) -> u8 {
        self.max
    }

    /// Exponent: `k > 1` keeps the fan quiet when cool and ramps up hard when hot.
    #[must_use]
    pub const fn k(&self) -> f64 {
        self.k
    }

    /// Exact curve value (%) at `temp` °C, before rounding and before the safety floors.
    #[must_use]
    pub fn speed_at(&self, temp: u8) -> f64 {
        self.speed_at_celsius(f64::from(temp))
    }

    /// Exact curve value (%) at a fractional temperature (a filtered reading, ADR-0010). A non-finite `temp`
    /// gives `max` (the safe side).
    #[must_use]
    pub fn speed_at_celsius(&self, temp: f64) -> f64 {
        let (min, max) = (f64::from(self.min), f64::from(self.max));
        let (t0, t1) = (f64::from(self.t0), f64::from(self.t1));
        if !temp.is_finite() || temp >= t1 {
            return max;
        }
        if temp <= t0 {
            return min;
        }
        let x = (temp - t0) / (t1 - t0);
        (max - min).mul_add(x.powf(self.k), min).clamp(min, max)
    }

    /// Curve value at a fractional temperature rounded to the nearest whole percent.
    #[must_use]
    pub fn speed_percent_celsius(&self, temp: f64) -> u8 {
        let rounded = self
            .speed_at_celsius(temp)
            .round()
            .clamp(0.0, f64::from(MAX_SPEED));
        // In 0..=100 after the clamp: the cast cannot truncate nor lose the sign.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let speed = rounded as u8;
        speed
    }

    /// Curve value at `temp` rounded to the nearest whole percent.
    #[must_use]
    pub fn speed_percent(&self, temp: u8) -> u8 {
        let rounded = self.speed_at(temp).round().clamp(0.0, f64::from(MAX_SPEED));
        // In 0..=100 after the clamp: the cast cannot truncate nor lose the sign.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let speed = rounded as u8;
        speed
    }
}

/// Number of speed steps in an EC fan table (`docs/hardware/ec-map.md`).
pub const STEP_COUNT: usize = 7;
/// Number of up (and down) thresholds in an EC fan table: one per step above step 0.
pub const THRESHOLD_COUNT: usize = STEP_COUNT - 1;
/// Accepted range for table thresholds, in °C (same bounds as the kernel interface).
pub const THRESHOLD_RANGE: RangeInclusive<u8> = 20..=100;
/// Accepted hysteresis (`up[i] − down[i]`), in °C. At least 2 °C so a sensor jittering by ±1 °C cannot make
/// the fan hunt between two steps.
pub const HYSTERESIS_RANGE: RangeInclusive<u8> = 2..=10;

/// Why a curve specification was rejected (rule S3: rejected, never corrected).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SpecError {
    /// The idle speed (step 0) must not exceed the speed of the first active step.
    #[error("idle speed ({idle} %) must not exceed the start speed ({min} %)")]
    IdleAboveMin {
        /// Speed below the first threshold.
        idle: u8,
        /// Start speed of the curve.
        min: u8,
    },
    /// An up threshold is outside [`THRESHOLD_RANGE`].
    #[error("threshold {index}: {value} °C is outside 20..=100 °C")]
    ThresholdOutOfRange {
        /// Threshold index (0 = entry of step 1).
        index: usize,
        /// Offending value.
        value: u8,
    },
    /// Up thresholds must be strictly increasing.
    #[error("threshold {index}: thresholds must be strictly increasing")]
    ThresholdsNotIncreasing {
        /// Index of the first threshold not above its predecessor.
        index: usize,
    },
    /// Hysteresis outside [`HYSTERESIS_RANGE`].
    #[error("hysteresis {value} °C is outside 2..=10 °C")]
    HysteresisOutOfRange {
        /// Offending value.
        value: u8,
    },
    /// `up[0] − hysteresis` would be below 20 °C, the lowest down threshold the EC interface accepts.
    #[error("first threshold {up} °C minus hysteresis {hysteresis} °C is below 20 °C")]
    DownOutOfRange {
        /// First up threshold.
        up: u8,
        /// Hysteresis.
        hysteresis: u8,
    },
}

/// A complete, validated description of one fan's curve: the exponential [`FanCurveParams`], the speed below
/// the first threshold (`idle`), the six up thresholds and the hysteresis. [`crate::table::generate`] turns it
/// into an EC step table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FanCurveSpec {
    params: FanCurveParams,
    idle: u8,
    up: [u8; THRESHOLD_COUNT],
    hysteresis: u8,
}

impl FanCurveSpec {
    /// Validates and builds a specification.
    ///
    /// # Errors
    ///
    /// Returns a [`SpecError`] if any value is out of bounds (rule S3).
    pub fn new(
        params: FanCurveParams,
        idle: u8,
        up: [u8; THRESHOLD_COUNT],
        hysteresis: u8,
    ) -> Result<Self, SpecError> {
        if idle > params.min {
            return Err(SpecError::IdleAboveMin {
                idle,
                min: params.min,
            });
        }
        for (index, &value) in up.iter().enumerate() {
            if !THRESHOLD_RANGE.contains(&value) {
                return Err(SpecError::ThresholdOutOfRange { index, value });
            }
        }
        if let Some(index) = (1..THRESHOLD_COUNT).find(|&i| up[i] <= up[i - 1]) {
            return Err(SpecError::ThresholdsNotIncreasing { index });
        }
        if !HYSTERESIS_RANGE.contains(&hysteresis) {
            return Err(SpecError::HysteresisOutOfRange { value: hysteresis });
        }
        if up[0] < THRESHOLD_RANGE.start() + hysteresis {
            return Err(SpecError::DownOutOfRange {
                up: up[0],
                hysteresis,
            });
        }
        Ok(Self {
            params,
            idle,
            up,
            hysteresis,
        })
    }

    /// Exponential curve sampled at each threshold.
    #[must_use]
    pub const fn params(&self) -> FanCurveParams {
        self.params
    }

    /// Speed (%) of step 0, below the first threshold. 0 = fan stopped.
    #[must_use]
    pub const fn idle(&self) -> u8 {
        self.idle
    }

    /// Up thresholds (°C): step `i ≥ 1` is entered at `T ≥ up[i − 1]`.
    #[must_use]
    pub const fn up(&self) -> [u8; THRESHOLD_COUNT] {
        self.up
    }

    /// `up[i] − down[i]` (°C).
    #[must_use]
    pub const fn hysteresis(&self) -> u8 {
        self.hysteresis
    }
}

/// Curve presets, documented with their rationale in `docs/hardware/fan-safety.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurvePreset {
    /// Silencieux (Éco): off below 52 °C, gentle start at 30 %, 100 % from 88 °C (85 °C on the EC table, S2).
    Silent,
    /// Progressif (Balanced): off below 48 °C, starts at 30 %, 100 % from 85 °C.
    Progressive,
    /// Agressif (Gaming): never off (30 %), 100 % from 74 °C.
    Aggressive,
}

impl CurvePreset {
    /// All presets.
    pub const ALL: [Self; 3] = [Self::Silent, Self::Progressive, Self::Aggressive];

    /// Stable name (D-Bus, state file).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Progressive => "progressive",
            Self::Aggressive => "aggressive",
        }
    }

    /// Specification of the preset. Every preset is checked against [`FanCurveSpec::new`] in the tests.
    #[must_use]
    pub const fn spec(self) -> FanCurveSpec {
        let (t0, t1, min, max, k, idle, up, hysteresis) = match self {
            Self::Silent => (52, 88, 30, 100, 2.5, 0, [52, 60, 67, 75, 83, 85], 4),
            Self::Progressive => (48, 85, 30, 100, 2.2, 0, [48, 56, 64, 71, 79, 85], 4),
            Self::Aggressive => (40, 74, 40, 100, 1.4, 30, [40, 47, 54, 61, 68, 74], 3),
        };
        FanCurveSpec {
            params: FanCurveParams { t0, t1, min, max, k },
            idle,
            up,
            hysteresis,
        }
    }

    /// Exponential parameters of the preset.
    #[must_use]
    pub const fn params(self) -> FanCurveParams {
        self.spec().params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_params() {
        let p = FanCurveParams::new(50, 90, 20, 100, 2.5).unwrap();
        assert_eq!((p.t0(), p.t1(), p.min(), p.max()), (50, 90, 20, 100));
        assert!((p.k() - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn accepts_range_edges() {
        assert!(FanCurveParams::new(20, 100, 0, 100, 1.0).is_ok());
        assert!(FanCurveParams::new(20, 21, 0, 0, 4.0).is_ok());
        assert!(FanCurveParams::new(99, 100, 100, 100, 1.0).is_ok());
    }

    #[test]
    fn s3_rejects_temps_out_of_range() {
        assert_eq!(
            FanCurveParams::new(19, 90, 20, 100, 2.0),
            Err(ParamsError::TempOutOfRange { value: 19 })
        );
        assert_eq!(
            FanCurveParams::new(50, 101, 20, 100, 2.0),
            Err(ParamsError::TempOutOfRange { value: 101 })
        );
    }

    #[test]
    fn s3_rejects_t0_not_below_t1() {
        assert_eq!(
            FanCurveParams::new(60, 60, 20, 100, 2.0),
            Err(ParamsError::TempOrder { t0: 60, t1: 60 })
        );
        assert_eq!(
            FanCurveParams::new(70, 60, 20, 100, 2.0),
            Err(ParamsError::TempOrder { t0: 70, t1: 60 })
        );
    }

    #[test]
    fn s3_rejects_speeds_out_of_range() {
        assert_eq!(
            FanCurveParams::new(50, 90, 101, 101, 2.0),
            Err(ParamsError::SpeedOutOfRange { value: 101 })
        );
        assert_eq!(
            FanCurveParams::new(50, 90, 20, 101, 2.0),
            Err(ParamsError::SpeedOutOfRange { value: 101 })
        );
        assert_eq!(
            FanCurveParams::new(50, 90, 60, 40, 2.0),
            Err(ParamsError::SpeedOrder { min: 60, max: 40 })
        );
    }

    #[test]
    fn s3_rejects_bad_exponent() {
        for k in [0.99, 4.01, 0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                matches!(
                    FanCurveParams::new(50, 90, 20, 100, k),
                    Err(ParamsError::ExponentOutOfRange { .. })
                ),
                "k = {k} must be rejected"
            );
        }
    }

    #[test]
    fn speed_is_min_at_and_below_t0_and_max_at_and_above_t1() {
        let p = FanCurveParams::new(50, 90, 20, 90, 2.5).unwrap();
        assert!((p.speed_at(20) - 20.0).abs() < 1e-9);
        assert!((p.speed_at(50) - 20.0).abs() < 1e-9);
        assert!((p.speed_at(90) - 90.0).abs() < 1e-9);
        assert!((p.speed_at(100) - 90.0).abs() < 1e-9);
    }

    #[test]
    fn speed_follows_the_exponential_formula() {
        let p = FanCurveParams::new(50, 90, 20, 100, 2.5).unwrap();
        // 20 + 80 · (10/40)^2.5 = 20 + 80 · 0.03125 = 22.5
        assert!((p.speed_at(60) - 22.5).abs() < 1e-9);
        // 20 + 80 · (20/40)^2.5 ≈ 34.142
        assert!((p.speed_at(70) - 34.142_135_6).abs() < 1e-6);
        assert_eq!(p.speed_percent(70), 34);
    }

    #[test]
    fn fractional_temperatures_follow_the_same_curve() {
        let p = FanCurveParams::new(50, 90, 20, 100, 2.5).unwrap();
        for t in 20..=100u8 {
            assert!(
                (p.speed_at_celsius(f64::from(t)) - p.speed_at(t)).abs() < 1e-9,
                "{t}"
            );
            assert_eq!(p.speed_percent_celsius(f64::from(t)), p.speed_percent(t));
        }
        assert!(p.speed_at_celsius(60.5) > p.speed_at(60) && p.speed_at_celsius(60.5) < p.speed_at(61));
        assert!((p.speed_at_celsius(f64::NAN) - 100.0).abs() < 1e-9);
        assert!((p.speed_at_celsius(-5.0) - 20.0).abs() < 1e-9);
    }

    #[test]
    fn linear_exponent_gives_a_straight_line() {
        let p = FanCurveParams::new(40, 80, 0, 100, 1.0).unwrap();
        assert!((p.speed_at(60) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn presets_are_valid_params() {
        for preset in CurvePreset::ALL {
            let p = preset.params();
            assert_eq!(
                FanCurveParams::new(p.t0(), p.t1(), p.min(), p.max(), p.k()),
                Ok(p),
                "{preset:?}"
            );
        }
    }

    fn spec(idle: u8, up: [u8; 6], h: u8) -> Result<FanCurveSpec, SpecError> {
        FanCurveSpec::new(FanCurveParams::new(50, 82, 30, 100, 2.0).unwrap(), idle, up, h)
    }

    #[test]
    fn presets_are_valid_specs() {
        for preset in CurvePreset::ALL {
            let s = preset.spec();
            assert_eq!(
                FanCurveSpec::new(s.params(), s.idle(), s.up(), s.hysteresis()),
                Ok(s),
                "{preset:?}"
            );
            assert_eq!(preset.params(), s.params());
        }
    }

    #[test]
    fn s3_spec_rejects_idle_above_start_speed() {
        assert_eq!(
            spec(31, [50, 57, 64, 70, 76, 82], 4),
            Err(SpecError::IdleAboveMin { idle: 31, min: 30 })
        );
        assert!(spec(30, [50, 57, 64, 70, 76, 82], 4).is_ok());
    }

    #[test]
    fn s3_spec_rejects_thresholds_out_of_range_or_unordered() {
        assert_eq!(
            spec(0, [50, 57, 64, 70, 76, 101], 4),
            Err(SpecError::ThresholdOutOfRange { index: 5, value: 101 })
        );
        assert_eq!(
            spec(0, [19, 57, 64, 70, 76, 82], 2),
            Err(SpecError::ThresholdOutOfRange { index: 0, value: 19 })
        );
        assert_eq!(
            spec(0, [50, 57, 57, 70, 76, 82], 4),
            Err(SpecError::ThresholdsNotIncreasing { index: 2 })
        );
        assert_eq!(
            spec(0, [50, 57, 64, 60, 76, 82], 4),
            Err(SpecError::ThresholdsNotIncreasing { index: 3 })
        );
    }

    #[test]
    fn s3_spec_rejects_bad_hysteresis() {
        for h in [0, 1, 11, 255] {
            assert_eq!(
                spec(0, [50, 57, 64, 70, 76, 82], h),
                Err(SpecError::HysteresisOutOfRange { value: h })
            );
        }
        assert!(spec(0, [50, 57, 64, 70, 76, 82], 2).is_ok());
        assert!(spec(0, [50, 57, 64, 70, 76, 82], 10).is_ok());
        assert_eq!(
            spec(0, [23, 57, 64, 70, 76, 82], 4),
            Err(SpecError::DownOutOfRange {
                up: 23,
                hysteresis: 4
            })
        );
        assert!(spec(0, [24, 57, 64, 70, 76, 82], 4).is_ok());
    }

    #[test]
    fn preset_names_are_stable() {
        let names: Vec<&str> = CurvePreset::ALL.iter().map(|p| p.as_str()).collect();
        assert_eq!(names, ["silent", "progressive", "aggressive"]);
    }
}
