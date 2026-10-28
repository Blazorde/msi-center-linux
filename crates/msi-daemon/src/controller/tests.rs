use std::path::Path;
use std::sync::{Arc, Mutex};

use msi_core::curve::{CurvePreset, FanCurveParams};
use msi_core::fan_profile::FanProfile;
use msi_core::hybrid::{CpuCurve, hybrid_cpu_table, hybrid_cpu_table_for};
use msi_core::staged::Stage;
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

/// CPU table under hybrid control (ADR-0010) for a live `target`.
fn expected_cpu(preset: CurvePreset, target: u8) -> msi_core::table::RawTables {
    hybrid_cpu_table(&preset.spec(), &FACTORY_CPU, target)
        .unwrap()
        .curve
        .tables()
}

/// CPU table before any temperature is known: target = the curve's start speed.
fn expected_cpu_initial(preset: CurvePreset) -> msi_core::table::RawTables {
    expected_cpu(preset, preset.spec().params().min())
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
    assert_eq!(s.cpu, expected_cpu_initial(CurvePreset::Progressive));
    assert_eq!((s.cpu.up, s.cpu.down), (FACTORY_CPU.up, FACTORY_CPU.down));
    assert_eq!(s.gpu, expected(CurvePreset::Progressive));
    assert_eq!(s.fan_mode, FanMode::Advanced);
    assert_eq!(s.fan_mode_writes, [FanMode::Advanced]);
    assert_eq!(s.shift_mode, ShiftMode::Comfort);
    // ADR-0010: the CPU thresholds are never written, only the speeds.
    let writes = |fan: Fan| -> Vec<TableWrite> {
        s.writes
            .iter()
            .filter(|(f, _)| *f == fan)
            .map(|(_, w)| *w)
            .collect()
    };
    let cpu = writes(Fan::Cpu);
    assert!(cpu.iter().all(|w| matches!(w, TableWrite::Speeds(_))), "{cpu:?}");
    // Factory → Progressive lowers the GPU thresholds: the down table is written before the up table.
    let gpu = writes(Fan::Gpu);
    assert!(matches!(gpu[1], TableWrite::Down(_)), "{gpu:?}");
    assert!(matches!(gpu[2], TableWrite::Up(_)), "{gpu:?}");
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
            let s = r.ec.state();
            assert_eq!(s.cpu, expected_cpu_initial(to.target().curve), "{from} → {to}");
            assert_eq!(s.gpu, expected(to.target().curve), "{from} → {to}");
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
    assert_eq!((s.cpu.up, s.cpu.down), (FACTORY_CPU.up, FACTORY_CPU.down));
    assert_eq!(s.gpu, expected(CurvePreset::Silent));
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
    assert_ne!(s.writes.len(), 0);
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
    assert_eq!(r.ec.state().gpu, expected(CurvePreset::Silent));
    let live = r.ctl.fan_curve(Fan::Cpu).unwrap().table;
    assert_eq!(r.ec.state().cpu, live);
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

// ---- ADR-0010: hybrid CPU control ----

fn cpu_writes(ec: &MemoryEc) -> Vec<TableWrite> {
    ec.state()
        .writes
        .iter()
        .filter(|(f, _)| *f == Fan::Cpu)
        .map(|(_, w)| *w)
        .collect()
}

#[test]
fn hybrid_tick_writes_only_cpu_speeds_and_only_when_they_change() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    let applied = r.ec.state().writes.len();
    // 45 °C: Silent is idle below 48 °C once the filter has 3 readings → steps 0–3 at 0 %.
    set_temps(&r.ec, Some(45), Some(40));
    tick_secs(&mut r.ctl, 0, 2);
    assert_eq!(r.ec.state().writes.len(), applied, "filter not ready yet");
    let readings = r.ctl.tick(Clock(2 * SEC));
    assert_eq!(r.ec.state().cpu, expected_cpu(CurvePreset::Silent, 0));
    assert_eq!(readings.cpu_fan_target, Some(0));
    // Steady temperature: nothing more is written.
    let n = r.ec.state().writes.len();
    tick_secs(&mut r.ctl, 3, 30);
    assert_eq!(r.ec.state().writes.len(), n);
    // Heating to 70 °C: the target ramps (start speed, then +5 %/s) to the curve value 42 %; the last points
    // are within the dead band, so they wait their 20 s hold.
    set_temps(&r.ec, Some(70), Some(40));
    tick_secs(&mut r.ctl, 33, 60);
    assert_eq!(r.ec.state().cpu, expected_cpu(CurvePreset::Silent, 42));
    let readings = r.ctl.tick(Clock(93 * SEC));
    assert_eq!(readings.cpu_fan_target, Some(42));
    // Only CPU speeds were written by the loop, GPU untouched, every vector kernel-valid and S2-safe.
    let all = r.ec.state().writes.clone();
    for (fan, w) in &all[applied..] {
        assert_eq!(*fan, Fan::Cpu);
        let TableWrite::Speeds(speeds) = w else {
            panic!("only speeds may be written: {w:?}")
        };
        StepCurve::new(*speeds, FACTORY_CPU.up, FACTORY_CPU.down).unwrap();
        assert!(
            speeds[4] >= 60 && speeds[5] == 100 && speeds[6] == 100,
            "{speeds:?}"
        );
    }
    // FanCurve("cpu") is the live vector.
    let live = r.ctl.fan_curve(Fan::Cpu).unwrap();
    assert_eq!(live.table, r.ec.state().cpu);
    assert_eq!(r.ctl.take_events().len(), 0);
}

