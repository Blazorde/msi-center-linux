//! Staged CPU fan curve (ADR-0013): any number of stages, each with its own rise temperature, fall-back
//! temperature and speed.
//!
//! The EC only has 7 steps per fan, but the CPU fan is driven live by the daemon (ADR-0010: the EC ignores the
//! CPU thresholds we write and applies the speeds we write within 2 s). So the CPU fan can follow a curve with
//! as many stages as the owner wants; the 7 EC steps only carry the live target (see
//! [`crate::hybrid::hybrid_cpu_table_for`]). The GPU keeps its 7-step table.
//!
//! ```text
//!   speed
//!    100 │                              ┌─────────
//!     70 │                    ┌─────────┘ stage 3
//!     45 │          ┌─────────┘ stage 2
//!     30 │  ┌───────┘ stage 1
//!      0 │──┘ idle
//!        └──────────────────────────────────────── °C
//!           up₁    up₂      up₃
//! ```
//!
//! Stage `i` is **entered** when the temperature reaches `up[i]` and **left** (back to the stage below, or to
//! idle) when it falls under `down[i]`. `down[i] < up[i]` is the per-stage hysteresis: that is the "fall-back"
//! temperature, set by the owner for each stage.
//!
//! Three pure pieces, fed once per supervision tick (~1 Hz) by `msi-daemon`:
//!
//! 1. [`StagedCurve`]: the validated description (rule S3: out-of-bounds input is rejected, never corrected).
//! 2. [`StageSelector`]: which stage is in force, with the rise / fall-back hysteresis.
//! 3. [`StagedPilot`]: the live speed, with the same ramps as the exponential pilot (the fan never jumps up or
//!    down, except to start and to stop).
//!
//! # Safety
//!
//! - S1 holds on the curve: temperatures strictly increasing, speeds never decreasing.
//! - S2 floors (≥ 30 % from 60 °C, ≥ 50 % from 75 °C, 100 % from 85 °C) are applied as a `max()` on the
//!   target, whatever the stages say, and reported by [`StagedCurve::raised_floors`]: nothing is silent.
//! - A non-finite temperature selects the hottest stage (the safe side).
//! - The EC still holds the hot steps 4–6 at ≥ 60 / 80 / 100 % at all times (ADR-0010), so a frozen daemon is
//!   covered exactly as with the exponential curve. S4 and S9 are unchanged.

use std::ops::RangeInclusive;

use crate::curve::{MAX_SPEED, THRESHOLD_COUNT, THRESHOLD_RANGE};
use crate::hybrid::{SLEW_DOWN_PCT, SLEW_UP_PCT};
use crate::table::{MIN_HYSTERESIS, SPEED_FLOORS, SpeedFloor, StepCurve};

/// Most stages a curve can have: one per °C. The first stage rises at 22 °C at the earliest (its fall-back
/// must stay ≥ 20 °C with a gap of ≥ 2 °C) and the last at 100 °C, with strictly increasing rise
/// temperatures: 22..=100 gives 79 stages (checked by a test).
pub const MAX_STAGES: usize = 79;
/// Accepted range for the rise and fall-back temperatures, in °C (same bounds as the EC tables).
pub const STAGE_TEMP_RANGE: RangeInclusive<u8> = THRESHOLD_RANGE;

/// One stage of a [`StagedCurve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Stage {
    /// Temperature (°C) at and above which the stage is entered.
    pub up: u8,
    /// Temperature (°C) under which the stage is left (the fall-back). Strictly below `up`, by ≥ 2 °C.
    pub down: u8,
    /// Fan speed (%) while the stage is in force.
    pub speed: u8,
}

