//! Demo mode (`MSI_CENTER_DEMO=1`): realistic fake data for screenshots and demonstrations.
//!
//! Everything in this module is pure and deterministic (a function of a tick counter and the demo state).
//! [`service`] serves it as in-process `org.blazorde.MsiCenter1` / `org.blazorde.Clipd1` objects over private
//! peer-to-peer connections: no bus is ever contacted, nothing touches the hardware.

pub mod service;

use std::collections::HashMap;

use msi_core::fan_profile::FanProfile;
use msi_core::profile::{GpuMode, Profile};
use zbus::{OwnedVariant as OwnedValue, Variant as Value};

use crate::model::ClipWire;

/// Environment variable enabling the demo mode.
pub const ENV: &str = "MSI_CENTER_DEMO";

/// Whether `value` (the content of [`ENV`]) enables the demo mode: `1`, `true`, `yes` or `on`.
#[must_use]
pub fn enabled_by(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Whether the demo mode is enabled in this process.
#[must_use]
pub fn enabled() -> bool {
    enabled_by(std::env::var(ENV).ok().as_deref())
}

/// The fake "renderer" process keeping the RTX 4070 awake after leaving Gaming.
pub const BUSY_HOLDER: (u32, &str) = (4242, "blender");
/// Ticks (seconds) the fake holder keeps the dGPU after switching to Éco.
pub const BUSY_TICKS: u64 = 8;

/// Mutable state of the fake daemon.
#[allow(clippy::struct_excessive_bools, reason = "mirrors the D-Bus properties")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemoState {
    /// Selected profile.
    pub profile: Profile,
    /// Automatic profile on AC.
    pub auto_profile_on_power: bool,
    /// On AC power.
    pub on_ac: bool,
    /// Owner's Cooler Boost.
    pub cooler_boost: bool,
    /// Charge limit %.
    pub charge_limit: u8,
    /// `active`, `suspended` or `removed`.
    pub gpu_state: &'static str,
    /// Tick until which [`BUSY_HOLDER`] holds the dGPU.
    pub busy_until: Option<u64>,
    /// Fan mode (`advanced`, or `auto` under the Auto fan profile and after `RestoreFactory`).
    pub fan_mode: &'static str,
    /// Fan profile chosen per profile when it differs from the default.
    pub fan_profiles: HashMap<Profile, FanProfile>,
}

impl Default for DemoState {
    fn default() -> Self {
        Self {
            profile: Profile::Balanced,
            auto_profile_on_power: true,
            on_ac: true,
            cooler_boost: false,
            charge_limit: 80,
            gpu_state: gpu_state_of(Profile::Balanced),
            busy_until: None,
            fan_mode: "advanced",
            fan_profiles: HashMap::new(),
        }
    }
}

/// EC fan mode under `fp`.
#[must_use]
pub fn fan_mode_of(fp: FanProfile) -> &'static str {
    if fp == FanProfile::Auto {
        "auto"
    } else {
        "advanced"
    }
}

/// dGPU state a profile leads to when nothing holds the card.
#[must_use]
pub const fn gpu_state_of(p: Profile) -> &'static str {
    match p.target().gpu {
        GpuMode::Removed => "removed",
        GpuMode::RuntimeSuspend => "suspended",
        GpuMode::Active => "active",
    }
}

/// What [`DemoState::set_profile`] reports besides the new state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileOutcome {
    /// Applied completely.
    Applied,
    /// Éco applied to the EC, but the dGPU is held: kept in runtime suspend (`gpu_busy` safety event).
    GpuBusy(String),
}

impl DemoState {
    /// Selects `p` at tick `now`. Leaving Gaming for Éco simulates a process still holding the dGPU.
    pub fn set_profile(&mut self, p: Profile, now: u64) -> ProfileOutcome {
        let from_gaming = self.profile == Profile::Gaming;
        self.profile = p;
        self.fan_mode = fan_mode_of(self.fan_profile());
        if p == Profile::Eco && from_gaming {
            self.busy_until = Some(now + BUSY_TICKS);
            self.gpu_state = "suspended";
            return ProfileOutcome::GpuBusy(format!(
                "dGPU held by {} (pid {}): kept in runtime suspend",
                BUSY_HOLDER.1, BUSY_HOLDER.0
            ));
        }
        self.busy_until = None;
        self.gpu_state = gpu_state_of(p);
        ProfileOutcome::Applied
    }

