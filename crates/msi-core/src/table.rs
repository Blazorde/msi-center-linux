//! EC curve tables: layout (breakpoints), sampling of the exponential curve, and safety rules S1–S3.
//!
//! # Table semantics
//!
//! A table is a list of `(temperature, speed)` points. How the EC uses it between breakpoints is not known yet
//! (step or interpolation, see `docs/hardware/ec-map.md` once the spike is done). Safety checks therefore use the
//! **step** reading, which is the conservative lower bound of both: the speed in effect at `T` is the speed of the
//! last point whose temperature is `≤ T` (see [`CurveTable::effective_speed`]). Because the speed below the first
//! breakpoint is unknown too, the first breakpoint must be at or below the lowest S2 floor temperature (60 °C).

use std::ops::RangeInclusive;

use crate::curve::{FanCurveParams, MAX_SPEED};

/// Accepted range for table temperatures, in °C.
pub const TABLE_TEMP_RANGE: RangeInclusive<u8> = 0..=100;
/// Accepted number of points in a table.
pub const POINT_COUNT_RANGE: RangeInclusive<usize> = 2..=16;

/// A minimum speed that must be in effect from `temp` upwards (rule S2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SpeedFloor {
    /// Temperature (°C) from which the floor applies.
    pub temp: u8,
    /// Minimum speed (%).
    pub speed: u8,
}

/// Rule S2: ≥ 30 % from 60 °C, ≥ 50 % from 75 °C, 100 % from 85 °C. Sorted by temperature and speed.
pub const SPEED_FLOORS: [SpeedFloor; 3] = [
    SpeedFloor { temp: 60, speed: 30 },
    SpeedFloor { temp: 75, speed: 50 },
    SpeedFloor { temp: 85, speed: 100 },
];

/// Provisional breakpoints of the EC curve table.
///
/// TODO(ec-spike): replace with the real point count and temperatures from `docs/hardware/ec-map.md`. Seven
/// points is the usual size on MSI laptops of this generation; the temperatures include the S2 floor
/// temperatures (60, 75, 85 °C) so the floors bind exactly where they are defined.
pub const PROVISIONAL_LAYOUT_TEMPS: [u8; 7] = [40, 50, 60, 70, 75, 80, 85];

/// Why a layout or a table was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableError {
    /// Point count outside [`POINT_COUNT_RANGE`].
    #[error("a table needs 2..=16 points, got {count}")]
    PointCount {
        /// Number of points given.
        count: usize,
    },
    /// `temps` and `speeds` differ in length.
    #[error("{temps} temperatures for {speeds} speeds")]
    LengthMismatch {
        /// Number of temperatures.
        temps: usize,
        /// Number of speeds.
        speeds: usize,
    },
    /// S3: temperature outside [`TABLE_TEMP_RANGE`].
    #[error("point {index}: temperature {value} °C is above 100 °C")]
    TempOutOfRange {
        /// Point index.
        index: usize,
        /// Offending value.
        value: u8,
    },
    /// S3: speed above 100 %.
    #[error("point {index}: speed {value} % is above 100 %")]
    SpeedOutOfRange {
        /// Point index.
        index: usize,
        /// Offending value.
        value: u8,
    },
    /// S1: temperatures must be strictly increasing.
    #[error("point {index}: temperatures must be strictly increasing")]
    TempsNotIncreasing {
        /// Index of the first point that is not above its predecessor.
        index: usize,
    },
    /// S1: speeds must never decrease.
    #[error("point {index}: speeds must never decrease")]
    SpeedsDecreasing {
        /// Index of the first point below its predecessor.
        index: usize,
    },
    /// S2 cannot be checked: the first breakpoint is above a floor temperature.
    #[error("first breakpoint ({first} °C) is above the {floor_temp} °C safety floor")]
    FloorNotCovered {
        /// First breakpoint temperature.
        first: u8,
        /// Floor temperature not covered.
        floor_temp: u8,
    },
    /// S2: the speed in effect at a floor temperature is too low.
    #[error("{speed} % in effect at {} °C, below the {} % floor", floor.temp, floor.speed)]
    BelowFloor {
        /// The violated floor.
        floor: SpeedFloor,
        /// Speed in effect at `floor.temp`.
        speed: u8,
    },
}