/// Why a staged curve was rejected (rule S3: rejected, never corrected).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StagedError {
    /// A curve needs at least one stage.
    #[error("a curve needs at least one stage")]
    NoStages,
    /// More than [`MAX_STAGES`] stages.
    #[error("{count} stages is above the maximum of 79")]
    TooManyStages {
        /// Number of stages given.
        count: usize,
    },
    /// A rise temperature is outside [`STAGE_TEMP_RANGE`].
    #[error("stage {index}: rise temperature {value} °C is outside 20..=100 °C")]
    UpOutOfRange {
        /// Stage index (0-based).
        index: usize,
        /// Offending value.
        value: u8,
    },
    /// A fall-back temperature is outside [`STAGE_TEMP_RANGE`].
    #[error("stage {index}: fall-back temperature {value} °C is outside 20..=100 °C")]
    DownOutOfRange {
        /// Stage index (0-based).
        index: usize,
        /// Offending value.
        value: u8,
    },
    /// The fall-back temperature must be strictly below the rise temperature.
    #[error("stage {index}: fall-back ({down} °C) must be below the rise ({up} °C)")]
    DownNotBelowUp {
        /// Stage index (0-based).
        index: usize,
        /// Rise temperature.
        up: u8,
        /// Fall-back temperature.
        down: u8,
    },
    /// Fewer than 2 °C between fall-back and rise: a sensor jittering by ±1 °C would make the fan hunt.
    #[error("stage {index}: {gap} °C between fall-back and rise is below 2 °C")]
    HysteresisTooSmall {
        /// Stage index (0-based).
        index: usize,
        /// `up − down`.
        gap: u8,
    },
    /// Rise temperatures must be strictly increasing.
    #[error("stage {index}: rise temperatures must be strictly increasing")]
    UpNotIncreasing {
        /// Index of the first stage not above its predecessor.
        index: usize,
    },
    /// Fall-back temperatures must be strictly increasing.
    #[error("stage {index}: fall-back temperatures must be strictly increasing")]
    DownNotIncreasing {
        /// Index of the first stage not above its predecessor.
        index: usize,
    },
    /// A speed is above 100 %.
    #[error("stage {index}: speed {value} % is above 100 %")]
    SpeedOutOfRange {
        /// Stage index (0-based).
        index: usize,
        /// Offending value.
        value: u8,
    },
    /// Speeds must never decrease from one stage to the next.
    #[error("stage {index}: speeds must never decrease")]
    SpeedsDecreasing {
        /// Index of the first stage below its predecessor.
        index: usize,
    },
    /// The idle speed must not exceed the first stage's speed.
    #[error("idle speed ({idle} %) must not exceed the first stage's speed ({first} %)")]
    IdleAboveFirst {
        /// Speed below the first stage.
        idle: u8,
        /// Speed of the first stage.
        first: u8,
    },
}

/// A floor of rule S2 that is above what the curve gives at its temperature, so it takes over there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FloorRaise {
    /// The floor that applies.
    pub floor: SpeedFloor,
    /// Speed (%) the stages alone give at `floor.temp` while heating up.
    pub curve_speed: u8,
}

/// A validated staged curve. Only obtainable through [`StagedCurve::new`], so an instance is always within
/// bounds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StagedCurve {
    idle: u8,
    stages: Vec<Stage>,
}

impl StagedCurve {
    /// Validates and builds a curve.
    ///
    /// # Errors
    ///
    /// Returns the first violated rule as a [`StagedError`] (S3: nothing is corrected).
    pub fn new(idle: u8, stages: Vec<Stage>) -> Result<Self, StagedError> {
        let Some(first) = stages.first() else {
            return Err(StagedError::NoStages);
        };
        if stages.len() > MAX_STAGES {
            return Err(StagedError::TooManyStages { count: stages.len() });
        }
        for (index, s) in stages.iter().enumerate() {
            if !STAGE_TEMP_RANGE.contains(&s.up) {
                return Err(StagedError::UpOutOfRange { index, value: s.up });
            }
            if !STAGE_TEMP_RANGE.contains(&s.down) {
                return Err(StagedError::DownOutOfRange { index, value: s.down });
            }
            if s.down >= s.up {
                return Err(StagedError::DownNotBelowUp {
                    index,
                    up: s.up,
                    down: s.down,
                });
            }
            if s.up - s.down < MIN_HYSTERESIS {
                return Err(StagedError::HysteresisTooSmall {
                    index,
                    gap: s.up - s.down,
                });
            }
            if s.speed > MAX_SPEED {
                return Err(StagedError::SpeedOutOfRange {
                    index,
                    value: s.speed,
                });
            }
        }
        for (i, pair) in stages.windows(2).enumerate() {
            let index = i + 1;
            if pair[1].up <= pair[0].up {
                return Err(StagedError::UpNotIncreasing { index });
            }
            if pair[1].down <= pair[0].down {
                return Err(StagedError::DownNotIncreasing { index });
            }
            if pair[1].speed < pair[0].speed {
                return Err(StagedError::SpeedsDecreasing { index });
            }
        }
        if idle > first.speed {
            return Err(StagedError::IdleAboveFirst {
                idle,
                first: first.speed,
            });
        }
        Ok(Self { idle, stages })
    }