#[test]
fn hybrid_isolated_spikes_do_not_move_the_cpu_fan() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    set_temps(&r.ec, Some(45), Some(40));
    tick_secs(&mut r.ctl, 0, 5);
    let n = cpu_writes(&r.ec).len();
    for t in 5..120 {
        let temp = if t % 4 == 0 { 92 } else { 45 };
        set_temps(&r.ec, Some(temp), Some(40));
        r.ctl.tick(Clock(t * SEC));
    }
    assert_eq!(cpu_writes(&r.ec).len(), n);
    assert_eq!(r.ec.state().cpu.speeds[0], 0);
}

#[test]
fn hybrid_loop_never_writes_after_restore_factory() {
    let mut r = rig();
    r.ctl.start().unwrap();
    tick_secs(&mut r.ctl, 0, 5);
    r.ctl.restore_factory().unwrap();
    let n = r.ec.state().writes.len();
    for (i, t) in [45, 60, 75, 80, 50, 40].into_iter().enumerate() {
        set_temps(&r.ec, Some(t), Some(40));
        let readings = r.ctl.tick(Clock((10 + i as u64 * 10) * SEC));
        assert_eq!(readings.cpu_fan_target, None);
        tick_secs(&mut r.ctl, 11 + i as u64 * 10, 9);
    }
    let s = r.ec.state();
    assert_eq!(s.writes.len(), n);
    assert_eq!((s.cpu, s.fan_mode), (FACTORY_CPU, FanMode::Auto));
}

#[test]
fn hybrid_s7_readback_mismatch_on_a_live_write_falls_back_and_stops() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ec.state().corrupt_readback = Some(Fan::Cpu);
    set_temps(&r.ec, Some(45), Some(40));
    tick_secs(&mut r.ctl, 0, 3);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::ReadbackMismatch]);
    assert!(!r.ctl.is_applied());
    r.ec.state().corrupt_readback = None;
    let s = r.ec.state().clone();
    assert_eq!(
        (s.cpu, s.gpu, s.fan_mode),
        (FACTORY_CPU, FACTORY_GPU, FanMode::Auto)
    );
    let n = s.writes.len();
    set_temps(&r.ec, Some(70), Some(40));
    tick_secs(&mut r.ctl, 3, 9);
    assert_eq!(r.ec.state().writes.len(), n, "no live write once handed back");
}

#[test]
fn hybrid_live_write_failure_falls_back_to_auto() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ec.state().fail_table_writes = true;
    set_temps(&r.ec, Some(45), Some(40));
    tick_secs(&mut r.ctl, 0, 3);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::WriteFailed]);
    assert!(!r.ctl.is_applied());
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
}

#[test]
fn hybrid_waits_for_the_drift_check_when_the_ec_left_advanced() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ec.state().fan_mode = FanMode::Auto;
    let n = r.ec.state().writes.len();
    set_temps(&r.ec, Some(45), Some(40));
    // Ticks 1–9: no live write in `auto`; tick 10: drift check re-applies (with the current target).
    tick_secs(&mut r.ctl, 0, DRIFT_CHECK_EVERY - 1);
    assert_eq!(r.ec.state().writes.len(), n);
    tick_secs(&mut r.ctl, DRIFT_CHECK_EVERY - 1, 1);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::EcReset]);
    let s = r.ec.state();
    assert_eq!(s.fan_mode, FanMode::Advanced);
    assert_eq!(s.cpu, expected_cpu(CurvePreset::Silent, 0));
}

#[test]
fn hybrid_drift_check_compares_with_the_live_vector() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    set_temps(&r.ec, Some(45), Some(40));
    tick_secs(&mut r.ctl, 0, 3 * DRIFT_CHECK_EVERY);
    // The live vector (0 %) differs from the one applied at start (30 %): not a drift.
    assert_eq!(r.ctl.take_events().len(), 0);
    // Someone rewrites the CPU speeds: drift → re-applied.
    r.ec.state().cpu = FACTORY_CPU;
    tick_secs(&mut r.ctl, 3 * DRIFT_CHECK_EVERY, DRIFT_CHECK_EVERY);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::EcReset]);
    assert_eq!(r.ec.state().cpu, expected_cpu(CurvePreset::Silent, 0));
}

