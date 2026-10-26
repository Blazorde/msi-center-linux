//! Property tests for the safety invariants of `docs/hardware/fan-safety.md`.

// Test-only file: strategy helpers are not `#[test]` fns, so clippy does not apply `allow-expect-in-tests`.
#![allow(clippy::expect_used)]

use msi_core::curve::{FanCurveParams, FanCurveSpec};
use msi_core::ec::CONF_G2_5;
use msi_core::supervisor::{BOOST_ON_TEMP, BoostDecision, SensorReading, ThermalSupervisor};
use msi_core::table::{RawTables, SPEED_FLOORS, StepCurve, generate, write_plan, write_plan_raw};
use proptest::prelude::*;

fn valid_params() -> impl Strategy<Value = FanCurveParams> {
    (20u8..100, 0u8..=100, 1.0f64..=4.0)
        .prop_flat_map(|(t0, min, k)| (Just(t0), t0 + 1..=100, Just(min), min..=100, Just(k)))
        .prop_map(|(t0, t1, min, max, k)| {
            FanCurveParams::new(t0, t1, min, max, k).expect("strategy only builds valid params")
        })
}

/// Six strictly increasing temperatures in `lo..=100`.
fn increasing(lo: u8) -> impl Strategy<Value = [u8; 6]> {
    proptest::collection::btree_set(lo..=100, 6).prop_map(|set| {
        let v: Vec<u8> = set.into_iter().collect();
        [v[0], v[1], v[2], v[3], v[4], v[5]]
    })
}

fn valid_spec() -> impl Strategy<Value = FanCurveSpec> {
    (valid_params(), 2u8..=10).prop_flat_map(|(params, h)| {
        (0..=params.min(), increasing(20 + h)).prop_map(move |(idle, up)| {
            FanCurveSpec::new(params, idle, up, h).expect("strategy only builds valid specs")
        })
    })
}

/// Any table the kernel accepts (not necessarily S2-safe, like the factory one).
fn kernel_valid_tables() -> impl Strategy<Value = RawTables> {
    (
        (proptest::collection::vec(0u8..=100, 6), 80u8..=100).prop_map(|(mut v, last)| {
            v.push(last);
            v
        }),
        increasing(21),
        proptest::collection::vec(1u8..=10, 6),
    )
        .prop_map(|(mut speeds, up, gaps)| {
            speeds.sort_unstable();
            // down[i] = max(up[i] − gap, previous down + 1, 20), always < up[i].
            let mut down = [0u8; 6];
            for i in 0..6 {
                let floor = if i == 0 { 20 } else { down[i - 1] + 1 };
                down[i] = up[i].saturating_sub(gaps[i]).max(floor).min(up[i] - 1);
            }
            RawTables {
                speeds: speeds.try_into().expect("7 speeds"),
                up,
                down,
            }
        })
        .prop_filter("kernel-valid", |t| t.kernel_check().is_ok())
}

fn assert_safe(curve: &StepCurve) {
    let t = curve.tables();
    assert_eq!(t.kernel_check(), Ok(()), "{curve:?}");
    assert!(
        t.up.iter().zip(t.down).all(|(u, d)| u - d >= 2),
        "hysteresis: {curve:?}"
    );
    for floor in SPEED_FLOORS {
        for temp in floor.temp..=100 {
            assert!(
                curve.min_speed_at(temp) >= floor.speed,
                "S2 at {temp} °C: {curve:?}"
            );
        }
    }
}

