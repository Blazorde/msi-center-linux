//! Staged CPU curve view-model (ADR-0013): the editable draft, French validation messages and the drawing
//! geometry (pure, no GTK).
//!
//! The draft may be invalid while the owner is typing (that is the point of a draft); [`Draft::validate`]
//! runs the very same S1–S3 checks as the daemon (`msi_core::staged::StagedCurve::new`) so the reason is
//! shown in French before anything is sent. The daemon stays the authority and validates again.

use msi_core::staged::{FloorRaise, MAX_STAGES, Stage, StagedCurve, StagedError};

use crate::chart::{T_MAX, T_MIN};
use crate::format::NNBSP;

/// Stage on the wire: `(y up, y down, y speed)`.
pub type StageWire = (u8, u8, u8);
/// S2 floor on the wire: `(y floor_temp, y floor_speed, y curve_speed)`.
pub type FloorWire = (u8, u8, u8);
/// `CpuStages` reply: `(y idle, a(yyy) stages, a(yyy) floors, b saved, b active, b supported)`.
pub type CpuStagesWire = (u8, Vec<StageWire>, Vec<FloorWire>, bool, bool, bool);

/// Highest value the editor lets a temperature or a speed reach.
const MAX_VALUE: u8 = 100;
/// Smallest rise-to-fall-back gap the daemon accepts (°C).
const MIN_GAP: u8 = 2;

/// What `CpuStages` returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedState {
    /// The curve (the saved one, else the starting point the daemon generated).
    pub curve: StagedCurve,
    /// Whether a staged curve is saved for the selected profile.
    pub saved: bool,
    /// Whether it drives the CPU fan now.
    pub active: bool,
    /// Whether the firmware allows live CPU control (otherwise staged curves are refused).
    pub supported: bool,
}

impl StagedState {
    /// From the wire form; `None` if the daemon sent a curve that does not validate.
    #[must_use]
    pub fn from_wire(w: &CpuStagesWire) -> Option<Self> {
        let (idle, stages, _floors, saved, active, supported) = w;
        Some(Self {
            curve: curve_from_wire(*idle, stages).ok()?,
            saved: *saved,
            active: *active,
            supported: *supported,
        })
    }
}

/// Validates the wire form of a curve.
///
/// # Errors
/// The violated rule.
pub fn curve_from_wire(idle: u8, stages: &[StageWire]) -> Result<StagedCurve, StagedError> {
    StagedCurve::new(
        idle,
        stages
            .iter()
            .map(|&(up, down, speed)| Stage { up, down, speed })
            .collect(),
    )
}

/// Wire form of the stages of a curve.
#[must_use]
pub fn stages_to_wire(curve: &StagedCurve) -> Vec<StageWire> {
    curve.stages().iter().map(|s| (s.up, s.down, s.speed)).collect()
}

/// The curve being edited. Fields are public so the UI can bind them; use the setters, which keep every value
/// in `0..=100`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// Speed (%) below the first stage.
    pub idle: u8,
    /// Stages, coolest first (may be invalid until [`Draft::validate`] says otherwise).
    pub stages: Vec<Stage>,
}

impl Draft {
    /// The draft of an existing curve.
    #[must_use]
    pub fn from_curve(curve: &StagedCurve) -> Self {
        Self {
            idle: curve.idle(),
            stages: curve.stages().to_vec(),
        }
    }

    /// Runs the daemon's rules (S1–S3) and returns the curve, or the reason in French.
    ///
    /// # Errors
    /// A French sentence naming the stage (numbered from 1, as the editor shows them) and the rule.
    pub fn validate(&self) -> Result<StagedCurve, String> {
        StagedCurve::new(self.idle, self.stages.clone()).map_err(|e| error_fr(&e))
    }

    /// Whether another stage can be appended.
    #[must_use]
    pub fn can_add(&self) -> bool {
        self.next_stage().is_some()
    }

