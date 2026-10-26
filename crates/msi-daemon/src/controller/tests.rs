use std::path::Path;
use std::sync::{Arc, Mutex};

use msi_core::curve::CurvePreset;
use msi_core::table::TableWrite;
use tempfile::TempDir;

use super::*;
use crate::ec::{FACTORY_CPU, FACTORY_GPU, MemoryEc, MemoryState};
use crate::fs::FsRoot;
use crate::fs::testutil::{read, symlink, write};
use crate::telemetry::hwmon::Thermals;

struct Rig {
    dir: TempDir,
    ec: MemoryEc,
    ctl: Controller,
}

fn base_tree(r: &Path) {
    write(r, "sys/bus/pci/rescan", "");
    write(r, "sys/class/power_supply/ADP1/online", "1\n");
    write(
        r,
        "sys/class/power_supply/BAT1/charge_control_end_threshold",
        "100\n",
    );
    std::fs::create_dir_all(r.join("proc")).unwrap();
}

/// Minimal NVIDIA dGPU at 0000:01:00.0.
fn add_dgpu(r: &Path) {
    let gpu = "sys/bus/pci/devices/0000:01:00.0";
    write(r, &format!("{gpu}/vendor"), "0x10de\n");
    write(r, &format!("{gpu}/class"), "0x030000\n");
    write(r, &format!("{gpu}/power/control"), "on\n");
    write(r, &format!("{gpu}/power/runtime_status"), "active\n");
    write(r, &format!("{gpu}/remove"), "");
}

fn rig_with(state: MemoryState, prepare: impl FnOnce(&Path)) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    base_tree(dir.path());
    prepare(dir.path());
    let ec = MemoryEc::new(state);
    let root = FsRoot::new(dir.path());
    let (ctl, warnings) = Controller::new(
        Box::new(ec.clone()),
        PowerSupply::new(root.clone()),
        GpuManager::new(root),
        None,
        StateStore::new(dir.path().join("state/state.toml")),
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    Rig { dir, ec, ctl }
}

/// Telemetry that only reports fan rpm (S9 tests).
#[derive(Debug)]
struct RpmTelemetry(Arc<Mutex<[Option<u32>; 2]>>);

impl Telemetry for RpmTelemetry {
    fn sample(&mut self, _now: Clock) -> Sample {
        let [fan1, fan2] = *self.0.lock().unwrap();
        Sample {
            cpu_pct: None,
            ram_pct: None,
            igpu_pct: None,
            dgpu_pct: None,
            npu_pct: None,
            thermals: Thermals {
                fan1_rpm: fan1,
                fan2_rpm: fan2,
                ..Thermals::default()
            },
            dgpu_power_w: None,
            dgpu_state: GpuState::Removed,
        }
    }
    fn set_dgpu_policy(&mut self, _policy: DgpuPolicy) {}
    fn release_dgpu(&mut self) {}
}

fn rig() -> Rig {
    rig_with(MemoryState::default(), |_| {})
}

fn expected(preset: CurvePreset) -> msi_core::table::RawTables {
    generate(&preset.spec()).unwrap().curve.tables()
}

fn kinds(events: &[SafetyEvent]) -> Vec<EventKind> {
    events.iter().map(|e| e.kind).collect()
}

#[test]
fn s8_unknown_firmware_is_read_only_and_writes_nothing() {
    let mut r = rig_with(
        MemoryState {
            fw_version: "14K2EMS1.999".into(),
            ..MemoryState::default()
        },
        |_| {},
    );
    let reason = r.ctl.status().read_only.unwrap();
    assert!(reason.contains("14K2EMS1.999"), "{reason}");
    assert!(matches!(r.ctl.start(), Err(ControlError::ReadOnly(_))));
    assert!(matches!(
        r.ctl.set_profile(Profile::Gaming),
        Err(ControlError::ReadOnly(_))
    ));
    assert!(matches!(
        r.ctl.set_cooler_boost(true),
        Err(ControlError::ReadOnly(_))
    ));
    assert!(matches!(
        r.ctl.set_charge_limit(80),
        Err(ControlError::ReadOnly(_))
    ));
    for _ in 0..3 {
        r.ctl.tick(Clock(0));
    }
    r.ctl.shutdown();
    let s = r.ec.state();
    assert_eq!(s.writes.len(), 0);
    assert_eq!(s.fan_mode_writes.len(), 0);
    assert!(!s.cooler_boost);
    assert_eq!(
        read(
            r.dir.path(),
            "sys/class/power_supply/BAT1/charge_control_end_threshold"
        ),
        "100\n"
    );
}

