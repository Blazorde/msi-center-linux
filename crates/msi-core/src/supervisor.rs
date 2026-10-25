//! Thermal supervisor, rule S4: CPU or GPU ≥ 95 °C → Cooler Boost forced until both are back below 85 °C.
//!
//! Pure state machine: the daemon feeds it readings (~1 s) and applies the decision.
//!
//! # Spike filter
//!
//! The EC's instantaneous CPU temperature (register `0x68`) has isolated spikes (up to ~90 °C at idle, see
//! `docs/hardware/ec-map.md`). Each sensor is therefore filtered over its **last two readings**:
//!
//! - boost is forced only if **both** readings are ≥ [`BOOST_ON_TEMP`] (two-sample moving minimum): a single
//!   spike is ignored, a real overheat is caught one sample (~1 s) later;
//! - boost is released only if **both** readings are < [`BOOST_OFF_TEMP`] (two-sample moving maximum);
//! - a failed reading forces boost immediately (fail-safe, no filtering);
//! - right after start, or after a failed/absent sensor comes back, one reading is not enough to force nor
//!   to release: the current state is kept until a second reading arrives.

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

/// What one sensor says after filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Absent: no opinion.
    Absent,
    /// Failed or two consecutive readings ≥ 95 °C.
    Hot,
    /// Last two readings below 85 °C.
    Cool,
    /// Anything else (hysteresis band, single spike, not enough history).
    Hold,
}

/// Previous usable reading of one sensor.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct SensorFilter {
    prev: Option<f64>,
}

impl SensorFilter {
    fn update(&mut self, reading: SensorReading) -> Verdict {
        let temp = match reading {
            SensorReading::Celsius(t) if t.is_finite() => t,
            SensorReading::Celsius(_) | SensorReading::Failed => {
                self.prev = None;
                return Verdict::Hot;
            }
            SensorReading::Absent => {
                self.prev = None;
                return Verdict::Absent;
            }
        };
        let prev = self.prev.replace(temp);
        match prev {
            Some(p) if p.min(temp) >= BOOST_ON_TEMP => Verdict::Hot,
            Some(p) if p.max(temp) < BOOST_OFF_TEMP => Verdict::Cool,
            _ => Verdict::Hold,
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

/// Hysteresis state machine for S4 with the spike filter. Starts [`BoostDecision::Released`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ThermalSupervisor {
    state: BoostDecision,
    cpu: SensorFilter,
    gpu: SensorFilter,
}

impl ThermalSupervisor {
    /// A supervisor in the released state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: BoostDecision::Released,
            cpu: SensorFilter { prev: None },
            gpu: SensorFilter { prev: None },
        }
    }

    /// Current decision.
    #[must_use]
    pub const fn state(&self) -> BoostDecision {
        self.state
    }