    /// Fan profile of the selected profile.
    #[must_use]
    pub fn fan_profile(&self) -> FanProfile {
        self.fan_profiles
            .get(&self.profile)
            .copied()
            .unwrap_or_else(|| self.profile.default_fan_profile())
    }

    /// Selects `fp` for the selected profile (the default forgets the override).
    pub fn set_fan_profile(&mut self, fp: FanProfile) {
        if fp == self.profile.default_fan_profile() {
            self.fan_profiles.remove(&self.profile);
        } else {
            self.fan_profiles.insert(self.profile, fp);
        }
        self.fan_mode = fan_mode_of(fp);
    }

    /// Advances to tick `now`; `true` if a property changed (the holder went away).
    pub fn tick(&mut self, now: u64) -> bool {
        if self.busy_until.is_some_and(|t| now >= t) {
            self.busy_until = None;
            self.gpu_state = gpu_state_of(self.profile);
            return true;
        }
        false
    }

    /// Processes holding the dGPU.
    #[must_use]
    pub fn holders(&self) -> Vec<(u32, String)> {
        match (self.busy_until, self.gpu_state) {
            (Some(_), _) => vec![(BUSY_HOLDER.0, BUSY_HOLDER.1.to_owned())],
            (None, "active") => vec![(3121, "steam".to_owned()), (3388, "cyberpunk2077.exe".to_owned())],
            _ => Vec::new(),
        }
    }
}

/// A smooth pseudo-random wave in `0.0..=1.0` (sum of two sines, phase per `seed`).
fn wave(tick: u64, seed: f64) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "small tick counts")]
    let t = tick as f64;
    let v = (t * 0.21 + seed).sin() * 0.6 + (t * 0.053 + seed * 2.3).sin() * 0.4;
    f64::midpoint(v, 1.0)
}

/// One telemetry sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// CPU load %.
    pub cpu_pct: f64,
    /// RAM %.
    pub ram_pct: f64,
    /// iGPU %.
    pub igpu_pct: f64,
    /// dGPU % (only while active).
    pub dgpu_pct: Option<f64>,
    /// NPU %.
    pub npu_pct: f64,
    /// CPU °C.
    pub cpu_temp: f64,
    /// GPU °C (only while the dGPU is powered).
    pub gpu_temp: Option<f64>,
    /// Package W.
    pub power_w: f64,
    /// CPU fan rpm.
    pub fan1_rpm: u32,
    /// GPU fan rpm.
    pub fan2_rpm: u32,
    /// CPU fan duty %.
    pub fan1_pct: u8,
    /// GPU fan duty %.
    pub fan2_pct: u8,
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "values are clamped to 0..=100 first"
)]
fn pct_u8(v: f64) -> u8 {
    v.round().clamp(0.0, 100.0) as u8
}