#[test]
fn s8_missing_curve_files_are_read_only() {
    let r = rig_with(
        MemoryState {
            curve_supported: false,
            ..MemoryState::default()
        },
        |_| {},
    );
    assert!(r.ctl.status().read_only.unwrap().contains("fan curve attributes"));
}

#[test]
fn apply_writes_curves_then_advanced_then_shift_mode() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    let s = r.ec.state();
    assert_eq!(s.cpu, expected(CurvePreset::Progressive));
    assert_eq!(s.gpu, expected(CurvePreset::Progressive));
    assert_eq!(s.fan_mode, FanMode::Advanced);
    assert_eq!(s.fan_mode_writes, [FanMode::Advanced]);
    assert_eq!(s.shift_mode, ShiftMode::Comfort);
    // Factory → Progressive lowers the thresholds: the down table is written before the up table.
    let cpu: Vec<&TableWrite> = s
        .writes
        .iter()
        .filter(|(f, _)| *f == Fan::Cpu)
        .map(|(_, w)| w)
        .collect();
    assert!(matches!(cpu[1], TableWrite::Down(_)), "{cpu:?}");
    assert!(matches!(cpu[2], TableWrite::Up(_)), "{cpu:?}");
    assert!(r.ctl.is_applied());
}

#[test]
fn every_profile_applies_through_the_kernel_rules_from_any_other() {
    let mut r = rig();
    for from in Profile::ALL {
        for to in Profile::ALL {
            r.ctl.set_profile(from).unwrap_or_else(|e| match e {
                ControlError::Gpu(_) => {}
                other => panic!("{from} : {other}"),
            });
            let _ = r.ctl.set_profile(to);
            assert_eq!(r.ec.state().cpu, expected(to.target().curve), "{from} → {to}");
        }
    }
}

#[test]
fn reapply_is_idempotent() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    let writes = r.ec.state().writes.len();
    r.ctl.reapply().unwrap();
    assert_eq!(r.ec.state().writes.len(), writes);
}

#[test]
fn s7_readback_mismatch_falls_back_to_auto() {
    let mut r = rig();
    r.ec.state().corrupt_readback = Some(Fan::Gpu);
    let err = r.ctl.set_profile(Profile::Balanced).unwrap_err();
    assert!(matches!(err, ControlError::SafetyFallback(_)), "{err}");
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::ReadbackMismatch]);
    assert!(!r.ctl.is_applied());
}

#[test]
fn s7_write_failure_falls_back_to_auto() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    r.ec.state().fail_table_writes = true;
    assert!(matches!(
        r.ctl.set_profile(Profile::Gaming),
        Err(ControlError::SafetyFallback(_))
    ));
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::WriteFailed]);
}

#[test]
fn s7_shift_mode_drift_is_detected() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    r.ec.state().shift_mode = ShiftMode::Turbo;
    for _ in 0..DRIFT_CHECK_EVERY {
        r.ctl.tick(Clock(0));
    }
    // Re-applied (EC reset) and verified.
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::EcReset]);
    assert_eq!(r.ec.state().shift_mode, ShiftMode::Comfort);
}

#[test]
fn ec_reset_is_detected_and_reapplied() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    {
        let mut s = r.ec.state();
        s.cpu = FACTORY_CPU;
        s.fan_mode = FanMode::Auto;
    }
    for _ in 0..DRIFT_CHECK_EVERY {
        r.ctl.tick(Clock(0));
    }
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::EcReset]);
    let s = r.ec.state();
    assert_eq!(s.cpu, expected(CurvePreset::Silent));
    assert_eq!(s.fan_mode, FanMode::Advanced);
}

