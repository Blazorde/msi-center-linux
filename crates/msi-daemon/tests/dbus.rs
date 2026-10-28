//! D-Bus round trip over a private peer-to-peer connection (a socket pair): no system bus, no polkit, no
//! hardware. The EC is the in-memory `MemoryEc` (kernel rules enforced); power supplies and PCI are a fake
//! sysfs tree.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt as _;
use msi_core::curve::CurvePreset;
use msi_core::ec::FanMode;
use msi_core::table::generate;
use msi_daemon::client::MsiCenter1Proxy;
use msi_daemon::controller::Controller;
use msi_daemon::ec::{FACTORY_CPU, MemoryEc, MemoryState};
use msi_daemon::fs::FsRoot;
use msi_daemon::gpu::GpuManager;
use msi_daemon::polkit::{Action, Authorizer, StaticAuthorizer};
use msi_daemon::power::PowerSupply;
use msi_daemon::service::{MsiCenter, OBJECT_PATH, spec_to_wire};
use msi_daemon::state::StateStore;
use msi_daemon::telemetry::Clock;
use tempfile::TempDir;
use zbus::connection::Builder;

struct Harness {
    _dir: TempDir,
    server: zbus::Connection,
    _client: zbus::Connection,
    proxy: MsiCenter1Proxy<'static>,
    ec: MemoryEc,
}

fn put(root: &std::path::Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

async fn harness_with(state: MemoryState, auth: Arc<dyn Authorizer>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    put(r, "sys/bus/pci/rescan", "");
    put(r, "sys/class/power_supply/ADP1/online", "1\n");
    put(
        r,
        "sys/class/power_supply/BAT1/charge_control_end_threshold",
        "100\n",
    );
    std::fs::create_dir_all(r.join("proc")).unwrap();
    let ec = MemoryEc::new(state);
    let root = FsRoot::new(r);
    let (ctl, _) = Controller::new(
        Box::new(ec.clone()),
        PowerSupply::new(root.clone()),
        GpuManager::new(root),
        None,
        StateStore::new(r.join("state.toml")),
    );
    let svc = MsiCenter::new(Arc::new(Mutex::new(ctl)), auth);
    let (a, b) = UnixStream::pair().unwrap();
    let server = Builder::unix_stream(a)
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at(OBJECT_PATH, svc)
        .build();
    let client = Builder::unix_stream(b).p2p().build();
    let (server, client) = tokio::try_join!(server, client).unwrap();
    let proxy = MsiCenter1Proxy::builder(&client)
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .unwrap();
    Harness {
        _dir: dir,
        server,
        _client: client,
        proxy,
        ec,
    }
}

async fn harness() -> Harness {
    harness_with(MemoryState::default(), Arc::new(StaticAuthorizer::allow_all())).await
}

fn error_name(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        other => panic!("expected a method error, got {other:?}"),
    }
}

async fn tick(h: &Harness, n: u64) {
    let iface = h
        .server
        .object_server()
        .interface::<_, MsiCenter>(OBJECT_PATH)
        .await
        .unwrap();
    iface
        .get()
        .await
        .tick(iface.signal_emitter(), Clock(n))
        .await
        .unwrap();
}