#[test]
fn hybrid_resume_reapplies_the_current_target_idempotently() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    set_temps(&r.ec, Some(60), Some(40));
    tick_secs(&mut r.ctl, 0, 20);
    let n = r.ec.state().writes.len();
    r.ctl.reapply().unwrap();
    assert_eq!(r.ec.state().writes.len(), n);
    // EC reset during suspend: re-applied with the live target, not the start speed.
    let live = r.ec.state().cpu;
    r.ec.state().cpu = FACTORY_CPU;
    r.ctl.reapply().unwrap();
    assert_eq!(r.ec.state().cpu, live);
}

#[test]
fn hybrid_falls_back_to_generated_cpu_tables_on_firmware_without_factory_tables() {
    let mut r = rig_with(
        MemoryState {
            fw_version: "14K2EMS1.109".into(),
            ..MemoryState::default()
        },
        |_| {},
    );
    r.ctl.set_profile(Profile::Eco).unwrap();
    assert_eq!(r.ec.state().cpu, expected(CurvePreset::Silent));
    let n = r.ec.state().writes.len();
    set_temps(&r.ec, Some(70), Some(40));
    let readings = r.ctl.tick(Clock(0));
    tick_secs(&mut r.ctl, 1, 30);
    assert_eq!(readings.cpu_fan_target, None);
    assert_eq!(r.ec.state().writes.len(), n);
}

#[test]
fn hybrid_set_fan_curve_cpu_returns_the_live_vector() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    let spec = CurvePreset::Progressive.spec();
    let g = r.ctl.set_fan_curve(Fan::Cpu, spec).unwrap();
    assert_eq!(g.curve.tables(), r.ec.state().cpu);
    assert_eq!(g.curve.up(), FACTORY_CPU.up);
    // GPU keeps the generated table.
    let g = r.ctl.set_fan_curve(Fan::Gpu, spec).unwrap();
    assert_eq!(g.curve.tables(), expected(CurvePreset::Progressive));
}

// ---- Fan profiles ----

/// A second controller on the same state file and EC (daemon restart).
fn restart(r: &Rig) -> Controller {
    let root = FsRoot::new(r.dir.path());
    let (ctl, warnings) = Controller::new(
        Box::new(r.ec.clone()),
        PowerSupply::new(root.clone()),
        GpuManager::new(root),
        None,
        StateStore::new(r.dir.path().join("state/state.toml")),
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    ctl
}

fn quiet_custom() -> FanCurveSpec {
    FanCurveSpec::new(
        FanCurveParams::new(45, 80, 30, 100, 2.5).unwrap(),
        0,
        [45, 52, 60, 67, 73, 80],
        3,
    )
    .unwrap()
}

#[test]
fn fan_profile_defaults_follow_the_performance_profile() {
    let mut r = rig();
    for (p, fp) in [
        (Profile::Eco, FanProfile::Silent),
        (Profile::Balanced, FanProfile::Balanced),
        (Profile::Gaming, FanProfile::Turbo),
    ] {
        r.ctl.set_profile(p).unwrap();
        let st = r.ctl.status();
        assert_eq!((st.fan_profile, st.default_fan_profile), (fp, fp), "{p}");
        assert_eq!(r.ec.state().gpu, expected(fp.preset().unwrap()), "{p}");
    }
}

#[test]
fn auto_writes_factory_tables_then_auto_then_shift_mode() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    let s = r.ec.state().clone();
    assert_eq!((s.cpu, s.gpu), (FACTORY_CPU, FACTORY_GPU));
    assert_eq!(s.fan_mode, FanMode::Auto);
    assert_eq!(s.fan_mode_writes.last(), Some(&FanMode::Auto));
    // Shift mode and GPU handling are the performance profile's, unchanged.
    assert_eq!(s.shift_mode, ShiftMode::Turbo);
    assert!(r.ctl.is_auto_applied());
    assert!(!r.ctl.is_applied());
    assert!(!r.ctl.is_handed_back());
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Auto);
    assert_eq!(r.ctl.status().default_fan_profile, FanProfile::Turbo);
    // FanCurve reports the factory table, nothing raised.
    let view = r.ctl.fan_curve(Fan::Cpu).unwrap();
    assert_eq!(view.table, FACTORY_CPU);
    assert_eq!(view.raised, []);
    assert!(!view.custom);
    assert_eq!(r.ctl.fan_curve(Fan::Gpu).unwrap().table, FACTORY_GPU);
    assert_eq!(r.ctl.take_events().len(), 0);
}

#[test]
fn auto_stops_the_hybrid_cpu_loop() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    set_temps(&r.ec, Some(70), Some(40));
    tick_secs(&mut r.ctl, 0, 10);
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    let n = r.ec.state().writes.len();
    for (i, t) in [45, 60, 75, 80, 50, 40].into_iter().enumerate() {
        set_temps(&r.ec, Some(t), Some(40));
        let readings = r.ctl.tick(Clock((10 + i as u64 * 10) * SEC));
        assert_eq!(readings.cpu_fan_target, None);
        tick_secs(&mut r.ctl, 11 + i as u64 * 10, 9);
    }
    let s = r.ec.state();
    assert_eq!(s.writes.len(), n);
    assert_eq!((s.cpu, s.fan_mode), (FACTORY_CPU, FanMode::Auto));
    assert_eq!(r.ctl.take_events().len(), 0, "Auto is not a drift");
}

