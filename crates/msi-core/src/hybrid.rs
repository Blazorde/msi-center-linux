//! Hybrid CPU fan control (ADR-0010): the EC ignores written CPU thresholds and picks its CPU step from a
//! smoothed temperature well below `cpu/realtime_temperature`, but applies written CPU **speeds** live (< 2 s).
//! So the daemon keeps the factory thresholds, keeps the hot steps 4–6 safe at all times, and drives the speed
//! of steps 0–3 live from its exponential curve evaluated on a filtered temperature.
//!
//! Three pure pieces, fed once per supervision tick (~1 Hz) by `msi-daemon`:
//!
//! 1. [`TempFilter`]: median of the last [`MEDIAN_WINDOW`] readings (an isolated spike of the EC sensor
//!    `0x68`, up to ~90 °C at idle, or two in a row, never passes) followed by an exponential moving average
//!    ([`EMA_ALPHA`]).
//! 2. [`CpuFanPilot`]: target speed `f(T_filtered)` of the active curve, with an idle hysteresis (the fan starts
//!    at `T0` and stops below `T0 − hysteresis`), a dead band ([`DEADBAND_PCT`], or [`HOLD_MS`] of persistent
//!    difference) and a slew rate ([`SLEW_UP_PCT`], [`SLEW_DOWN_PCT`] per update): the fan never "pumps".
//! 3. [`hybrid_cpu_table`]: the speed vector to write. Steps 0–3 = target (so whichever of them the EC sits in,
//!    the fan follows our curve), steps 4–6 never below `max(curve, 60 / 80 / 100 %)`, then the S2 floors on
//!    the ascending step of the factory thresholds. The result is a [`StepCurve`]: S1–S3, S2 and the kernel
//!    rules are checked by [`StepCurve::new`], never assumed.
//!
//! # Safety argument
//!
//! - Hot steps (EC smoothed ≥ 77 °C with the factory thresholds) are always ≥ 60 / 100 / 100 %, whatever the
//!   target and even if the daemon freezes: S2 puts 100 % on step 5 because 85 °C falls between its entry
//!   (83 °C) and step 6 (90 °C).
//! - Every vector passes S2 **statically** on the factory thresholds (≥ 30 % on step 1, ≥ 50 % on step 3,
//!   100 % on step 5), so a vector frozen by a dead daemon is never below the S2 floors on the steps the EC
//!   reaches when hot. Only step 0 (EC smoothed below 57 °C) can be at `idle`.
//! - The target is a function of the real (filtered) temperature: rising temperature raises steps 0–3 before the
//!   EC changes step. S4 (Cooler Boost at 95 °C) and S9 (stalled fan) are unchanged.

use crate::curve::{FanCurveSpec, MAX_SPEED, STEP_COUNT};
use crate::staged::{StagedCurve, StagedPilot};
use crate::table::{
    GeneratedCurve, RaisedPoint, RawTables, SPEED_FLOORS, SafetyReport, SpeedFloor, StepCurve, TableError,
};

/// Readings kept for the median.
pub const MEDIAN_WINDOW: usize = 5;
/// Readings needed before the filter outputs anything.
pub const FILTER_MIN_SAMPLES: usize = 3;
/// Weight of the new median in the moving average (≈ 3 s time constant at 1 Hz).
pub const EMA_ALPHA: f64 = 0.3;
/// First step whose speed is a permanent floor rather than the live target.
pub const FIRST_HOT_STEP: usize = 4;
/// Minimum speed (%) of steps 4, 5 and 6, whatever the curve (ADR-0010).
pub const HOT_STEP_FLOORS: [u8; STEP_COUNT - FIRST_HOT_STEP] = [60, 80, 100];
/// A target change smaller than this (%) waits [`HOLD_MS`] before being applied.
pub const DEADBAND_PCT: u8 = 3;
/// How long (ms) a small target change must persist before it is applied.
pub const HOLD_MS: u64 = 20_000;
/// Largest increase (%) of the output per update (≈ 1 s): 30 → 100 % in 14 s.
pub const SLEW_UP_PCT: u8 = 5;
/// Largest decrease (%) of the output per update (≈ 1 s): slow, quiet ramp down.
pub const SLEW_DOWN_PCT: u8 = 1;