#[tokio::test]
async fn set_profile_applies_the_curves_and_updates_properties() {
    let h = harness().await;
    let p = &h.proxy;
    assert_eq!(p.profile().await.unwrap(), "balanced");
    assert_eq!(p.fan_mode().await.unwrap(), "auto");
    assert!(!p.read_only().await.unwrap());
    assert_eq!(p.firmware_version().await.unwrap(), "14K2EMS1.110");
    assert_eq!(p.charge_limit().await.unwrap(), 100);
    assert_eq!(p.gpu_state().await.unwrap(), "removed");
    assert!(p.on_ac_power().await.unwrap());

    p.set_profile("eco").await.unwrap();
    assert_eq!(p.profile().await.unwrap(), "eco");
    assert_eq!(p.fan_mode().await.unwrap(), "advanced");
    let silent = generate(&CurvePreset::Silent.spec()).unwrap().curve.tables();
    assert_eq!(h.ec.state().gpu, silent);
    assert_eq!(h.ec.state().fan_mode, FanMode::Advanced);

    // ADR-0010: the CPU keeps the factory thresholds; steps 0–3 at the live target (start speed 30 % until
    // the temperature filter is ready), hot steps floored.
    let cpu = h.ec.state().cpu;
    assert_eq!(cpu.speeds, [30, 30, 30, 50, 60, 100, 100]);
    assert_eq!((cpu.up, cpu.down), (FACTORY_CPU.up, FACTORY_CPU.down));
    let (spec, table, raised, custom) = p.fan_curve("cpu").await.unwrap();
    assert_eq!(spec, spec_to_wire(&CurvePreset::Silent.spec()));
    assert_eq!(table.0, cpu.speeds.to_vec());
    assert_eq!(table.1, cpu.up.to_vec());
    assert_eq!(table.2, cpu.down.to_vec());
    // Step 3 raised by the S2 floor of 50 % at 75 °C, step 4 by its hot floor (curve 58 % at 77 °C), step 5
    // by the S2 floor of 100 % at 85 °C (curve 78 % at 83 °C).
    assert_eq!(
        raised,
        [
            (3, 70, 30, 50, 75, 50),
            (4, 77, 58, 60, 77, 60),
            (5, 83, 78, 100, 85, 100)
        ]
    );
    assert!(!custom);
    let (_, table, raised, _) = p.fan_curve("gpu").await.unwrap();
    assert_eq!(table.0, silent.speeds.to_vec());
    // Silent reaches 100 % at 88 °C: S2 raises step 6 (entered at 85 °C) from 86 to 100 %.
    assert_eq!(raised, [(6, 85, 86, 100, 85, 100)]);
}

#[tokio::test]
async fn custom_curve_is_applied_and_raised_steps_are_reported() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_profile("balanced").await.unwrap();
    // Flat 30 % curve with late thresholds: S2 raises steps and says so.
    let spec = (0, 70, 90, 30, 30, 1.0, vec![70, 76, 80, 86, 90, 95], 4);
    let (preview, preview_raised) = p.preview_fan_curve(&spec).await.unwrap();
    let (table, raised) = p.set_fan_curve("gpu", &spec).await.unwrap();
    assert_eq!((&table, &raised), (&preview, &preview_raised));
    // Step 0 is in effect at 60 °C → raised to 30 %; at 75 °C step 1 → 50 %; at 85 °C step 3 → 100 %.
    assert_eq!(table.0, vec![30, 50, 50, 100, 100, 100, 100]);
    assert_eq!(raised[0], (0, 0, 0, 30, 60, 30));
    assert_eq!(h.ec.state().gpu.speeds.to_vec(), table.0);
    assert!(p.fan_curve("gpu").await.unwrap().3);
    p.reset_fan_curve("gpu").await.unwrap();
    assert!(!p.fan_curve("gpu").await.unwrap().3);
}

#[tokio::test]
async fn invalid_arguments_are_rejected_before_anything_is_written() {
    let h = harness().await;
    let p = &h.proxy;
    for e in [
        p.set_profile("turbo").await.unwrap_err(),
        p.set_fan_profile("max").await.unwrap_err(),
        p.set_fan_profile("Turbo").await.unwrap_err(),
        p.custom_fan_curve("fan3").await.unwrap_err(),
        p.fan_curve("fan3").await.unwrap_err(),
        p.set_charge_limit(49).await.unwrap_err(),
        p.set_fan_curve("cpu", &(0, 50, 40, 30, 100, 2.0, vec![50, 57, 64, 70, 76, 82], 4))
            .await
            .unwrap_err(),
        p.preview_fan_curve(&(0, 50, 82, 30, 100, 2.0, vec![50, 57], 4))
            .await
            .unwrap_err(),
    ] {
        assert_eq!(
            error_name(&e),
            "org.blazorde.MsiCenter1.Error.InvalidArgument",
            "{e}"
        );
    }
    assert_eq!(h.ec.state().writes.len(), 0);
}