/// Checks point count, temperature range, strictly increasing temperatures and S2 coverage.
fn check_temps(temps: &[u8]) -> Result<(), TableError> {
    if !POINT_COUNT_RANGE.contains(&temps.len()) {
        return Err(TableError::PointCount { count: temps.len() });
    }
    for (index, &value) in temps.iter().enumerate() {
        if !TABLE_TEMP_RANGE.contains(&value) {
            return Err(TableError::TempOutOfRange { index, value });
        }
    }
    if let Some(index) = (1..temps.len()).find(|&i| temps[i] <= temps[i - 1]) {
        return Err(TableError::TempsNotIncreasing { index });
    }
    let first = temps[0];
    let lowest_floor = SPEED_FLOORS[0].temp;
    if first > lowest_floor {
        return Err(TableError::FloorNotCovered {
            first,
            floor_temp: lowest_floor,
        });
    }
    Ok(())
}

/// Index of the point in effect at `temp` (step reading): last point `≤ temp`, or the first one.
fn effective_index(temps: &[u8], temp: u8) -> usize {
    temps.iter().rposition(|&t| t <= temp).unwrap_or(0)
}

/// Temperature breakpoints of the EC curve table. Validated: 2..=16 strictly increasing temperatures in
/// 0..=100 °C, the first one at or below 60 °C.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CurveLayout {
    temps: Vec<u8>,
}

impl CurveLayout {
    /// Validates and builds a layout.
    ///
    /// # Errors
    ///
    /// Returns a [`TableError`] if the breakpoints are invalid (see the type documentation).
    pub fn new(temps: Vec<u8>) -> Result<Self, TableError> {
        check_temps(&temps)?;
        Ok(Self { temps })
    }

    /// Provisional layout ([`PROVISIONAL_LAYOUT_TEMPS`]), pending the EC spike.
    #[must_use]
    pub fn provisional() -> Self {
        Self {
            temps: PROVISIONAL_LAYOUT_TEMPS.to_vec(),
        }
    }

    /// Number of points.
    #[must_use]
    pub fn points(&self) -> usize {
        self.temps.len()
    }

    /// Breakpoint temperatures (°C).
    #[must_use]
    pub fn temps(&self) -> &[u8] {
        &self.temps
    }
}

impl Default for CurveLayout {
    fn default() -> Self {
        Self::provisional()
    }
}

/// A curve table that satisfies S1, S2 and S3. The only way to get one is [`CurveTable::new`] (which rejects
/// unsafe tables) or [`sample`], so every `CurveTable` is safe to send to the EC.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CurveTable {
    temps: Vec<u8>,
    speeds: Vec<u8>,
}

impl CurveTable {
    /// Validates an arbitrary table (e.g. from a future point editor). Nothing is corrected: an unsafe
    /// table is rejected (S3).
    ///
    /// # Errors
    ///
    /// Returns the first violated rule as a [`TableError`].
    pub fn new(temps: Vec<u8>, speeds: Vec<u8>) -> Result<Self, TableError> {
        if temps.len() != speeds.len() {
            return Err(TableError::LengthMismatch {
                temps: temps.len(),
                speeds: speeds.len(),
            });
        }
        check_temps(&temps)?;
        for (index, &value) in speeds.iter().enumerate() {
            if value > MAX_SPEED {
                return Err(TableError::SpeedOutOfRange { index, value });
            }
        }
        if let Some(index) = (1..speeds.len()).find(|&i| speeds[i] < speeds[i - 1]) {
            return Err(TableError::SpeedsDecreasing { index });
        }
        let table = Self { temps, speeds };
        for floor in SPEED_FLOORS {
            let speed = table.effective_speed(floor.temp);
            if speed < floor.speed {
                return Err(TableError::BelowFloor { floor, speed });
            }
        }
        Ok(table)
    }