#[test]
fn eco_with_a_held_dgpu_succeeds_in_runtime_suspend_and_never_kills() {
    let mut r = rig_with(MemoryState::default(), |p| {
        add_dgpu(p);
        write(p, "proc/3876/comm", "gnome-shell\n");
        symlink(p, "proc/3876/fd/14", "/dev/nvidia0");
    });
    r.ctl.set_profile(Profile::Eco).unwrap();
    let events = r.ctl.take_events();
    assert_eq!(kinds(&events), [EventKind::GpuBusy]);
    assert!(
        events[0].detail.contains("gnome-shell (3876)"),
        "{}",
        events[0].detail
    );
    assert!(events[0].value.abs() < f64::EPSILON);
    // The EC part is applied, the dGPU is not removed but runtime-suspended, the process is still there.
    assert_eq!(r.ec.state().fan_mode, FanMode::Advanced);
    assert_eq!(r.ec.state().shift_mode, ShiftMode::Eco);
    assert_eq!(read(r.dir.path(), "sys/bus/pci/devices/0000:01:00.0/remove"), "");
    assert_eq!(
        read(r.dir.path(), "sys/bus/pci/devices/0000:01:00.0/power/control"),
        "auto"
    );
    assert!(r.dir.path().join("proc/3876/comm").exists());
    assert_eq!(r.ctl.settings().profile, Profile::Eco);
    assert_eq!(r.ctl.gpu_holders()[0].pid, 3876);
}

#[test]
fn eco_removes_a_free_dgpu_and_balanced_enables_runtime_pm() {
    let mut r = rig_with(MemoryState::default(), add_dgpu);
    r.ctl.set_profile(Profile::Eco).unwrap();
    assert_eq!(read(r.dir.path(), "sys/bus/pci/devices/0000:01:00.0/remove"), "1");
    r.ctl.set_profile(Profile::Balanced).unwrap();
    assert_eq!(
        read(r.dir.path(), "sys/bus/pci/devices/0000:01:00.0/power/control"),
        "auto"
    );
    r.ctl.set_profile(Profile::Gaming).unwrap();
    assert_eq!(
        read(r.dir.path(), "sys/bus/pci/devices/0000:01:00.0/power/control"),
        "on"
    );
}

#[test]
fn balanced_rescans_when_the_dgpu_is_gone() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    assert_eq!(read(r.dir.path(), "sys/bus/pci/rescan"), "1");
}

fn set_temps(ec: &MemoryEc, cpu: Option<u8>, gpu: Option<u8>) {
    let mut s = ec.state();
    s.cpu_temp = cpu;
    s.gpu_temp = gpu;
}

#[test]
fn s4_forces_boost_after_two_hot_readings_and_restores_the_owner_choice() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    set_temps(&r.ec, Some(96), Some(60));
    r.ctl.tick(Clock(0));
    assert!(!r.ec.state().cooler_boost, "a single reading must not force");
    r.ctl.tick(Clock(1));
    assert!(r.ec.state().cooler_boost);
    assert!(r.ctl.status().boost_forced);
    let events = r.ctl.take_events();
    assert_eq!(kinds(&events), [EventKind::BoostForced]);
    assert!((events[0].value - 96.0).abs() < f64::EPSILON);

    // Turning it off is refused while forced; someone clearing it is re-forced.
    assert!(matches!(
        r.ctl.set_cooler_boost(false),
        Err(ControlError::InvalidArgument(_))
    ));
    r.ec.state().cooler_boost = false;
    r.ctl.tick(Clock(2));
    assert!(r.ec.state().cooler_boost);

    set_temps(&r.ec, Some(80), Some(60));
    r.ctl.tick(Clock(3));
    r.ctl.tick(Clock(4));
    assert!(!r.ec.state().cooler_boost);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::BoostReleased]);
}

#[test]
fn s4_ignores_isolated_spikes() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    for (i, t) in [55, 100, 55, 98, 56].into_iter().enumerate() {
        set_temps(&r.ec, Some(t), Some(50));
        r.ctl.tick(Clock(i as u64));
    }
    assert!(!r.ec.state().cooler_boost);
    assert_eq!(r.ctl.take_events().len(), 0);
}

#[test]
fn s4_unreadable_cpu_sensor_forces_boost_immediately() {
    let mut r = rig();
    set_temps(&r.ec, None, Some(50));
    r.ctl.tick(Clock(0));
    assert!(r.ec.state().cooler_boost);
    assert_eq!(r.ctl.take_events()[0].detail, "CPU temperature unreadable");
}