proptest! {
    #[test]
    fn generated_table_is_safe(spec in valid_spec()) {
        let g = generate(&spec).expect("valid specs generate");
        prop_assert_eq!(g.curve.up(), spec.up());
        assert_safe(&g.curve);
    }

    #[test]
    fn floors_only_raise_and_every_change_is_reported(spec in valid_spec()) {
        let g = generate(&spec).expect("valid specs generate");
        for (i, &s) in g.curve.speeds().iter().enumerate() {
            let curve = if i == 0 { spec.idle() } else { spec.params().speed_percent(spec.up()[i - 1]) };
            prop_assert!(s >= curve);
            let reported = g.report.raised.iter().find(|p| p.index == i);
            if s == curve {
                prop_assert!(reported.is_none());
            } else {
                let p = reported.expect("raised step must be reported");
                prop_assert_eq!((p.curve_speed, p.applied_speed), (curve, s));
            }
        }
    }

    #[test]
    fn any_accepted_table_is_safe(
        speeds in any::<[u8; 7]>(),
        up in any::<[u8; 6]>(),
        down in any::<[u8; 6]>(),
    ) {
        if let Ok(curve) = StepCurve::new(speeds, up, down) {
            assert_safe(&curve);
        }
    }

    #[test]
    fn write_plan_keeps_kernel_rules_and_safety_at_every_step(
        current in kernel_valid_tables(),
        spec in valid_spec(),
    ) {
        let target = generate(&spec).expect("valid specs generate").curve;
        let mut state = current;
        for write in write_plan(&current, &target) {
            state.apply(&write);
            prop_assert_eq!(state.kernel_check(), Ok(()), "{:?}", state);
            let as_safe_as_current = (20..=100).all(|t| state.min_speed_at(t) >= current.min_speed_at(t));
            let as_safe_as_target = (20..=100).all(|t| state.min_speed_at(t) >= target.min_speed_at(t));
            prop_assert!(as_safe_as_current || as_safe_as_target, "{:?}", state);
        }
        prop_assert_eq!(state, target.tables());
    }

    #[test]
    fn factory_restore_plan_keeps_kernel_rules_at_every_step(
        current in kernel_valid_tables(),
    ) {
        for factory in CONF_G2_5.factory_tables.iter().flat_map(|f| [f.cpu, f.gpu]) {
            let mut state = current;
            for write in write_plan_raw(&current, &factory) {
                state.apply(&write);
                prop_assert_eq!(state.kernel_check(), Ok(()), "{:?}", state);
            }
            prop_assert_eq!(state, factory);
        }
    }

    #[test]
    fn invalid_params_are_rejected(t0 in any::<u8>(), t1 in any::<u8>(), min in any::<u8>(), max in any::<u8>(), k in any::<f64>()) {
        let valid = (20..=100).contains(&t0) && (20..=100).contains(&t1) && t0 < t1
            && min <= max && max <= 100 && (1.0..=4.0).contains(&k);
        prop_assert_eq!(FanCurveParams::new(t0, t1, min, max, k).is_ok(), valid);
    }

    #[test]
    fn s4_two_consecutive_readings_at_or_above_95_force_boost(
        hot in proptest::collection::vec(BOOST_ON_TEMP..200.0, 2),
        other in -20.0f64..200.0,
        hot_is_cpu in any::<bool>(),
        prior in proptest::collection::vec((-20.0f64..120.0, -20.0f64..120.0), 0..10),
    ) {
        let mut sup = ThermalSupervisor::new();
        for (c, g) in prior {
            sup.update(SensorReading::Celsius(c), SensorReading::Celsius(g));
        }
        let mut last = BoostDecision::Released;
        for h in hot {
            let (cpu, gpu) = if hot_is_cpu { (h, other) } else { (other, h) };
            last = sup.update(SensorReading::Celsius(cpu), SensorReading::Celsius(gpu));
        }
        prop_assert_eq!(last, BoostDecision::Forced);
    }

    #[test]
    fn s4_isolated_spikes_never_force_boost(
        base in proptest::collection::vec(20.0f64..94.9, 1..30),
        spike in BOOST_ON_TEMP..150.0,
        every in 2usize..5,
    ) {
        let mut sup = ThermalSupervisor::new();
        for (i, b) in base.iter().enumerate() {
            let cpu = if i % every == 0 { spike } else { *b };
            prop_assert_eq!(
                sup.update(SensorReading::Celsius(cpu), SensorReading::Absent),
                BoostDecision::Released
            );
        }
    }
}
