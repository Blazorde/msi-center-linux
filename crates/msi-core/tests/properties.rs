//! Property tests for the safety invariants of `docs/hardware/fan-safety.md`.

// Test-only file: strategy helpers are not `#[test]` fns, so clippy does not apply `allow-expect-in-tests`.
#![allow(clippy::expect_used)]

use msi_core::curve::FanCurveParams;
use msi_core::supervisor::{BOOST_ON_TEMP, BoostDecision, SensorReading, ThermalSupervisor};
use msi_core::table::{CurveLayout, CurveTable, SPEED_FLOORS, sample};
use proptest::prelude::*;

fn valid_params() -> impl Strategy<Value = FanCurveParams> {
    (20u8..100, 0u8..=100, 1.0f64..=4.0)
        .prop_flat_map(|(t0, min, k)| (Just(t0), t0 + 1..=100, Just(min), min..=100, Just(k)))
        .prop_map(|(t0, t1, min, max, k)| {
            FanCurveParams::new(t0, t1, min, max, k).expect("strategy only builds valid params")
        })
}

fn valid_layout() -> impl Strategy<Value = CurveLayout> {
    // 2..=16 distinct sorted temperatures in 0..=100, the first one at or below 60 °C.
    (0u8..=60, proptest::collection::btree_set(0u8..=100, 1..=15)).prop_map(|(first, rest)| {
        let mut temps: Vec<u8> = rest.into_iter().filter(|&t| t > first).collect();
        temps.insert(0, first);
        if temps.len() < 2 {
            temps.push(100);
        }
        CurveLayout::new(temps).expect("strategy only builds valid layouts")
    })
}

fn assert_safe(table: &CurveTable) {
    assert!(table.speeds().iter().all(|&s| s <= 100), "S3: {table:?}");
    assert!(
        table.temps().windows(2).all(|w| w[0] < w[1]),
        "S1 temps: {table:?}"
    );
    assert!(
        table.speeds().windows(2).all(|w| w[0] <= w[1]),
        "S1 speeds: {table:?}"
    );
    for floor in SPEED_FLOORS {
        for temp in floor.temp..=100 {
            assert!(
                table.effective_speed(temp) >= floor.speed,
                "S2 at {temp} °C: {table:?}"
            );
        }
    }
}

proptest! {
    #[test]
    fn sampled_provisional_table_is_safe(params in valid_params()) {
        let sampled = sample(&params, &CurveLayout::default()).expect("valid inputs sample");
        assert_safe(&sampled.table);
    }

    #[test]
    fn sampled_table_is_safe_on_any_layout(params in valid_params(), layout in valid_layout()) {
        let sampled = sample(&params, &layout).expect("valid inputs sample");
        prop_assert_eq!(sampled.table.temps(), layout.temps());
        assert_safe(&sampled.table);
    }

    #[test]
    fn floors_only_raise_and_every_change_is_reported(params in valid_params(), layout in valid_layout()) {
        let sampled = sample(&params, &layout).expect("valid inputs sample");
        for (i, (&t, &s)) in sampled.table.temps().iter().zip(sampled.table.speeds()).enumerate() {
            let curve = params.speed_percent(t);
            prop_assert!(s >= curve);
            let reported = sampled.report.raised.iter().find(|p| p.index == i);
            if s == curve {
                prop_assert!(reported.is_none());
            } else {
                let p = reported.expect("raised point must be reported");
                prop_assert_eq!((p.temp, p.curve_speed, p.applied_speed), (t, curve, s));
            }
        }
    }

    #[test]
    fn any_accepted_table_is_safe(
        temps in proptest::collection::vec(any::<u8>(), 0..20),
        speeds in proptest::collection::vec(any::<u8>(), 0..20),
    ) {
        if let Ok(table) = CurveTable::new(temps, speeds) {
            assert_safe(&table);
        }
    }

    #[test]
    fn invalid_params_are_rejected(t0 in any::<u8>(), t1 in any::<u8>(), min in any::<u8>(), max in any::<u8>(), k in any::<f64>()) {
        let valid = (20..=100).contains(&t0) && (20..=100).contains(&t1) && t0 < t1
            && min <= max && max <= 100 && (1.0..=4.0).contains(&k);
        prop_assert_eq!(FanCurveParams::new(t0, t1, min, max, k).is_ok(), valid);
    }

    #[test]
    fn s4_any_reading_at_or_above_95_forces_boost(
        hot in BOOST_ON_TEMP..200.0,
        other in -20.0f64..200.0,
        hot_is_cpu in any::<bool>(),
        prior in proptest::collection::vec((-20.0f64..120.0, -20.0f64..120.0), 0..10),
    ) {
        let mut sup = ThermalSupervisor::new();
        for (c, g) in prior {
            sup.update(SensorReading::Celsius(c), SensorReading::Celsius(g));
        }
        let (cpu, gpu) = if hot_is_cpu { (hot, other) } else { (other, hot) };
        prop_assert_eq!(
            sup.update(SensorReading::Celsius(cpu), SensorReading::Celsius(gpu)),
            BoostDecision::Forced
        );
    }
}
