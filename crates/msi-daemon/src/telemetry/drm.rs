//! iGPU busy % from DRM fdinfo (`/proc/<pid>/fdinfo/<fd>`, see the kernel's `drm-usage-stats.rst`).
//!
//! i915 reports `drm-engine-<name>: <ns> ns` (+ `drm-engine-capacity-<name>`); xe reports
//! `drm-cycles-<name>` / `drm-total-cycles-<name>`. Both are handled. Scanning every fd of every process is the
//! expensive part, so the list of DRM fdinfo files is cached and refreshed every `rescan_every` samples.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use super::{Clock, percent_f};
use crate::fs::FsRoot;
use crate::gpu::{INTEL_VENDOR, find_display_function};

/// Cumulative usage counter of one engine for one client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineCounter {
    /// i915 style: busy nanoseconds, over `capacity` engine instances.
    Time {
        /// Busy time.
        ns: u64,
        /// Number of engines of this class (`drm-engine-capacity-*`, default 1).
        capacity: u64,
    },
    /// xe style: busy GPU cycles and total GPU cycles elapsed.
    Cycles {
        /// Busy cycles.
        busy: u64,
        /// Total cycles.
        total: u64,
    },
}

/// One DRM client parsed from an fdinfo file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmClient {
    /// PCI address of the device (`drm-pdev`).
    pub pdev: String,
    /// Client id, unique per DRM device (`drm-client-id`). Duplicated fds share it.
    pub client_id: u64,
    /// Counters by engine name.
    pub engines: BTreeMap<String, EngineCounter>,
}

fn leading_u64(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}

/// Parses a DRM fdinfo file. `None` if it is not a DRM fd with usage stats.
#[must_use]
pub fn parse_fdinfo(text: &str) -> Option<DrmClient> {
    let mut pdev = None;
    let mut client_id = None;
    let mut time: BTreeMap<&str, u64> = BTreeMap::new();
    let mut capacity: BTreeMap<&str, u64> = BTreeMap::new();
    let mut cycles: BTreeMap<&str, u64> = BTreeMap::new();
    let mut total_cycles: BTreeMap<&str, u64> = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if key == "drm-pdev" {
            pdev = Some(value.to_owned());
        } else if key == "drm-client-id" {
            client_id = value.parse().ok();
        } else if let Some(engine) = key.strip_prefix("drm-engine-capacity-") {
            capacity.extend(leading_u64(value).map(|v| (engine, v)));
        } else if let Some(engine) = key.strip_prefix("drm-engine-") {
            time.extend(leading_u64(value).map(|v| (engine, v)));
        } else if let Some(engine) = key.strip_prefix("drm-total-cycles-") {
            total_cycles.extend(leading_u64(value).map(|v| (engine, v)));
        } else if let Some(engine) = key.strip_prefix("drm-cycles-") {
            cycles.extend(leading_u64(value).map(|v| (engine, v)));
        }
    }
    let mut engines = BTreeMap::new();
    for (name, ns) in time {
        let capacity = capacity.get(name).copied().unwrap_or(1).max(1);
        engines.insert(name.to_owned(), EngineCounter::Time { ns, capacity });
    }
    for (name, busy) in cycles {
        if let Some(&total) = total_cycles.get(name) {
            engines.insert(name.to_owned(), EngineCounter::Cycles { busy, total });
        }
    }
    Some(DrmClient {
        pdev: pdev?,
        client_id: client_id?,
        engines,
    })
}

/// Usage of all clients of one device at one instant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrmSnapshot {
    /// Monotonic timestamp.
    pub at_ns: u64,
    /// Engine counters by client id.
    pub clients: HashMap<u64, BTreeMap<String, EngineCounter>>,
}

/// Busiest-engine utilisation between two snapshots, in percent. Only clients present in both are counted
/// (a client that just appeared or vanished carries no usable delta). `None` if no time elapsed.
#[must_use]
pub fn busiest_engine_percent(prev: &DrmSnapshot, cur: &DrmSnapshot) -> Option<f64> {
    let dt = cur.at_ns.checked_sub(prev.at_ns).filter(|&dt| dt > 0)?;
    let mut per_engine: BTreeMap<&str, f64> = BTreeMap::new();
    for (id, engines) in &cur.clients {
        let Some(before) = prev.clients.get(id) else {
            continue;
        };
        for (name, now) in engines {
            let share = match (before.get(name), now) {
                (Some(EngineCounter::Time { ns: a, .. }), EngineCounter::Time { ns: b, capacity }) => {
                    percent_f(b.saturating_sub(*a), dt.saturating_mul(*capacity))
                }
                (
                    Some(EngineCounter::Cycles { busy: a, total: ta }),
                    EngineCounter::Cycles { busy: b, total: tb },
                ) => percent_f(b.saturating_sub(*a), tb.saturating_sub(*ta)),
                _ => None,
            };
            if let Some(share) = share {
                *per_engine.entry(name).or_default() += share;
            }
        }
    }
    Some(per_engine.into_values().fold(0.0, f64::max).clamp(0.0, 100.0))
}

