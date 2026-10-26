//! Fan stall detector, rule S9: while our tables are in force (fan mode `advanced`), a fan that reports 0 %
//! (and 0 rpm when rpm is available) although its **filtered** temperature is ≥ [`STALL_TEMP`] for
//! [`STALL_DURATION_MS`] or more means the fan is not cooling a hot chip. The daemon then hands control back to
//! the firmware (factory tables + fan mode `auto`) and stays read-only until restarted.
//!
//! Why a fixed temperature and not the table's first threshold: the EC drives the CPU fan from a smoothed,
//! lagging temperature well below `cpu/realtime_temperature` (spike of 2026-10-25: realtime 61–68 °C while the
//! fan stays on a low step), and ignores written CPU thresholds. A fan at 0 % at 57–60 °C realtime is normal
//! step-0 behaviour; at a sustained 70 °C it is not, whatever the table.
//!
//! Pure state machine fed once per supervision tick (~1 Hz).
//!
//! # Filtering and debounce
//!
//! - temperature: the **minimum of the last two readings** must be ≥ [`STALL_TEMP`] (an isolated spike of the
//!   EC sensor `0x68`, up to ~90 °C at idle, never counts);
//! - the stall must last [`STALL_DURATION_MS`] (between the first and the latest stalled samples);
//! - a single non-stalled sample between stalled ones (downward spike, one bad rpm reading) is ignored; two
//!   consecutive non-stalled samples reset the timer;
//! - an unreadable temperature or speed is not a stall (S4 handles failed sensors) and breaks the filter.

/// Filtered temperature (°C) at or above which a stopped fan counts as stalled.
pub const STALL_TEMP: u8 = 70;
/// How long (ms) the stall must last before S9 trips.
pub const STALL_DURATION_MS: u64 = 30_000;
/// Consecutive non-stalled samples that reset the timer.
const RESET_AFTER: u8 = 2;

/// One fan as seen by the daemon during a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FanSample {
    /// Temperature of the sensor driving the fan (°C), `None` if unreadable.
    pub temp: Option<u8>,
    /// Real-time speed reported by the EC (%), `None` if unreadable.
    pub speed_pct: Option<u8>,
    /// Real-time rpm (hwmon), `None` if unavailable.
    pub rpm: Option<u32>,
}

impl FanSample {
    /// `true` when the fan is stopped: 0 %, and 0 rpm when rpm is available.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.speed_pct == Some(0) && self.rpm.is_none_or(|r| r == 0)
    }
}

/// Debounced S9 detector for one fan. Starts idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StallDetector {
    prev_temp: Option<u8>,
    since_ms: Option<u64>,
    misses: u8,
}

