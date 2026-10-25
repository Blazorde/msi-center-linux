//! EC step tables: the measured fan curve model, generation from a [`FanCurveSpec`], safety rules S1–S3 and
//! the ordered write plan.
//!
//! # Table semantics (`docs/hardware/ec-map.md`)
//!
//! Each fan has 7 speeds (%) and 6 up / 6 down thresholds (°C). Step `i ≥ 1` is entered when `T ≥ up[i − 1]`
//! and left when `T < down[i − 1]`; step 0 applies below the first threshold. Because `down[i] < up[i]`, the
//! step reached while the temperature rises (the **ascending** step, [`StepCurve::ascending_step`]) is the
//! lowest step the EC can be in at `T`. Safety floors (S2) are therefore checked on the ascending step: it
//! is the worst case.

use std::fmt;
use std::ops::RangeInclusive;

use crate::curve::{FanCurveSpec, HYSTERESIS_RANGE, MAX_SPEED, STEP_COUNT, THRESHOLD_COUNT, THRESHOLD_RANGE};

/// Accepted range for table thresholds, in °C (kernel interface bounds).
pub const TABLE_TEMP_RANGE: RangeInclusive<u8> = THRESHOLD_RANGE;
/// The kernel rejects a speed table whose last (hottest) step is below this speed (%).
pub const MIN_LAST_SPEED: u8 = 80;
/// Smallest accepted gap `up[i] − down[i]`, in °C.
pub const MIN_HYSTERESIS: u8 = *HYSTERESIS_RANGE.start();

/// Speeds of one fan table.
pub type Speeds = [u8; STEP_COUNT];
/// Thresholds of one fan table.
pub type Thresholds = [u8; THRESHOLD_COUNT];

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

/// Which threshold table a [`TableError`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThresholdTable {
    /// Up thresholds.
    Up,
    /// Down thresholds.
    Down,
}

impl fmt::Display for ThresholdTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Up => "up",
            Self::Down => "down",
        })
    }
}

/// Why a table was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableError {
    /// S3: speed above 100 %.
    #[error("step {index}: speed {value} % is above 100 %")]
    SpeedOutOfRange {
        /// Step index.
        index: usize,
        /// Offending value.
        value: u8,
    },
    /// S1: speeds must never decrease.
    #[error("step {index}: speeds must never decrease")]
    SpeedsDecreasing {
        /// Index of the first step below its predecessor.
        index: usize,
    },
    /// The hottest step is below [`MIN_LAST_SPEED`] (kernel rule).
    #[error("last step speed {value} % is below 80 %")]
    LastSpeedTooLow {
        /// Offending value.
        value: u8,
    },
    /// S3: threshold outside [`TABLE_TEMP_RANGE`].
    #[error("{table} threshold {index}: {value} °C is outside 20..=100 °C")]
    TempOutOfRange {
        /// Table concerned.
        table: ThresholdTable,
        /// Threshold index.
        index: usize,
        /// Offending value.
        value: u8,
    },
    /// S1: thresholds must be strictly increasing.
    #[error("{table} threshold {index}: thresholds must be strictly increasing")]
    TempsNotIncreasing {
        /// Table concerned.
        table: ThresholdTable,
        /// Index of the first threshold not above its predecessor.
        index: usize,
    },
    /// `down[i]` must be strictly below `up[i]`.
    #[error("threshold {index}: down ({down} °C) must be below up ({up} °C)")]
    DownNotBelowUp {
        /// Threshold index.
        index: usize,
        /// Up threshold.
        up: u8,
        /// Down threshold.
        down: u8,
    },
    /// Hysteresis below [`MIN_HYSTERESIS`].
    #[error("threshold {index}: hysteresis {gap} °C is below 2 °C")]
    HysteresisTooSmall {
        /// Threshold index.
        index: usize,
        /// `up[index] − down[index]`.
        gap: u8,
    },
    /// S2: the speed of the ascending step at a floor temperature is too low.
    #[error("{speed} % in effect at {} °C, below the {} % floor", floor.temp, floor.speed)]
    BelowFloor {
        /// The violated floor.
        floor: SpeedFloor,
        /// Speed of the ascending step at `floor.temp`.
        speed: u8,
    },
}