/// Spike-robust temperature filter: median of the last [`MEDIAN_WINDOW`] readings, then a moving average.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TempFilter {
    window: [u8; MEDIAN_WINDOW],
    len: usize,
    next: usize,
    ema: Option<f64>,
}

impl TempFilter {
    /// An empty filter.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            window: [0; MEDIAN_WINDOW],
            len: 0,
            next: 0,
            ema: None,
        }
    }

    /// Forgets every reading.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Current filtered value (°C), `None` before [`FILTER_MIN_SAMPLES`] readings.
    #[must_use]
    pub const fn value(&self) -> Option<f64> {
        self.ema
    }

    /// Median of the readings in the window (upper median for an even count: the safe side).
    fn median(&self) -> u8 {
        let mut v = self.window;
        let v = &mut v[..self.len];
        v.sort_unstable();
        v[self.len / 2]
    }

    /// Feeds one reading (`None` = unreadable: ignored, the previous value is kept; S4 handles failed sensors)
    /// and returns the filtered temperature.
    pub fn update(&mut self, reading: Option<u8>) -> Option<f64> {
        let Some(t) = reading else {
            return self.ema;
        };
        self.window[self.next] = t;
        self.next = (self.next + 1) % MEDIAN_WINDOW;
        self.len = (self.len + 1).min(MEDIAN_WINDOW);
        if self.len < FILTER_MIN_SAMPLES {
            return None;
        }
        let median = f64::from(self.median());
        let ema = self.ema.map_or(median, |e| EMA_ALPHA.mul_add(median - e, e));
        self.ema = Some(ema);
        self.ema
    }
}

/// Live target speed of the CPU fan (steps 0–3) from the active curve and the filtered temperature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuFanPilot {
    output: Option<u8>,
    active: bool,
    pending_since_ms: Option<u64>,
}

impl CpuFanPilot {
    /// A pilot with no output yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            output: None,
            active: false,
            pending_since_ms: None,
        }
    }

    /// Forgets the previous output.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Last output (%), `None` before the first [`Self::update`].
    #[must_use]
    pub const fn output(&self) -> Option<u8> {
        self.output
    }

    /// Output to use when nothing was computed yet: the curve's start speed (the fan spins until the filter
    /// has enough readings).
    #[must_use]
    pub const fn initial(spec: &FanCurveSpec) -> u8 {
        spec.params().min()
    }

    /// Takes `output` (%) as the previous output, e.g. when another kind of curve was driving the fan. The
    /// idle state is recomputed from the next temperature.
    fn seed(&mut self, output: u8) {
        self.output = Some(output.min(MAX_SPEED));
        self.pending_since_ms = None;
    }

    /// Computes the next output (%) for `spec` at the filtered temperature `filtered` (`None` = not known
    /// yet: hold the previous output, or [`Self::initial`]).
    ///
    /// - Idle: below `T0 − hysteresis` the output is `idle` (it starts again at `T0`).
    /// - Active: the curve value, but a change smaller than [`DEADBAND_PCT`] waits [`HOLD_MS`] (a rise to the
    ///   curve maximum never waits), and each update moves by at most [`SLEW_UP_PCT`] up or
    ///   [`SLEW_DOWN_PCT`] down. Starting from idle the output jumps to the start speed `min` first (speeds
    ///   between 0 and the start speed are not used).
    ///
    /// The output is always within `idle..=max` of `spec`.
    pub fn update(&mut self, spec: &FanCurveSpec, filtered: Option<f64>, now_ms: u64) -> u8 {
        let params = spec.params();
        let Some(temp) = filtered else {
            return *self.output.get_or_insert(Self::initial(spec));
        };
        let t0 = f64::from(params.t0());
        if self.active {
            self.active = temp >= t0 - f64::from(spec.hysteresis());
        } else {
            self.active = temp >= t0 || temp.is_nan();
        }
        if !self.active {
            self.pending_since_ms = None;
            self.output = Some(spec.idle());
            return spec.idle();
        }
        let current = match self.output {
            Some(o) if o >= params.min() => o.min(params.max()),
            // Starting (from idle or from nothing): the start speed first, then the slew rate.
            _ => {
                self.pending_since_ms = None;
                self.output = Some(params.min());
                return params.min();
            }
        };
        let desired = params.speed_percent_celsius(temp);
        if desired == current {
            self.pending_since_ms = None;
            self.output = Some(current);
            return current;
        }
        let small = desired.abs_diff(current) < DEADBAND_PCT;
        let urgent = desired > current && desired == params.max();
        if small && !urgent {
            let since = *self.pending_since_ms.get_or_insert(now_ms);
            if now_ms.saturating_sub(since) < HOLD_MS {
                self.output = Some(current);
                return current;
            }
        }
        let next = if desired > current {
            desired.min(current.saturating_add(SLEW_UP_PCT))
        } else {
            desired.max(current.saturating_sub(SLEW_DOWN_PCT))
        };
        self.output = Some(next);
        next
    }
}

