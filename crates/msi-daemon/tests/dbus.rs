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
    assert_eq!(f64::try_from(&*data["cpu_temp"]).unwrap(), 97.0);
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
    ] {
        assert!(xml.contains(fragment), "missing {fragment}\n{xml}");
    }
    let start = xml.find(r#"<interface name="org.blazorde.MsiCenter1">"#).unwrap();
    let iface = &xml[start..start + xml[start..].find("</interface>").unwrap()];
    assert_eq!(iface.matches("<method ").count(), 12, "{iface}");
    assert_eq!(iface.matches("<signal ").count(), 2, "{iface}");
    assert_eq!(iface.matches("<property ").count(), 13, "{iface}");
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
