//! Thermal supervisor, rule S4: CPU or GPU ≥ 95 °C → Cooler Boost forced until both are back below 85 °C.
//!
//! Pure state machine: the daemon feeds it readings (~1 s) and applies the decision.

/// Cooler Boost is forced at or above this temperature (°C).
pub const BOOST_ON_TEMP: f64 = 95.0;
/// Cooler Boost is released only when every sensor is strictly below this temperature (°C).
pub const BOOST_OFF_TEMP: f64 = 85.0;

/// One sensor reading as seen by the daemon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SensorReading {
    /// Temperature in °C. A non-finite value is treated as [`SensorReading::Failed`].
    Celsius(f64),
    /// The device is legitimately absent (e.g. dGPU removed in Éco): ignored.
    Absent,
    /// The sensor could not be read: fail-safe, treated as hot.
    Failed,
}

impl SensorReading {
    /// `Some(temp)` for a usable reading, `None` for an absent device, and `+∞` for a failed one so it
    /// behaves as "too hot" everywhere.
    fn effective(self) -> Option<f64> {
        match self {
            Self::Celsius(t) if t.is_finite() => Some(t),
            Self::Celsius(_) | Self::Failed => Some(f64::INFINITY),
            Self::Absent => None,
        }
    }
}

/// Supervisor decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BoostDecision {
    /// No safety override: the active profile controls the fans.
    #[default]
    Released,
    /// Cooler Boost must be forced on.
    Forced,
}

/// Hysteresis state machine for S4. Starts [`BoostDecision::Released`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThermalSupervisor {
    state: BoostDecision,
}

impl ThermalSupervisor {
    /// A supervisor in the released state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: BoostDecision::Released,
        }
    }

    /// Current decision.
    #[must_use]
    pub const fn state(&self) -> BoostDecision {
        self.state
    }

    /// Feeds new readings and returns the decision to apply.
    ///
    /// - any reading ≥ [`BOOST_ON_TEMP`] (or failed) → [`BoostDecision::Forced`];
    /// - forced, and every present reading < [`BOOST_OFF_TEMP`] → [`BoostDecision::Released`];
    /// - otherwise the state is kept (hysteresis band 85–95 °C).
    ///
    /// If both sensors are absent the CPU is unreadable, which cannot be normal: treated as failed.
    pub fn update(&mut self, cpu: SensorReading, gpu: SensorReading) -> BoostDecision {
        let hottest = [cpu, gpu]
            .into_iter()
            .filter_map(SensorReading::effective)
            .reduce(f64::max)
            .unwrap_or(f64::INFINITY);
        self.state = if hottest >= BOOST_ON_TEMP {
            BoostDecision::Forced
        } else if hottest < BOOST_OFF_TEMP {
            BoostDecision::Released
        } else {
            self.state
        };
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BoostDecision::{Forced, Released};
    use SensorReading::{Absent, Celsius, Failed};

    fn run(steps: &[(SensorReading, SensorReading)]) -> Vec<BoostDecision> {
        let mut sup = ThermalSupervisor::new();
        steps.iter().map(|&(c, g)| sup.update(c, g)).collect()
    }

    #[test]
    fn s4_starts_released() {
        assert_eq!(ThermalSupervisor::new().state(), Released);
        assert_eq!(ThermalSupervisor::default().state(), Released);
    }

    #[test]
    fn s4_forces_at_exactly_95_on_cpu() {
        assert_eq!(run(&[(Celsius(94.9), Absent)]), vec![Released]);
        assert_eq!(run(&[(Celsius(95.0), Absent)]), vec![Forced]);
    }

    #[test]
    fn s4_forces_at_exactly_95_on_gpu() {
        assert_eq!(run(&[(Celsius(50.0), Celsius(94.9))]), vec![Released]);
        assert_eq!(run(&[(Celsius(50.0), Celsius(95.0))]), vec![Forced]);
    }

    #[test]
    fn s4_hysteresis_keeps_boost_until_both_below_85() {
        let decisions = run(&[
            (Celsius(96.0), Celsius(70.0)),
            (Celsius(90.0), Celsius(70.0)),
            (Celsius(85.0), Celsius(70.0)), // 85 is not < 85
            (Celsius(84.9), Celsius(70.0)),
        ]);
        assert_eq!(decisions, vec![Forced, Forced, Forced, Released]);
    }

    #[test]
    fn s4_release_needs_every_sensor_below_85() {
        let decisions = run(&[
            (Celsius(60.0), Celsius(95.0)),
            (Celsius(60.0), Celsius(86.0)),
            (Celsius(84.0), Celsius(85.0)),
            (Celsius(84.0), Celsius(84.0)),
        ]);
        assert_eq!(decisions, vec![Forced, Forced, Forced, Released]);
    }

    #[test]
    fn s4_hysteresis_band_does_not_force_from_released() {
        assert_eq!(
            run(&[(Celsius(90.0), Celsius(94.0)), (Celsius(85.0), Absent)]),
            vec![Released, Released]
        );
    }

    #[test]
    fn s4_reforces_after_release() {
        let decisions = run(&[
            (Celsius(95.0), Absent),
            (Celsius(80.0), Absent),
            (Celsius(95.5), Absent),
        ]);
        assert_eq!(decisions, vec![Forced, Released, Forced]);
    }

    #[test]
    fn s4_absent_gpu_is_ignored() {
        let decisions = run(&[(Celsius(95.0), Absent), (Celsius(70.0), Absent)]);
        assert_eq!(decisions, vec![Forced, Released]);
    }

    #[test]
    fn s4_failed_sensor_forces_and_holds_boost() {
        let decisions = run(&[
            (Celsius(60.0), Failed),
            (Celsius(60.0), Celsius(60.0)),
            (Failed, Absent),
        ]);
        assert_eq!(decisions, vec![Forced, Released, Forced]);
    }

    #[test]
    fn s4_non_finite_reading_is_a_failure() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(run(&[(Celsius(bad), Absent)]), vec![Forced], "{bad}");
        }
    }

    #[test]
    fn s4_no_sensor_at_all_is_a_failure() {
        assert_eq!(run(&[(Absent, Absent)]), vec![Forced]);
    }
}
