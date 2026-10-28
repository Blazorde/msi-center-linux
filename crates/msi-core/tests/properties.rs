//! Property tests for the safety invariants of `docs/hardware/fan-safety.md`.

// Test-only file: strategy helpers are not `#[test]` fns, so clippy does not apply `allow-expect-in-tests`.
#![allow(clippy::expect_used)]

use msi_core::curve::{FanCurveParams, FanCurveSpec};
use msi_core::ec::{CONF_G2_5, FACTORY_14K2EMS1_110};
use msi_core::hybrid::{
    CpuCurve, CpuFanPilot, FIRST_HOT_STEP, HOT_STEP_FLOORS, SLEW_DOWN_PCT, SLEW_UP_PCT, TempFilter,
    hybrid_cpu_table, hybrid_cpu_table_for,
};
use msi_core::staged::{MAX_STAGES, Stage, StageSelector, StagedCurve, StagedPilot, safety_floor};
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

/// A valid staged curve with 1 to 24 stages and any fall-back gap.
fn valid_staged() -> impl Strategy<Value = StagedCurve> {
    (1usize..=24)
        .prop_flat_map(|n| {
            (
                proptest::collection::btree_set(22u8..=100, n),
                proptest::collection::vec(2u8..=60, n),
                proptest::collection::vec(0u8..=100, n),
            )
        })
        .prop_flat_map(|(ups, gaps, mut speeds)| {
            speeds.sort_unstable();
            let first = speeds[0];
            (Just((ups, gaps, speeds)), 0..=first)
        })
        .prop_map(|((ups, gaps, speeds), idle)| {
            // down[i] = up[i] − gap, kept above the previous down (strictly increasing), ≥ 20 and ≤ up − 2.
            let mut prev_down = 19u8;
            let stages: Vec<Stage> = ups
                .into_iter()
                .zip(gaps)
                .zip(speeds)
                .map(|((up, gap), speed)| {
                    let down = up.saturating_sub(gap).max(prev_down + 1).min(up - 2);
                    prev_down = down;
                    Stage { up, down, speed }
                })
                .collect();
            StagedCurve::new(idle, stages).expect("strategy only builds valid staged curves")
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

    #[test]
    fn hybrid_cpu_vector_is_always_valid_and_safe(spec in valid_spec(), target in 0u8..=100) {
        let factory = FACTORY_14K2EMS1_110.cpu;
        let target = target.clamp(spec.idle(), spec.params().max());
        let g = hybrid_cpu_table(&spec, &factory, target).expect("every target in idle..=max is writable");
        let c = g.curve;
        // Factory thresholds kept, kernel rules, S1–S3, S2 on the ascending step at every temperature.
        prop_assert_eq!((c.up(), c.down()), (factory.up, factory.down));
        assert_safe(&c);
        let s = c.speeds();
        prop_assert!(s[6] >= 80);
        // Steps 0–3 follow the target (raised only by S2), hot steps never below max(curve, 60/80/100).
        prop_assert_eq!(s[0], target);
        prop_assert!(s[1..FIRST_HOT_STEP].iter().all(|&v| v >= target));
        for (h, &floor) in HOT_STEP_FLOORS.iter().enumerate() {
            let i = FIRST_HOT_STEP + h;
            prop_assert!(s[i] >= floor.max(spec.params().speed_percent(factory.up[i - 1])).max(target));
        }
        // S2 for the ascending step under the factory thresholds.
        for floor in SPEED_FLOORS {
            prop_assert!(c.min_speed_at(floor.temp) >= floor.speed);
        }
    }

    #[test]
    fn hybrid_pilot_output_stays_in_range_and_moves_gently(
        spec in valid_spec(),
        temps in proptest::collection::vec(proptest::option::of(0u8..=120), 1..200),
    ) {
        let mut filter = TempFilter::new();
        let mut pilot = CpuFanPilot::new();
        let factory = FACTORY_14K2EMS1_110.cpu;
        let (idle, min, max) = (spec.idle(), spec.params().min(), spec.params().max());
        let mut prev: Option<u8> = None;
        for (i, t) in temps.into_iter().enumerate() {
            let out = pilot.update(&spec, filter.update(t), i as u64 * 1000);
            prop_assert!((idle..=max).contains(&out), "{} not in {}..={}", out, idle, max);
            prop_assert!(hybrid_cpu_table(&spec, &factory, out).is_ok());
            if let Some(p) = prev {
                // Only two jumps are allowed: stopping (to idle) and starting (from idle to the start speed).
                let gentle = out <= p.saturating_add(SLEW_UP_PCT.max(min.saturating_sub(p)))
                    && (out >= p.saturating_sub(SLEW_DOWN_PCT) || out == idle);
                prop_assert!(gentle, "{} -> {}", p, out);
            }
            prev = Some(out);
        }
    }

    #[test]
    fn hybrid_filter_ignores_isolated_spikes(
        base in 30u8..=80,
        spike in 0u8..=120,
        every in 3usize..8,
        n in 10usize..60,
    ) {
        let mut f = TempFilter::new();
        for i in 0..n {
            let t = if i % every == every - 1 { spike } else { base };
            if let Some(v) = f.update(Some(t)) {
                prop_assert!((v - f64::from(base)).abs() < 1e-9, "{} vs {}", v, base);
            }
        }
    }

    // ---- Staged curves (ADR-0013) ----

    #[test]
    fn staged_accepted_curves_respect_s1_and_s3(
        idle in 0u8..=105,
        raw in proptest::collection::vec((18u8..=32, 16u8..=32, 0u8..=105), 0..5),
    ) {
        let stages: Vec<Stage> = raw.into_iter().map(|(up, down, speed)| Stage { up, down, speed }).collect();
        if let Ok(c) = StagedCurve::new(idle, stages) {
            let s = c.stages();
            prop_assert!(!s.is_empty() && s.len() <= MAX_STAGES);
            prop_assert!(c.idle() <= s[0].speed);
            for st in s {
                prop_assert!((20..=100).contains(&st.up) && (20..=100).contains(&st.down));
                prop_assert!(st.up - st.down >= 2 && st.speed <= 100);
            }
            for w in s.windows(2) {
                prop_assert!(w[1].up > w[0].up && w[1].down > w[0].down && w[1].speed >= w[0].speed);
            }
        }
    }

    #[test]
    fn staged_effective_speed_meets_s2_never_decreases_and_floors_are_reported(c in valid_staged()) {
        let mut prev = 0;
        for temp in 0..=120u8 {
            let s = c.effective_speed(temp);
            prop_assert!(s >= prev, "decreases at {} °C", temp);
            prop_assert!(s >= c.ascending_speed(temp) && s >= safety_floor(f64::from(temp)));
            prev = s;
        }
        for floor in SPEED_FLOORS {
            prop_assert!(c.effective_speed(floor.temp) >= floor.speed);
        }
        let raised = c.raised_floors();
        for floor in SPEED_FLOORS {
            let below = c.ascending_speed(floor.temp) < floor.speed;
            prop_assert_eq!(raised.iter().any(|r| r.floor == floor), below, "{:?}", floor);
        }
    }

    #[test]
    fn staged_selector_is_consistent_and_never_below_the_ascending_stage(
        c in valid_staged(),
        temps in proptest::collection::vec(0u8..=120, 1..200),
    ) {
        let mut sel = StageSelector::new();
        for t in temps {
            let at = f64::from(t);
            let stage = sel.update(&c, at);
            let stages = c.stages();
            match stage {
                // In force: not under its fall-back, and the next stage is not yet reached.
                Some(i) => {
                    prop_assert!(at >= f64::from(stages[i].down), "{} under down of {}", t, i);
                    if let Some(next) = stages.get(i + 1) {
                        prop_assert!(at < f64::from(next.up), "{} reached stage {}", t, i + 1);
                    }
                }
                None => prop_assert!(at < f64::from(stages[0].up)),
            }
            // Cooling can only keep a stage longer than heating would: never lower than the ascending one.
            prop_assert!(stage >= c.ascending_stage(t), "{} °C: {:?} < {:?}", t, stage, c.ascending_stage(t));
        }
    }

    #[test]
    fn staged_pilot_output_stays_in_range_and_moves_gently(
        c in valid_staged(),
        temps in proptest::collection::vec(proptest::option::of(0u8..=120), 1..200),
    ) {
        let mut filter = TempFilter::new();
        let mut pilot = StagedPilot::new();
        let (start, factory) = (c.start_speed(), FACTORY_14K2EMS1_110.cpu);
        let mut prev: Option<u8> = None;
        for t in temps {
            let filtered = filter.update(t);
            let out = pilot.update(&c, filtered);
            prop_assert!(out <= 100);
            // Same writable, S2-safe vector as the exponential pilot, for any output.
            let g = hybrid_cpu_table_for(&CpuCurve::Staged(c.clone()), &factory, out.clamp(c.idle(), c.max_speed()))
                .expect("every output is writable");
            assert_safe(&g.curve);
            if let (Some(p), Some(temp)) = (prev, filtered) {
                let stage = pilot.stage();
                let target = stage.map_or(c.idle(), |i| c.stages()[i].speed).max(safety_floor(temp));
                if target >= start {
                    // Only a start (from under the first stage's speed) may jump up.
                    prop_assert!(out <= p.saturating_add(SLEW_UP_PCT).max(start), "{} -> {}", p, out);
                    prop_assert!(out >= p.saturating_sub(SLEW_DOWN_PCT), "{} -> {}", p, out);
                } else {
                    // Idle or floor-only: follows the target directly.
                    prop_assert_eq!(out, target);
                }
            }
            prev = Some(out);
        }
    }

    #[test]
    fn staged_hybrid_vector_is_always_valid_and_safe(c in valid_staged(), target in 0u8..=100) {
        let factory = FACTORY_14K2EMS1_110.cpu;
        let target = target.clamp(c.idle(), c.max_speed());
        let curve = CpuCurve::Staged(c);
        let g = hybrid_cpu_table_for(&curve, &factory, target).expect("every target in idle..=max is writable");
        let t = g.curve;
        prop_assert_eq!((t.up(), t.down()), (factory.up, factory.down));
        assert_safe(&t);
        let s = t.speeds();
        prop_assert_eq!(s[0], target);
        prop_assert!(s[6] >= 80);
        for (h, &floor) in HOT_STEP_FLOORS.iter().enumerate() {
            let i = FIRST_HOT_STEP + h;
            prop_assert!(s[i] >= floor.max(curve.speed_percent(factory.up[i - 1])).max(target));
        }
    }
}
