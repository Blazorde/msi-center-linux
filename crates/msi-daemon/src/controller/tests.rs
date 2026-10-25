use std::path::Path;

use msi_core::curve::{CurvePreset, FanCurveParams};
use msi_core::table::TableWrite;
use tempfile::TempDir;

use super::*;
use crate::ec::{FACTORY_CPU, MemoryEc, MemoryState};
use crate::fs::FsRoot;
use crate::fs::testutil::{read, symlink, write};

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
fn eco_refuses_dgpu_removal_while_held_and_never_kills() {
    let mut r = rig_with(MemoryState::default(), |p| {
        add_dgpu(p);
        write(p, "proc/3876/comm", "gnome-shell\n");
        symlink(p, "proc/3876/fd/14", "/dev/nvidia0");
    });
    let err = r.ctl.set_profile(Profile::Eco).unwrap_err();
    match err {
        ControlError::Gpu(GpuError::Busy(h)) => assert_eq!(h[0].name, "gnome-shell"),
        other => panic!("{other}"),
    }
    // The EC part is applied, the dGPU is untouched, the process is still there.
    assert_eq!(r.ec.state().fan_mode, FanMode::Advanced);
    assert_eq!(r.ec.state().shift_mode, ShiftMode::Eco);
    assert_eq!(read(r.dir.path(), "sys/bus/pci/devices/0000:01:00.0/remove"), "");
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
fn s5_shutdown_restores_auto() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ctl.shutdown();
    assert_eq!(r.ec.state().fan_mode, FanMode::Auto);
}

#[test]
fn ac_unplug_switches_profile_when_enabled() {
    let mut r = rig();
    r.ctl.start().unwrap();
    assert_eq!(r.ctl.settings().profile, Profile::Balanced);
    r.ctl.tick(Clock(0));
    write(r.dir.path(), "sys/class/power_supply/ADP1/online", "0\n");
    r.ctl.tick(Clock(1));
    assert_eq!(
        r.ctl.settings().profile,
        Profile::Balanced,
        "switch is off by default"
    );

    r.ctl.set_auto_profile_on_power(true).unwrap();
    assert_eq!(
        r.ctl.settings().profile,
        Profile::Eco,
        "enabling applies the battery profile"
    );
    assert_eq!(r.ec.state().cpu, expected(CurvePreset::Silent));
    write(r.dir.path(), "sys/class/power_supply/ADP1/online", "1\n");
    r.ctl.tick(Clock(2));
    assert_eq!(r.ctl.settings().profile, Profile::Balanced);
    assert_eq!(r.ec.state().cpu, expected(CurvePreset::Progressive));
    assert_eq!(r.ctl.status().on_ac, Some(true));
}

#[test]
fn settings_survive_a_restart() {
    let mut r = rig();
    r.ctl.set_profile(Profile::Gaming).unwrap();
    r.ctl.set_charge_limit(80).unwrap();
    let custom = msi_core::curve::FanCurveSpec::new(
        FanCurveParams::new(45, 80, 30, 100, 2.5).unwrap(),
        0,
        [45, 52, 60, 67, 73, 80],
        3,
    )
    .unwrap();
    let g = r.ctl.set_fan_curve(Fan::Gpu, custom).unwrap();
    assert_eq!(r.ec.state().gpu, g.curve.tables());
    assert_eq!(
        read(
            r.dir.path(),
            "sys/class/power_supply/BAT1/charge_control_end_threshold"
        ),
        "80"
    );

    // New daemon, EC reset to factory.
    write(
        r.dir.path(),
        "sys/class/power_supply/BAT1/charge_control_end_threshold",
        "100\n",
    );
    let ec = MemoryEc::default();
    let root = FsRoot::new(r.dir.path());
    let (mut ctl, warnings) = Controller::new(
        Box::new(ec.clone()),
        PowerSupply::new(root.clone()),
        GpuManager::new(root),
        None,
        StateStore::new(r.dir.path().join("state/state.toml")),
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    ctl.start().unwrap();
    assert_eq!(ctl.settings().profile, Profile::Gaming);
    assert_eq!(ec.state().gpu, g.curve.tables());
    assert_eq!(ec.state().cpu, expected(CurvePreset::Aggressive));
    assert_eq!(
        read(
            r.dir.path(),
            "sys/class/power_supply/BAT1/charge_control_end_threshold"
        ),
        "80"
    );
    let (spec, _, is_custom) = ctl.fan_curve(Fan::Gpu).unwrap();
    assert!(is_custom);
    assert_eq!(spec, custom);

    ctl.reset_fan_curve(Fan::Gpu).unwrap();
    assert_eq!(ec.state().gpu, expected(CurvePreset::Aggressive));
    assert!(!ctl.fan_curve(Fan::Gpu).unwrap().2);
}

#[test]
fn s3_charge_limit_out_of_range_is_rejected() {
    let mut r = rig();
    for bad in [0, 49, 101] {
        assert!(matches!(
            r.ctl.set_charge_limit(bad),
            Err(ControlError::InvalidArgument(_))
        ));
    }
    assert_eq!(
        read(
            r.dir.path(),
            "sys/class/power_supply/BAT1/charge_control_end_threshold"
        ),
        "100\n"
    );
}

#[test]
fn status_reflects_the_ec() {
    let mut r = rig();
    let s = r.ctl.status();
    assert_eq!(
        (s.fan_mode.as_str(), s.fw_version.as_str()),
        ("auto", "14K2EMS1.110")
    );
    assert_eq!(s.charge_limit, Some(100));
    assert_eq!(s.gpu_state, GpuState::Removed);
    assert_eq!(s.read_only, None);
    r.ctl.set_profile(Profile::Gaming).unwrap();
    assert_eq!(r.ctl.status().fan_mode, "advanced");
    let readings = r.ctl.tick(Clock(0));
    assert_eq!(readings.cpu_temp, Some(50));
    assert_eq!(readings.fan_pct[0], Some(47));
}