/// Collects DRM usage of the Intel iGPU.
#[derive(Debug)]
pub struct IgpuReader {
    root: FsRoot,
    pdev: String,
    fdinfo: Vec<PathBuf>,
    rescan_every: u32,
    until_rescan: u32,
}

impl IgpuReader {
    /// Default number of samples between two full `/proc/*/fd` scans.
    pub const DEFAULT_RESCAN_EVERY: u32 = 10;

    /// Finds the Intel display device; `None` without one.
    #[must_use]
    pub fn discover(root: &FsRoot, rescan_every: u32) -> Option<Self> {
        let igpu = find_display_function(root, INTEL_VENDOR)?;
        Some(Self {
            root: root.clone(),
            pdev: igpu.address,
            fdinfo: Vec::new(),
            rescan_every,
            until_rescan: 0,
        })
    }

    /// PCI address of the iGPU.
    #[must_use]
    pub fn pdev(&self) -> &str {
        &self.pdev
    }

    fn rescan(&mut self) {
        self.fdinfo.clear();
        let Ok(procs) = fs::read_dir(self.root.proc("")) else {
            return;
        };
        for proc in procs.flatten() {
            if !proc
                .file_name()
                .to_str()
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            let Ok(fds) = fs::read_dir(proc.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                let is_drm = fs::read_link(fd.path()).is_ok_and(|t| t.starts_with("/dev/dri/"));
                if is_drm {
                    self.fdinfo.push(proc.path().join("fdinfo").join(fd.file_name()));
                }
            }
        }
    }