#[tokio::test]
async fn mutating_calls_need_authorization() {
    let h = harness_with(
        MemoryState::default(),
        Arc::new(StaticAuthorizer::new(&[Action::SetBattery])),
    )
    .await;
    let p = &h.proxy;
    for e in [
        p.set_profile("gaming").await.unwrap_err(),
        p.set_auto_profile_on_power(true).await.unwrap_err(),
        p.set_cooler_boost(true).await.unwrap_err(),
        p.reset_fan_curve("cpu").await.unwrap_err(),
        p.set_cpu_stages(0, &[(50, 46, 30)]).await.unwrap_err(),
        p.reset_cpu_stages().await.unwrap_err(),
        p.set_fan_profile("auto").await.unwrap_err(),
        p.restore_factory().await.unwrap_err(),
    ] {
        assert_eq!(
            error_name(&e),
            "org.blazorde.MsiCenter1.Error.NotAuthorized",
            "{e}"
        );
    }
    assert_eq!(h.ec.state().fan_mode_writes.len(), 0);
    // Allowed action and read-only calls work.
    p.set_charge_limit(80).await.unwrap();
    assert_eq!(p.charge_limit().await.unwrap(), 80);
    assert!(p.fan_curve("cpu").await.is_ok());
    assert!(p.cpu_stages().await.is_ok());
    assert!(p.preview_cpu_stages(0, &[(50, 46, 30)]).await.is_ok());
    assert_eq!(p.gpu_holders().await.unwrap(), vec![]);
}

#[tokio::test]
async fn s8_read_only_daemon_refuses_writes() {
    let h = harness_with(
        MemoryState {
            fw_version: "14K2EMS1.200".into(),
            ..MemoryState::default()
        },
        Arc::new(StaticAuthorizer::allow_all()),
    )
    .await;
    let p = &h.proxy;
    assert!(p.read_only().await.unwrap());
    assert!(p.read_only_reason().await.unwrap().contains("14K2EMS1.200"));
    let e = p.set_profile("gaming").await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.MsiCenter1.Error.ReadOnly");
    assert_eq!(h.ec.state().writes.len(), 0);
}

#[tokio::test]
async fn s7_readback_mismatch_is_a_safety_fallback_with_an_event() {
    let h = harness().await;
    let p = &h.proxy;
    let mut events = p.receive_safety_event().await.unwrap();
    h.ec.state().corrupt_readback = Some(msi_daemon::ec::Fan::Cpu);
    let e = p.set_profile("gaming").await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.MsiCenter1.Error.SafetyFallback");
    let ev = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ev.args().unwrap().kind, "readback_mismatch");
    assert_eq!(p.fan_mode().await.unwrap(), "auto");
}

#[tokio::test]
async fn s4_tick_forces_cooler_boost_and_emits_signals() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_profile("balanced").await.unwrap();
    let mut telemetry = p.receive_telemetry().await.unwrap();
    let mut events = p.receive_safety_event().await.unwrap();
    h.ec.state().cpu_temp = Some(97);
    tick(&h, 1).await;
    tick(&h, 2).await;
    let ev = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .unwrap()
        .unwrap();
    let args = ev.args().unwrap();
    assert_eq!((args.kind.as_str(), args.value), ("boost_forced", 97.0));
    assert!(p.boost_forced().await.unwrap());
    assert!(p.cooler_boost().await.unwrap());
    let e = p.set_cooler_boost(false).await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.MsiCenter1.Error.InvalidArgument");

    let t = tokio::time::timeout(Duration::from_secs(5), telemetry.next())
        .await
        .unwrap()
        .unwrap();
    let data = t.args().unwrap().data;
    assert!((f64::try_from(&*data["cpu_temp"]).unwrap() - 97.0).abs() < 1e-9);
    assert_eq!(u8::try_from(&*data["fan1_pct"]).unwrap(), 100);
    // ADR-0010: live CPU target (start speed until the filter has three readings).
    assert_eq!(u8::try_from(&*data["cpu_fan_target_pct"]).unwrap(), 30);
}