#[test]
fn auto_to_hybrid_and_back_through_the_kernel_rules() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    for fp in [
        FanProfile::Auto,
        FanProfile::Turbo,
        FanProfile::Auto,
        FanProfile::Silent,
        FanProfile::Balanced,
        FanProfile::Auto,
        FanProfile::Custom,
        FanProfile::Auto,
    ] {
        r.ctl.set_fan_profile(fp).unwrap();
        let s = r.ec.state().clone();
        match fp.preset() {
            Some(preset) => {
                assert_eq!(s.fan_mode, FanMode::Advanced, "{fp}");
                assert_eq!(s.gpu, expected(preset), "{fp}");
                assert_eq!((s.cpu.up, s.cpu.down), (FACTORY_CPU.up, FACTORY_CPU.down));
                assert!(r.ctl.is_applied() && !r.ctl.is_auto_applied());
            }
            None if fp == FanProfile::Auto => {
                assert_eq!(
                    (s.cpu, s.gpu, s.fan_mode),
                    (FACTORY_CPU, FACTORY_GPU, FanMode::Auto)
                );
                assert!(!r.ctl.is_applied() && r.ctl.is_auto_applied());
            }
            // Custom without a saved curve: the default fan profile's preset (Progressive).
            None => assert_eq!(s.gpu, expected(CurvePreset::Progressive)),
        }
        assert_eq!(s.shift_mode, ShiftMode::Comfort);
    }
    assert_eq!(r.ctl.take_events().len(), 0);
}

#[test]
fn switching_profile_applies_its_remembered_fan_profile() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ctl.set_fan_profile(FanProfile::Turbo).unwrap();
    assert_eq!(r.ec.state().gpu, expected(CurvePreset::Aggressive));
    assert_eq!(r.ec.state().shift_mode, ShiftMode::Eco);
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Balanced);
    assert_eq!(r.ec.state().gpu, expected(CurvePreset::Progressive));
    r.ctl.set_profile(Profile::Eco).unwrap();
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Turbo);
    assert_eq!(r.ec.state().gpu, expected(CurvePreset::Aggressive));
    r.ctl.set_profile(Profile::Gaming).unwrap();
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Auto);
    let s = r.ec.state().clone();
    assert_eq!(
        (s.gpu, s.fan_mode, s.shift_mode),
        (FACTORY_GPU, FanMode::Auto, ShiftMode::Turbo)
    );
    // Back to the default: the override is forgotten.
    r.ctl.set_fan_profile(FanProfile::Turbo).unwrap();
    assert!(!r.ctl.settings().fan_profiles.contains_key(&Profile::Gaming));
}

#[test]
fn fan_profiles_persist_and_auto_is_reapplied_at_start() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ctl.set_fan_profile(FanProfile::Balanced).unwrap();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ctl.shutdown();
    // The EC is left in some other state, the daemon restarts.
    r.ec.state().fan_mode = FanMode::Advanced;
    let mut ctl = restart(&r);
    assert_eq!(ctl.settings().fan_profile(Profile::Gaming), FanProfile::Auto);
    assert_eq!(ctl.settings().fan_profile(Profile::Eco), FanProfile::Balanced);
    ctl.start().unwrap();
    assert!(ctl.is_auto_applied());
    let s = r.ec.state().clone();
    assert_eq!(
        (s.cpu, s.gpu, s.fan_mode),
        (FACTORY_CPU, FACTORY_GPU, FanMode::Auto)
    );
}

#[test]
fn auto_is_reapplied_after_resume_and_by_the_drift_check() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    // Idempotent re-apply.
    let writes = r.ec.state().writes.len();
    r.ctl.reapply().unwrap();
    assert_eq!(r.ec.state().writes.len(), writes);
    // Something changed the EC behind our back (resume): re-applied.
    {
        let mut s = r.ec.state();
        s.gpu = expected(CurvePreset::Aggressive);
        s.fan_mode = FanMode::Advanced;
    }
    r.ctl.reapply().unwrap();
    let s = r.ec.state().clone();
    assert_eq!((s.gpu, s.fan_mode), (FACTORY_GPU, FanMode::Auto));
    // Drift check (every 10 s) sees a change and re-applies.
    r.ec.state().fan_mode = FanMode::Advanced;
    tick_secs(&mut r.ctl, 0, DRIFT_CHECK_EVERY);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::EcReset]);
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
}

#[test]
fn s4_still_forces_cooler_boost_under_auto() {
    let mut r = rig();
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    set_temps(&r.ec, Some(96), Some(60));
    tick_secs(&mut r.ctl, 0, 2);
    assert!(r.ec.state().cooler_boost);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::BoostForced]);
}