    /// Breakpoint temperatures (°C).
    #[must_use]
    pub fn temps(&self) -> &[u8] {
        &self.temps
    }

    /// Speeds (%), one per breakpoint.
    #[must_use]
    pub fn speeds(&self) -> &[u8] {
        &self.speeds
    }

    /// Number of points.
    #[must_use]
    pub fn points(&self) -> usize {
        self.temps.len()
    }

    /// Speed in effect at `temp` under the conservative step reading (see the module documentation).
    #[must_use]
    pub fn effective_speed(&self, temp: u8) -> u8 {
        self.speeds[effective_index(&self.temps, temp)]
    }
}

/// A point the S2 floors raised above the curve value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RaisedPoint {
    /// Point index.
    pub index: usize,
    /// Point temperature (°C).
    pub temp: u8,
    /// Speed given by the curve (%).
    pub curve_speed: u8,
    /// Speed written to the table (%).
    pub applied_speed: u8,
    /// The floor that set `applied_speed`.
    pub floor: SpeedFloor,
}

/// What the safety floors changed while sampling. Must be reported to the user (D-Bus, UI), never hidden.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SafetyReport {
    /// Points raised by S2, in index order.
    pub raised: Vec<RaisedPoint>,
}

impl SafetyReport {
    /// `true` when the curve already met every floor.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.raised.is_empty()
    }
}

/// A sampled curve and the report of what the safety floors changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SampledCurve {
    /// The table to send to the EC.
    pub table: CurveTable,
    /// Points raised by S2.
    pub report: SafetyReport,
}