    /// Reads every cached DRM fdinfo of the iGPU, refreshing the cache when due.
    pub fn snapshot(&mut self, at: Clock) -> DrmSnapshot {
        if self.until_rescan == 0 {
            self.rescan();
            self.until_rescan = self.rescan_every.max(1);
        }
        self.until_rescan -= 1;
        let mut snapshot = DrmSnapshot {
            at_ns: at.0,
            clients: HashMap::new(),
        };
        for path in &self.fdinfo {
            let Some(client) = fs::read_to_string(path).ok().and_then(|t| parse_fdinfo(&t)) else {
                continue;
            };
            if client.pdev == self.pdev {
                snapshot.clients.entry(client.client_id).or_insert(client.engines);
            }
        }
        snapshot
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::fs::testutil::{symlink, write};

    const I915: &str = "pos:\t0\nflags:\t02104002\ndrm-driver:\ti915\ndrm-client-id:\t42\ndrm-pdev:\t0000:00:02.0\n\
drm-total-system0:\t733928 KiB\ndrm-engine-render:\t13293403968 ns\ndrm-engine-copy:\t72020 ns\n\
drm-engine-video:\t0 ns\ndrm-engine-capacity-video:\t2\ndrm-engine-video-enhance:\t0 ns\ndrm-engine-compute:\t219960 ns\n";

    const XE: &str = "drm-driver:\txe\ndrm-client-id:\t7\ndrm-pdev:\t0000:00:02.0\ndrm-cycles-rcs:\t500\n\
drm-total-cycles-rcs:\t1000\ndrm-cycles-bcs:\t0\ndrm-total-cycles-bcs:\t1000\ndrm-engine-capacity-vcs:\t2\n";

    fn time(ns: u64, capacity: u64) -> EngineCounter {
        EngineCounter::Time { ns, capacity }
    }

    #[test]
    fn parses_i915_fdinfo() {
        let c = parse_fdinfo(I915).unwrap();
        assert_eq!((c.pdev.as_str(), c.client_id), ("0000:00:02.0", 42));
        assert_eq!(c.engines["render"], time(13_293_403_968, 1));
        assert_eq!(c.engines["video"], time(0, 2));
        assert_eq!(c.engines["video-enhance"], time(0, 1));
        assert_eq!(c.engines.len(), 5);
    }

    #[test]
    fn parses_xe_fdinfo() {
        let c = parse_fdinfo(XE).unwrap();
        assert_eq!(
            c.engines["rcs"],
            EngineCounter::Cycles {
                busy: 500,
                total: 1000
            }
        );
        assert_eq!(c.engines.len(), 2);
    }

    #[test]
    fn non_drm_fdinfo_is_rejected() {
        assert_eq!(
            parse_fdinfo("pos:\t0\nflags:\t02\nmnt_id:\t41\nino:\t1346\n"),
            None
        );
    }

    fn snap(at_ns: u64, clients: &[(u64, &[(&str, EngineCounter)])]) -> DrmSnapshot {
        DrmSnapshot {
            at_ns,
            clients: clients
                .iter()
                .map(|(id, e)| (*id, e.iter().map(|(n, c)| ((*n).to_owned(), *c)).collect()))
                .collect(),
        }
    }

    #[test]
    fn busiest_engine_sums_clients_and_respects_capacity() {
        let a = snap(
            0,
            &[
                (1, &[("render", time(0, 1)), ("video", time(0, 2))]),
                (2, &[("render", time(0, 1))]),
            ],
        );
        let b = snap(
            1_000_000_000,
            &[
                (
                    1,
                    &[
                        ("render", time(200_000_000, 1)),
                        ("video", time(1_800_000_000, 2)),
                    ],
                ),
                (2, &[("render", time(100_000_000, 1))]),
                // New client: no previous value, ignored.
                (3, &[("render", time(900_000_000, 1))]),
            ],
        );
        // render: 20 % + 10 % = 30 %; video: 1.8 s over 2 engines = 90 %.
        assert!((busiest_engine_percent(&a, &b).unwrap() - 90.0).abs() < 1e-9);
    }

    #[test]
    fn cycles_ratio_and_degenerate_cases() {
        let busy = |b, t| EngineCounter::Cycles { busy: b, total: t };
        let a = snap(0, &[(7, &[("rcs", busy(0, 0))])]);
        let b = snap(10, &[(7, &[("rcs", busy(250, 1000))])]);
        assert!((busiest_engine_percent(&a, &b).unwrap() - 25.0).abs() < 1e-9);
        assert_eq!(busiest_engine_percent(&a, &a), None);
        assert_eq!(
            busiest_engine_percent(&DrmSnapshot::default(), &snap(5, &[])),
            Some(0.0)
        );
        // Counter reset (client id reused) never yields negative or > 100 %.
        let c = snap(20, &[(7, &[("rcs", busy(0, 2000))])]);
        assert_eq!(busiest_engine_percent(&b, &c), Some(0.0));
    }

    #[test]
    fn reader_filters_igpu_dedups_clients_and_caches_fd_list() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        write(r, "sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n");
        write(r, "sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n");
        // gnome-shell: two dup'd fds of client 42 on the iGPU, one fd on the dGPU (no engine keys).
        symlink(r, "proc/3876/fd/34", "/dev/dri/card1");
        symlink(r, "proc/3876/fd/35", "/dev/dri/card1");
        symlink(r, "proc/3876/fd/19", "/dev/dri/renderD129");
        symlink(r, "proc/3876/fd/1", "/dev/null");
        write(r, "proc/3876/fdinfo/34", I915);
        write(r, "proc/3876/fdinfo/35", I915);
        write(
            r,
            "proc/3876/fdinfo/19",
            "drm-driver:\tnvidia-drm\ndrm-client-id:\t3\ndrm-pdev:\t0000:01:00.0\n",
        );

        let mut reader = IgpuReader::discover(&FsRoot::new(r), 2).unwrap();
        assert_eq!(reader.pdev(), "0000:00:02.0");
        let a = reader.snapshot(Clock(0));
        assert_eq!(a.clients.len(), 1);

        // A new client appears: invisible until the next rescan.
        symlink(r, "proc/6164/fd/9", "/dev/dri/renderD128");
        write(
            r,
            "proc/6164/fdinfo/9",
            &I915.replace("client-id:\t42", "client-id:\t43"),
        );
        let b = reader.snapshot(Clock(1));
        assert_eq!(b.clients.len(), 1);
        let c = reader.snapshot(Clock(2));
        assert_eq!(c.clients.len(), 2);
    }

    #[test]
    fn no_intel_gpu_means_no_reader() {
        let dir = tempfile::tempdir().unwrap();
        assert!(IgpuReader::discover(&FsRoot::new(dir.path()), 1).is_none());
    }
}
