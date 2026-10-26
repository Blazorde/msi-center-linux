//! Geometry of the drawings: fan step chart, arc gauges and the "Golden Gate" backdrop (pure, in pixels).

use std::f64::consts::PI;

use msi_core::table::SPEED_FLOORS;

use crate::curve::Table;

/// Lowest temperature on the chart (°C), the lowest threshold the EC accepts.
pub const T_MIN: f64 = 20.0;
/// Highest temperature on the chart (°C).
pub const T_MAX: f64 = 100.0;

/// Margins around the plot area.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Padding {
    /// Left (room for the % labels).
    pub left: f64,
    /// Right.
    pub right: f64,
    /// Top.
    pub top: f64,
    /// Bottom (room for the °C labels).
    pub bottom: f64,
}

impl Padding {
    /// Default chart margins.
    pub const CHART: Self = Self {
        left: 44.0,
        right: 16.0,
        top: 16.0,
        bottom: 30.0,
    };
}

/// Plot area of the step chart: maps °C to x and % to y.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Left edge.
    pub x0: f64,
    /// Top edge.
    pub y0: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Frame {
    /// Plot area of a `width × height` widget, or `None` if too small to draw anything.
    #[must_use]
    pub fn new(width: f64, height: f64, pad: Padding) -> Option<Self> {
        let w = width - pad.left - pad.right;
        let h = height - pad.top - pad.bottom;
        (w >= 40.0 && h >= 40.0).then_some(Self {
            x0: pad.left,
            y0: pad.top,
            w,
            h,
        })
    }

    /// x of a temperature (clamped to the chart range).
    #[must_use]
    pub fn x(&self, temp: f64) -> f64 {
        let t = temp.clamp(T_MIN, T_MAX);
        self.x0 + (t - T_MIN) / (T_MAX - T_MIN) * self.w
    }

    /// y of a speed % (clamped to 0..=100; 100 % at the top).
    #[must_use]
    pub fn y(&self, pct: f64) -> f64 {
        let p = pct.clamp(0.0, 100.0);
        self.y0 + self.h - p / 100.0 * self.h
    }

    /// Pixel point of a data point.
    #[must_use]
    pub fn point(&self, (temp, pct): (f64, f64)) -> (f64, f64) {
        (self.x(temp), self.y(pct))
    }
}

fn staircase(speeds: [u8; 7], thresholds: [u8; 6]) -> Vec<(f64, f64)> {
    let mut pts = vec![(T_MIN, f64::from(speeds[0]))];
    for (i, &t) in thresholds.iter().enumerate() {
        let t = f64::from(t);
        pts.push((t, f64::from(speeds[i])));
        pts.push((t, f64::from(speeds[i + 1])));
    }
    pts.push((T_MAX, f64::from(speeds[6])));
    pts
}

/// Rising path of the table in data coordinates (°C, %): flat steps joined by vertical risers at the up
/// thresholds.
#[must_use]
pub fn rising_path(t: &Table) -> Vec<(f64, f64)> {
    staircase(t.speeds, t.up)
}

/// Falling path (risers at the down thresholds), drawn dashed to show the hysteresis.
#[must_use]
pub fn falling_path(t: &Table) -> Vec<(f64, f64)> {
    staircase(t.speeds, t.down)
}

/// Minimum speed imposed by the S2 floors, as a staircase from the first floor to `T_MAX`.
#[must_use]
pub fn floor_path() -> Vec<(f64, f64)> {
    let mut pts = Vec::new();
    for (i, f) in SPEED_FLOORS.iter().enumerate() {
        let t = f64::from(f.temp);
        if i > 0 {
            let prev = f64::from(SPEED_FLOORS[i - 1].speed);
            pts.push((t, prev));
        }
        pts.push((t, f64::from(f.speed)));
    }
    pts.push((T_MAX, f64::from(SPEED_FLOORS[SPEED_FLOORS.len() - 1].speed)));
    pts
}

/// Centre of each step's flat segment (where its label and "raised" mark go), in data coordinates.
#[must_use]
pub fn step_centres(t: &Table) -> [(f64, f64); 7] {
    let mut out = [(0.0, 0.0); 7];
    for (i, o) in out.iter_mut().enumerate() {
        let start = if i == 0 { T_MIN } else { f64::from(t.up[i - 1]) };
        let end = if i == 6 { T_MAX } else { f64::from(t.up[i]) };
        *o = (f64::midpoint(start, end), f64::from(t.speeds[i]));
    }
    out
}

/// Live marker: current temperature, the speed of the ascending step and its index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    /// °C.
    pub temp: f64,
    /// % of the ascending step.
    pub speed: f64,
    /// Step index.
    pub step: usize,
}

