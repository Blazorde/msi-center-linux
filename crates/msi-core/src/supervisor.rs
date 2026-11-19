//! Thermal supervisor, rule S4: CPU or GPU ≥ 95 °C → Cooler Boost forced until both are back below 85 °C.
//!
//! Pure state machine: the daemon feeds it readings (~1 s) and applies the decision.
//!
//! # Spike filter
//!
//! The EC's instantaneous CPU temperature (register `0x68`) has isolated spikes (up to ~90 °C at idle, see
//! `docs/hardware/ec-map.md`), and the CPU bursts to 95–98 °C for a few seconds under PL2 (measured
//! 2026-11-19: Cooler Boost forced for 3–10 s several times an hour). Each sensor is therefore filtered:
//!
//! - boost is forced only if the last [`FORCE_WINDOW`] readings (~10 s) are **all** ≥ [`BOOST_ON_TEMP`]
//!   (sustained heat, not a burst), or if the last **two** readings are ≥ [`BOOST_CRITICAL_TEMP`] (a real
//!   emergency, caught one sample later);
//! - boost is released only if **both** readings are < [`BOOST_OFF_TEMP`] (two-sample moving maximum);
//! - a failed reading forces boost immediately (fail-safe, no filtering);
//! - right after start, or after a failed/absent sensor comes back, one reading is not enough to force nor
//!   to release: the current state is kept until a second reading arrives.

/// Cooler Boost is forced at or above this temperature (°C).
pub const BOOST_ON_TEMP: f64 = 95.0;
/// Cooler Boost is released only when every sensor is strictly below this temperature (°C).
pub const BOOST_OFF_TEMP: f64 = 85.0;
/// Two consecutive readings at or above this temperature (°C) force Cooler Boost without waiting for
/// [`FORCE_WINDOW`]. The 185H throttles at 110 °C; bursts measured on this machine peak at 98 °C.
pub const BOOST_CRITICAL_TEMP: f64 = 100.0;
/// Consecutive readings (~1 s each) that must all be ≥ [`BOOST_ON_TEMP`] to force Cooler Boost.
pub const FORCE_WINDOW: usize = 10;

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
    /// Failed, [`FORCE_WINDOW`] readings ≥ 95 °C, or two readings ≥ 100 °C.
    Hot,
    /// Last two readings below 85 °C.
    Cool,
    /// Anything else (hysteresis band, single spike, not enough history).
    Hold,
}

/// Last usable readings of one sensor, newest first (`len` of them are valid).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct SensorFilter {
    recent: [f64; FORCE_WINDOW],
    len: usize,
}

impl SensorFilter {
    fn update(&mut self, reading: SensorReading) -> Verdict {
        let temp = match reading {
            SensorReading::Celsius(t) if t.is_finite() => t,
            SensorReading::Celsius(_) | SensorReading::Failed => {
                self.len = 0;
                return Verdict::Hot;
            }
            SensorReading::Absent => {
                self.len = 0;
                return Verdict::Absent;
            }
        };
        self.recent.copy_within(..FORCE_WINDOW - 1, 1);
        self.recent[0] = temp;
        self.len = (self.len + 1).min(FORCE_WINDOW);
        let last_two = (self.len >= 2).then(|| {
            (
                self.recent[0].min(self.recent[1]),
                self.recent[0].max(self.recent[1]),
            )
        });
        let sustained = self.len == FORCE_WINDOW && self.recent.iter().all(|&t| t >= BOOST_ON_TEMP);
        match last_two {
            Some((low, _)) if low >= BOOST_CRITICAL_TEMP => Verdict::Hot,
            _ if sustained => Verdict::Hot,
            Some((_, high)) if high < BOOST_OFF_TEMP => Verdict::Cool,
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
            cpu: SensorFilter {
                recent: [0.0; FORCE_WINDOW],
                len: 0,
            },
            gpu: SensorFilter {
                recent: [0.0; FORCE_WINDOW],
                len: 0,
            },
        }
    }

    /// Current decision.
    #[must_use]
    pub const fn state(&self) -> BoostDecision {
        self.state
    }