/// Samples `params` on `layout`, then applies the S2 floors as an explicit, reported transformation.
///
/// Each floor `(Tf, Sf)` raises the point in effect at `Tf` and every later point to at least `Sf`; this keeps
/// the speeds non-decreasing (S1).
///
/// # Errors
///
/// Returns a [`TableError`] only if the result fails [`CurveTable::new`], which would be a bug: valid params
/// on a valid layout always produce a safe table (covered by property tests). Never panics.
pub fn sample(params: &FanCurveParams, layout: &CurveLayout) -> Result<SampledCurve, TableError> {
    let temps = layout.temps();
    let mut speeds: Vec<u8> = temps.iter().map(|&t| params.speed_percent(t)).collect();
    let mut raised = Vec::new();
    for (index, (&temp, speed)) in temps.iter().zip(speeds.iter_mut()).enumerate() {
        let binding = SPEED_FLOORS
            .iter()
            .filter(|floor| effective_index(temps, floor.temp) <= index)
            .max_by_key(|floor| floor.speed);
        if let Some(&floor) = binding.filter(|floor| *speed < floor.speed) {
            raised.push(RaisedPoint {
                index,
                temp,
                curve_speed: *speed,
                applied_speed: floor.speed,
                floor,
            });
            *speed = floor.speed;
        }
    }
    let table = CurveTable::new(temps.to_vec(), speeds)?;
    Ok(SampledCurve {
        table,
        report: SafetyReport { raised },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurvePreset;

    fn safe_temps() -> Vec<u8> {
        vec![40, 50, 60, 70, 75, 80, 85]
    }

    #[test]
    fn provisional_layout_is_valid_with_seven_points() {
        let layout = CurveLayout::default();
        assert_eq!(layout.points(), 7);
        assert_eq!(CurveLayout::new(PROVISIONAL_LAYOUT_TEMPS.to_vec()), Ok(layout));
    }

    #[test]
    fn layout_rejects_bad_point_counts() {
        assert_eq!(
            CurveLayout::new(vec![50]),
            Err(TableError::PointCount { count: 1 })
        );
        assert_eq!(
            CurveLayout::new((0..17).collect()),
            Err(TableError::PointCount { count: 17 })
        );
        assert!(CurveLayout::new((0..16).collect()).is_ok());
    }

    #[test]
    fn s1_layout_rejects_non_increasing_temps() {
        assert_eq!(
            CurveLayout::new(vec![40, 60, 60]),
            Err(TableError::TempsNotIncreasing { index: 2 })
        );
        assert_eq!(
            CurveLayout::new(vec![40, 60, 50]),
            Err(TableError::TempsNotIncreasing { index: 2 })
        );
    }

    #[test]
    fn s3_layout_rejects_temps_above_100() {
        assert_eq!(
            CurveLayout::new(vec![40, 101]),
            Err(TableError::TempOutOfRange { index: 1, value: 101 })
        );
    }

    #[test]
    fn s2_layout_must_cover_the_lowest_floor() {
        assert_eq!(
            CurveLayout::new(vec![61, 70, 85]),
            Err(TableError::FloorNotCovered {
                first: 61,
                floor_temp: 60
            })
        );
        assert!(CurveLayout::new(vec![60, 70, 85]).is_ok());
    }

    #[test]
    fn accepts_a_safe_table() {
        let table = CurveTable::new(safe_temps(), vec![0, 20, 30, 40, 50, 70, 100]).unwrap();
        assert_eq!(table.points(), 7);
        assert_eq!(table.speeds(), &[0, 20, 30, 40, 50, 70, 100]);
    }

    #[test]
    fn rejects_length_mismatch() {
        assert_eq!(
            CurveTable::new(safe_temps(), vec![100; 6]),
            Err(TableError::LengthMismatch { temps: 7, speeds: 6 })
        );
    }

    #[test]
    fn s1_table_rejects_non_increasing_temps() {
        assert_eq!(
            CurveTable::new(vec![40, 50, 50, 85], vec![100; 4]),
            Err(TableError::TempsNotIncreasing { index: 2 })
        );
    }

    #[test]
    fn s1_table_rejects_decreasing_speeds() {
        assert_eq!(
            CurveTable::new(safe_temps(), vec![0, 20, 40, 35, 50, 70, 100]),
            Err(TableError::SpeedsDecreasing { index: 3 })
        );
    }

    #[test]
    fn s1_table_accepts_equal_speeds() {
        assert!(CurveTable::new(safe_temps(), vec![30, 30, 30, 50, 50, 100, 100]).is_ok());
    }

    #[test]
    fn s3_table_rejects_speed_above_100() {
        assert_eq!(
            CurveTable::new(safe_temps(), vec![0, 20, 30, 40, 50, 70, 101]),
            Err(TableError::SpeedOutOfRange { index: 6, value: 101 })
        );
    }

    #[test]
    fn s2_floor_30_percent_at_60() {
        let floor = SPEED_FLOORS[0];
        assert_eq!(
            CurveTable::new(safe_temps(), vec![0, 20, 29, 40, 50, 70, 100]),
            Err(TableError::BelowFloor { floor, speed: 29 })
        );
    }

    #[test]
    fn s2_floor_50_percent_at_75() {
        let floor = SPEED_FLOORS[1];
        assert_eq!(
            CurveTable::new(safe_temps(), vec![0, 20, 30, 40, 49, 70, 100]),
            Err(TableError::BelowFloor { floor, speed: 49 })
        );
    }

    #[test]
    fn s2_floor_100_percent_at_85() {
        let floor = SPEED_FLOORS[2];
        assert_eq!(
            CurveTable::new(safe_temps(), vec![0, 20, 30, 40, 50, 70, 99]),
            Err(TableError::BelowFloor { floor, speed: 99 })
        );
    }

    #[test]
    fn s2_floor_uses_the_point_in_effect_below_the_floor_temp() {
        // No point at 60 °C: the 55 °C point is in effect at 60 °C and must already meet the 30 % floor.
        let floor = SPEED_FLOORS[0];
        assert_eq!(
            CurveTable::new(vec![40, 55, 65, 75, 85], vec![10, 25, 40, 50, 100]),
            Err(TableError::BelowFloor { floor, speed: 25 })
        );
        assert!(CurveTable::new(vec![40, 55, 65, 75, 85], vec![10, 30, 40, 50, 100]).is_ok());
    }

    #[test]
    fn s2_floor_100_percent_needs_a_point_at_or_below_85() {
        // The last point is at 90 °C: at 85 °C the 80 °C point is in effect.
        let floor = SPEED_FLOORS[2];
        assert_eq!(
            CurveTable::new(vec![50, 60, 75, 80, 90], vec![20, 30, 50, 80, 100]),
            Err(TableError::BelowFloor { floor, speed: 80 })
        );
    }

    #[test]
    fn effective_speed_is_a_step_function() {
        let table = CurveTable::new(safe_temps(), vec![0, 20, 30, 40, 50, 70, 100]).unwrap();
        assert_eq!(table.effective_speed(0), 0);
        assert_eq!(table.effective_speed(49), 0);
        assert_eq!(table.effective_speed(50), 20);
        assert_eq!(table.effective_speed(74), 40);
        assert_eq!(table.effective_speed(100), 100);
    }

    #[test]
    fn s2_floor_silent_preset_is_raised_and_reported() {
        let sampled = sample(&CurvePreset::Silent.params(), &CurveLayout::default()).unwrap();
        // Curve: 20, 20, 22.5→23, 34, 45, 59, 77 — 60 °C, 75 °C and 85 °C are below their floors.
        assert_eq!(sampled.table.speeds(), &[20, 20, 30, 34, 50, 59, 100]);
        let raised: Vec<(u8, u8, u8)> = sampled
            .report
            .raised
            .iter()
            .map(|p| (p.temp, p.curve_speed, p.applied_speed))
            .collect();
        assert_eq!(raised, vec![(60, 23, 30), (75, 45, 50), (85, 77, 100)]);
        assert!(!sampled.report.is_clean());
    }

    #[test]
    fn s2_floor_aggressive_preset_needs_no_correction() {
        let sampled = sample(&CurvePreset::Aggressive.params(), &CurveLayout::default()).unwrap();
        assert!(sampled.report.is_clean(), "{:?}", sampled.report);
    }

    #[test]
    fn s2_floor_raises_points_after_the_floor_when_layout_skips_the_floor_temp() {
        // Flat 0 % curve: the 55 °C point is in effect at 60 °C, the 65 °C point at 75 °C and the 80 °C
        // point at 85 °C, so each of them carries the floor of the next floor temperature.
        let params = FanCurveParams::new(90, 100, 0, 0, 1.0).unwrap();
        let layout = CurveLayout::new(vec![40, 55, 65, 80, 90]).unwrap();
        let sampled = sample(&params, &layout).unwrap();
        assert_eq!(sampled.table.speeds(), &[0, 30, 50, 100, 100]);
        let floors: Vec<u8> = sampled.report.raised.iter().map(|p| p.floor.temp).collect();
        assert_eq!(floors, vec![60, 75, 85, 85]);
    }

    #[test]
    fn s1_sampled_presets_are_monotonic() {
        for preset in CurvePreset::ALL {
            let sampled = sample(&preset.params(), &CurveLayout::default()).unwrap();
            assert!(
                sampled.table.speeds().windows(2).all(|w| w[0] <= w[1]),
                "{preset:?}"
            );
        }
    }

    #[test]
    fn sampling_never_lowers_the_curve() {
        let params = FanCurveParams::new(30, 60, 40, 100, 1.0).unwrap();
        let sampled = sample(&params, &CurveLayout::default()).unwrap();
        for (&t, &s) in sampled.table.temps().iter().zip(sampled.table.speeds()) {
            assert!(s >= params.speed_percent(t));
        }
        assert!(sampled.report.is_clean());
    }
}