/// Marker for `temp` on `t`.
#[must_use]
pub fn marker(t: &Table, temp: f64) -> Marker {
    let step = t.ascending_step(temp);
    Marker {
        temp,
        speed: f64::from(t.speeds[step]),
        step,
    }
}

/// Grid temperatures (every 10 °C).
#[must_use]
pub fn temp_ticks() -> Vec<f64> {
    (2..=10).map(|d| f64::from(d) * 10.0).collect()
}

/// Grid speeds.
pub const PCT_TICKS: [f64; 5] = [0.0, 25.0, 50.0, 75.0, 100.0];

/// Start angle of an arc gauge (radians, cairo convention: 0 = 3 o'clock, clockwise).
pub const GAUGE_START: f64 = 0.75 * PI;
/// Sweep of an arc gauge (270°).
pub const GAUGE_SWEEP: f64 = 1.5 * PI;

/// Fraction (0..=1) of `value` between `min` and `max`; 0 for NaN or an empty range.
#[must_use]
pub fn gauge_fraction(value: f64, min: f64, max: f64) -> f64 {
    if max <= min || value.is_nan() {
        return 0.0;
    }
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

/// End angle of the value arc for `fraction`.
#[must_use]
pub fn gauge_end(fraction: f64) -> f64 {
    GAUGE_START + GAUGE_SWEEP * fraction.clamp(0.0, 1.0)
}

/// Deterministic RGBA noise (`size × size`, straight alpha) for the frosted-glass grain: grey pixels with a
/// low random alpha. xorshift32, so the texture is the same at every start.
#[must_use]
pub fn noise_rgba(size: usize, seed: u32) -> Vec<u8> {
    let mut state = seed.max(1);
    let mut out = Vec::with_capacity(size * size * 4);
    for _ in 0..size * size {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let [a, b, ..] = state.to_le_bytes();
        let v = if a & 1 == 0 { 255 } else { 0 };
        out.extend_from_slice(&[v, v, v, b % 48]);
    }
    out
}

/// Silhouette of a suspension bridge (towers, deck, main cable, hangers) for the backdrop, in pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Bridge {
    /// Tower rectangles `(x, y, w, h)`.
    pub towers: Vec<(f64, f64, f64, f64)>,
    /// Deck line `(x0, y, x1)`.
    pub deck: (f64, f64, f64),
    /// Main cable polyline.
    pub cable: Vec<(f64, f64)>,
    /// Vertical hangers `(x, y_top, y_bottom)`.
    pub hangers: Vec<(f64, f64, f64)>,
}