    /// Feeds new readings and returns the decision to apply.
    ///
    /// - any sensor hot (failed, [`FORCE_WINDOW`] readings ≥ [`BOOST_ON_TEMP`], or two ≥ [`BOOST_CRITICAL_TEMP`])
    ///   → [`BoostDecision::Forced`];
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

/// A Cooler Boost that S4 did not force (turned on from the app, the Fn key, or found on at start) is switched
/// off once every sensor stays below this temperature (°C) for [`BOOST_AUTO_OFF_MS`].
pub const BOOST_AUTO_OFF_TEMP: f64 = 75.0;
/// How long (ms) the sensors must stay below [`BOOST_AUTO_OFF_TEMP`] before Cooler Boost goes off by itself.
pub const BOOST_AUTO_OFF_MS: u64 = 30_000;

/// Switches off a Cooler Boost left on once the machine has cooled down (owner request 2026-11-19: « quand le
/// cooler boost est activé je suis obligé de le désactiver à la main »). Never acts while S4 forces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BoostAutoOff {
    cool_since_ms: Option<u64>,
}

impl BoostAutoOff {
    /// A timer that has seen nothing yet.
    #[must_use]
    pub const fn new() -> Self {
        Self { cool_since_ms: None }
    }

    /// Feeds one tick. `cpu` should be spike-filtered (an isolated spike would restart the timer). Returns
    /// `true` when Cooler Boost must be switched off now; the timer then starts over.
    pub fn update(
        &mut self,
        boost_on: bool,
        forced: bool,
        cpu: SensorReading,
        gpu: SensorReading,
        now_ms: u64,
    ) -> bool {
        let cool = |r: SensorReading, absent_ok: bool| match r {
            SensorReading::Celsius(t) => t.is_finite() && t < BOOST_AUTO_OFF_TEMP,
            SensorReading::Absent => absent_ok,
            SensorReading::Failed => false,
        };
        if !boost_on || forced || !cool(cpu, false) || !cool(gpu, true) {
            self.cool_since_ms = None;
            return false;
        }
        let since = *self.cool_since_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) >= BOOST_AUTO_OFF_MS {
            self.cool_since_ms = None;
            return true;
        }
        false
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

    /// `FORCE_WINDOW` readings of `(cpu, gpu)`: enough to force Cooler Boost when they are ≥ 95 °C.
    fn sustained(cpu: SensorReading, gpu: SensorReading) -> Vec<(SensorReading, SensorReading)> {
        vec![(cpu, gpu); FORCE_WINDOW]
    }

    /// Decisions over [`sustained`] hot readings: released until the last one.
    fn forced_at_window_end() -> Vec<BoostDecision> {
        let mut d = vec![Released; FORCE_WINDOW - 1];
        d.push(Forced);
        d
    }

    fn then(
        mut a: Vec<(SensorReading, SensorReading)>,
        b: &[(SensorReading, SensorReading)],
    ) -> Vec<(SensorReading, SensorReading)> {
        a.extend_from_slice(b);
        a
    }

    fn expect(mut a: Vec<BoostDecision>, b: &[BoostDecision]) -> Vec<BoostDecision> {
        a.extend_from_slice(b);
        a
    }

    #[test]
    fn s4_forces_after_ten_readings_at_95_on_cpu() {
        assert_eq!(run(&sustained(Celsius(95.0), Absent)), forced_at_window_end());
        let mut almost = sustained(Celsius(95.0), Absent);
        almost[0] = (Celsius(94.9), Absent);
        assert_eq!(run(&almost), vec![Released; FORCE_WINDOW]);
    }