/// The sample at `tick` for `state`: higher load, temperatures and fan speeds in Gaming, calm in Éco.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rpm values are positive and small"
)]
pub fn sample(tick: u64, state: &DemoState) -> Sample {
    let (load, temp, watts) = match state.profile {
        Profile::Eco => (14.0, 49.0, 9.0),
        Profile::Balanced => (27.0, 61.0, 21.0),
        Profile::Gaming => (63.0, 79.0, 58.0),
    };
    let cpu_pct = load + wave(tick, 0.3) * load * 0.5;
    let cpu_temp = temp + wave(tick, 1.1) * 6.0;
    let dgpu_on = state.gpu_state == "active";
    let gpu_temp = (state.gpu_state != "removed").then(|| {
        if dgpu_on {
            temp - 6.0 + wave(tick, 2.0) * 7.0
        } else {
            41.0 + wave(tick, 2.0) * 2.0
        }
    });
    // Duty from a simple ramp, as the EC would: 0 % below 50 °C, 100 % near 90 °C.
    let duty = |c: f64| {
        if c < 50.0 {
            0.0
        } else {
            ((c - 45.0) / 45.0 * 100.0).min(100.0)
        }
    };
    let fan1_pct = if state.cooler_boost {
        100
    } else {
        pct_u8(duty(cpu_temp))
    };
    let fan2_pct = if state.cooler_boost {
        100
    } else {
        pct_u8(gpu_temp.map_or(0.0, duty))
    };
    let rpm = |pct: u8, seed: f64| {
        if pct == 0 {
            0
        } else {
            (f64::from(pct) * 58.0 + 900.0 + wave(tick, seed) * 60.0) as u32
        }
    };
    Sample {
        cpu_pct,
        ram_pct: 41.0 + wave(tick, 0.7) * 4.0,
        igpu_pct: if dgpu_on {
            6.0
        } else {
            9.0 + wave(tick, 3.1) * 14.0
        },
        dgpu_pct: dgpu_on.then(|| 71.0 + wave(tick, 0.9) * 20.0),
        npu_pct: 3.0 + wave(tick, 4.2) * 9.0,
        cpu_temp,
        gpu_temp,
        power_w: watts + wave(tick, 1.7) * watts * 0.35,
        fan1_rpm: rpm(fan1_pct, 0.5),
        fan2_rpm: rpm(fan2_pct, 1.5),
        fan1_pct,
        fan2_pct,
    }
}

fn ov(v: Value<'_>) -> Option<OwnedValue> {
    OwnedValue::try_from(v).ok()
}

/// The `Telemetry` signal payload of a sample (same keys as the daemon; unknown values omitted).
#[must_use]
pub fn telemetry_map(s: &Sample, state: &DemoState, cpu_target: Option<u8>) -> HashMap<String, OwnedValue> {
    let mut m = HashMap::new();
    let mut put = |k: &str, v: Option<Value<'_>>| {
        if let Some(v) = v.and_then(ov) {
            m.insert(k.to_owned(), v);
        }
    };
    put("cpu_pct", Some(Value::F64(s.cpu_pct)));
    put("ram_pct", Some(Value::F64(s.ram_pct)));
    put("igpu_pct", Some(Value::F64(s.igpu_pct)));
    put("dgpu_pct", s.dgpu_pct.map(Value::F64));
    put("npu_pct", Some(Value::F64(s.npu_pct)));
    put("cpu_temp", Some(Value::F64(s.cpu_temp)));
    put("gpu_temp", s.gpu_temp.map(Value::F64));
    put("power_w", Some(Value::F64(s.power_w)));
    put("fan1_rpm", Some(Value::U32(s.fan1_rpm)));
    put("fan2_rpm", Some(Value::U32(s.fan2_rpm)));
    put("fan1_pct", Some(Value::U8(s.fan1_pct)));
    put("fan2_pct", Some(Value::U8(s.fan2_pct)));
    put("dgpu_state", Some(Value::from(state.gpu_state)));
    put("cooler_boost", Some(Value::Bool(state.cooler_boost)));
    put("cpu_fan_target_pct", cpu_target.map(Value::U8));
    m
}

/// A fake clip: `(wire entry, content)`.
pub type DemoClip = (ClipWire, String, Vec<u8>);

