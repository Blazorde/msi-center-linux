//! CPU load from `/proc/stat` deltas and RAM usage from `/proc/meminfo`.

use super::percent;

/// Aggregate CPU jiffies from the `cpu` line of `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    /// Non-idle jiffies (user, nice, system, irq, softirq, steal).
    pub busy: u64,
    /// All jiffies (busy + idle + iowait). Guest time is already included in user/nice.
    pub total: u64,
}

/// Parses the aggregate `cpu ` line.
#[must_use]
pub fn parse_proc_stat(text: &str) -> Option<CpuTimes> {
    let line = text.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let idle = fields.get(3)? + fields.get(4).copied().unwrap_or(0);
    let total: u64 = fields.iter().sum();
    Some(CpuTimes {
        busy: total.saturating_sub(idle),
        total,
    })
}

/// CPU usage between two samples, in percent. `None` if no time elapsed or counters went backwards.
#[must_use]
pub fn cpu_percent(prev: CpuTimes, cur: CpuTimes) -> Option<f64> {
    let total = cur.total.checked_sub(prev.total)?;
    // iowait may decrease on Linux, so busy can move faster than total: percent() clamps.
    percent(cur.busy.saturating_sub(prev.busy), total)
}

/// RAM in use (`MemTotal - MemAvailable`) in percent.
#[must_use]
pub fn ram_percent(meminfo: &str) -> Option<f64> {
    let field = |name: &str| -> Option<u64> {
        let line = meminfo.lines().find(|l| l.split(':').next() == Some(name))?;
        line.split_whitespace().nth(1)?.parse().ok()
    };
    let total = field("MemTotal")?;
    let available = field("MemAvailable")?;
    percent(total.saturating_sub(available), total)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  6847 260 3732 639874 55225 383 262 0 0 0\ncpu0 1 2 3 4 5 6 7 0 0 0\nintr 1\n";

    #[test]
    fn parses_aggregate_line() {
        let t = parse_proc_stat(STAT).unwrap();
        assert_eq!(t.total, 6847 + 260 + 3732 + 639_874 + 55225 + 383 + 262);
        assert_eq!(t.busy, 6847 + 260 + 3732 + 383 + 262);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_proc_stat("intr 1"), None);
        assert_eq!(parse_proc_stat("cpu  a b c d"), None);
        assert_eq!(parse_proc_stat("cpu  1 2"), None);
    }

    #[test]
    fn percent_from_deltas() {
        let a = CpuTimes {
            busy: 100,
            total: 1000,
        };
        let b = CpuTimes {
            busy: 150,
            total: 1200,
        };
        assert!((cpu_percent(a, b).unwrap() - 25.0).abs() < 1e-9);
        assert_eq!(cpu_percent(a, a), None);
        assert_eq!(cpu_percent(b, a), None);
        // busy outpacing total (iowait going backwards) is clamped.
        assert_eq!(
            cpu_percent(
                a,
                CpuTimes {
                    busy: 400,
                    total: 1100
                }
            ),
            Some(100.0)
        );
    }

    #[test]
    fn ram_uses_mem_available() {
        let info = "MemTotal:       32322140 kB\nMemFree:  1 kB\nMemAvailable:   28803132 kB\n";
        let pct = ram_percent(info).unwrap();
        assert!((pct - 10.887).abs() < 0.01, "{pct}");
        assert_eq!(ram_percent("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), None);
        assert_eq!(ram_percent("MemFree: 1 kB\n"), None);
    }
}