#[tokio::test]
async fn introspection_matches_documented_signatures() {
    let h = harness().await;
    let intro = zbus::fdo::IntrospectableProxy::builder(h.proxy.inner().connection())
        .destination("org.blazorde.MsiCenter1")
        .path(OBJECT_PATH)
        .build()
        .await
        .unwrap();
    let xml = intro.introspect().await.unwrap();
    for fragment in [
        r#"<interface name="org.blazorde.MsiCenter1">"#,
        r#"<method name="SetProfile">"#,
        r#"<arg name="profile" type="s" direction="in"/>"#,
        r#"<method name="SetAutoProfileOnPower">"#,
        r#"<method name="FanCurve">"#,
        r#"<arg name="spec" type="(yyyyydayy)" direction="out"/>"#,
        r#"<arg name="table" type="(ayayay)" direction="out"/>"#,
        r#"<arg name="raised" type="a(yyyyyy)" direction="out"/>"#,
        r#"<arg name="custom" type="b" direction="out"/>"#,
        r#"<method name="SetFanCurve">"#,
        r#"<arg name="spec" type="(yyyyydayy)" direction="in"/>"#,
        r#"<method name="ResetFanCurve">"#,
        r#"<method name="PreviewFanCurve">"#,
        r#"<method name="SetCoolerBoost">"#,
        r#"<arg name="enabled" type="b" direction="in"/>"#,
        r#"<method name="RestoreFactory">"#,
        r#"<method name="SetChargeLimit">"#,
        r#"<arg name="limit" type="y" direction="in"/>"#,
        r#"<method name="GpuHolders">"#,
        r#"<method name="SetFanProfile">"#,
        r#"<arg name="fan_profile" type="s" direction="in"/>"#,
        r#"<method name="CustomFanCurve">"#,
        r#"<method name="CpuStages">"#,
        r#"<arg name="stages" type="a(yyy)" direction="out"/>"#,
        r#"<arg name="floors" type="a(yyy)" direction="out"/>"#,
        r#"<arg name="supported" type="b" direction="out"/>"#,
        r#"<method name="SetCpuStages">"#,
        r#"<arg name="idle" type="y" direction="in"/>"#,
        r#"<arg name="stages" type="a(yyy)" direction="in"/>"#,
        r#"<method name="ResetCpuStages">"#,
        r#"<method name="PreviewCpuStages">"#,
        r#"<property name="FanProfile" type="s" access="read"/>"#,
        r#"<property name="DefaultFanProfile" type="s" access="read"/>"#,
        r#"<arg name="holders" type="a(us)" direction="out"/>"#,
        r#"<signal name="Telemetry">"#,
        r#"<arg name="data" type="a{sv}"/>"#,
        r#"<signal name="SafetyEvent">"#,
        r#"<arg name="kind" type="s"/>"#,
        r#"<arg name="value" type="d"/>"#,
        r#"<arg name="detail" type="s"/>"#,
        r#"<property name="Profile" type="s" access="read"/>"#,
        r#"<property name="AutoProfileOnPower" type="b" access="read"/>"#,
        r#"<property name="OnAcPower" type="b" access="read"/>"#,
        r#"<property name="FanMode" type="s" access="read"/>"#,
        r#"<property name="CoolerBoost" type="b" access="read"/>"#,
        r#"<property name="BoostForced" type="b" access="read"/>"#,
        r#"<property name="ChargeLimit" type="y" access="read"/>"#,
        r#"<property name="GpuState" type="s" access="read"/>"#,
        r#"<property name="ReadOnly" type="b" access="read"/>"#,
        r#"<property name="ReadOnlyReason" type="s" access="read"/>"#,
        r#"<property name="FirmwareVersion" type="s" access="read"/>"#,
        r#"<property name="CpuPowerLimits" type="(uu)" access="read"/>"#,
    ] {
        assert!(xml.contains(fragment), "missing {fragment}\n{xml}");
    }
    let start = xml.find(r#"<interface name="org.blazorde.MsiCenter1">"#).unwrap();
    let iface = &xml[start..start + xml[start..].find("</interface>").unwrap()];
    assert_eq!(iface.matches("<method ").count(), 16, "{iface}");
    assert_eq!(iface.matches("<signal ").count(), 2, "{iface}");
    assert_eq!(iface.matches("<property ").count(), 14, "{iface}");
}

#[tokio::test]
async fn restore_factory_hands_control_back_to_the_ec() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_profile("gaming").await.unwrap();
    assert_eq!(p.fan_mode().await.unwrap(), "advanced");
    p.restore_factory().await.unwrap();
    assert_eq!(p.fan_mode().await.unwrap(), "auto");
    let s = h.ec.state();
    assert_eq!(s.cpu, msi_daemon::ec::FACTORY_CPU);
    assert_eq!(s.gpu, msi_daemon::ec::FACTORY_GPU);
}