    /// The staged equivalent of an EC step table: `speeds[0]` is the idle speed and each of the 6 thresholds
    /// becomes a stage (`up`, `down` and the speed of the step it enters). The starting point of the editor: a
    /// preset's table, which the owner then refines stage by stage.
    ///
    /// # Errors
    ///
    /// A [`StagedError`] only if `curve` breaks the staged rules, which a [`StepCurve`] cannot (same
    /// bounds, strictly increasing thresholds, hysteresis ≥ 2 °C, non-decreasing speeds).
    pub fn from_step_curve(curve: &StepCurve) -> Result<Self, StagedError> {
        let (speeds, up, down) = (curve.speeds(), curve.up(), curve.down());
        let stages = (0..THRESHOLD_COUNT)
            .map(|i| Stage {
                up: up[i],
                down: down[i],
                speed: speeds[i + 1],
            })
            .collect();
        Self::new(speeds[0], stages)
    }

    /// Speed (%) below the first stage. 0 = fan stopped.
    #[must_use]
    pub const fn idle(&self) -> u8 {
        self.idle
    }

    /// The stages, from the coolest to the hottest.
    #[must_use]
    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }

    /// Speed (%) of the first stage: the speed the fan starts at.
    #[must_use]
    pub fn start_speed(&self) -> u8 {
        self.stages.first().map_or(self.idle, |s| s.speed)
    }

    /// Highest speed (%) the pilot can ask for: the last stage's, or the top S2 floor if that is higher.
    #[must_use]
    pub fn max_speed(&self) -> u8 {
        let top = SPEED_FLOORS.last().map_or(MAX_SPEED, |f| f.speed);
        self.stages.last().map_or(top, |s| s.speed.max(top))
    }

    /// Index of the stage reached at `temp` while heating up (the highest whose rise is `≤ temp`), `None`
    /// below the first rise. It is the lowest stage the fan can be in at `temp`, so S2 is checked on it.
    #[must_use]
    pub fn ascending_stage(&self, temp: u8) -> Option<usize> {
        self.stages
            .iter()
            .take_while(|s| s.up <= temp)
            .count()
            .checked_sub(1)
    }

    /// Speed (%) of the stages alone at `temp` while heating up, before the S2 floors.
    #[must_use]
    pub fn ascending_speed(&self, temp: u8) -> u8 {
        self.ascending_stage(temp)
            .map_or(self.idle, |i| self.stages[i].speed)
    }

    /// Speed (%) at `temp` while heating up with the S2 floors applied: what the fan is at least asked for.
    #[must_use]
    pub fn effective_speed(&self, temp: u8) -> u8 {
        self.ascending_speed(temp).max(safety_floor(f64::from(temp)))
    }

    /// The S2 floors that are above the stages at their own temperature (empty when the curve already meets
    /// every floor). Shown to the owner, never applied silently.
    #[must_use]
    pub fn raised_floors(&self) -> Vec<FloorRaise> {
        SPEED_FLOORS
            .iter()
            .filter_map(|&floor| {
                let curve_speed = self.ascending_speed(floor.temp);
                (curve_speed < floor.speed).then_some(FloorRaise { floor, curve_speed })
            })
            .collect()
    }
}