/// The three tables of one fan exactly as the EC holds them. Unvalidated: the factory tables, for example,
/// break S2 (96 % maximum), so read-backs use this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawTables {
    /// Speeds (%).
    pub speeds: Speeds,
    /// Up thresholds (°C).
    pub up: Thresholds,
    /// Down thresholds (°C).
    pub down: Thresholds,
}

fn check_thresholds(table: ThresholdTable, temps: Thresholds) -> Result<(), TableError> {
    for (index, &value) in temps.iter().enumerate() {
        if !TABLE_TEMP_RANGE.contains(&value) {
            return Err(TableError::TempOutOfRange { table, index, value });
        }
    }
    if let Some(index) = (1..THRESHOLD_COUNT).find(|&i| temps[i] <= temps[i - 1]) {
        return Err(TableError::TempsNotIncreasing { table, index });
    }
    Ok(())
}

/// Number of thresholds `≤ temp`, i.e. the step reached at `temp` while heating up.
fn ascending(up: Thresholds, temp: u8) -> usize {
    up.iter().filter(|&&u| u <= temp).count()
}

impl RawTables {
    /// Checks the rules the kernel interface enforces (it answers `EINVAL` otherwise): speeds in 0..=100,
    /// non-decreasing, last speed ≥ [`MIN_LAST_SPEED`], thresholds strictly increasing in 20..=100, `down[i] < up[i]`. Safety floors and the
    /// minimum hysteresis are **not** kernel rules (see [`StepCurve::new`]).
    ///
    /// # Errors
    ///
    /// Returns the first violated rule.
    pub fn kernel_check(&self) -> Result<(), TableError> {
        for (index, &value) in self.speeds.iter().enumerate() {
            if value > MAX_SPEED {
                return Err(TableError::SpeedOutOfRange { index, value });
            }
        }
        if let Some(index) = (1..STEP_COUNT).find(|&i| self.speeds[i] < self.speeds[i - 1]) {
            return Err(TableError::SpeedsDecreasing { index });
        }
        let last = self.speeds[STEP_COUNT - 1];
        if last < MIN_LAST_SPEED {
            return Err(TableError::LastSpeedTooLow { value: last });
        }
        check_thresholds(ThresholdTable::Up, self.up)?;
        check_thresholds(ThresholdTable::Down, self.down)?;
        if let Some(index) = (0..THRESHOLD_COUNT).find(|&i| self.down[i] >= self.up[i]) {
            return Err(TableError::DownNotBelowUp {
                index,
                up: self.up[index],
                down: self.down[index],
            });
        }
        Ok(())
    }

    /// Speed of the ascending step at `temp` (the lowest speed the EC can apply at `temp`).
    #[must_use]
    pub fn min_speed_at(&self, temp: u8) -> u8 {
        self.speeds[ascending(self.up, temp)]
    }

    /// Applies one write of a [`write_plan`] to these tables.
    pub fn apply(&mut self, write: &TableWrite) {
        match *write {
            TableWrite::Speeds(s) => self.speeds = s,
            TableWrite::Up(u) => self.up = u,
            TableWrite::Down(d) => self.down = d,
        }
    }
}

/// A fan table that satisfies S1, S2, S3, `down < up` and the minimum hysteresis. Only obtainable through
/// [`StepCurve::new`] (rejects) or [`generate`] (raises and reports), so every `StepCurve` is safe to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StepCurve {
    speeds: Speeds,
    up: Thresholds,
    down: Thresholds,
}

impl StepCurve {
    /// Validates a raw table (e.g. from a future step editor). Nothing is corrected (S3).
    ///
    /// # Errors
    ///
    /// Returns the first violated rule as a [`TableError`].
    pub fn new(speeds: Speeds, up: Thresholds, down: Thresholds) -> Result<Self, TableError> {
        let raw = RawTables { speeds, up, down };
        raw.kernel_check()?;
        if let Some(index) = (0..THRESHOLD_COUNT).find(|&i| up[i] - down[i] < MIN_HYSTERESIS) {
            return Err(TableError::HysteresisTooSmall {
                index,
                gap: up[index] - down[index],
            });
        }
        for floor in SPEED_FLOORS {
            let speed = raw.min_speed_at(floor.temp);
            if speed < floor.speed {
                return Err(TableError::BelowFloor { floor, speed });
            }
        }
        Ok(Self { speeds, up, down })
    }