/// The curve that drives the live CPU target: the exponential curve of a preset or of the owner (ADR-0006), or
/// the owner's staged curve with any number of stages (ADR-0013).
#[derive(Debug, Clone, PartialEq)]
pub enum CpuCurve {
    /// Exponential curve.
    Exponential(FanCurveSpec),
    /// Staged curve.
    Staged(StagedCurve),
}

impl CpuCurve {
    /// Output to use before any temperature is known: the curve's start speed (the fan spins).
    #[must_use]
    pub fn initial(&self) -> u8 {
        match self {
            Self::Exponential(spec) => CpuFanPilot::initial(spec),
            Self::Staged(curve) => curve.start_speed(),
        }
    }

    /// Speed (%) below the curve's start (0 = fan stopped).
    #[must_use]
    pub fn idle(&self) -> u8 {
        match self {
            Self::Exponential(spec) => spec.idle(),
            Self::Staged(curve) => curve.idle(),
        }
    }

    /// Highest speed (%) the pilot can ask for.
    #[must_use]
    pub fn max_speed(&self) -> u8 {
        match self {
            Self::Exponential(spec) => spec.params().max(),
            Self::Staged(curve) => curve.max_speed(),
        }
    }

    /// Curve value (%) at `temp` °C, before the hot floors of [`hybrid_cpu_table_for`]. Staged curves include
    /// the S2 floors here.
    #[must_use]
    pub fn speed_percent(&self, temp: u8) -> u8 {
        match self {
            Self::Exponential(spec) => spec.params().speed_percent(temp),
            Self::Staged(curve) => curve.effective_speed(temp),
        }
    }
}

/// Live CPU pilot for either kind of [`CpuCurve`]. Keeps the output when the owner switches from one kind to
/// the other, so the fan does not drop to a start speed while the new curve takes over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuDriver {
    exponential: CpuFanPilot,
    staged: StagedPilot,
    staged_active: Option<bool>,
}