/// Rule S2: the minimum speed (%) at `temp` °C, whatever the curve. 0 below the first floor. A non-finite
/// `temp` gives the highest floor (the safe side).
#[must_use]
pub fn safety_floor(temp: f64) -> u8 {
    if !temp.is_finite() {
        return SPEED_FLOORS.last().map_or(MAX_SPEED, |f| f.speed);
    }
    SPEED_FLOORS
        .iter()
        .filter(|f| temp >= f64::from(f.temp))
        .map(|f| f.speed)
        .max()
        .unwrap_or(0)
}

/// Which stage of a [`StagedCurve`] is in force, with the per-stage rise / fall-back hysteresis.
///
/// Heating up, the next stage is entered at its `up`. Cooling down, the stage is left under its `down`.
/// Because `down < up` for every stage, a temperature between the two keeps the current stage: the fan does not
/// hunt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageSelector {
    current: Option<usize>,
}

impl StageSelector {
    /// A selector with no stage in force (idle).
    #[must_use]
    pub const fn new() -> Self {
        Self { current: None }
    }

    /// Forgets the stage in force.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Stage in force (`None` = idle).
    #[must_use]
    pub const fn current(&self) -> Option<usize> {
        self.current
    }

    /// Moves to the stage that `temp` (°C, filtered) calls for and returns it. A non-finite `temp` selects the
    /// hottest stage.
    pub fn update(&mut self, curve: &StagedCurve, temp: f64) -> Option<usize> {
        let stages = curve.stages();
        let last = stages.len().checked_sub(1)?;
        if !temp.is_finite() {
            self.current = Some(last);
            return self.current;
        }
        // The curve may have been replaced by a shorter one since the last update.
        let mut cur = self.current.map(|i| i.min(last));
        while let Some(i) = cur {
            if temp < f64::from(stages[i].down) {
                cur = i.checked_sub(1);
            } else {
                break;
            }
        }
        loop {
            let next = cur.map_or(0, |i| i + 1);
            match stages.get(next) {
                Some(s) if temp >= f64::from(s.up) => cur = Some(next),
                _ => break,
            }
        }
        self.current = cur;
        cur
    }
}

/// Live speed of the CPU fan under a [`StagedCurve`].
///
/// - Target: the speed of the stage in force, raised to the S2 floor at the filtered temperature.
/// - Starting (from idle, or from nothing): the output jumps to the first stage's speed, then follows the
///   ramps. Stopping (the target falls under the start speed): the output jumps to the target.
/// - Otherwise at most [`SLEW_UP_PCT`] up and [`SLEW_DOWN_PCT`] down per update (≈ 1 s): quiet ramps.
///
/// No dead band: the per-stage hysteresis already keeps the target still while the temperature wobbles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StagedPilot {
    selector: StageSelector,
    output: Option<u8>,
}

