//! French formatting of measures (pure). Missing values render as an em dash, never as 0.

/// Narrow no-break space (U+202F), the French thousands separator and the space before units.
pub const NNBSP: char = '\u{202f}';

/// Placeholder for an unknown value.
pub const UNKNOWN: &str = "—";

/// Groups the digits of `n` by three with a narrow no-break space: `12345` → `12 345`.
#[must_use]
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(NNBSP);
        }
        out.push(c);
    }
    out
}

/// Rounds a finite, non-negative measure to a `u64` (saturating; NaN and negatives give 0).
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the u64 range first"
)]
pub fn round_u64(v: f64) -> u64 {
    if v.is_nan() || v <= 0.0 {
        0
    } else if v >= 1.8e19 {
        u64::MAX
    } else {
        v.round() as u64
    }
}

/// Temperature: `62 °C`.
#[must_use]
pub fn temp(c: Option<f64>) -> String {
    c.filter(|v| v.is_finite())
        .map_or_else(|| UNKNOWN.to_owned(), |v| format!("{v:.0}{NNBSP}°C"))
}

/// Fan speed: `2 070 tr/min`.
#[must_use]
pub fn rpm(r: Option<u32>) -> String {
    r.map_or_else(
        || UNKNOWN.to_owned(),
        |v| format!("{}{NNBSP}tr/min", group_thousands(u64::from(v))),
    )
}

/// Percentage: `42 %`.
#[must_use]
pub fn pct(p: Option<f64>) -> String {
    p.filter(|v| v.is_finite())
        .map_or_else(|| UNKNOWN.to_owned(), |v| format!("{}{NNBSP}%", round_u64(v)))
}

/// Integer percentage: `42 %`.
#[must_use]
pub fn pct_u8(p: u8) -> String {
    format!("{p}{NNBSP}%")
}

/// Power: `12,5 W` (French decimal comma).
#[must_use]
pub fn watts(w: Option<f64>) -> String {
    w.filter(|v| v.is_finite()).map_or_else(
        || UNKNOWN.to_owned(),
        |v| format!("{}{NNBSP}W", format!("{v:.1}").replace('.', ",")),
    )
}

/// Decimal with a comma: `1,8`.
#[must_use]
pub fn decimal(v: f64, digits: usize) -> String {
    format!("{v:.digits$}").replace('.', ",")
}

/// Size in bytes with binary units: `512 o`, `1,5 Kio`, `10,0 Mio`.
#[must_use]
#[allow(clippy::cast_precision_loss, reason = "display only")]
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["Kio", "Mio", "Gio", "Tio"];
    if n < 1024 {
        return format!("{n}{NNBSP}o");
    }
    let mut v = n as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    format!("{}{NNBSP}{}", decimal(v, 1), UNITS[unit])
}

/// Relative time between two Unix timestamps in milliseconds: `à l'instant`, `il y a 5 min`, `il y a 2 h`,
/// `il y a 3 j`. A timestamp in the future is shown as `à l'instant`.
#[must_use]
pub fn ago(then_ms: i64, now_ms: i64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        i64::MIN..60 => "à l'instant".to_owned(),
        60..3600 => format!("il y a {}{NNBSP}min", secs / 60),
        3600..86_400 => format!("il y a {}{NNBSP}h", secs / 3600),
        _ => format!("il y a {}{NNBSP}j", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> String {
        s.replace(NNBSP, " ")
    }

    #[test]
    fn groups_thousands_the_french_way() {
        assert_eq!(plain(&group_thousands(0)), "0");
        assert_eq!(plain(&group_thousands(999)), "999");
        assert_eq!(plain(&group_thousands(2070)), "2 070");
        assert_eq!(plain(&group_thousands(1_234_567)), "1 234 567");
    }

    #[test]
    fn formats_measures_and_unknowns() {
        assert_eq!(plain(&temp(Some(61.6))), "62 °C");
        assert_eq!(temp(None), UNKNOWN);
        assert_eq!(temp(Some(f64::NAN)), UNKNOWN);
        assert_eq!(plain(&rpm(Some(2070))), "2 070 tr/min");
        assert_eq!(rpm(None), UNKNOWN);
        assert_eq!(plain(&pct(Some(42.4))), "42 %");
        assert_eq!(plain(&pct(Some(-3.0))), "0 %");
        assert_eq!(plain(&pct_u8(80)), "80 %");
        assert_eq!(plain(&watts(Some(12.46))), "12,5 W");
        assert_eq!(decimal(1.8, 1), "1,8");
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(plain(&bytes(512)), "512 o");
        assert_eq!(plain(&bytes(1536)), "1,5 Kio");
        assert_eq!(plain(&bytes(10 * 1024 * 1024)), "10,0 Mio");
    }

    #[test]
    fn formats_relative_times() {
        let now = 1_000_000_000;
        assert_eq!(ago(now - 5_000, now), "à l'instant");
        assert_eq!(ago(now + 5_000, now), "à l'instant");
        assert_eq!(plain(&ago(now - 5 * 60_000, now)), "il y a 5 min");
        assert_eq!(plain(&ago(now - 2 * 3_600_000, now)), "il y a 2 h");
        assert_eq!(plain(&ago(now - 3 * 86_400_000, now)), "il y a 3 j");
    }

    #[test]
    fn rounding_saturates() {
        assert_eq!(round_u64(f64::NAN), 0);
        assert_eq!(round_u64(2.5), 3);
        assert_eq!(round_u64(1e30), u64::MAX);
    }
}