    /// Feeds new readings and returns the decision to apply.
    ///
    /// - any sensor hot (failed, or two consecutive readings ≥ [`BOOST_ON_TEMP`]) → [`BoostDecision::Forced`];
    /// - every present sensor cool (last two readings < [`BOOST_OFF_TEMP`]) → [`BoostDecision::Released`];
    /// - otherwise the state is kept (hysteresis band 85–95 °C, isolated spikes).
    ///
    /// If both sensors are absent the CPU is unreadable, which cannot be normal: treated as failed.
    pub fn update(&mut self, cpu: SensorReading, gpu: SensorReading) -> BoostDecision {
        let verdicts = [self.cpu.update(cpu), self.gpu.update(gpu)];
        let present: Vec<Verdict> = verdicts.into_iter().filter(|&v| v != Verdict::Absent).collect();
        self.state = if present.is_empty() || present.contains(&Verdict::Hot) {
            BoostDecision::Forced
        } else if present.iter().all(|&v| v == Verdict::Cool) {
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
    fn s4_forces_after_two_readings_at_95_on_cpu() {
        assert_eq!(
            run(&[(Celsius(94.9), Absent), (Celsius(95.0), Absent)]),
            vec![Released, Released]
        );
        assert_eq!(
            run(&[(Celsius(95.0), Absent), (Celsius(95.0), Absent)]),
            vec![Released, Forced]
        );
    }

    #[test]
    fn s4_forces_after_two_readings_at_95_on_gpu() {
        assert_eq!(
            run(&[(Celsius(50.0), Celsius(95.0)), (Celsius(50.0), Celsius(96.0))]),
            vec![Released, Forced]
        );
    }

    #[test]
    fn s4_single_spike_is_ignored() {
        // Idle at 55 °C with a 100 °C spike, as measured on register 0x68.
        assert_eq!(
            run(&[
                (Celsius(55.0), Absent),
                (Celsius(100.0), Absent),
                (Celsius(55.0), Absent),
                (Celsius(99.0), Absent),
                (Celsius(56.0), Absent),
            ]),
            vec![Released; 5]
        );
    }

    #[test]
    fn s4_hysteresis_keeps_boost_until_both_below_85() {
        let decisions = run(&[
            (Celsius(96.0), Celsius(70.0)),
            (Celsius(96.0), Celsius(70.0)),
            (Celsius(90.0), Celsius(70.0)),
            (Celsius(85.0), Celsius(70.0)), // 85 is not < 85
            (Celsius(84.9), Celsius(70.0)), // previous reading still 85
            (Celsius(84.9), Celsius(70.0)),
        ]);
        assert_eq!(
            decisions,
            vec![Released, Forced, Forced, Forced, Forced, Released]
        );
    }

    #[test]
    fn s4_downward_spike_does_not_release() {
        let decisions = run(&[
            (Celsius(96.0), Absent),
            (Celsius(96.0), Absent),
            (Celsius(40.0), Absent),
            (Celsius(92.0), Absent),
        ]);
        assert_eq!(decisions, vec![Released, Forced, Forced, Forced]);
    }

    #[test]
    fn s4_release_needs_every_sensor_below_85() {
        let decisions = run(&[
            (Celsius(60.0), Celsius(95.0)),
            (Celsius(60.0), Celsius(95.0)),
            (Celsius(60.0), Celsius(86.0)),
            (Celsius(84.0), Celsius(84.0)),
            (Celsius(84.0), Celsius(84.0)),
        ]);
        assert_eq!(decisions, vec![Released, Forced, Forced, Forced, Released]);
    }

    #[test]
    fn s4_hysteresis_band_does_not_force_from_released() {
        assert_eq!(
            run(&[
                (Celsius(90.0), Celsius(94.0)),
                (Celsius(90.0), Celsius(94.0)),
                (Celsius(85.0), Absent)
            ]),
            vec![Released, Released, Released]
        );
    }

    #[test]
    fn s4_reforces_after_release() {
        let decisions = run(&[
            (Celsius(95.0), Absent),
            (Celsius(95.0), Absent),
            (Celsius(80.0), Absent),
            (Celsius(80.0), Absent),
            (Celsius(95.5), Absent),
            (Celsius(95.5), Absent),
        ]);
        assert_eq!(
            decisions,
            vec![Released, Forced, Forced, Released, Released, Forced]
        );
    }

    #[test]
    fn s4_absent_gpu_is_ignored() {
        let decisions = run(&[
            (Celsius(95.0), Absent),
            (Celsius(95.0), Absent),
            (Celsius(70.0), Absent),
            (Celsius(70.0), Absent),
        ]);
        assert_eq!(decisions, vec![Released, Forced, Forced, Released]);
    }

    #[test]
    fn s4_failed_sensor_forces_immediately() {
        let decisions = run(&[
            (Celsius(60.0), Failed),
            (Celsius(60.0), Celsius(60.0)),
            (Celsius(60.0), Celsius(60.0)),
            (Failed, Absent),
        ]);
        assert_eq!(decisions, vec![Forced, Forced, Released, Forced]);
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