#[test]
fn s4_gpu_reading_zero_is_absent_not_failed() {
    let mut r = rig();
    set_temps(&r.ec, Some(50), Some(0));
    for t in 0..3 {
        r.ctl.tick(Clock(t));
    }
    assert!(!r.ec.state().cooler_boost);
}

#[test]
fn s5_shutdown_restores_factory_tables_then_auto() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ec.state().writes.clear();
    r.ctl.shutdown();
    let s = r.ec.state();
    assert_eq!((s.cpu, s.gpu), (FACTORY_CPU, FACTORY_GPU));
    assert_eq!(s.fan_mode, FanMode::Auto);
    assert_eq!(s.fan_mode_writes.last(), Some(&FanMode::Auto));
    assert!(!s.writes.is_empty());
}

#[test]
fn s5_shutdown_with_unknown_factory_tables_only_restores_auto() {
    let mut r = rig_with(
        MemoryState {
            fw_version: "14K2EMS1.109".into(),
            ..MemoryState::default()
        },
        |_| {},
    );
    r.ctl.set_profile(Profile::Gaming).unwrap();
    let applied = r.ec.state().cpu;
    r.ctl.shutdown();
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    assert_eq!(r.ec.state().cpu, applied);
}

#[test]
fn s7_fallback_restores_the_factory_tables() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    r.ec.state().shift_mode = ShiftMode::Turbo;
    r.ec.state().corrupt_readback = Some(Fan::Gpu);
    assert!(matches!(
        r.ctl.set_profile(Profile::Gaming),
        Err(ControlError::SafetyFallback(_))
    ));
    let s = r.ec.state();
    assert_eq!(s.cpu, FACTORY_CPU);
    assert_eq!(s.fan_mode, FanMode::Auto);
}

#[test]
fn restore_factory_hands_control_back_until_a_profile_is_applied() {
    let mut r = rig();
    r.ctl.start().unwrap();
    r.ctl.restore_factory().unwrap();
    assert!(r.ctl.is_handed_back());
    assert!(!r.ctl.is_applied());
    {
        let s = r.ec.state();
        assert_eq!((s.cpu, s.gpu), (FACTORY_CPU, FACTORY_GPU));
    }
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    // Resume, drift check and AC switch leave the firmware in control.
    r.ctl.reapply().unwrap();
    r.ctl.set_auto_profile_on_power(false).unwrap();
    for t in 0..DRIFT_CHECK_EVERY {
        r.ctl.tick(Clock(t));
    }
    write(r.dir.path(), "sys/class/power_supply/ADP1/online", "0\n");
    r.ctl.tick(Clock(99));
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    assert_eq!(r.ctl.take_events().len(), 0);
    // Idempotent.
    let writes = r.ec.state().writes.len();
    r.ctl.restore_factory().unwrap();
    assert_eq!(r.ec.state().writes.len(), writes);
    // Taking control again.
    r.ctl.set_profile(Profile::Eco).unwrap();
    assert!(!r.ctl.is_handed_back());
    assert_eq!(r.ec.state().fan_mode, FanMode::Advanced);
    assert_eq!(r.ec.state().cpu, expected(CurvePreset::Silent));
}

#[test]
fn restore_factory_is_refused_under_s8() {
    let mut r = rig_with(
        MemoryState {
            fw_version: "14K2EMS1.999".into(),
            ..MemoryState::default()
        },
        |_| {},
    );
    assert!(matches!(r.ctl.restore_factory(), Err(ControlError::ReadOnly(_))));
    assert_eq!(r.ec.state().fan_mode_writes.len(), 0);
}

const SEC: u64 = 1_000_000_000;

/// Ticks once per second from `from` (s) for `n` seconds.
fn tick_secs(ctl: &mut Controller, from: u64, n: u64) {
    for t in from..from + n {
        ctl.tick(Clock(t * SEC));
    }
}