    /// The stage [`Draft::add`] would append: 5 °C above the last rise, same fall-back gap, 10 points faster.
    fn next_stage(&self) -> Option<Stage> {
        if self.stages.len() >= MAX_STAGES {
            return None;
        }
        let Some(last) = self.stages.last() else {
            return Some(Stage {
                up: 55,
                down: 50,
                speed: self.idle.max(30),
            });
        };
        let up = last.up.saturating_add(5).min(MAX_VALUE);
        if up <= last.up {
            return None;
        }
        let gap = last.up.saturating_sub(last.down).max(MIN_GAP);
        let down = up.saturating_sub(gap).max(last.down.saturating_add(1));
        (up.saturating_sub(down) >= MIN_GAP).then_some(Stage {
            up,
            down,
            speed: last.speed.saturating_add(10).min(MAX_VALUE),
        })
    }

    /// Appends a stage after the last one. Returns its index, `None` if there is no room (79 stages, or no
    /// temperature left above the last rise).
    pub fn add(&mut self) -> Option<usize> {
        let stage = self.next_stage()?;
        self.stages.push(stage);
        Some(self.stages.len() - 1)
    }

    /// Inserts a stage halfway between stage `index` and the next one. Returns its index, `None` if there is
    /// no room between the two (fewer than 4 °C on a threshold, or already 79 stages) or `index` is the last
    /// stage (use [`Draft::add`]).
    pub fn insert_after(&mut self, index: usize) -> Option<usize> {
        if self.stages.len() >= MAX_STAGES {
            return None;
        }
        let (a, b) = (*self.stages.get(index)?, *self.stages.get(index + 1)?);
        let up = u8::midpoint(a.up, b.up);
        let down = u8::midpoint(a.down, b.down);
        let stage = Stage {
            up,
            down,
            speed: u8::midpoint(a.speed, b.speed),
        };
        let fits =
            up > a.up && up < b.up && down > a.down && down < b.down && up.saturating_sub(down) >= MIN_GAP;
        if !fits {
            return None;
        }
        self.stages.insert(index + 1, stage);
        Some(index + 1)
    }

    /// Removes stage `index` (an out-of-range index does nothing).
    pub fn remove(&mut self, index: usize) {
        if index < self.stages.len() {
            self.stages.remove(index);
        }
    }

    /// Sets the idle speed (clamped to 100).
    pub fn set_idle(&mut self, v: u8) {
        self.idle = v.min(MAX_VALUE);
    }

    /// Sets the rise temperature of stage `index` (clamped to 100).
    pub fn set_up(&mut self, index: usize, v: u8) {
        if let Some(s) = self.stages.get_mut(index) {
            s.up = v.min(MAX_VALUE);
        }
    }

    /// Sets the fall-back temperature of stage `index` (clamped to 100).
    pub fn set_down(&mut self, index: usize, v: u8) {
        if let Some(s) = self.stages.get_mut(index) {
            s.down = v.min(MAX_VALUE);
        }
    }

    /// Sets the speed of stage `index` (clamped to 100).
    pub fn set_speed(&mut self, index: usize, v: u8) {
        if let Some(s) = self.stages.get_mut(index) {
            s.speed = v.min(MAX_VALUE);
        }
    }
}