/// Bridge across a `w × h` area: deck at 84 % of the height, towers at 26 % and 74 % of the width.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sample counts from positive pixel sizes"
)]
pub fn bridge(w: f64, h: f64) -> Bridge {
    let deck_y = h * 0.84;
    let top_y = h * 0.50;
    let tower_w = (w * 0.012).max(4.0);
    let left = w * 0.26;
    let right = w * 0.74;
    let towers = vec![
        (left - tower_w / 2.0, top_y, tower_w, deck_y - top_y + h * 0.06),
        (right - tower_w / 2.0, top_y, tower_w, deck_y - top_y + h * 0.06),
    ];
    let sag_y = deck_y - h * 0.035;
    let cable_y = |x: f64| -> f64 {
        if x < left {
            // Side span: from the anchorage on the deck up to the tower top.
            let u = x / left;
            deck_y - (deck_y - top_y) * u * u
        } else if x > right {
            let u = (w - x) / (w - right);
            deck_y - (deck_y - top_y) * u * u
        } else {
            // Main span: parabola through both tower tops, lowest at mid-span.
            let mid = f64::midpoint(left, right);
            let half = (right - left) / 2.0;
            let u = (x - mid) / half;
            sag_y - (sag_y - top_y) * u * u
        }
    };
    let samples = (w / 6.0).max(8.0) as usize;
    let cable = (0..=samples)
        .map(|i| {
            #[allow(clippy::cast_precision_loss, reason = "small sample index")]
            let x = w * i as f64 / samples as f64;
            (x, cable_y(x))
        })
        .collect();
    let spacing = (w / 48.0).max(8.0);
    let mut hangers = Vec::new();
    let mut x = spacing;
    while x < w {
        let y = cable_y(x);
        if deck_y - y > 2.0 {
            hangers.push((x, y, deck_y));
        }
        x += spacing;
    }
    Bridge {
        towers,
        deck: (0.0, deck_y, w),
        cable,
        hangers,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn silent() -> Table {
        Table {
            speeds: [0, 30, 33, 43, 57, 76, 100],
            up: [50, 57, 64, 70, 76, 82],
            down: [46, 53, 60, 66, 72, 78],
        }
    }

    #[test]
    fn frame_maps_the_chart_range() {
        let f = Frame::new(440.0, 246.0, Padding::CHART).unwrap();
        assert!((f.x(T_MIN) - 44.0).abs() < 1e-9);
        assert!((f.x(T_MAX) - (440.0 - 16.0)).abs() < 1e-9);
        assert!((f.x(5.0) - f.x(T_MIN)).abs() < 1e-9, "clamped");
        assert!((f.y(100.0) - 16.0).abs() < 1e-9);
        assert!((f.y(0.0) - (246.0 - 30.0)).abs() < 1e-9);
        assert!(f.y(50.0) > f.y(60.0), "higher speed is higher up");
        assert_eq!(Frame::new(50.0, 50.0, Padding::CHART), None);
    }

    #[test]
    fn rising_path_is_a_staircase() {
        let p = rising_path(&silent());
        assert_eq!(p.len(), 14);
        assert_eq!(p[0], (20.0, 0.0));
        assert_eq!(p[1], (50.0, 0.0));
        assert_eq!(p[2], (50.0, 30.0));
        assert_eq!(*p.last().unwrap(), (100.0, 100.0));
        for w in p.windows(2) {
            assert!(w[1].0 >= w[0].0, "temperatures never go back");
            assert!(w[1].1 >= w[0].1, "speeds never decrease");
            assert!(
                (w[1].0 - w[0].0).abs() < 1e-9 || (w[1].1 - w[0].1).abs() < 1e-9,
                "only horizontal or vertical segments"
            );
        }
        let f = falling_path(&silent());
        assert_eq!(f[1], (46.0, 0.0));
    }

    #[test]
    fn floor_path_matches_s2() {
        assert_eq!(
            floor_path(),
            vec![
                (60.0, 30.0),
                (75.0, 30.0),
                (75.0, 50.0),
                (85.0, 50.0),
                (85.0, 100.0),
                (100.0, 100.0)
            ]
        );
    }

    #[test]
    fn marker_uses_the_ascending_step() {
        let t = silent();
        assert_eq!(marker(&t, 40.0).step, 0);
        let m = marker(&t, 65.0);
        assert_eq!((m.step, m.speed), (3, 43.0));
        assert_eq!(step_centres(&t)[0], (35.0, 0.0));
        assert_eq!(step_centres(&t)[6], (91.0, 100.0));
        assert_eq!(temp_ticks().first(), Some(&20.0));
        assert_eq!(temp_ticks().last(), Some(&100.0));
    }

    #[test]
    fn gauges() {
        assert!((gauge_fraction(50.0, 0.0, 100.0) - 0.5).abs() < 1e-9);
        assert!((gauge_fraction(150.0, 0.0, 100.0) - 1.0).abs() < 1e-9);
        assert!(gauge_fraction(f64::NAN, 0.0, 100.0).abs() < 1e-9);
        assert!(gauge_fraction(5.0, 10.0, 10.0).abs() < 1e-9);
        assert!((gauge_end(0.0) - GAUGE_START).abs() < 1e-9);
        assert!((gauge_end(1.0) - (GAUGE_START + GAUGE_SWEEP)).abs() < 1e-9);
    }

    #[test]
    fn noise_is_deterministic_and_faint() {
        let a = noise_rgba(16, 7);
        assert_eq!(a.len(), 16 * 16 * 4);
        assert_eq!(a, noise_rgba(16, 7));
        assert!(a.chunks(4).all(|p| p[3] < 48));
        assert_ne!(a, noise_rgba(16, 8));
    }

    #[test]
    fn bridge_is_symmetric_and_hangs_above_the_deck() {
        let b = bridge(1200.0, 800.0);
        assert_eq!(b.towers.len(), 2);
        let (l, r) = (b.towers[0], b.towers[1]);
        assert!(((l.0 + l.2 / 2.0) + (r.0 + r.2 / 2.0) - 1200.0).abs() < 1e-6);
        let deck = b.deck.1;
        assert!(b.cable.iter().all(|&(_, y)| y <= deck + 1e-9));
        let mid = b
            .cable
            .iter()
            .filter(|(x, _)| (*x - 600.0).abs() < 7.0)
            .map(|p| p.1)
            .next()
            .unwrap();
        assert!(mid < deck && mid > l.1, "sags between tower top and deck");
        assert!(b.hangers.len() > 10);
        assert!(b.hangers.iter().all(|&(_, top, bottom)| top < bottom));
    }
}