#[tokio::test]
async fn s9_fan_stall_sets_read_only_and_emits_fan_stalled() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_profile("eco").await.unwrap();
    let mut events = p.receive_safety_event().await.unwrap();
    {
        let mut s = h.ec.state();
        s.cpu_temp = Some(78);
        s.fan_speed_override = [Some(0), None];
    }
    for t in 0..=31 {
        tick(&h, t * 1_000_000_000).await;
    }
    let ev = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .unwrap()
        .unwrap();
    let args = ev.args().unwrap();
    assert_eq!((args.kind.as_str(), args.value), ("fan_stalled", 78.0));
    assert!(p.read_only().await.unwrap());
    assert!(p.read_only_reason().await.unwrap().starts_with("S9 fan stall"));
    assert_eq!(p.fan_mode().await.unwrap(), "auto");
    let e = p.set_profile("gaming").await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.MsiCenter1.Error.ReadOnly");
}

#[tokio::test]
async fn fan_profiles_switch_persist_per_profile_and_signal_changes() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_profile("gaming").await.unwrap();
    assert_eq!(p.fan_profile().await.unwrap(), "turbo");
    assert_eq!(p.default_fan_profile().await.unwrap(), "turbo");
    let props = zbus::fdo::PropertiesProxy::builder(p.inner().connection())
        .destination("org.blazorde.MsiCenter1")
        .path(OBJECT_PATH)
        .build()
        .await
        .unwrap();
    let mut stream = props.receive_properties_changed().await.unwrap();
    p.set_fan_profile("auto").await.unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let c = stream.next().await.unwrap();
            let args = c.args().unwrap();
            if let Some(v) = args.changed_properties().get("FanProfile") {
                return <&str>::try_from(v).unwrap().to_owned();
            }
        }
    })
    .await;
    assert_eq!(
        changed.as_deref(),
        Ok("auto"),
        "no PropertiesChanged for FanProfile"
    );
    assert_eq!(p.fan_profile().await.unwrap(), "auto");
    assert_eq!(p.fan_mode().await.unwrap(), "auto");
    let (_, table, raised, custom) = p.fan_curve("cpu").await.unwrap();
    assert_eq!(table.0, FACTORY_CPU.speeds.to_vec());
    assert!(raised.is_empty() && !custom);
    // Another profile has its own choice; coming back restores Auto.
    p.set_profile("eco").await.unwrap();
    assert_eq!(p.fan_profile().await.unwrap(), "silent");
    assert_eq!(p.default_fan_profile().await.unwrap(), "silent");
    assert_eq!(p.fan_mode().await.unwrap(), "advanced");
    p.set_profile("gaming").await.unwrap();
    assert_eq!(p.fan_profile().await.unwrap(), "auto");
    assert_eq!(p.fan_mode().await.unwrap(), "auto");
}