impl StallDetector {
    /// An idle detector.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            prev_temp: None,
            since_ms: None,
            misses: 0,
        }
    }

    /// Forgets the history (new tables applied, control handed back).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Filtered temperature: minimum of this reading and the previous one (`None` without two readings).
    fn filtered(&mut self, temp: Option<u8>) -> Option<u8> {
        let prev = std::mem::replace(&mut self.prev_temp, temp);
        Some(prev?.min(temp?))
    }

    /// Feeds one sample taken at `now_ms` (monotonic). Returns `true` when S9 trips: the latest sample is
    /// stalled and the stall has lasted at least [`STALL_DURATION_MS`].
    #[must_use]
    pub fn update(&mut self, now_ms: u64, sample: FanSample) -> bool {
        let hot = self.filtered(sample.temp).is_some_and(|t| t >= STALL_TEMP);
        if hot && sample.is_stopped() {
            self.misses = 0;
            let since = *self.since_ms.get_or_insert(now_ms);
            return now_ms.saturating_sub(since) >= STALL_DURATION_MS;
        }
        if self.since_ms.is_some() {
            self.misses += 1;
            if self.misses >= RESET_AFTER {
                self.since_ms = None;
                self.misses = 0;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stopped(temp: u8) -> FanSample {
        FanSample {
            temp: Some(temp),
            speed_pct: Some(0),
            rpm: None,
        }
    }

    fn spinning(temp: u8) -> FanSample {
        FanSample {
            temp: Some(temp),
            speed_pct: Some(30),
            rpm: Some(2070),
        }
    }

    /// Feeds one sample per second from t = 0 and returns the trip decisions.
    fn run(samples: &[FanSample]) -> Vec<bool> {
        let mut d = StallDetector::new();
        (0u64..)
            .zip(samples)
            .map(|(i, &s)| d.update(i * 1000, s))
            .collect()
    }

    fn first_trip(samples: &[FanSample]) -> Option<usize> {
        run(samples).iter().position(|&t| t)
    }

    #[test]
    fn s9_stopped_means_0_percent_and_0_rpm_when_known() {
        assert!(stopped(80).is_stopped());
        assert!(
            FanSample {
                speed_pct: Some(0),
                rpm: Some(0),
                ..stopped(80)
            }
            .is_stopped()
        );
        // The EC says 0 % but the fan still turns (spinning down): not stopped.
        assert!(
            !FanSample {
                speed_pct: Some(0),
                rpm: Some(1500),
                ..stopped(80)
            }
            .is_stopped()
        );
        assert!(!spinning(80).is_stopped());
        assert!(
            !FanSample {
                speed_pct: None,
                rpm: Some(0),
                ..stopped(80)
            }
            .is_stopped()
        );
    }

    #[test]
    fn s9_trips_after_30_s_of_a_stopped_fan_at_70() {
        // Filter needs two readings: the first stalled sample is at t = 1 s, the trip at t = 31 s.
        assert_eq!(first_trip(&[stopped(70); 40]), Some(31));
        assert_eq!(first_trip(&[stopped(95); 40]), Some(31));
    }

    #[test]
    fn s9_below_70_never_trips() {
        // First real run: CPU at 57–68 °C with the fan on step 0 is the factory behaviour.
        let samples: Vec<FanSample> = (0..120).map(|i| stopped(57 + (i % 12))).collect();
        assert_eq!(first_trip(&samples), None);
        assert_eq!(first_trip(&[stopped(69); 120]), None);
    }

    #[test]
    fn s9_isolated_hot_spikes_never_trip() {
        // Idle at 55 °C with 90 °C spikes every other second: the two-reading minimum stays at 55.
        let samples: Vec<FanSample> = (0..120)
            .map(|i| if i % 2 == 0 { stopped(90) } else { stopped(55) })
            .collect();
        assert_eq!(first_trip(&samples), None);
    }

    #[test]
    fn s9_single_good_sample_is_ignored_two_reset() {
        let mut one_gap = vec![stopped(75); 40];
        one_gap[10] = spinning(75);
        assert_eq!(
            first_trip(&one_gap),
            Some(31),
            "one non-stalled sample does not reset"
        );

        let mut two_gap = vec![stopped(75); 50];
        two_gap[10] = spinning(75);
        two_gap[11] = spinning(75);
        assert_eq!(first_trip(&two_gap), Some(42), "restarted at 12 s");
    }

    #[test]
    fn s9_unreadable_temperature_breaks_the_filter() {
        let mut s = vec![stopped(80); 60];
        s[20] = FanSample { temp: None, ..s[20] };
        // t = 20 not hot, t = 21 not hot (filter refilled at 22): two misses → restart at 22.
        assert_eq!(first_trip(&s), Some(52));
    }

    #[test]
    fn s9_reset_forgets_the_history() {
        let mut d = StallDetector::new();
        assert!(!d.update(0, stopped(80)));
        assert!(!d.update(1_000, stopped(80)));
        d.reset();
        assert!(
            !d.update(31_000, stopped(80)),
            "one reading after reset is not filtered yet"
        );
        assert!(!d.update(32_000, stopped(80)));
        assert!(d.update(62_000, stopped(80)));
    }

    #[test]
    fn s9_clock_going_backwards_does_not_trip() {
        let mut d = StallDetector::new();
        assert!(!d.update(50_000, stopped(80)));
        assert!(!d.update(51_000, stopped(80)));
        assert!(!d.update(10_000, stopped(80)));
    }

    #[test]
    fn s9_a_spinning_fan_never_trips() {
        assert_eq!(first_trip(&[spinning(90); 120]), None);
    }
}