impl StagedPilot {
    /// A pilot with no output yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            selector: StageSelector::new(),
            output: None,
        }
    }

    /// Forgets the stage and the previous output.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Forgets the stage in force but keeps the output: use it when the curve is replaced, so the fan does not
    /// drop to a start speed while the new curve takes over.
    pub fn restart_selection(&mut self) {
        self.selector.reset();
    }

    /// Takes `output` (%) as the previous output, e.g. when another kind of curve was driving the fan.
    pub fn seed(&mut self, output: u8) {
        self.output = Some(output.min(MAX_SPEED));
    }

    /// Last output (%), `None` before the first [`Self::update`].
    #[must_use]
    pub const fn output(&self) -> Option<u8> {
        self.output
    }

    /// Stage in force (`None` = idle).
    #[must_use]
    pub const fn stage(&self) -> Option<usize> {
        self.selector.current()
    }

    /// Computes the next output (%) at the filtered temperature `filtered` (`None` = not known yet: hold the
    /// previous output, or the first stage's speed so the fan spins).
    pub fn update(&mut self, curve: &StagedCurve, filtered: Option<f64>) -> u8 {
        let start = curve.start_speed();
        let Some(temp) = filtered else {
            return *self.output.get_or_insert(start);
        };
        let stage = self.selector.update(curve, temp);
        let target = stage
            .map_or(curve.idle(), |i| curve.stages()[i].speed)
            .max(safety_floor(temp))
            .min(MAX_SPEED);
        let next = match self.output {
            // Below the start speed: idle, or only a floor is asking. Follow the target directly.
            _ if target < start => target,
            None => start,
            Some(cur) if cur < start => start,
            Some(cur) if target > cur => target.min(cur.saturating_add(SLEW_UP_PCT)),
            Some(cur) if target < cur => target.max(cur.saturating_sub(SLEW_DOWN_PCT)),
            Some(cur) => cur,
        };
        self.output = Some(next);
        next
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn st(up: u8, down: u8, speed: u8) -> Stage {
        Stage { up, down, speed }
    }

    /// 4 stages: 50 °C 30 %, 60 °C 45 %, 70 °C 70 %, 80 °C 100 %, fall-back 4 °C below each.
    fn curve() -> StagedCurve {
        StagedCurve::new(
            0,
            vec![st(50, 46, 30), st(60, 56, 45), st(70, 66, 70), st(80, 76, 100)],
        )
        .unwrap()
    }

    #[test]
    fn max_stages_is_one_per_degree() {
        let all: Vec<Stage> = (22..=100u8).map(|t| st(t, t - 2, 100)).collect();
        assert_eq!(all.len(), MAX_STAGES);
        assert!(StagedCurve::new(0, all).is_ok());
        // An 80th stage does not fit: 21 °C would need a fall-back under 20 °C.
        let one_more: Vec<Stage> = (21..=100u8).map(|t| st(t, t - 2, 100)).collect();
        assert_eq!(
            StagedCurve::new(0, one_more),
            Err(StagedError::TooManyStages {
                count: MAX_STAGES + 1
            })
        );
        assert_eq!(
            StagedCurve::new(0, vec![st(21, 19, 30)]),
            Err(StagedError::DownOutOfRange { index: 0, value: 19 })
        );
    }

    #[test]
    fn accepts_one_stage_and_many() {
        assert!(StagedCurve::new(0, vec![st(60, 55, 100)]).is_ok());
        let c = curve();
        assert_eq!(c.stages().len(), 4);
        assert_eq!((c.idle(), c.start_speed()), (0, 30));
    }

    #[test]
    fn s3_rejects_no_stage() {
        assert_eq!(StagedCurve::new(0, vec![]), Err(StagedError::NoStages));
    }

    #[test]
    fn s3_rejects_temperatures_out_of_range() {
        assert_eq!(
            StagedCurve::new(0, vec![st(101, 95, 50)]),
            Err(StagedError::UpOutOfRange { index: 0, value: 101 })
        );
        assert_eq!(
            StagedCurve::new(0, vec![st(40, 19, 50)]),
            Err(StagedError::DownOutOfRange { index: 0, value: 19 })
        );
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 46, 30), st(19, 15, 40)]),
            Err(StagedError::UpOutOfRange { index: 1, value: 19 })
        );
    }

    #[test]
    fn s3_rejects_fall_back_not_below_rise_or_too_close() {
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 50, 30)]),
            Err(StagedError::DownNotBelowUp {
                index: 0,
                up: 50,
                down: 50
            })
        );
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 55, 30)]),
            Err(StagedError::DownNotBelowUp {
                index: 0,
                up: 50,
                down: 55
            })
        );
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 49, 30)]),
            Err(StagedError::HysteresisTooSmall { index: 0, gap: 1 })
        );
        assert!(StagedCurve::new(0, vec![st(50, 48, 30)]).is_ok());
        // Wide fall-back gaps are the owner's choice.
        assert!(StagedCurve::new(0, vec![st(90, 20, 30)]).is_ok());
    }

    #[test]
    fn s1_rejects_unordered_stages() {
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 46, 30), st(50, 47, 40)]),
            Err(StagedError::UpNotIncreasing { index: 1 })
        );
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 46, 30), st(45, 40, 40)]),
            Err(StagedError::UpNotIncreasing { index: 1 })
        );
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 46, 30), st(60, 46, 40)]),
            Err(StagedError::DownNotIncreasing { index: 1 })
        );
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 46, 30), st(60, 56, 29)]),
            Err(StagedError::SpeedsDecreasing { index: 1 })
        );
        // Equal speeds are fine: a plateau.
        assert!(StagedCurve::new(0, vec![st(50, 46, 30), st(60, 56, 30)]).is_ok());
    }

    #[test]
    fn s3_rejects_speed_above_100_and_idle_above_first_stage() {
        assert_eq!(
            StagedCurve::new(0, vec![st(50, 46, 101)]),
            Err(StagedError::SpeedOutOfRange { index: 0, value: 101 })
        );
        assert_eq!(
            StagedCurve::new(31, vec![st(50, 46, 30)]),
            Err(StagedError::IdleAboveFirst { idle: 31, first: 30 })
        );
        assert!(StagedCurve::new(30, vec![st(50, 46, 30)]).is_ok());
    }

    #[test]
    fn ascending_stage_and_speed() {
        let c = curve();
        assert_eq!(c.ascending_stage(49), None);
        assert_eq!(c.ascending_stage(50), Some(0));
        assert_eq!(c.ascending_stage(59), Some(0));
        assert_eq!(c.ascending_stage(60), Some(1));
        assert_eq!(c.ascending_stage(100), Some(3));
        assert_eq!(c.ascending_speed(20), 0);
        assert_eq!(c.ascending_speed(65), 45);
        assert_eq!(c.ascending_speed(85), 100);
    }

    #[test]
    fn safety_floor_follows_s2() {
        assert_eq!(safety_floor(59.9), 0);
        assert_eq!(safety_floor(60.0), 30);
        assert_eq!(safety_floor(74.9), 30);
        assert_eq!(safety_floor(75.0), 50);
        assert_eq!(safety_floor(84.9), 50);
        assert_eq!(safety_floor(85.0), 100);
        assert_eq!(safety_floor(f64::NAN), 100);
        assert_eq!(safety_floor(f64::INFINITY), 100);
    }

    #[test]
    fn effective_speed_is_the_stage_or_the_floor() {
        // A very quiet curve: 20 % up to 90 °C.
        let quiet = StagedCurve::new(0, vec![st(40, 36, 20), st(90, 80, 100)]).unwrap();
        assert_eq!(quiet.ascending_speed(70), 20);
        assert_eq!(quiet.effective_speed(59), 20);
        assert_eq!(quiet.effective_speed(60), 30);
        assert_eq!(quiet.effective_speed(75), 50);
        assert_eq!(quiet.effective_speed(85), 100);
        let report = quiet.raised_floors();
        assert_eq!(
            report
                .iter()
                .map(|r| (r.floor.temp, r.curve_speed))
                .collect::<Vec<_>>(),
            [(60, 20), (75, 20), (85, 20)]
        );
        assert!(curve().raised_floors().is_empty());
    }

    #[test]
    fn floors_are_monotonic_with_the_stages() {
        let c = StagedCurve::new(0, vec![st(40, 36, 20), st(90, 80, 100)]).unwrap();
        let speeds: Vec<u8> = (20..=100).map(|t| c.effective_speed(t)).collect();
        assert!(speeds.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn max_speed_covers_the_top_floor() {
        assert_eq!(curve().max_speed(), 100);
        let low = StagedCurve::new(0, vec![st(40, 36, 20)]).unwrap();
        assert_eq!(low.max_speed(), 100);
    }

    fn select(sel: &mut StageSelector, c: &StagedCurve, temps: &[f64]) -> Vec<Option<usize>> {
        temps.iter().map(|&t| sel.update(c, t)).collect()
    }

    #[test]
    fn selector_rises_at_up_and_falls_back_under_down() {
        let c = curve();
        let mut sel = StageSelector::new();
        assert_eq!(
            select(&mut sel, &c, &[49.9, 50.0, 55.0, 60.0, 70.0, 80.0]),
            [None, Some(0), Some(0), Some(1), Some(2), Some(3)]
        );
        // Cooling: every stage stays until its own fall-back temperature.
        assert_eq!(
            select(
                &mut sel,
                &c,
                &[79.0, 76.0, 75.9, 70.0, 66.0, 65.9, 56.0, 55.9, 46.0, 45.9]
            ),
            [
                Some(3),
                Some(3),
                Some(2),
                Some(2),
                Some(2),
                Some(1),
                Some(1),
                Some(0),
                Some(0),
                None
            ]
        );
    }

    #[test]
    fn selector_does_not_hunt_between_up_and_down() {
        let c = curve();
        let mut sel = StageSelector::new();
        select(&mut sel, &c, &[60.0]);
        // Jitter between 56 and 60 °C: stage 1 holds.
        let out = select(&mut sel, &c, &[59.0, 57.0, 60.0, 56.0, 58.0, 59.9]);
        assert!(out.iter().all(|&s| s == Some(1)), "{out:?}");
    }

    #[test]
    fn selector_jumps_several_stages_at_once() {
        let c = curve();
        let mut sel = StageSelector::new();
        assert_eq!(sel.update(&c, 85.0), Some(3));
        assert_eq!(sel.update(&c, 30.0), None);
    }

    #[test]
    fn selector_with_overlapping_ranges_stays_stable() {
        // Wide fall-back gaps: stage 1 falls back at 30 °C, below stage 0's rise.
        let c = StagedCurve::new(0, vec![st(50, 40, 30), st(60, 45, 50)]).unwrap();
        let mut sel = StageSelector::new();
        select(&mut sel, &c, &[60.0]);
        assert_eq!(sel.update(&c, 46.0), Some(1));
        assert_eq!(sel.update(&c, 44.0), Some(0));
        assert_eq!(sel.update(&c, 41.0), Some(0));
        assert_eq!(sel.update(&c, 39.0), None);
        // Re-entering needs the rise temperature again.
        assert_eq!(sel.update(&c, 49.0), None);
        assert_eq!(sel.update(&c, 50.0), Some(0));
    }

    #[test]
    fn selector_non_finite_temperature_is_the_hottest_stage() {
        let c = curve();
        let mut sel = StageSelector::new();
        assert_eq!(sel.update(&c, f64::NAN), Some(3));
        assert_eq!(sel.update(&c, f64::INFINITY), Some(3));
    }

    #[test]
    fn selector_survives_a_shorter_curve() {
        let mut sel = StageSelector::new();
        sel.update(&curve(), 85.0);
        let short = StagedCurve::new(0, vec![st(50, 46, 30)]).unwrap();
        assert_eq!(sel.update(&short, 55.0), Some(0));
    }

    fn run(p: &mut StagedPilot, c: &StagedCurve, temps: &[f64]) -> Vec<u8> {
        temps.iter().map(|&t| p.update(c, Some(t))).collect()
    }

    #[test]
    fn pilot_holds_the_first_stage_speed_without_temperature() {
        let mut p = StagedPilot::new();
        assert_eq!(p.update(&curve(), None), 30);
        assert_eq!(p.output(), Some(30));
    }

    #[test]
    fn pilot_idles_below_the_first_stage_and_starts_at_its_speed() {
        let c = curve();
        let mut p = StagedPilot::new();
        assert_eq!(run(&mut p, &c, &[40.0, 49.9]), [0, 0]);
        assert_eq!(run(&mut p, &c, &[50.0]), [30]);
        assert_eq!(p.stage(), Some(0));
        // Back down: runs at the first stage speed until its fall-back, then stops.
        assert_eq!(run(&mut p, &c, &[47.0, 46.0]), [30, 30]);
        assert_eq!(run(&mut p, &c, &[45.9]), [0]);
    }

    #[test]
    fn pilot_never_stops_a_spinning_idle_speed() {
        let c = StagedCurve::new(25, vec![st(50, 46, 40)]).unwrap();
        let mut p = StagedPilot::new();
        assert_eq!(run(&mut p, &c, &[30.0]), [25]);
        assert_eq!(run(&mut p, &c, &[50.0]), [40]);
        assert_eq!(run(&mut p, &c, &[40.0]), [25]);
    }

    #[test]
    fn pilot_ramps_up_by_slew_steps_and_down_slowly() {
        let c = curve();
        let mut p = StagedPilot::new();
        run(&mut p, &c, &[50.0]);
        // Straight to 80 °C (stage 3, 100 %): +5 % per second.
        let up = run(&mut p, &c, &[80.0; 16]);
        assert_eq!(&up[..3], [35, 40, 45]);
        assert_eq!(*up.last().unwrap(), 100);
        assert!(up.windows(2).all(|w| w[1] >= w[0] && w[1] - w[0] <= SLEW_UP_PCT));
        // Back to 70 °C (stage 3 holds above 76, then stage 2 = 70 %): −1 % per second after the fall-back.
        assert_eq!(run(&mut p, &c, &[77.0, 77.0]), [100, 100]);
        assert_eq!(run(&mut p, &c, &[75.0; 3]), [99, 98, 97]);
    }

    #[test]
    fn pilot_applies_s2_floors_over_a_quiet_curve() {
        let quiet = StagedCurve::new(0, vec![st(40, 36, 20), st(90, 80, 100)]).unwrap();
        let mut p = StagedPilot::new();
        run(&mut p, &quiet, &[40.0]);
        // 62 °C: stage 0 is 20 %, the floor asks 30 %.
        let out = run(&mut p, &quiet, &[62.0; 5]);
        assert_eq!(out[0], 25);
        assert_eq!(*out.last().unwrap(), 30);
        // The floor on its own (idle curve, hot enough): follows the target directly below the start speed.
        let hot_idle = StagedCurve::new(0, vec![st(70, 66, 60)]).unwrap();
        let mut q = StagedPilot::new();
        assert_eq!(run(&mut q, &hot_idle, &[65.0]), [30]);
    }

    #[test]
    fn pilot_output_never_exceeds_100() {
        let c = StagedCurve::new(0, vec![st(50, 46, 100)]).unwrap();
        let mut p = StagedPilot::new();
        for out in run(&mut p, &c, &[50.0, 99.0, f64::NAN, 100.0]) {
            assert!(out <= 100);
        }
    }

    #[test]
    fn every_preset_table_is_a_valid_staged_curve() {
        use crate::curve::CurvePreset;
        use crate::table::generate;
        for preset in CurvePreset::ALL {
            let table = generate(&preset.spec()).unwrap().curve;
            let c = StagedCurve::from_step_curve(&table).unwrap();
            assert_eq!(c.stages().len(), 6, "{preset:?}");
            assert_eq!(c.idle(), table.speeds()[0]);
            // Heating up, the staged curve gives exactly the speed of the EC step at every temperature.
            for temp in 20..=100u8 {
                assert_eq!(
                    c.ascending_speed(temp),
                    table.min_speed_at(temp),
                    "{preset:?} {temp} °C"
                );
            }
            // The table already meets S2: nothing to raise.
            assert!(c.raised_floors().is_empty(), "{preset:?}");
        }
    }

    #[test]
    fn pilot_seed_and_restart_keep_the_output() {
        let c = curve();
        let mut p = StagedPilot::new();
        p.seed(80);
        assert_eq!(p.output(), Some(80));
        assert_eq!(run(&mut p, &c, &[60.0]), [79]);
        p.restart_selection();
        assert_eq!(p.stage(), None);
        assert_eq!(p.output(), Some(79));
        p.reset();
        assert_eq!(p.output(), None);
    }
}