#[test]
fn s9_does_not_watch_the_firmware_curve_under_auto() {
    let mut r = rig();
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    {
        let mut s = r.ec.state();
        s.cpu_temp = Some(74);
        s.fan_speed_override = [Some(0), None];
    }
    tick_secs(&mut r.ctl, 0, 60);
    assert_eq!(r.ctl.status().read_only, None);
    assert_eq!(r.ctl.take_events().len(), 0);
}

#[test]
fn s7_auto_readback_mismatch_falls_back() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    r.ec.state().corrupt_readback = Some(Fan::Gpu);
    assert!(matches!(
        r.ctl.set_fan_profile(FanProfile::Auto),
        Err(ControlError::SafetyFallback(_))
    ));
    assert!(!r.ctl.is_auto_applied());
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::WriteFailed]);
}

#[test]
fn s7_auto_shift_mode_drift_is_detected() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    r.ec.state().shift_mode = ShiftMode::Eco;
    tick_secs(&mut r.ctl, 0, DRIFT_CHECK_EVERY);
    assert_eq!(kinds(&r.ctl.take_events()), [EventKind::EcReset]);
    assert_eq!(r.ec.state().shift_mode, ShiftMode::Turbo);
}

#[test]
fn s8_and_s9_refuse_fan_profiles_and_write_nothing() {
    let mut r = rig_with(
        MemoryState {
            fw_version: "14K2EMS1.999".into(),
            ..MemoryState::default()
        },
        |_| {},
    );
    for fp in FanProfile::ALL {
        assert!(matches!(
            r.ctl.set_fan_profile(fp),
            Err(ControlError::ReadOnly(_))
        ));
    }
    assert_eq!(r.ec.state().fan_mode_writes.len(), 0);
    assert_eq!(r.ec.state().writes.len(), 0);
    // Refused choices are not remembered.
    assert!(r.ctl.settings().fan_profiles.is_empty());
}

#[test]
fn auto_on_firmware_without_factory_tables_only_sets_auto() {
    let mut r = rig_with(
        MemoryState {
            fw_version: "14K2EMS1.109".into(),
            ..MemoryState::default()
        },
        |_| {},
    );
    r.ctl.set_profile(Profile::Gaming).unwrap();
    let applied = r.ec.state().cpu;
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    assert!(r.ctl.is_auto_applied());
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    assert_eq!(r.ec.state().cpu, applied);
    assert_eq!(r.ctl.fan_curve(Fan::Cpu).unwrap().table, applied);
}

#[test]
fn set_fan_curve_switches_to_custom_and_reset_goes_back_to_the_default() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    let g = r.ctl.set_fan_curve(Fan::Gpu, quiet_custom()).unwrap();
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Custom);
    let s = r.ec.state().clone();
    assert_eq!(s.fan_mode, FanMode::Advanced);
    assert_eq!(s.gpu, generate(&quiet_custom()).unwrap().curve.tables());
    assert_eq!(g.curve.tables(), s.gpu);
    // The other fan of Custom follows the profile's default preset until edited.
    assert_eq!(s.cpu, expected_cpu_initial(CurvePreset::Aggressive));
    assert!(r.ctl.fan_curve(Fan::Gpu).unwrap().custom);
    assert_eq!(r.ctl.custom_fan_curve(Fan::Gpu).unwrap().0, quiet_custom());
    assert_eq!(
        r.ctl.custom_fan_curve(Fan::Cpu).unwrap().0,
        CurvePreset::Aggressive.spec()
    );
    // The custom curve is kept when another fan profile is picked, and comes back with Custom.
    r.ctl.set_fan_profile(FanProfile::Silent).unwrap();
    assert_eq!(r.ec.state().gpu, expected(CurvePreset::Silent));
    r.ctl.set_fan_profile(FanProfile::Custom).unwrap();
    assert_eq!(
        r.ec.state().gpu,
        generate(&quiet_custom()).unwrap().curve.tables()
    );
    // Reset: no custom curve left → back to Turbo.
    r.ctl.reset_fan_curve(Fan::Gpu).unwrap();
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Turbo);
    assert_eq!(r.ec.state().gpu, expected(CurvePreset::Aggressive));
}

#[test]
fn custom_curves_are_per_profile_and_survive_a_restart() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ctl.set_fan_curve(Fan::Gpu, quiet_custom()).unwrap();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Balanced);
    let mut ctl = restart(&r);
    assert_eq!(ctl.settings().fan_profile(Profile::Eco), FanProfile::Custom);
    ctl.set_profile(Profile::Eco).unwrap();
    assert_eq!(
        r.ec.state().gpu,
        generate(&quiet_custom()).unwrap().curve.tables()
    );
}

#[test]
fn set_fan_profile_takes_control_back_after_restore_factory() {
    let mut r = rig();
    r.ctl.start().unwrap();
    r.ctl.restore_factory().unwrap();
    r.ctl.set_fan_profile(FanProfile::Turbo).unwrap();
    assert!(!r.ctl.is_handed_back());
    assert_eq!(r.ec.state().fan_mode, FanMode::Advanced);
    assert_eq!(r.ec.state().gpu, expected(CurvePreset::Aggressive));
}