    /// Speeds (%), one per step.
    #[must_use]
    pub const fn speeds(&self) -> Speeds {
        self.speeds
    }

    /// Up thresholds (°C).
    #[must_use]
    pub const fn up(&self) -> Thresholds {
        self.up
    }

    /// Down thresholds (°C).
    #[must_use]
    pub const fn down(&self) -> Thresholds {
        self.down
    }

    /// The tables to write to the EC.
    #[must_use]
    pub const fn tables(&self) -> RawTables {
        RawTables {
            speeds: self.speeds,
            up: self.up,
            down: self.down,
        }
    }

    /// Step reached at `temp` while heating up (`0..=6`).
    #[must_use]
    pub fn ascending_step(&self, temp: u8) -> usize {
        ascending(self.up, temp)
    }

    /// Lowest speed the EC can apply at `temp` (speed of the ascending step).
    #[must_use]
    pub fn min_speed_at(&self, temp: u8) -> u8 {
        self.speeds[self.ascending_step(temp)]
    }
}

/// A step the S2 floors raised above the curve value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RaisedPoint {
    /// Step index (`0..=6`).
    pub index: usize,
    /// Temperature (°C) at which the step is entered (`up[index − 1]`, 0 for step 0).
    pub temp: u8,
    /// Speed given by the curve (%).
    pub curve_speed: u8,
    /// Speed written to the table (%).
    pub applied_speed: u8,
    /// The floor that set `applied_speed`.
    pub floor: SpeedFloor,
}

/// What the safety floors changed while generating. Must be reported to the user (D-Bus, UI), never hidden.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SafetyReport {
    /// Steps raised by S2, in index order.
    pub raised: Vec<RaisedPoint>,
}

impl SafetyReport {
    /// `true` when the curve already met every floor.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.raised.is_empty()
    }
}

/// A generated table and the report of what the safety floors changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GeneratedCurve {
    /// The table to send to the EC.
    pub curve: StepCurve,
    /// Steps raised by S2.
    pub report: SafetyReport,
}

/// Generates the EC table of `spec`: `speeds[0] = idle`, `speeds[i] = f(up[i − 1])` (rounded),
/// `down[i] = up[i] − hysteresis`. Then applies the S2 floors as an explicit, reported transformation: each
/// floor `(Tf, Sf)` raises the ascending step at `Tf` and every later step to at least `Sf` (keeps S1).
///
/// # Errors
///
/// Returns a [`TableError`] only if the result fails [`StepCurve::new`], which would be a bug: a valid spec
/// always produces a safe table (covered by property tests). Never panics.
pub fn generate(spec: &FanCurveSpec) -> Result<GeneratedCurve, TableError> {
    let params = spec.params();
    let up = spec.up();
    let down = up.map(|u| u - spec.hysteresis());
    let mut speeds = [spec.idle(); STEP_COUNT];
    for (speed, &threshold) in speeds[1..].iter_mut().zip(&up) {
        *speed = params.speed_percent(threshold);
    }
    let mut raised = Vec::new();
    for (index, speed) in speeds.iter_mut().enumerate() {
        let binding = SPEED_FLOORS
            .iter()
            .filter(|floor| ascending(up, floor.temp) <= index)
            .max_by_key(|floor| floor.speed);
        if let Some(&floor) = binding.filter(|floor| *speed < floor.speed) {
            raised.push(RaisedPoint {
                index,
                temp: index.checked_sub(1).map_or(0, |i| up[i]),
                curve_speed: *speed,
                applied_speed: floor.speed,
                floor,
            });
            *speed = floor.speed;
        }
    }
    let curve = StepCurve::new(speeds, up, down)?;
    Ok(GeneratedCurve {
        curve,
        report: SafetyReport { raised },
    })
}

/// One sysfs write of a fan table (the kernel takes a full table per write).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableWrite {
    /// `fan_curve_speeds`.
    Speeds(Speeds),
    /// `fan_curve_up_temps`.
    Up(Thresholds),
    /// `fan_curve_down_temps`.
    Down(Thresholds),
}