impl CpuDriver {
    /// A driver with no output yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            exponential: CpuFanPilot::new(),
            staged: StagedPilot::new(),
            staged_active: None,
        }
    }

    /// Forgets every output.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Last output (%) of the pilot in use, `None` before the first [`Self::update`].
    #[must_use]
    pub const fn output(&self) -> Option<u8> {
        match self.staged_active {
            Some(true) => self.staged.output(),
            Some(false) => self.exponential.output(),
            None => None,
        }
    }

    /// Live target for `curve`: the last output kept within the curve's range (the curve may have changed
    /// since), or the curve's start speed before any output.
    #[must_use]
    pub fn target(&self, curve: &CpuCurve) -> u8 {
        self.output()
            .map_or_else(|| curve.initial(), |o| o.clamp(curve.idle(), curve.max_speed()))
    }

    /// Computes the next output (%) for `curve` at the filtered temperature `filtered`. See
    /// [`CpuFanPilot::update`] and [`StagedPilot::update`].
    pub fn update(&mut self, curve: &CpuCurve, filtered: Option<f64>, now_ms: u64) -> u8 {
        let staged = matches!(curve, CpuCurve::Staged(_));
        if self.staged_active != Some(staged) {
            if let Some(previous) = self.output() {
                if staged {
                    self.staged.seed(previous);
                } else {
                    self.exponential.seed(previous);
                }
            }
            self.staged.restart_selection();
            self.staged_active = Some(staged);
        }
        match curve {
            CpuCurve::Exponential(spec) => self.exponential.update(spec, filtered, now_ms),
            CpuCurve::Staged(c) => self.staged.update(c, filtered),
        }
    }
}

/// The CPU table to write for a live `target` (%) under `spec`, keeping the thresholds of `base` (the factory
/// CPU tables, which the EC uses whatever is written):
///
/// - steps 0–3: `target`;
/// - steps 4–6: `max(target, f(up[i − 1]))`, raised to at least [`HOT_STEP_FLOORS`];
/// - then the S2 floors on the ascending step of `base.up` (same rule as [`crate::table::generate`]).
///
/// Every raise above the target / curve value is listed in the report (hot floors use the step entry
/// temperature as `floor.temp`).
///
/// # Errors
///
/// A [`TableError`] if the result fails [`StepCurve::new`] (e.g. `target` above 100 %, or `base` thresholds
/// that the kernel would reject). Nothing is corrected.
pub fn hybrid_cpu_table(
    spec: &FanCurveSpec,
    base: &RawTables,
    target: u8,
) -> Result<GeneratedCurve, TableError> {
    hybrid_cpu_table_for(&CpuCurve::Exponential(*spec), base, target)
}