// ---- ADR-0013: staged CPU curve ----

/// 4 stages: 50 °C 30 %, 60 °C 45 %, 70 °C 70 %, 80 °C 100 %; each falls back 4 °C under its rise.
fn four_stages() -> StagedCurve {
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

/// CPU table under hybrid control for a live `target` on a staged curve.
fn expected_staged_cpu(curve: &StagedCurve, target: u8) -> msi_core::table::RawTables {
    hybrid_cpu_table_for(&CpuCurve::Staged(curve.clone()), &FACTORY_CPU, target)
        .unwrap()
        .curve
        .tables()
}

/// Ticks `n` seconds at `cpu` °C from second `from`; returns the last readings.
fn hold(r: &mut Rig, from: u64, n: u64, cpu: u8) -> Readings {
    set_temps(&r.ec, Some(cpu), Some(40));
    tick_secs(&mut r.ctl, from, n - 1);
    r.ctl.tick(Clock((from + n - 1) * SEC))
}

#[test]
fn staged_cpu_curve_follows_its_stages_and_falls_back_per_stage() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    let curve = four_stages();
    let view = r.ctl.set_cpu_stages(curve.clone()).unwrap();
    assert!(view.saved && view.active && view.supported);
    assert!(view.floors.is_empty());
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Custom);
    // Applied before any temperature is known: the fan spins at the first stage's speed.
    assert_eq!(r.ec.state().cpu, expected_staged_cpu(&curve, 30));
    let cpu = r.ec.state().cpu;
    assert_eq!((cpu.up, cpu.down), (FACTORY_CPU.up, FACTORY_CPU.down));
    // The GPU keeps its own 7-step table.
    assert_eq!(r.ec.state().gpu, expected(CurvePreset::Silent));

    // 44 °C: under the first stage → idle.
    let readings = hold(&mut r, 0, 12, 44);
    assert_eq!(readings.cpu_fan_target, Some(0));
    assert_eq!(r.ec.state().cpu, expected_staged_cpu(&curve, 0));
    // 55 °C: stage 0 (50 → 30 %).
    let readings = hold(&mut r, 12, 30, 55);
    assert_eq!(readings.cpu_fan_target, Some(30));
    // 65 °C: stage 1 (45 %), reached by +5 %/s from the start speed.
    let readings = hold(&mut r, 42, 30, 65);
    assert_eq!(readings.cpu_fan_target, Some(45));
    assert_eq!(r.ec.state().cpu, expected_staged_cpu(&curve, 45));
    // FanCurve("cpu") is the live vector, and the view says it follows the staged curve.
    assert_eq!(r.ctl.fan_curve(Fan::Cpu).unwrap().table, r.ec.state().cpu);
    // Cooling to 58 °C (between stage 1's fall-back, 56, and its rise, 60): stage 1 holds.
    let before = cpu_writes(&r.ec).len();
    let readings = hold(&mut r, 72, 40, 58);
    assert_eq!(readings.cpu_fan_target, Some(45));
    assert_eq!(cpu_writes(&r.ec).len(), before, "no write while the stage holds");
    // 54 °C: under 56 → back to stage 0 (30 %), ramping down by −1 %/s.
    let readings = hold(&mut r, 112, 40, 54);
    assert_eq!(readings.cpu_fan_target, Some(30));
    // 44 °C: under stage 0's fall-back (46) → idle.
    let readings = hold(&mut r, 152, 20, 44);
    assert_eq!(readings.cpu_fan_target, Some(0));
    // Every vector written by the loop was kernel-valid and S2-safe, and only speeds were written.
    for w in cpu_writes(&r.ec) {
        let TableWrite::Speeds(speeds) = w else {
            panic!("only speeds may be written: {w:?}")
        };
        StepCurve::new(speeds, FACTORY_CPU.up, FACTORY_CPU.down).unwrap();
        assert!(
            speeds[4] >= 60 && speeds[5] == 100 && speeds[6] == 100,
            "{speeds:?}"
        );
    }
    assert!(r.ctl.take_events().is_empty());
}

#[test]
fn staged_curve_hot_steps_follow_the_stages_at_their_entry_temperature() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    let curve = four_stages();
    r.ctl.set_cpu_stages(curve.clone()).unwrap();
    // Factory hot steps are entered at 77 / 83 / 90 °C: 70 % (stage 2), 100 %, 100 %.
    let speeds = r.ec.state().cpu.speeds;
    assert_eq!(speeds[4..], [70, 100, 100]);
    assert_eq!(speeds[..4], [30, 30, 30, 50]);
}