    #[test]
    fn s4_forces_after_ten_readings_at_95_on_gpu() {
        assert_eq!(
            run(&sustained(Celsius(50.0), Celsius(96.0))),
            forced_at_window_end()
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
    fn s4_short_burst_is_ignored() {
        // PL2 burst measured 2026-11-19: 95–98 °C for a few seconds, then back to the 70s.
        let mut steps = vec![(Celsius(70.0), Absent)];
        steps.extend(vec![(Celsius(98.0), Absent); FORCE_WINDOW - 1]);
        steps.push((Celsius(80.0), Absent));
        steps.extend(vec![(Celsius(97.0), Absent); FORCE_WINDOW - 1]);
        assert_eq!(run(&steps), vec![Released; steps.len()]);
    }

    #[test]
    fn s4_critical_temperature_forces_after_two_readings() {
        assert_eq!(
            run(&[
                (Celsius(70.0), Absent),
                (Celsius(100.0), Absent),
                (Celsius(101.0), Absent)
            ]),
            vec![Released, Released, Forced]
        );
        assert_eq!(
            run(&[(Celsius(60.0), Celsius(100.0)), (Celsius(60.0), Celsius(100.0))]),
            vec![Released, Forced]
        );
    }

    #[test]
    fn s4_hysteresis_keeps_boost_until_both_below_85() {
        let decisions = run(&then(
            sustained(Celsius(96.0), Celsius(70.0)),
            &[
                (Celsius(90.0), Celsius(70.0)),
                (Celsius(85.0), Celsius(70.0)), // 85 is not < 85
                (Celsius(84.9), Celsius(70.0)), // previous reading still 85
                (Celsius(84.9), Celsius(70.0)),
            ],
        ));
        assert_eq!(
            decisions,
            expect(forced_at_window_end(), &[Forced, Forced, Forced, Released])
        );
    }

    #[test]
    fn s4_downward_spike_does_not_release() {
        let decisions = run(&then(
            sustained(Celsius(96.0), Absent),
            &[(Celsius(40.0), Absent), (Celsius(92.0), Absent)],
        ));
        assert_eq!(decisions, expect(forced_at_window_end(), &[Forced, Forced]));
    }

    #[test]
    fn s4_release_needs_every_sensor_below_85() {
        let decisions = run(&then(
            sustained(Celsius(60.0), Celsius(95.0)),
            &[
                (Celsius(60.0), Celsius(86.0)),
                (Celsius(84.0), Celsius(84.0)),
                (Celsius(84.0), Celsius(84.0)),
            ],
        ));
        assert_eq!(
            decisions,
            expect(forced_at_window_end(), &[Forced, Forced, Released])
        );
    }

    #[test]
    fn s4_hysteresis_band_does_not_force_from_released() {
        let steps = then(
            sustained(Celsius(90.0), Celsius(94.0)),
            &[(Celsius(85.0), Absent)],
        );
        assert_eq!(run(&steps), vec![Released; steps.len()]);
    }

    #[test]
    fn s4_reforces_after_release() {
        let steps = then(
            then(
                sustained(Celsius(95.0), Absent),
                &[(Celsius(80.0), Absent), (Celsius(80.0), Absent)],
            ),
            &sustained(Celsius(95.5), Absent),
        );
        let expected = expect(
            expect(forced_at_window_end(), &[Forced, Released]),
            &forced_at_window_end(),
        );
        assert_eq!(run(&steps), expected);
    }

    #[test]
    fn s4_absent_gpu_is_ignored() {
        let decisions = run(&then(
            sustained(Celsius(95.0), Absent),
            &[(Celsius(70.0), Absent), (Celsius(70.0), Absent)],
        ));
        assert_eq!(decisions, expect(forced_at_window_end(), &[Forced, Released]));
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

    #[test]
    fn auto_off_after_30_s_below_75() {
        let mut a = BoostAutoOff::new();
        assert!(!a.update(true, false, Celsius(70.0), Absent, 0));
        assert!(!a.update(true, false, Celsius(70.0), Celsius(60.0), BOOST_AUTO_OFF_MS - 1));
        assert!(a.update(true, false, Celsius(70.0), Celsius(60.0), BOOST_AUTO_OFF_MS));
    }

    #[test]
    fn auto_off_restarts_when_hot_again() {
        let mut a = BoostAutoOff::new();
        assert!(!a.update(true, false, Celsius(70.0), Absent, 0));
        assert!(!a.update(true, false, Celsius(75.0), Absent, 20_000)); // 75 is not < 75
        assert!(!a.update(true, false, Celsius(70.0), Absent, 25_000));
        assert!(!a.update(true, false, Celsius(70.0), Absent, 25_000 + BOOST_AUTO_OFF_MS - 1));
        assert!(a.update(true, false, Celsius(70.0), Absent, 25_000 + BOOST_AUTO_OFF_MS));
    }

    #[test]
    fn auto_off_never_acts_while_forced_off_or_unreadable() {
        let mut a = BoostAutoOff::new();
        for t in [0, 40_000, 80_000] {
            assert!(!a.update(true, true, Celsius(60.0), Absent, t), "forced by S4");
        }
        let mut a = BoostAutoOff::new();
        for t in [0, 40_000, 80_000] {
            assert!(!a.update(false, false, Celsius(60.0), Absent, t), "already off");
        }
        let mut a = BoostAutoOff::new();
        for t in [0, 40_000, 80_000] {
            assert!(!a.update(true, false, Failed, Absent, t), "CPU unreadable");
            assert!(!a.update(true, false, Absent, Absent, t + 1), "CPU absent");
            assert!(
                !a.update(true, false, Celsius(60.0), Failed, t + 2),
                "GPU unreadable"
            );
        }
    }
}