/// French reason for a rejected curve. Stages are numbered from 1, as the editor shows them.
#[must_use]
pub fn error_fr(e: &StagedError) -> String {
    match *e {
        StagedError::NoStages => "Il faut au moins un palier.".to_owned(),
        StagedError::TooManyStages { count } => {
            format!("{count} paliers, c'est trop : le maximum est {MAX_STAGES}.")
        }
        StagedError::UpOutOfRange { index, value } => format!(
            "Palier {} : la température de montée ({value}{NNBSP}°C) doit être comprise entre 20 et 100{NNBSP}°C.",
            index + 1
        ),
        StagedError::DownOutOfRange { index, value } => format!(
            "Palier {} : la température de retombée ({value}{NNBSP}°C) doit être comprise entre 20 et 100{NNBSP}°C.",
            index + 1
        ),
        StagedError::DownNotBelowUp { index, up, down } => format!(
            "Palier {} : la retombée ({down}{NNBSP}°C) doit être sous la montée ({up}{NNBSP}°C).",
            index + 1
        ),
        StagedError::HysteresisTooSmall { index, gap } => format!(
            "Palier {} : seulement {gap}{NNBSP}°C entre la retombée et la montée ; il en faut au moins 2 pour \
             que le ventilateur ne se mette pas à osciller.",
            index + 1
        ),
        StagedError::UpNotIncreasing { index } => format!(
            "Palier {} : sa température de montée doit être au-dessus de celle du palier précédent.",
            index + 1
        ),
        StagedError::DownNotIncreasing { index } => format!(
            "Palier {} : sa température de retombée doit être au-dessus de celle du palier précédent.",
            index + 1
        ),
        StagedError::SpeedOutOfRange { index, value } => {
            format!("Palier {} : {value}{NNBSP}% dépasse 100{NNBSP}%.", index + 1)
        }
        StagedError::SpeedsDecreasing { index } => format!(
            "Palier {} : la puissance ne peut pas être plus basse que celle du palier précédent.",
            index + 1
        ),
        StagedError::IdleAboveFirst { idle, first } => format!(
            "La puissance au repos ({idle}{NNBSP}%) ne peut pas dépasser celle du premier palier ({first}{NNBSP}%)."
        ),
    }
}

/// One line for an S2 floor that takes over from the stages.
#[must_use]
pub fn floor_fr(f: &FloorRaise) -> String {
    format!(
        "Dès {}{NNBSP}°C, le plancher de sécurité impose {}{NNBSP}% (vos paliers donnent {}{NNBSP}%).",
        f.floor.temp, f.floor.speed, f.curve_speed
    )
}

/// One line for the wire form of a floor.
#[must_use]
pub fn floor_wire_fr(&(temp, floor, curve): &FloorWire) -> String {
    format!(
        "Dès {temp}{NNBSP}°C, le plancher de sécurité impose {floor}{NNBSP}% (vos paliers donnent {curve}{NNBSP}%)."
    )
}

/// "4 paliers, de 30 % à 100 %".
#[must_use]
pub fn summary(curve: &StagedCurve) -> String {
    let n = curve.stages().len();
    let plural = if n > 1 { "paliers" } else { "palier" };
    format!(
        "{n} {plural}, de {}{NNBSP}% à {}{NNBSP}%",
        curve.start_speed(),
        curve.stages().last().map_or(curve.idle(), |s| s.speed)
    )
}

fn staircase(idle: u8, stages: &[Stage], edge: impl Fn(&Stage) -> u8) -> Vec<(f64, f64)> {
    let mut pts = vec![(T_MIN, f64::from(idle))];
    let mut level = f64::from(idle);
    for s in stages {
        let t = f64::from(edge(s));
        let next = f64::from(s.speed);
        pts.push((t, level));
        pts.push((t, next));
        level = next;
    }
    pts.push((T_MAX, level));
    pts
}

/// Rising path in data coordinates (°C, %): flat stages joined by risers at each rise temperature.
#[must_use]
pub fn rising_path(curve: &StagedCurve) -> Vec<(f64, f64)> {
    staircase(curve.idle(), curve.stages(), |s| s.up)
}

/// Falling path (risers at the fall-back temperatures), drawn dashed to show the hysteresis.
#[must_use]
pub fn falling_path(curve: &StagedCurve) -> Vec<(f64, f64)> {
    staircase(curve.idle(), curve.stages(), |s| s.down)
}

/// Index of the stage reached at `temp` while heating up, `None` below the first rise (idle).
#[must_use]
pub fn stage_at(curve: &StagedCurve, temp: f64) -> Option<usize> {
    if !temp.is_finite() {
        return curve.stages().len().checked_sub(1);
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255"
    )]
    let t = temp.floor().clamp(0.0, 255.0) as u8;
    curve.ascending_stage(t)
}