/// The demo clipboard history at `now_ms` (most recently used first).
#[must_use]
pub fn clips(now_ms: i64) -> Vec<DemoClip> {
    let min = 60_000;
    let text = |id: u64, ago: i64, pinned: bool, tags: &[&str], s: &str| -> DemoClip {
        let t = now_ms - ago * min;
        (
            (
                id,
                "text/plain".to_owned(),
                s.lines().next().unwrap_or("").to_owned(),
                s.len() as u64,
                t,
                t,
                pinned,
                tags.iter().map(|&t| t.to_owned()).collect(),
            ),
            "text/plain".to_owned(),
            s.as_bytes().to_vec(),
        )
    };
    let png = sunset_png(320, 200);
    let t = now_ms - 7 * min;
    vec![
        text(
            9,
            1,
            false,
            &[],
            "cargo clippy --workspace --all-targets -- -D warnings",
        ),
        text(
            8,
            3,
            true,
            &["rust", "travail"],
            "fn main() -> anyhow::Result<()> {",
        ),
        (
            (
                7,
                "image/png".to_owned(),
                String::new(),
                png.len() as u64,
                t,
                t,
                false,
                Vec::new(),
            ),
            "image/png".to_owned(),
            png,
        ),
        text(6, 12, false, &[], "https://github.com/Blazorde/msi-center-linux"),
        text(
            5,
            25,
            true,
            &["adresse"],
            "Golden Gate Bridge, San Francisco, CA 94129",
        ),
        text(4, 48, false, &[], "Réunion déplacée à 14 h 30, salle Presidio."),
        text(3, 90, true, &["travail"], "ssh blaz@stealth14.local"),
        text(2, 180, false, &[], "#C0362C International Orange"),
    ]
}

