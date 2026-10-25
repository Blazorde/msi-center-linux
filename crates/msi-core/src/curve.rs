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
        let (min, max) = (f64::from(self.min), f64::from(self.max));
        if temp <= self.t0 {
            return min;
        }
        if temp >= self.t1 {
            return max;
        }
        let x = f64::from(temp - self.t0) / f64::from(self.t1 - self.t0);
        (max - min).mul_add(x.powf(self.k), min).clamp(min, max)
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

/// Curve presets from `docs/hardware/fan-safety.md` (indicative, to be calibrated after the EC spike).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurvePreset {
    /// Silencieuse (Éco): T0 50, T1 90, 20–100 %, k 2.5.
    Silent,
    /// Progressive (Balanced): T0 45, T1 87, 25–100 %, k 2.0.
    Progressive,
    /// Agressive (Gaming): T0 40, T1 82, 35–100 %, k 1.5.
    Aggressive,
}

impl CurvePreset {
    /// All presets.
    pub const ALL: [Self; 3] = [Self::Silent, Self::Progressive, Self::Aggressive];

    /// Parameters of the preset. Every preset is checked against [`FanCurveParams::new`] in the tests.
    #[must_use]
    pub const fn params(self) -> FanCurveParams {
        let (t0, t1, min, max, k) = match self {
            Self::Silent => (50, 90, 20, 100, 2.5),
            Self::Progressive => (45, 87, 25, 100, 2.0),
            Self::Aggressive => (40, 82, 35, 100, 1.5),
        };
        FanCurveParams { t0, t1, min, max, k }
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
        let p = CurvePreset::Silent.params();
        // 20 + 80 · (10/40)^2.5 = 20 + 80 · 0.03125 = 22.5
        assert!((p.speed_at(60) - 22.5).abs() < 1e-9);
        // 20 + 80 · (20/40)^2.5 ≈ 34.142
        assert!((p.speed_at(70) - 34.142_135_6).abs() < 1e-6);
        assert_eq!(p.speed_percent(70), 34);
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
}