#[test]
fn staged_curve_s2_floor_takes_over_from_a_quiet_stage_and_is_reported() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
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
    let view = r.ctl.set_cpu_stages(quiet).unwrap();
    // Nothing is silent: the three S2 floors above the 20 % stage are reported.
    let floors: Vec<(u8, u8, u8)> = view
        .floors
        .iter()
        .map(|f| (f.floor.temp, f.floor.speed, f.curve_speed))
        .collect();
    assert_eq!(floors, [(60, 30, 20), (75, 50, 20), (85, 100, 20)]);
    // At 65 °C the stage asks 20 %, the floor 30 %: the fan runs at 30 %.
    let readings = hold(&mut r, 0, 40, 65);
    assert_eq!(readings.cpu_fan_target, Some(30));
    // At 78 °C the floor is 50 %.
    let readings = hold(&mut r, 40, 60, 78);
    assert_eq!(readings.cpu_fan_target, Some(50));
}

#[test]
fn staged_cpu_curve_is_saved_reapplied_after_a_restart_and_applies_only_under_custom() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    let curve = four_stages();
    r.ctl.set_cpu_stages(curve.clone()).unwrap();
    let mut ctl = restart(&r);
    assert_eq!(ctl.settings().staged_curve(Profile::Eco), Some(&curve));
    assert_eq!(ctl.settings().fan_profile(Profile::Eco), FanProfile::Custom);
    ctl.set_profile(Profile::Eco).unwrap();
    assert_eq!(r.ec.state().cpu, expected_staged_cpu(&curve, 30));
    // Picking another fan profile keeps the staged curve but stops following it.
    ctl.set_fan_profile(FanProfile::Silent).unwrap();
    assert_eq!(r.ec.state().cpu, expected_cpu_initial(CurvePreset::Silent));
    let view = ctl.cpu_stages().unwrap();
    assert!(view.saved && !view.active);
    ctl.set_fan_profile(FanProfile::Custom).unwrap();
    assert!(ctl.cpu_stages().unwrap().active);
    assert_eq!(r.ec.state().cpu, expected_staged_cpu(&curve, 30));
}

#[test]
fn staged_curves_are_per_performance_profile() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ctl.set_cpu_stages(four_stages()).unwrap();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    assert!(!r.ctl.cpu_stages().unwrap().saved);
    assert_eq!(r.ec.state().cpu, expected_cpu_initial(CurvePreset::Aggressive));
    r.ctl.set_profile(Profile::Eco).unwrap();
    assert!(r.ctl.cpu_stages().unwrap().active);
}

#[test]
fn an_exponential_cpu_edit_replaces_the_staged_curve_and_reset_removes_it() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ctl.set_cpu_stages(four_stages()).unwrap();
    // The GPU edit leaves the staged CPU curve alone.
    r.ctl.set_fan_curve(Fan::Gpu, quiet_custom()).unwrap();
    assert!(r.ctl.cpu_stages().unwrap().active);
    // The last CPU edit wins.
    r.ctl
        .set_fan_curve(Fan::Cpu, CurvePreset::Progressive.spec())
        .unwrap();
    let view = r.ctl.cpu_stages().unwrap();
    assert!(!view.saved && !view.active);
    assert_eq!(r.ec.state().cpu, expected_cpu_initial(CurvePreset::Progressive));

    // Staged again, then reset: back to the exponential curves, still custom for the GPU.
    r.ctl.set_cpu_stages(four_stages()).unwrap();
    r.ctl.reset_cpu_stages().unwrap();
    assert!(!r.ctl.cpu_stages().unwrap().saved);
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Custom);
    // No custom curve left at all → back to the profile's default fan profile.
    r.ctl.reset_fan_curve(Fan::Cpu).unwrap();
    r.ctl.reset_fan_curve(Fan::Gpu).unwrap();
    r.ctl.set_cpu_stages(four_stages()).unwrap();
    r.ctl.reset_cpu_stages().unwrap();
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Silent);
    assert_eq!(r.ec.state().cpu, expected_cpu_initial(CurvePreset::Silent));
    // Resetting nothing writes nothing.
    let writes = r.ec.state().writes.len();
    r.ctl.reset_cpu_stages().unwrap();
    assert_eq!(r.ec.state().writes.len(), writes);
}

#[test]
fn reset_fan_curve_cpu_also_removes_the_staged_curve() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ctl.set_cpu_stages(four_stages()).unwrap();
    r.ctl.reset_fan_curve(Fan::Cpu).unwrap();
    assert!(!r.ctl.cpu_stages().unwrap().saved);
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Silent);
}

#[test]
fn cpu_stages_view_starts_from_the_preset_table() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    let view = r.ctl.cpu_stages().unwrap();
    assert!(!view.saved && !view.active && view.supported);
    let table = generate(&CurvePreset::Progressive.spec()).unwrap().curve;
    assert_eq!(view.curve, StagedCurve::from_step_curve(&table).unwrap());
    assert_eq!(view.curve.stages().len(), 6);
    assert!(view.floors.is_empty());
}