#[tokio::test]
async fn custom_fan_curve_is_the_editor_of_the_custom_fan_profile() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_profile("eco").await.unwrap();
    // Not edited yet: the preset of the default fan profile, with its step table.
    let (spec, table, raised) = p.custom_fan_curve("gpu").await.unwrap();
    assert_eq!(spec, spec_to_wire(&CurvePreset::Silent.spec()));
    let silent = generate(&CurvePreset::Silent.spec()).unwrap();
    assert_eq!(table.0, silent.curve.speeds().to_vec());
    assert_eq!(raised.len(), 1, "Silent: step 6 raised by S2");
    // SetFanCurve stores it and switches to Custom.
    let quiet = (0, 45, 80, 30, 100, 2.5, vec![45, 52, 60, 67, 73, 80], 3);
    p.set_fan_curve("gpu", &quiet).await.unwrap();
    assert_eq!(p.fan_profile().await.unwrap(), "custom");
    assert_eq!(p.custom_fan_curve("gpu").await.unwrap().0, quiet);
    // Picking a preset keeps the custom curve for later.
    p.set_fan_profile("turbo").await.unwrap();
    assert_eq!(p.custom_fan_curve("gpu").await.unwrap().0, quiet);
    assert!(!p.fan_curve("gpu").await.unwrap().3);
    p.set_fan_profile("custom").await.unwrap();
    assert_eq!(p.fan_curve("gpu").await.unwrap().0, quiet);
}

#[tokio::test]
async fn invalid_custom_curve_under_auto_changes_nothing() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_fan_profile("auto").await.unwrap();
    let writes = h.ec.state().writes.len();
    let modes = h.ec.state().fan_mode_writes.len();
    let e = p
        .set_fan_curve("gpu", &(0, 50, 40, 30, 100, 2.0, vec![50, 57, 64, 70, 76, 82], 4))
        .await
        .unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.MsiCenter1.Error.InvalidArgument");
    assert_eq!(h.ec.state().writes.len(), writes);
    assert_eq!(h.ec.state().fan_mode_writes.len(), modes);
    assert_eq!(p.fan_profile().await.unwrap(), "auto");
    assert_eq!(h.ec.state().fan_mode, FanMode::Auto);
}

// ---- ADR-0013: staged CPU curve ----

/// 4 stages: 50 °C 30 %, 60 °C 45 %, 70 °C 70 %, 80 °C 100 %, each falling back 4 °C under its rise.
fn four_stages() -> Vec<(u8, u8, u8)> {
    vec![(50, 46, 30), (60, 56, 45), (70, 66, 70), (80, 76, 100)]
}