/// Speed (%) the fan is at least asked for at `temp` while heating up: the stage reached, or idle, raised by
/// the S2 floor (the same `max()` the daemon applies). Used for the live marker.
#[must_use]
pub fn speed_at(curve: &StagedCurve, temp: f64) -> u8 {
    let stage = stage_at(curve, temp).map_or(curve.idle(), |i| curve.stages()[i].speed);
    stage.max(msi_core::staged::safety_floor(temp))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn draft(idle: u8, stages: &[(u8, u8, u8)]) -> Draft {
        Draft {
            idle,
            stages: stages
                .iter()
                .map(|&(up, down, speed)| Stage { up, down, speed })
                .collect(),
        }
    }

    #[test]
    fn a_new_draft_grows_one_valid_stage_at_a_time_up_to_the_limit() {
        let mut d = draft(0, &[]);
        assert!(d.can_add());
        let mut count = 0;
        while let Some(i) = d.add() {
            count += 1;
            assert_eq!(i, count - 1);
            assert!(d.validate().is_ok(), "{count} stages: {:?}", d.validate());
        }
        // Rises 55, 60, …, 100 then nothing is left above the last rise.
        assert_eq!(count, 10);
        assert!(!d.can_add());
        assert_eq!(d.stages.last().unwrap().up, 100);
        assert_eq!(d.stages.last().unwrap().speed, 100);
    }

    #[test]
    fn added_stage_copies_the_gap_and_steps_up() {
        let mut d = draft(20, &[(60, 54, 40)]);
        let i = d.add().unwrap();
        assert_eq!(
            d.stages[i],
            Stage {
                up: 65,
                down: 59,
                speed: 50
            }
        );
        // A narrow last stage keeps the 2 °C minimum.
        let mut d = draft(0, &[(99, 98, 90)]);
        assert_eq!(d.add(), None, "99+5 is capped at 100 with a gap of 1: no room");
    }

    #[test]
    fn insert_after_takes_the_middle_or_refuses() {
        let mut d = draft(0, &[(50, 44, 30), (70, 64, 60)]);
        let i = d.insert_after(0).unwrap();
        assert_eq!(i, 1);
        assert_eq!(
            d.stages[1],
            Stage {
                up: 60,
                down: 54,
                speed: 45
            }
        );
        assert!(d.validate().is_ok());
        // Neighbours too close: nothing fits between.
        let mut tight = draft(0, &[(50, 44, 30), (51, 45, 40)]);
        assert_eq!(tight.insert_after(0), None);
        // The last stage has no neighbour above it.
        assert_eq!(d.insert_after(2), None);
        assert_eq!(d.insert_after(9), None);
    }

    #[test]
    fn setters_clamp_and_ignore_bad_indexes() {
        let mut d = draft(0, &[(50, 44, 30)]);
        d.set_idle(250);
        d.set_up(0, 200);
        d.set_down(0, 101);
        d.set_speed(0, 255);
        assert_eq!(
            (d.idle, d.stages[0]),
            (
                100,
                Stage {
                    up: 100,
                    down: 100,
                    speed: 100
                }
            )
        );
        d.set_up(7, 1);
        d.set_down(7, 1);
        d.set_speed(7, 1);
        d.remove(7);
        assert_eq!(d.stages.len(), 1);
        d.remove(0);
        assert!(d.stages.is_empty());
    }

    #[test]
    fn every_rule_has_a_french_message_naming_the_stage_from_one() {
        let cases = [
            (draft(0, &[]), "au moins un palier"),
            (draft(0, &[(50, 44, 30), (50, 46, 40)]), "Palier 2"),
            (draft(0, &[(50, 44, 30), (60, 44, 40)]), "Palier 2"),
            (draft(0, &[(50, 50, 30)]), "Palier 1 : la retombée"),
            (draft(0, &[(50, 49, 30)]), "au moins 2"),
            (draft(0, &[(50, 44, 30), (60, 54, 20)]), "plus basse"),
            (draft(0, &[(50, 44, 101)]), "dépasse 100"),
            (draft(0, &[(15, 10, 30)]), "entre 20 et 100"),
            (draft(0, &[(50, 15, 30)]), "entre 20 et 100"),
            (draft(40, &[(50, 44, 30)]), "au repos"),
        ];
        for (d, needle) in cases {
            let msg = d.validate().unwrap_err();
            assert!(msg.contains(needle), "{needle:?} not in {msg:?}");
        }
        let many = Draft {
            idle: 0,
            stages: vec![
                Stage {
                    up: 50,
                    down: 44,
                    speed: 30
                };
                80
            ],
        };
        assert!(many.validate().unwrap_err().contains("80 paliers"));
    }

    #[test]
    fn paths_are_staircases_over_the_whole_chart() {
        let curve = draft(20, &[(55, 50, 30), (70, 64, 60)]).validate().unwrap();
        assert_eq!(
            rising_path(&curve),
            [
                (T_MIN, 20.0),
                (55.0, 20.0),
                (55.0, 30.0),
                (70.0, 30.0),
                (70.0, 60.0),
                (T_MAX, 60.0)
            ]
        );
        let falling = falling_path(&curve);
        assert_eq!(falling[1], (50.0, 20.0));
        assert_eq!(falling[3], (64.0, 30.0));
        // Same levels, risers shifted down.
        assert!(
            rising_path(&curve)
                .iter()
                .zip(&falling)
                .all(|(r, f)| (r.1 - f.1).abs() < 1e-9 && f.0 <= r.0)
        );
    }

    #[test]
    fn stage_at_follows_the_rise_temperatures() {
        let curve = draft(0, &[(55, 50, 30), (70, 64, 60)]).validate().unwrap();
        assert_eq!(stage_at(&curve, 40.0), None);
        assert_eq!(stage_at(&curve, 54.9), None);
        assert_eq!(stage_at(&curve, 55.0), Some(0));
        assert_eq!(stage_at(&curve, 69.9), Some(0));
        assert_eq!(stage_at(&curve, 70.0), Some(1));
        assert_eq!(stage_at(&curve, 300.0), Some(1));
        assert_eq!(stage_at(&curve, -5.0), None);
        assert_eq!(
            stage_at(&curve, f64::NAN),
            Some(1),
            "unknown temperature: the hottest stage"
        );
    }

    #[test]
    fn speed_at_is_the_stage_raised_by_the_s2_floor() {
        let curve = draft(10, &[(55, 50, 20), (70, 64, 30)]).validate().unwrap();
        assert_eq!(speed_at(&curve, 40.0), 10);
        assert_eq!(speed_at(&curve, 56.0), 20);
        assert_eq!(speed_at(&curve, 61.0), 30, "S2: 30 % from 60 °C");
        assert_eq!(speed_at(&curve, 76.0), 50, "S2: 50 % from 75 °C");
        assert_eq!(
            speed_at(&curve, f64::NAN),
            100,
            "unknown temperature: the safe side"
        );
    }

    #[test]
    fn floors_and_summary_read_in_french() {
        let curve = draft(0, &[(55, 50, 20), (70, 64, 30)]).validate().unwrap();
        let floors = curve.raised_floors();
        assert!(!floors.is_empty());
        assert!(floor_fr(&floors[0]).starts_with("Dès 60"));
        assert!(floor_fr(&floors[0]).contains("plancher de sécurité"));
        assert_eq!(
            floor_wire_fr(&(75, 50, 30)),
            floor_fr(&FloorRaise {
                floor: msi_core::table::SpeedFloor { temp: 75, speed: 50 },
                curve_speed: 30,
            })
        );
        assert_eq!(summary(&curve), format!("2 paliers, de 20{NNBSP}% à 30{NNBSP}%"));
        let one = draft(0, &[(55, 50, 20)]).validate().unwrap();
        assert!(summary(&one).starts_with("1 palier,"));
    }

    #[test]
    fn wire_round_trip_and_state() {
        let curve = draft(10, &[(55, 50, 30), (70, 64, 60)]).validate().unwrap();
        let wire = (10, stages_to_wire(&curve), vec![], true, false, true);
        let st = StagedState::from_wire(&wire).unwrap();
        assert_eq!(st.curve, curve);
        assert!(st.saved && !st.active && st.supported);
        assert_eq!(Draft::from_curve(&curve).validate().unwrap(), curve);
        // A curve that does not validate is not trusted.
        let bad = (0, vec![(50, 50, 30)], vec![], true, true, true);
        assert!(StagedState::from_wire(&bad).is_none());
    }
}