#[test]
fn staged_curves_are_refused_without_factory_tables_and_change_nothing() {
    let mut r = rig_with(
        MemoryState {
            fw_version: "14K2EMS1.109".into(),
            ..MemoryState::default()
        },
        |_| {},
    );
    r.ctl.set_profile(Profile::Eco).unwrap();
    assert!(!r.ctl.cpu_stages().unwrap().supported);
    let writes = r.ec.state().writes.len();
    let err = r.ctl.set_cpu_stages(four_stages()).unwrap_err();
    assert!(matches!(err, ControlError::InvalidArgument(_)), "{err}");
    assert!(r.ctl.settings().staged.is_empty());
    assert_eq!(r.ctl.status().fan_profile, FanProfile::Silent);
    assert_eq!(r.ec.state().writes.len(), writes);
}

#[test]
fn staged_curves_are_refused_in_read_only_mode() {
    let mut r = rig_with(
        MemoryState {
            fw_version: "14K2EMS1.999".into(),
            ..MemoryState::default()
        },
        |_| {},
    );
    assert!(matches!(
        r.ctl.set_cpu_stages(four_stages()),
        Err(ControlError::ReadOnly(_))
    ));
    assert!(matches!(r.ctl.reset_cpu_stages(), Err(ControlError::ReadOnly(_))));
    assert_eq!(r.ec.state().writes.len(), 0);
}

#[test]
fn staged_curve_live_write_failure_falls_back_to_auto() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Eco).unwrap();
    r.ctl.set_cpu_stages(four_stages()).unwrap();
    // S7 applies to staged writes like to exponential ones: a corrupt read-back hands the fans back.
    r.ec.state().corrupt_readback = Some(Fan::Cpu);
    hold(&mut r, 0, 12, 55);
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
    assert!(kinds(&r.ctl.take_events()).contains(&EventKind::ReadbackMismatch));
    assert!(!r.ctl.is_applied());
}

fn rapl_zone(r: &Path, zone: &str) {
    let d = format!("sys/class/powercap/{zone}");
    write(r, &format!("{d}/name"), "package-0\n");
    write(r, &format!("{d}/constraint_0_name"), "long_term\n");
    write(r, &format!("{d}/constraint_1_name"), "short_term\n");
    write(r, &format!("{d}/constraint_0_power_limit_uw"), "110000000\n");
    write(r, &format!("{d}/constraint_1_power_limit_uw"), "160000000\n");
}

fn rig_with_rapl() -> Rig {
    let Rig { dir, ec, ctl } = rig_with(MemoryState::default(), |r| {
        rapl_zone(r, "intel-rapl:0");
        rapl_zone(r, "intel-rapl-mmio:0");
    });
    let ctl = ctl.with_rapl(Rapl::new(FsRoot::new(dir.path())));
    Rig { dir, ec, ctl }
}

fn pl(r: &Rig, zone: &str) -> (String, String) {
    let d = format!("sys/class/powercap/{zone}");
    (
        read(r.dir.path(), &format!("{d}/constraint_0_power_limit_uw")),
        read(r.dir.path(), &format!("{d}/constraint_1_power_limit_uw")),
    )
}

fn watts(pl1: u64, pl2: u64) -> (String, String) {
    ((pl1 * 1_000_000).to_string(), (pl2 * 1_000_000).to_string())
}

#[test]
fn adr_0014_each_profile_applies_its_cpu_power_limits_and_gaming_the_firmware_ones() {
    let mut r = rig_with_rapl();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    for z in crate::rapl::ZONES {
        assert_eq!(pl(&r, z), watts(35, 60), "{z}");
    }
    assert_eq!(
        r.ctl.status().cpu_power,
        Some(msi_core::profile::BALANCED_CPU_POWER)
    );
    r.ctl.set_profile(Profile::Eco).unwrap();
    assert_eq!(pl(&r, "intel-rapl:0"), watts(20, 35));
    r.ctl.set_profile(Profile::Gaming).unwrap();
    assert_eq!(pl(&r, "intel-rapl:0"), watts(110, 160));
    // Fan profile Auto still gets the profile's power limits.
    r.ctl.set_fan_profile(FanProfile::Auto).unwrap();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    assert_eq!(pl(&r, "intel-rapl-mmio:0"), watts(35, 60));
}

#[test]
fn adr_0014_changed_limits_are_reapplied_and_shutdown_restores_the_firmware_ones() {
    let mut r = rig_with_rapl();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    // The firmware rewrites both zones (one alone changes nothing: the CPU obeys the lowest).
    for z in crate::rapl::ZONES {
        let d = format!("sys/class/powercap/{z}");
        write(
            r.dir.path(),
            &format!("{d}/constraint_1_power_limit_uw"),
            "160000000\n",
        );
    }
    for _ in 0..DRIFT_CHECK_EVERY {
        r.ctl.tick(Clock(0));
    }
    for z in crate::rapl::ZONES {
        assert_eq!(pl(&r, z), watts(35, 60), "{z}");
    }
    r.ctl.shutdown();
    for z in crate::rapl::ZONES {
        assert_eq!(pl(&r, z), watts(110, 160), "{z}");
    }
}

#[test]
fn adr_0014_without_rapl_nothing_is_written_and_status_is_none() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Balanced).unwrap();
    assert_eq!(r.ctl.status().cpu_power, None);
}