/// Tags and their counts in `clips`.
#[must_use]
pub fn tag_counts(clips: &[DemoClip]) -> Vec<(String, u32)> {
    let mut counts: Vec<(String, u32)> = Vec::new();
    for ((.., tags), _, _) in clips {
        for t in tags {
            match counts.iter_mut().find(|(n, _)| n == t) {
                Some((_, c)) => *c += 1,
                None => counts.push((t.clone(), 1)),
            }
        }
    }
    counts.sort();
    counts
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    let start = out.len();
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// A small RGB PNG of a sunset over the bay (vertical gradient, sun disc), encoded without compression
/// (stored deflate blocks): a realistic image clip without an image dependency.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names,
    reason = "pixel arithmetic on small images"
)]
pub fn sunset_png(w: u32, h: u32) -> Vec<u8> {
    let lerp = |a: u8, b: u8, t: f64| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
    let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
    for y in 0..h {
        raw.push(0); // filter: none
        let t = f64::from(y) / f64::from(h.max(2) - 1);
        for x in 0..w {
            let (top, bottom) = if t < 0.62 {
                (([0xf2, 0x8c, 0x38], [0xc0, 0x36, 0x2c]), t / 0.62)
            } else {
                (([0x2b, 0x2d, 0x42], [0x16, 0x17, 0x24]), (t - 0.62) / 0.38)
            };
            let ((a, b), k) = (top, bottom);
            let mut px = [lerp(a[0], b[0], k), lerp(a[1], b[1], k), lerp(a[2], b[2], k)];
            let (dx, dy) = (
                f64::from(x) - f64::from(w) * 0.68,
                f64::from(y) - f64::from(h) * 0.5,
            );
            if (dx * dx + dy * dy).sqrt() < f64::from(h) * 0.16 && t < 0.62 {
                px = [0xff, 0xd9, 0x9a];
            }
            raw.extend_from_slice(&px);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    chunk(&mut out, *b"IHDR", &ihdr);
    chunk(&mut out, *b"IDAT", &z);
    chunk(&mut out, *b"IEND", &[]);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::Telemetry;

    #[test]
    fn env_values() {
        for v in ["1", "true", " YES ", "on"] {
            assert!(enabled_by(Some(v)), "{v}");
        }
        for v in ["", "0", "false", "demo"] {
            assert!(!enabled_by(Some(v)), "{v}");
        }
        assert!(!enabled_by(None));
    }

    #[test]
    fn profiles_drive_the_gpu_and_eco_after_gaming_is_busy() {
        let mut s = DemoState::default();
        assert_eq!(s.gpu_state, "suspended");
        assert_eq!(s.set_profile(Profile::Gaming, 0), ProfileOutcome::Applied);
        assert_eq!(s.gpu_state, "active");
        assert_eq!(s.holders().len(), 2);
        assert!(matches!(
            s.set_profile(Profile::Eco, 10),
            ProfileOutcome::GpuBusy(_)
        ));
        assert_eq!(s.gpu_state, "suspended");
        assert_eq!(s.holders(), vec![(BUSY_HOLDER.0, BUSY_HOLDER.1.to_owned())]);
        assert!(!s.tick(10 + BUSY_TICKS - 1));
        assert!(s.tick(10 + BUSY_TICKS));
        assert_eq!(s.gpu_state, "removed");
        assert_eq!(s.holders(), Vec::new());
        assert_eq!(s.set_profile(Profile::Eco, 30), ProfileOutcome::Applied);
    }

    #[test]
    fn fan_profiles_are_remembered_per_profile() {
        let mut s = DemoState::default();
        assert_eq!(s.fan_profile(), FanProfile::Balanced);
        s.set_fan_profile(FanProfile::Auto);
        assert_eq!(s.fan_mode, "auto");
        s.set_profile(Profile::Gaming, 0);
        assert_eq!((s.fan_profile(), s.fan_mode), (FanProfile::Turbo, "advanced"));
        s.set_profile(Profile::Balanced, 1);
        assert_eq!((s.fan_profile(), s.fan_mode), (FanProfile::Auto, "auto"));
        s.set_fan_profile(FanProfile::Balanced);
        assert!(s.fan_profiles.is_empty());
    }

    #[test]
    fn samples_are_plausible_and_follow_the_profile() {
        let mut s = DemoState::default();
        for tick in 0..120 {
            for p in Profile::ALL {
                s.set_profile(p, tick);
                s.tick(tick + 100);
                let x = sample(tick, &s);
                assert!((0.0..=100.0).contains(&x.cpu_pct));
                assert!((35.0..=95.0).contains(&x.cpu_temp), "{}", x.cpu_temp);
                assert!(x.fan1_pct <= 100 && x.fan2_pct <= 100);
                assert_eq!(x.fan1_rpm == 0, x.fan1_pct == 0);
                assert_eq!(x.dgpu_pct.is_some(), p == Profile::Gaming);
                assert_eq!(x.gpu_temp.is_none(), p == Profile::Eco);
            }
        }
        s.set_profile(Profile::Eco, 0);
        let eco = sample(5, &s);
        s.set_profile(Profile::Gaming, 0);
        let gaming = sample(5, &s);
        assert!(gaming.cpu_temp > eco.cpu_temp + 15.0);
        assert!(gaming.power_w > eco.power_w);
    }

    #[test]
    fn telemetry_decodes_like_the_daemon_payload() {
        let s = DemoState::default();
        let x = sample(3, &s);
        let t = Telemetry::from_map(&telemetry_map(&x, &s, Some(42)));
        assert_eq!(t.cpu_temp, Some(x.cpu_temp));
        assert_eq!(t.fan1_rpm, Some(x.fan1_rpm));
        assert_eq!(t.dgpu_pct, None);
        assert_eq!(t.dgpu_state.as_deref(), Some("suspended"));
        assert_eq!(t.cpu_fan_target, Some(42));
    }

    #[test]
    fn clips_and_tags() {
        let c = clips(1_000_000_000);
        assert!(c.len() >= 6);
        let ids: Vec<u64> = c.iter().map(|((id, ..), _, _)| *id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(ids, sorted, "most recent first");
        assert!(c.iter().any(|((_, mime, ..), _, _)| mime == "image/png"));
        let tags = tag_counts(&c);
        assert_eq!(
            tags,
            vec![("adresse".into(), 1), ("rust".into(), 1), ("travail".into(), 2)]
        );
    }

    #[test]
    fn png_is_well_formed() {
        let p = sunset_png(16, 8);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&p[12..16], b"IHDR");
        assert_eq!(&p[p.len() - 8..p.len() - 4], b"IEND");
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        // Raw size: (16 * 3 + 1) * 8 bytes in one stored block.
        assert!(p.len() > 49 * 8);
    }
}