#[tokio::test]
async fn staged_cpu_curve_round_trips_and_drives_the_cpu_live() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_profile("eco").await.unwrap();
    // Not edited yet: the 6 stages of the preset, not saved, not in force.
    let (idle, stages, floors, saved, active, supported) = p.cpu_stages().await.unwrap();
    assert_eq!(stages.len(), 6);
    assert_eq!(idle, 0);
    assert!(floors.is_empty() && !saved && !active && supported);

    // Preview validates and reports the floors without writing.
    let writes = h.ec.state().writes.len();
    let quiet = [(40, 36, 20), (90, 80, 100)];
    assert_eq!(
        p.preview_cpu_stages(0, &quiet).await.unwrap(),
        [(60, 30, 20), (75, 50, 20), (85, 100, 20)]
    );
    assert_eq!(h.ec.state().writes.len(), writes);

    // Set: saved, custom fan profile, in force, and the CPU table keeps the factory thresholds.
    let floors = p.set_cpu_stages(0, &four_stages()).await.unwrap();
    assert!(floors.is_empty());
    assert_eq!(p.fan_profile().await.unwrap(), "custom");
    let (idle, stages, _, saved, active, _) = p.cpu_stages().await.unwrap();
    assert_eq!((idle, stages, saved, active), (0, four_stages(), true, true));
    let cpu = h.ec.state().cpu;
    assert_eq!((cpu.up, cpu.down), (FACTORY_CPU.up, FACTORY_CPU.down));
    assert_eq!(cpu.speeds, [30, 30, 30, 50, 70, 100, 100]);

    // Live: 65 °C → stage 1 (45 %), reported by the telemetry signal.
    h.ec.state().cpu_temp = Some(65);
    for n in 1..=30 {
        tick(&h, n * 1_000_000_000).await;
    }
    assert_eq!(h.ec.state().cpu.speeds[0], 45);
    // The GPU still has its own 7-step table.
    let silent = generate(&CurvePreset::Silent.spec()).unwrap().curve.tables();
    assert_eq!(h.ec.state().gpu, silent);

    // Reset: the CPU follows the exponential curve again, the profile goes back to Silencieux.
    p.reset_cpu_stages().await.unwrap();
    assert_eq!(p.fan_profile().await.unwrap(), "silent");
    let (_, _, _, saved, active, _) = p.cpu_stages().await.unwrap();
    assert!(!saved && !active);
}

#[tokio::test]
async fn invalid_staged_curves_are_rejected_before_anything_is_written() {
    let h = harness().await;
    let p = &h.proxy;
    p.set_profile("balanced").await.unwrap();
    let writes = h.ec.state().writes.len();
    let too_many = vec![(50u8, 46u8, 30u8); 80];
    for (idle, stages) in [
        (0, vec![]),
        (0, vec![(50, 50, 30)]),
        (0, vec![(50, 49, 30)]),
        (0, vec![(60, 56, 45), (50, 46, 30)]),
        (0, vec![(50, 46, 40), (60, 56, 30)]),
        (0, vec![(50, 46, 101)]),
        (31, vec![(50, 46, 30)]),
        (0, too_many),
    ] {
        for e in [
            p.set_cpu_stages(idle, &stages).await.unwrap_err(),
            p.preview_cpu_stages(idle, &stages).await.unwrap_err(),
        ] {
            assert_eq!(
                error_name(&e),
                "org.blazorde.MsiCenter1.Error.InvalidArgument",
                "{idle} {stages:?}: {e}"
            );
        }
    }
    assert_eq!(h.ec.state().writes.len(), writes);
    assert_eq!(p.fan_profile().await.unwrap(), "balanced");
}

#[tokio::test]
async fn staged_curves_are_refused_without_factory_tables_and_in_read_only_mode() {
    // Firmware without known factory tables: no live CPU control, so no staged curve.
    let h = harness_with(
        MemoryState {
            fw_version: "14K2EMS1.109".into(),
            ..MemoryState::default()
        },
        Arc::new(StaticAuthorizer::allow_all()),
    )
    .await;
    h.proxy.set_profile("eco").await.unwrap();
    assert!(!h.proxy.cpu_stages().await.unwrap().5);
    let e = h.proxy.set_cpu_stages(0, &four_stages()).await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.MsiCenter1.Error.InvalidArgument");
    // Unsupported EC (S8): read-only.
    let h = harness_with(
        MemoryState {
            fw_version: "14K2EMS1.200".into(),
            ..MemoryState::default()
        },
        Arc::new(StaticAuthorizer::allow_all()),
    )
    .await;
    let e = h.proxy.set_cpu_stages(0, &four_stages()).await.unwrap_err();
    assert_eq!(error_name(&e), "org.blazorde.MsiCenter1.Error.ReadOnly");
    assert_eq!(h.ec.state().writes.len(), 0);
}