#[test]
fn s9_cpu_fan_stopped_at_70_for_30_s_restores_factory_and_locks() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    {
        let mut s = r.ec.state();
        s.cpu_temp = Some(74);
        s.fan_speed_override = [Some(0), None];
    }
    // Two readings fill the filter (t = 0, 1): the stall starts at 1 s and trips at 31 s.
    tick_secs(&mut r.ctl, 0, 31);
    assert!(r.ctl.is_applied(), "not before 30 s");
    assert_eq!(r.ctl.take_events().len(), 0);
    tick_secs(&mut r.ctl, 31, 1);
    let events = r.ctl.take_events();
    assert_eq!(kinds(&events), [EventKind::FanStalled]);
    assert!((events[0].value - 74.0).abs() < f64::EPSILON);
    assert!(
        events[0].detail.starts_with("cpu fan at 0 %"),
        "{}",
        events[0].detail
    );
    {
        let s = r.ec.state();
        assert_eq!((s.cpu, s.gpu), (FACTORY_CPU, FACTORY_GPU));
        assert_eq!(s.fan_mode, FanMode::Auto);
    }
    let reason = r.ctl.status().read_only.unwrap();
    assert!(reason.starts_with("S9 fan stall"), "{reason}");
    // Read-only until restart: nothing re-applies, nothing writes tables.
    let writes = r.ec.state().writes.len();
    assert!(matches!(
        r.ctl.set_profile(Profile::Gaming),
        Err(ControlError::ReadOnly(_))
    ));
    tick_secs(&mut r.ctl, 32, DRIFT_CHECK_EVERY * 2);
    r.ctl.reapply().unwrap_err();
    assert_eq!(r.ec.state().writes.len(), writes);
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    // S4 still protects.
    r.ec.state().cpu_temp = Some(97);
    tick_secs(&mut r.ctl, 100, 2);
    assert!(r.ec.state().cooler_boost);
}

#[test]
fn s9_needs_0_rpm_when_rpm_is_known() {
    let rpm = Arc::new(Mutex::new([Some(2100), Some(2000)]));
    let dir = tempfile::tempdir().unwrap();
    base_tree(dir.path());
    let ec = MemoryEc::default();
    let root = FsRoot::new(dir.path());
    let (mut ctl, _) = Controller::new(
        Box::new(ec.clone()),
        PowerSupply::new(root.clone()),
        GpuManager::new(root),
        Some(Box::new(RpmTelemetry(Arc::clone(&rpm)))),
        StateStore::new(dir.path().join("state.toml")),
    );
    ctl.set_profile(Profile::Balanced).unwrap();
    {
        let mut s = ec.state();
        s.gpu_temp = Some(80);
        s.fan_speed_override = [None, Some(0)];
    }
    // The EC reports 0 % but the GPU fan still turns: not a stall.
    tick_secs(&mut ctl, 0, 40);
    assert_eq!(ctl.take_events().len(), 0, "still spinning");
    rpm.lock().unwrap()[1] = Some(0);
    tick_secs(&mut ctl, 40, 31);
    let events = ctl.take_events();
    assert_eq!(kinds(&events), [EventKind::FanStalled]);
    assert!(events[0].detail.contains("gpu fan") && events[0].detail.contains("0 rpm"));
}

#[test]
fn s9_ignores_spikes_and_the_factory_like_step_0() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ec.state().fan_speed_override = [Some(0), Some(0)];
    // First real run: CPU at 57–68 °C with the fan at 0 %, plus 95 °C spikes every 4 s.
    for t in 0..120 {
        let temp = if t % 4 == 0 {
            95
        } else {
            57 + u8::try_from(t % 12).unwrap()
        };
        set_temps(&r.ec, Some(temp), Some(40));
        r.ctl.tick(Clock(t * SEC));
    }
    assert!(r.ctl.is_applied());
    assert_eq!(kinds(&r.ctl.take_events()), []);
}

#[test]
fn s9_only_watches_while_our_tables_are_in_force() {
    let mut r = rig();
    // Not applied yet (factory, auto): the firmware's business.
    r.ec.state().cpu_temp = Some(80);
    r.ec.state().fan_speed_override = [Some(0), None];
    tick_secs(&mut r.ctl, 0, 40);
    assert_eq!(r.ctl.take_events().len(), 0);
    assert_eq!(r.ctl.status().read_only, None);
}