/// [`hybrid_cpu_table`] for either kind of curve: the hot steps follow `curve` at their entry temperature,
/// whether it is exponential or staged.
///
/// # Errors
///
/// As [`hybrid_cpu_table`].
pub fn hybrid_cpu_table_for(
    curve: &CpuCurve,
    base: &RawTables,
    target: u8,
) -> Result<GeneratedCurve, TableError> {
    let up = base.up;
    let mut speeds = [target; STEP_COUNT];
    for (i, speed) in speeds.iter_mut().enumerate().skip(FIRST_HOT_STEP) {
        *speed = target.max(curve.speed_percent(up[i - 1]));
    }
    let mut raised = Vec::new();
    for (index, speed) in speeds.iter_mut().enumerate() {
        let entry = index.checked_sub(1).map_or(0, |i| up[i]);
        let hot = index.checked_sub(FIRST_HOT_STEP).map(|h| SpeedFloor {
            temp: entry,
            speed: HOT_STEP_FLOORS[h],
        });
        // On a tie the S2 floor (listed last) is reported: it is the stronger rule.
        let binding = hot
            .into_iter()
            .chain(
                SPEED_FLOORS
                    .iter()
                    .copied()
                    .filter(|floor| up.iter().filter(|&&u| u <= floor.temp).count() <= index),
            )
            .max_by_key(|floor| floor.speed);
        if let Some(floor) = binding.filter(|floor| *speed < floor.speed) {
            raised.push(RaisedPoint {
                index,
                temp: entry,
                curve_speed: *speed,
                applied_speed: floor.speed,
                floor,
            });
            *speed = floor.speed;
        }
    }
    let curve = StepCurve::new(speeds, up, base.down)?;
    Ok(GeneratedCurve {
        curve,
        report: SafetyReport { raised },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurvePreset;
    use crate::ec::FACTORY_14K2EMS1_110;

    const FACTORY: RawTables = FACTORY_14K2EMS1_110.cpu;

    fn feed(f: &mut TempFilter, readings: &[u8]) -> Option<f64> {
        let mut out = None;
        for &r in readings {
            out = f.update(Some(r));
        }
        out
    }

    #[test]
    fn filter_needs_three_readings() {
        let mut f = TempFilter::new();
        assert_eq!(f.update(Some(50)), None);
        assert_eq!(f.update(Some(50)), None);
        assert_eq!(f.update(Some(50)), Some(50.0));
        assert_eq!(f.value(), Some(50.0));
        f.reset();
        assert_eq!(f.value(), None);
    }

    #[test]
    fn filter_ignores_one_or_two_spikes_in_a_row() {
        let mut f = TempFilter::new();
        feed(&mut f, &[45; 5]);
        for spikes in [&[90][..], &[90, 92], &[45, 90, 45, 91, 45]] {
            let out = feed(&mut f, spikes).unwrap();
            assert!((out - 45.0).abs() < 1e-9, "{spikes:?}: {out}");
            feed(&mut f, &[45; 5]);
        }
        // A spike during start-up is ignored too.
        let mut f = TempFilter::new();
        assert_eq!(feed(&mut f, &[45, 90, 45]), Some(45.0));
    }

    #[test]
    fn filter_follows_a_real_rise_within_a_few_seconds() {
        let mut f = TempFilter::new();
        feed(&mut f, &[45; 5]);
        let mut out = 0.0;
        for s in 1..=10 {
            out = f.update(Some(70)).unwrap();
            if s == 2 {
                assert!(out < 46.0, "still the median of old readings: {out}");
            }
        }
        assert!(out > 68.0, "within 2 °C after 10 s: {out}");
    }

    #[test]
    fn filter_keeps_its_value_on_unreadable_sensor() {
        let mut f = TempFilter::new();
        feed(&mut f, &[60; 5]);
        assert_eq!(f.update(None), Some(60.0));
        assert_eq!(TempFilter::new().update(None), None);
    }

    fn silent() -> FanCurveSpec {
        CurvePreset::Silent.spec()
    }

    /// Runs the pilot once per second from 0 s over `temps` (filtered °C) and returns the outputs.
    fn run(p: &mut CpuFanPilot, spec: &FanCurveSpec, from_s: u64, temps: &[f64]) -> Vec<u8> {
        temps
            .iter()
            .enumerate()
            .map(|(i, &t)| p.update(spec, Some(t), (from_s + i as u64) * 1000))
            .collect()
    }

    #[test]
    fn pilot_starts_at_the_start_speed_without_temperature() {
        let mut p = CpuFanPilot::new();
        assert_eq!(p.update(&silent(), None, 0), 30);
        assert_eq!(p.output(), Some(30));
        assert_eq!(CpuFanPilot::initial(&CurvePreset::Aggressive.spec()), 40);
    }

    #[test]
    fn pilot_idle_hysteresis_does_not_start_stop_repeatedly() {
        let spec = silent(); // T0 = 52, hysteresis 4: on at 52, off below 48.
        let mut p = CpuFanPilot::new();
        assert_eq!(run(&mut p, &spec, 0, &[47.0, 51.9]), [0, 0]);
        assert_eq!(run(&mut p, &spec, 2, &[52.0]), [30]);
        // Back down to 48–51.9: keeps spinning at the start speed.
        assert_eq!(run(&mut p, &spec, 3, &[51.0, 49.0, 48.0]), [30, 30, 30]);
        assert_eq!(run(&mut p, &spec, 6, &[47.9]), [0]);
        assert_eq!(run(&mut p, &spec, 7, &[50.0, 51.0]), [0, 0]);
    }

    #[test]
    fn pilot_never_idles_with_a_spinning_idle_speed() {
        let spec = CurvePreset::Aggressive.spec(); // idle 30, min 40.
        let mut p = CpuFanPilot::new();
        assert_eq!(run(&mut p, &spec, 0, &[30.0]), [30]);
        assert_eq!(run(&mut p, &spec, 1, &[40.0]), [40]);
    }

    #[test]
    fn pilot_ramps_up_by_slew_steps_and_down_slowly() {
        let spec = silent();
        let mut p = CpuFanPilot::new();
        run(&mut p, &spec, 0, &[52.0]);
        // Sudden jump to 88 °C (curve 100 %): +5 % per second.
        let up = run(&mut p, &spec, 1, &[88.0; 16]);
        assert_eq!(&up[..3], [35, 40, 45]);
        assert_eq!(*up.last().unwrap(), 100);
        assert!(up.windows(2).all(|w| w[1] >= w[0] && w[1] - w[0] <= SLEW_UP_PCT));
        // Back to 60 °C (curve 32 %): −1 % per second.
        let down = run(&mut p, &spec, 17, &[60.0; 10]);
        assert_eq!(&down[..3], [99, 98, 97]);
    }

    #[test]
    fn pilot_dead_band_waits_then_applies_small_changes() {
        let spec = silent();
        let mut p = CpuFanPilot::new();
        // 71.5 °C → 45 %, reached by slew.
        let out = run(&mut p, &spec, 0, &[71.5; 10]);
        assert_eq!(*out.last().unwrap(), 45);
        // 72 °C → 46 %: smaller than the dead band, held for 20 s, then applied.
        let held = run(&mut p, &spec, 10, &[72.0; 21]);
        assert!(held[..20].iter().all(|&s| s == 45), "{held:?}");
        assert_eq!(held[20], 46);
        // Jitter of ±0.3 °C (46 / 47 %) never moves the output: the 47 % never lasts 20 s.
        let jitter: Vec<f64> = (0..60).map(|i| if i % 2 == 0 { 71.8 } else { 72.4 }).collect();
        let out = run(&mut p, &spec, 31, &jitter);
        assert!(out.iter().all(|&s| s == 46), "{out:?}");
    }

    #[test]
    fn pilot_holds_on_unknown_temperature() {
        let spec = silent();
        let mut p = CpuFanPilot::new();
        run(&mut p, &spec, 0, &[70.0; 10]);
        let before = p.output().unwrap();
        assert_eq!(p.update(&spec, None, 11_000), before);
        p.reset();
        assert_eq!(p.output(), None);
    }

    #[test]
    fn hybrid_table_puts_the_target_on_steps_0_to_3() {
        // Silent at 60 °C filtered: 32 %.
        let g = hybrid_cpu_table(&silent(), &FACTORY, 32).unwrap();
        assert_eq!(g.curve.speeds(), [32, 32, 32, 50, 60, 100, 100]);
        assert_eq!((g.curve.up(), g.curve.down()), (FACTORY.up, FACTORY.down));
        // Step 3 raised by S2 (75 °C); step 4 by its hot floor (curve 58 % at 77 °C); step 5 by S2 (85 °C,
        // curve 78 % at 83 °C); step 6 comes from the curve (100 % at 90 °C).
        let raised: Vec<(usize, u8, u8, u8)> = g
            .report
            .raised
            .iter()
            .map(|r| (r.index, r.curve_speed, r.applied_speed, r.floor.temp))
            .collect();
        assert_eq!(raised, [(3, 32, 50, 75), (4, 58, 60, 77), (5, 78, 100, 85)]);
    }

    #[test]
    fn hybrid_table_at_idle_keeps_the_hot_steps_safe() {
        let g = hybrid_cpu_table(&silent(), &FACTORY, 0).unwrap();
        assert_eq!(g.curve.speeds(), [0, 30, 30, 50, 60, 100, 100]);
        let g = hybrid_cpu_table(&CurvePreset::Progressive.spec(), &FACTORY, 0).unwrap();
        assert_eq!(g.curve.speeds(), [0, 30, 30, 50, 71, 100, 100]);
        let g = hybrid_cpu_table(&CurvePreset::Aggressive.spec(), &FACTORY, 30).unwrap();
        assert_eq!(g.curve.speeds(), [30, 30, 30, 50, 100, 100, 100]);
        let g = hybrid_cpu_table(&silent(), &FACTORY, 100).unwrap();
        assert_eq!(g.curve.speeds(), [100; 7]);
        assert!(g.report.is_clean());
    }

    #[test]
    fn hybrid_table_hot_floors_bind_a_low_curve() {
        // Flat 0 % curve: hot steps come from the 60 / 80 / 100 floors and S2.
        let spec = FanCurveSpec::new(
            crate::curve::FanCurveParams::new(90, 100, 0, 0, 1.0).unwrap(),
            0,
            [55, 65, 80, 88, 92, 96],
            4,
        )
        .unwrap();
        let g = hybrid_cpu_table(&spec, &FACTORY, 0).unwrap();
        assert_eq!(g.curve.speeds(), [0, 30, 30, 50, 60, 100, 100]);
        let hot: Vec<(usize, u8, u8)> = g
            .report
            .raised
            .iter()
            .filter(|r| r.index >= FIRST_HOT_STEP)
            .map(|r| (r.index, r.applied_speed, r.floor.temp))
            .collect();
        // Step 4: hot floor at its entry (77 °C); steps 5–6: S2 100 % at 85 °C beats the hot floors.
        assert_eq!(hot, [(4, 60, 77), (5, 100, 85), (6, 100, 85)]);
    }

    #[test]
    fn hybrid_table_rejects_invalid_input() {
        assert!(matches!(
            hybrid_cpu_table(&silent(), &FACTORY, 101),
            Err(TableError::SpeedOutOfRange { .. })
        ));
        let mut bad = FACTORY;
        bad.down[0] = bad.up[0];
        assert!(hybrid_cpu_table(&silent(), &bad, 40).is_err());
    }

    #[test]
    fn preset_targets_match_the_documented_table() {
        // docs/hardware/fan-safety.md, "Pilotage hybride du ventilo CPU".
        let p = silent().params();
        let at: Vec<u8> = [52.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0, 88.0]
            .iter()
            .map(|&t| p.speed_percent_celsius(t))
            .collect();
        assert_eq!(at, [30, 30, 32, 35, 42, 53, 67, 86, 100]);
        let p = CurvePreset::Progressive.spec().params();
        let at: Vec<u8> = [48.0, 55.0, 60.0, 65.0, 70.0, 75.0, 80.0, 85.0]
            .iter()
            .map(|&t| p.speed_percent_celsius(t))
            .collect();
        assert_eq!(at, [30, 32, 36, 43, 52, 65, 81, 100]);
        let p = CurvePreset::Aggressive.spec().params();
        let at: Vec<u8> = [40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 74.0]
            .iter()
            .map(|&t| p.speed_percent_celsius(t))
            .collect();
        assert_eq!(at, [40, 44, 51, 59, 69, 79, 90, 100]);
    }

    fn staged_curve() -> StagedCurve {
        use crate::staged::Stage;
        StagedCurve::new(
            0,
            vec![
                Stage {
                    up: 50,
                    down: 46,
                    speed: 30,
                },
                Stage {
                    up: 60,
                    down: 56,
                    speed: 45,
                },
                Stage {
                    up: 70,
                    down: 66,
                    speed: 70,
                },
                Stage {
                    up: 80,
                    down: 76,
                    speed: 100,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn hybrid_table_for_a_staged_curve_follows_its_stages_on_the_hot_steps() {
        // Factory entries of the hot steps: 77, 83 and 90 °C → 70 %, 100 %, 100 % on this curve.
        let curve = CpuCurve::Staged(staged_curve());
        let g = hybrid_cpu_table_for(&curve, &FACTORY, 45).unwrap();
        assert_eq!(g.curve.speeds(), [45, 45, 45, 50, 70, 100, 100]);
        assert_eq!((g.curve.up(), g.curve.down()), (FACTORY.up, FACTORY.down));
        let raised: Vec<(usize, u8, u8)> = g
            .report
            .raised
            .iter()
            .map(|r| (r.index, r.curve_speed, r.applied_speed))
            .collect();
        assert_eq!(raised, [(3, 45, 50)]);
    }

    #[test]
    fn hybrid_table_for_a_quiet_staged_curve_keeps_the_hot_steps_safe() {
        use crate::staged::Stage;
        let quiet = StagedCurve::new(
            0,
            vec![
                Stage {
                    up: 40,
                    down: 36,
                    speed: 20,
                },
                Stage {
                    up: 90,
                    down: 80,
                    speed: 100,
                },
            ],
        )
        .unwrap();
        let g = hybrid_cpu_table_for(&CpuCurve::Staged(quiet), &FACTORY, 0).unwrap();
        assert_eq!(g.curve.speeds(), [0, 30, 30, 50, 60, 100, 100]);
    }

    #[test]
    fn hybrid_table_for_an_exponential_curve_is_hybrid_cpu_table() {
        for preset in CurvePreset::ALL {
            let spec = preset.spec();
            for target in [0, 30, 55, 100] {
                assert_eq!(
                    hybrid_cpu_table_for(&CpuCurve::Exponential(spec), &FACTORY, target),
                    hybrid_cpu_table(&spec, &FACTORY, target),
                    "{preset:?} {target}"
                );
            }
        }
    }

    #[test]
    fn staged_target_above_100_is_rejected() {
        assert!(matches!(
            hybrid_cpu_table_for(&CpuCurve::Staged(staged_curve()), &FACTORY, 101),
            Err(TableError::SpeedOutOfRange { .. })
        ));
    }

    #[test]
    fn cpu_curve_accessors() {
        let exp = CpuCurve::Exponential(silent());
        assert_eq!((exp.initial(), exp.idle(), exp.max_speed()), (30, 0, 100));
        assert_eq!(exp.speed_percent(60), 32);
        let staged = CpuCurve::Staged(staged_curve());
        assert_eq!(
            (staged.initial(), staged.idle(), staged.max_speed()),
            (30, 0, 100)
        );
        assert_eq!(staged.speed_percent(65), 45);
        assert_eq!(staged.speed_percent(40), 0);
    }

    #[test]
    fn driver_keeps_the_output_when_the_curve_kind_changes() {
        let mut d = CpuDriver::new();
        assert_eq!(d.output(), None);
        let exp = CpuCurve::Exponential(silent());
        assert_eq!(d.target(&exp), 30);
        // Ramp 30 → 40 %, then the last 2 % wait out the 20 s dead band.
        for s in 0..40 {
            d.update(&exp, Some(70.0), s * 1000);
        }
        assert_eq!(d.output(), Some(42));
        // Staged at 70 °C asks 70 %: it takes over from 42 %, ramping by the slew rate, not from 30 %.
        let staged = CpuCurve::Staged(staged_curve());
        assert_eq!(d.update(&staged, Some(70.0), 40_000), 42 + SLEW_UP_PCT);
        assert_eq!(d.output(), Some(47));
        assert_eq!(d.target(&staged), 47);
        // And back: the exponential pilot (42 % at 70 °C) takes over from 47 %, ramping down.
        assert_eq!(d.update(&exp, Some(70.0), 41_000), 47 - SLEW_DOWN_PCT);
        d.reset();
        assert_eq!(d.output(), None);
    }

    #[test]
    fn driver_target_stays_within_the_new_curve_range() {
        let mut d = CpuDriver::new();
        let staged = CpuCurve::Staged(staged_curve());
        for _ in 0..30 {
            d.update(&staged, Some(85.0), 0);
        }
        assert_eq!(d.output(), Some(100));
        let aggressive = CpuCurve::Exponential(CurvePreset::Aggressive.spec());
        assert_eq!(d.target(&aggressive), 100);
        let capped = CpuCurve::Exponential(
            FanCurveSpec::new(
                crate::curve::FanCurveParams::new(50, 90, 30, 60, 2.0).unwrap(),
                0,
                [50, 57, 64, 70, 76, 82],
                4,
            )
            .unwrap(),
        );
        assert_eq!(d.target(&capped), 60);
    }
}