/// Ordered writes taking the EC from `current` to `target`, skipping writes that change nothing:
///
/// 1. speeds ← `max(current, target)` per step;
/// 2. down ← `min(current, target)` per threshold;
/// 3. up ← target;
/// 4. down ← target;
/// 5. speeds ← target.
///
/// If `current` passes [`RawTables::kernel_check`], every intermediate state passes it too (the kernel
/// checks `down[i] < up[i]` on each write), and every intermediate state is at least as safe as `current`
/// (steps 1–2) or as `target` (steps 3–5): the ascending speeds never drop below one of the two.
#[must_use]
pub fn write_plan(current: &RawTables, target: &StepCurve) -> Vec<TableWrite> {
    let goal = target.tables();
    let mut state = *current;
    let mut plan = Vec::new();
    let mut push = |state: &mut RawTables, write: TableWrite| {
        let before = *state;
        state.apply(&write);
        if *state != before {
            plan.push(write);
        }
    };
    let max_speeds: Speeds = std::array::from_fn(|i| current.speeds[i].max(goal.speeds[i]));
    let min_down: Thresholds = std::array::from_fn(|i| current.down[i].min(goal.down[i]));
    push(&mut state, TableWrite::Speeds(max_speeds));
    push(&mut state, TableWrite::Down(min_down));
    push(&mut state, TableWrite::Up(goal.up));
    push(&mut state, TableWrite::Down(goal.down));
    push(&mut state, TableWrite::Speeds(goal.speeds));
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::{CurvePreset, FanCurveParams};

    const UP: Thresholds = [50, 57, 64, 70, 76, 82];
    const DOWN: Thresholds = [46, 53, 60, 66, 72, 78];

    /// Factory CPU table of firmware 14K2EMS1.110.
    const FACTORY_CPU: RawTables = RawTables {
        speeds: [0, 36, 40, 50, 62, 80, 96],
        up: [57, 62, 70, 77, 83, 90],
        down: [53, 57, 65, 73, 80, 87],
    };

    #[test]
    fn accepts_a_safe_table() {
        let c = StepCurve::new([0, 30, 33, 43, 57, 76, 100], UP, DOWN).unwrap();
        assert_eq!(c.speeds(), [0, 30, 33, 43, 57, 76, 100]);
        assert_eq!((c.up(), c.down()), (UP, DOWN));
    }

    #[test]
    fn ascending_step_follows_the_up_thresholds() {
        let c = StepCurve::new([0, 30, 33, 43, 57, 76, 100], UP, DOWN).unwrap();
        assert_eq!(c.ascending_step(20), 0);
        assert_eq!(c.ascending_step(49), 0);
        assert_eq!(c.ascending_step(50), 1);
        assert_eq!(c.ascending_step(63), 2);
        assert_eq!(c.ascending_step(82), 6);
        assert_eq!(c.min_speed_at(60), 33);
        assert_eq!(c.min_speed_at(100), 100);
    }

    #[test]
    fn s1_rejects_decreasing_speeds() {
        assert_eq!(
            StepCurve::new([0, 30, 40, 35, 57, 76, 100], UP, DOWN),
            Err(TableError::SpeedsDecreasing { index: 3 })
        );
    }

    #[test]
    fn s1_rejects_non_increasing_thresholds() {
        assert_eq!(
            StepCurve::new(
                [0, 30, 33, 43, 57, 76, 100],
                [50, 57, 57, 70, 76, 82],
                [46, 53, 54, 66, 72, 78]
            ),
            Err(TableError::TempsNotIncreasing {
                table: ThresholdTable::Up,
                index: 2
            })
        );
        assert_eq!(
            StepCurve::new([0, 30, 33, 43, 57, 76, 100], UP, [46, 53, 53, 66, 72, 78]),
            Err(TableError::TempsNotIncreasing {
                table: ThresholdTable::Down,
                index: 2
            })
        );
    }

    #[test]
    fn kernel_rejects_a_last_speed_below_80() {
        let mut t = FACTORY_CPU;
        t.speeds[5] = 70;
        t.speeds[6] = 79;
        assert_eq!(t.kernel_check(), Err(TableError::LastSpeedTooLow { value: 79 }));
        t.speeds[6] = 80;
        assert_eq!(t.kernel_check(), Ok(()));
    }

    #[test]
    fn s3_rejects_out_of_range_values() {
        assert_eq!(
            StepCurve::new([0, 30, 33, 43, 57, 76, 101], UP, DOWN),
            Err(TableError::SpeedOutOfRange { index: 6, value: 101 })
        );
        assert_eq!(
            StepCurve::new([0, 30, 33, 43, 57, 76, 100], [50, 57, 64, 70, 76, 101], DOWN),
            Err(TableError::TempOutOfRange {
                table: ThresholdTable::Up,
                index: 5,
                value: 101
            })
        );
        assert_eq!(
            StepCurve::new([0, 30, 33, 43, 57, 76, 100], UP, [19, 53, 60, 66, 72, 78]),
            Err(TableError::TempOutOfRange {
                table: ThresholdTable::Down,
                index: 0,
                value: 19
            })
        );
    }

    #[test]
    fn rejects_down_not_below_up_and_small_hysteresis() {
        assert_eq!(
            StepCurve::new([0, 30, 33, 43, 57, 76, 100], UP, [46, 53, 64, 66, 72, 78]),
            Err(TableError::DownNotBelowUp {
                index: 2,
                up: 64,
                down: 64
            })
        );
        assert_eq!(
            StepCurve::new([0, 30, 33, 43, 57, 76, 100], UP, [46, 53, 60, 69, 72, 78]),
            Err(TableError::HysteresisTooSmall { index: 3, gap: 1 })
        );
        assert!(StepCurve::new([0, 30, 33, 43, 57, 76, 100], UP, [48, 55, 62, 68, 74, 80]).is_ok());
    }

    #[test]
    fn s2_floors_are_checked_on_the_ascending_step() {
        // At 60 °C the ascending step is 2 (50 and 57 reached): 29 % < 30 %.
        assert_eq!(
            StepCurve::new([0, 29, 29, 43, 57, 76, 100], UP, DOWN),
            Err(TableError::BelowFloor {
                floor: SPEED_FLOORS[0],
                speed: 29
            })
        );
        // At 75 °C the ascending step is 4.
        assert_eq!(
            StepCurve::new([0, 30, 33, 43, 49, 76, 100], UP, DOWN),
            Err(TableError::BelowFloor {
                floor: SPEED_FLOORS[1],
                speed: 49
            })
        );
        // Last threshold above 85 °C: step 5 is in effect at 85 °C and must be 100 %.
        assert_eq!(
            StepCurve::new(
                [0, 30, 33, 43, 57, 76, 100],
                [50, 57, 64, 70, 76, 86],
                [46, 53, 60, 66, 72, 82]
            ),
            Err(TableError::BelowFloor {
                floor: SPEED_FLOORS[2],
                speed: 76
            })
        );
    }

    #[test]
    fn s2_factory_table_is_unsafe_but_kernel_valid() {
        assert_eq!(FACTORY_CPU.kernel_check(), Ok(()));
        let err = StepCurve::new(FACTORY_CPU.speeds, FACTORY_CPU.up, FACTORY_CPU.down).unwrap_err();
        // 36 % at 60 °C and 50 % at 75 °C pass; at 85 °C step 5 (80 %) breaks the 100 % floor.
        assert_eq!(
            err,
            TableError::BelowFloor {
                floor: SPEED_FLOORS[2],
                speed: 80
            }
        );
    }

    #[test]
    fn presets_generate_the_documented_tables() {
        let expected = [
            (
                CurvePreset::Silent,
                [0, 30, 33, 43, 57, 76, 100],
                [50, 57, 64, 70, 76, 82],
                [46, 53, 60, 66, 72, 78],
            ),
            (
                CurvePreset::Progressive,
                [0, 30, 34, 45, 60, 78, 100],
                [42, 50, 58, 65, 72, 79],
                [38, 46, 54, 61, 68, 75],
            ),
            (
                CurvePreset::Aggressive,
                [30, 40, 47, 57, 71, 86, 100],
                [40, 47, 54, 61, 68, 74],
                [37, 44, 51, 58, 65, 71],
            ),
        ];
        for (preset, speeds, up, down) in expected {
            let g = generate(&preset.spec()).unwrap();
            assert_eq!(
                (g.curve.speeds(), g.curve.up(), g.curve.down()),
                (speeds, up, down),
                "{preset:?}"
            );
            assert!(g.report.is_clean(), "{preset:?}: {:?}", g.report);
        }
    }

    #[test]
    fn silent_preset_meets_the_owner_goals() {
        let c = generate(&CurvePreset::Silent.spec()).unwrap().curve;
        // Off at idle up to 49 °C, starts gently (30 %) before 55 °C.
        assert_eq!(c.min_speed_at(49), 0);
        assert_eq!(c.min_speed_at(50), 30);
        // No step larger than 25 points: no "off then full blast".
        assert!(c.speeds()[1..].windows(2).all(|w| w[1] - w[0] <= 25), "{c:?}");
    }

    #[test]
    fn s2_floor_raises_and_reports_steps() {
        // Flat 0 % curve with late thresholds: every floor binds.
        let spec = FanCurveSpec::new(
            FanCurveParams::new(90, 100, 0, 0, 1.0).unwrap(),
            0,
            [55, 65, 80, 88, 92, 96],
            4,
        )
        .unwrap();
        let g = generate(&spec).unwrap();
        // Ascending step at 60 °C = 1, at 75 °C = 2, at 85 °C = 3.
        assert_eq!(g.curve.speeds(), [0, 30, 50, 100, 100, 100, 100]);
        let raised: Vec<(usize, u8, u8, u8, u8)> = g
            .report
            .raised
            .iter()
            .map(|p| (p.index, p.temp, p.curve_speed, p.applied_speed, p.floor.temp))
            .collect();
        assert_eq!(
            raised,
            vec![
                (1, 55, 0, 30, 60),
                (2, 65, 0, 50, 75),
                (3, 80, 0, 100, 85),
                (4, 88, 0, 100, 85),
                (5, 92, 0, 100, 85),
                (6, 96, 0, 100, 85),
            ]
        );
    }

    #[test]
    fn s2_floor_can_raise_the_idle_step() {
        // First threshold above 60 °C: step 0 is in effect at 60 °C.
        let spec = FanCurveSpec::new(
            FanCurveParams::new(61, 84, 30, 100, 1.0).unwrap(),
            0,
            [61, 66, 71, 76, 80, 84],
            4,
        )
        .unwrap();
        let g = generate(&spec).unwrap();
        assert_eq!(g.curve.speeds()[0], 30);
        assert_eq!(g.report.raised[0].index, 0);
        assert_eq!(g.report.raised[0].temp, 0);
    }

    fn run_plan(current: &RawTables, target: &StepCurve) -> Vec<RawTables> {
        let mut state = *current;
        write_plan(current, target)
            .iter()
            .map(|w| {
                state.apply(w);
                state
            })
            .collect()
    }

    #[test]
    fn write_plan_from_factory_keeps_kernel_rules_at_every_step() {
        for preset in CurvePreset::ALL {
            let target = generate(&preset.spec()).unwrap().curve;
            let states = run_plan(&FACTORY_CPU, &target);
            for s in &states {
                assert_eq!(s.kernel_check(), Ok(()), "{preset:?}: {s:?}");
            }
            assert_eq!(states.last(), Some(&target.tables()));
        }
    }

    #[test]
    fn write_plan_lowering_thresholds_writes_down_first() {
        // Factory → Aggressive lowers every threshold: writing up first would put up[0] = 40 below down[0] = 53.
        // Aggressive speeds are all above the factory ones, so the final speeds write is not needed.
        let target = generate(&CurvePreset::Aggressive.spec()).unwrap().curve;
        let plan = write_plan(&FACTORY_CPU, &target);
        let kinds: Vec<&str> = plan
            .iter()
            .map(|w| match w {
                TableWrite::Speeds(_) => "speeds",
                TableWrite::Up(_) => "up",
                TableWrite::Down(_) => "down",
            })
            .collect();
        assert_eq!(kinds, ["speeds", "down", "up"]);
    }

    #[test]
    fn write_plan_raising_thresholds_writes_up_first() {
        let low = generate(&CurvePreset::Aggressive.spec()).unwrap().curve;
        let high = generate(&CurvePreset::Silent.spec()).unwrap().curve;
        // Speeds go down, so they are lowered last, after the thresholds moved up.
        let plan = write_plan(&low.tables(), &high);
        assert_eq!(
            plan,
            [
                TableWrite::Up(high.up()),
                TableWrite::Down(high.down()),
                TableWrite::Speeds(high.speeds())
            ]
        );
    }

    #[test]
    fn write_plan_is_empty_when_already_applied() {
        let c = generate(&CurvePreset::Progressive.spec()).unwrap().curve;
        assert_eq!(write_plan(&c.tables(), &c), Vec::new());
    }
}
