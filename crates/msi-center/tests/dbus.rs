//! Headless test of the app's D-Bus layer against the real `msi-daemon` and `clipd` interfaces, served on
//! private peer-to-peer connections (socket pairs): no bus, no polkit, no hardware, no display. The EC is the
//! daemon's in-memory `MemoryEc`; clipd stores in a temporary directory.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clip_core::{Library, Limits};
use futures_util::StreamExt as _;
use msi_center::curve::CurveSpec;
use msi_center::dbus::{self, Clips, Daemon, DaemonEvent};
use msi_center::errors::AppError;
use msi_center::model::{Fan, Scope};
use msi_core::curve::CurvePreset;
use msi_daemon::controller::Controller;
use msi_daemon::ec::{MemoryEc, MemoryState};
use msi_daemon::fs::FsRoot;
use msi_daemon::gpu::GpuManager;
use msi_daemon::polkit::{Action, Authorizer, StaticAuthorizer};
use msi_daemon::power::PowerSupply;
use msi_daemon::service::{MsiCenter, OBJECT_PATH};
use msi_daemon::state::StateStore;
use msi_daemon::telemetry::Clock;
use tempfile::TempDir;
use zbus::connection::Builder;

struct DaemonHarness {
    _dir: TempDir,
    server: zbus::Connection,
    _client: zbus::Connection,
    daemon: Daemon,
    ec: MemoryEc,
}

fn put(root: &std::path::Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

async fn daemon_with(state: MemoryState, auth: Arc<dyn Authorizer>) -> DaemonHarness {
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
    let daemon = Daemon::new(&client).await.unwrap();
    DaemonHarness {
        _dir: dir,
        server,
        _client: client,
        daemon,
        ec,
    }
}

async fn daemon() -> DaemonHarness {
    daemon_with(MemoryState::default(), Arc::new(StaticAuthorizer::allow_all())).await
}

async fn tick(h: &DaemonHarness, n: u64) {
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
async fn status_profile_and_curves() {
    let h = daemon().await;
    let d = &h.daemon;
    let s = d.status().await.unwrap();
    assert_eq!(s.profile, "balanced");
    assert_eq!(s.firmware_version, "14K2EMS1.110");
    assert_eq!(s.gpu_state, "removed");
    assert_eq!(s.charge_limit, 100);
    assert!(!s.read_only);

    d.set_profile("eco").await.unwrap();
    assert_eq!(d.status().await.unwrap().profile, "eco");
    assert_eq!(d.status().await.unwrap().fan_mode, "advanced");

    let c = d.fan_curve(Fan::Cpu).await.unwrap();
    assert_eq!(c.spec, CurveSpec::from_core(&CurvePreset::Silent.spec()));
    // ADR-0010: the CPU is driven live (factory thresholds, steps 0–3 at the target, hot steps floored).
    assert_eq!(c.generated.table.speeds, [30, 30, 30, 50, 80, 100, 100]);
    assert!(c.generated.table.is_live_cpu());
    assert!(c.generated.is_raised(3));
    assert!(!c.custom);
    let gpu = d.fan_curve(Fan::Gpu).await.unwrap();
    assert_eq!(gpu.generated.table.speeds, [0, 30, 33, 43, 57, 76, 100]);
    assert_eq!(gpu.generated.raised, vec![]);
    assert!(!gpu.generated.table.is_live_cpu());
}

#[tokio::test]
async fn preview_apply_and_reset_report_raised_steps() {
    let h = daemon().await;
    let d = &h.daemon;
    let spec = CurveSpec {
        idle: 0,
        t0: 70,
        t1: 90,
        min: 30,
        max: 30,
        k: 1.0,
        up: [70, 76, 80, 86, 90, 95],
        hysteresis: 4,
    };
    assert!(spec.validate().is_ok());
    let preview = d.preview_fan_curve(&spec).await.unwrap();
    assert_eq!(preview.table.speeds, [30, 50, 50, 100, 100, 100, 100]);
    assert!(preview.is_raised(0));
    assert!(preview.raised[0].describe().contains("plancher"));
    assert_eq!(h.ec.state().writes.len(), 0, "preview writes nothing");

    let applied = d.set_fan_curve(Fan::Gpu, &spec).await.unwrap();
    assert_eq!(applied, preview);
    assert_eq!(h.ec.state().gpu.speeds, applied.table.speeds);
    assert!(d.fan_curve(Fan::Gpu).await.unwrap().custom);
    d.reset_fan_curve(Fan::Gpu).await.unwrap();
    assert!(!d.fan_curve(Fan::Gpu).await.unwrap().custom);
}

#[tokio::test]
async fn errors_are_mapped() {
    let h = daemon_with(
        MemoryState::default(),
        Arc::new(StaticAuthorizer::new(&[Action::SetBattery])),
    )
    .await;
    let d = &h.daemon;
    assert_eq!(d.set_profile("gaming").await, Err(AppError::NotAuthorized));
    assert!(matches!(
        d.set_charge_limit(40).await,
        Err(AppError::InvalidArgument(_))
    ));
    let bad = CurveSpec {
        t0: 90,
        ..CurveSpec::from_core(&CurvePreset::Silent.spec())
    };
    assert!(matches!(
        d.preview_fan_curve(&bad).await,
        Err(AppError::InvalidArgument(_))
    ));
    d.set_charge_limit(80).await.unwrap();
    assert_eq!(d.status().await.unwrap().charge_limit, 80);
    assert_eq!(d.gpu_holders().await.unwrap(), vec![]);
    assert!(d.has_method(dbus::RESTORE_FACTORY).await);
    assert!(d.has_method("SetProfile").await);
    // RestoreFactory needs set-fan-curve.
    assert_eq!(d.restore_factory().await, Err(AppError::NotAuthorized));
}

#[tokio::test]
async fn read_only_daemon() {
    let h = daemon_with(
        MemoryState {
            fw_version: "14K2EMS1.200".into(),
            ..MemoryState::default()
        },
        Arc::new(StaticAuthorizer::allow_all()),
    )
    .await;
    let s = h.daemon.status().await.unwrap();
    assert!(s.read_only);
    assert!(s.read_only_reason.contains("14K2EMS1.200"));
    assert!(matches!(
        h.daemon.set_profile("gaming").await,
        Err(AppError::ReadOnly(_))
    ));
}

#[tokio::test]
async fn events_carry_telemetry_safety_and_status() {
    let h = daemon().await;
    let mut events = h.daemon.events().await.unwrap();
    h.daemon.set_profile("balanced").await.unwrap();
    h.ec.state().cpu_temp = Some(97);
    tick(&h, 1).await;
    tick(&h, 2).await;
    let (mut telemetry, mut safety, mut status) = (None, None, None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while telemetry.is_none() || safety.is_none() || status.is_none() {
        let ev = tokio::time::timeout_at(deadline, events.next())
            .await
            .expect("events within 5 s")
            .expect("stream open");
        match ev {
            DaemonEvent::Telemetry(t) => telemetry = Some(t),
            DaemonEvent::Safety(s) => safety = Some(s),
            DaemonEvent::Status(s) => status = Some(s),
        }
    }
    assert_eq!(telemetry.unwrap().cpu_temp, Some(97.0));
    let safety = safety.unwrap();
    assert_eq!(safety.kind, "boost_forced");
    assert!(safety.describe().starts_with("Surchauffe"));
    assert_eq!(status.unwrap().profile, "balanced");
}

struct ClipdHarness {
    _dir: TempDir,
    _server: zbus::Connection,
    _client: zbus::Connection,
    clips: Clips,
}

async fn clipd() -> ClipdHarness {
    let dir = tempfile::tempdir().unwrap();
    let paths = clipd::Paths::new(dir.path().join("run/clipd"), dir.path().join("data/clipd"));
    let lib = Library::new(clipd::SqliteStore::open(&paths).unwrap(), Limits::default());
    let (a, b) = UnixStream::pair().unwrap();
    let server = Builder::unix_stream(a)
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at(clipd::OBJECT_PATH, clipd::Clipd::new(lib))
        .build();
    let client = Builder::unix_stream(b).p2p().build();
    let (server, client) = tokio::try_join!(server, client).unwrap();
    let clips = Clips::new(&client).await.unwrap();
    ClipdHarness {
        _dir: dir,
        _server: server,
        _client: client,
        clips,
    }
}

#[tokio::test]
async fn clipboard_manager_round_trip() {
    let h = clipd().await;
    let c = &h.clips;
    let mut changes = c.changes().await.unwrap();
    let text = c.proxy().add("text/plain", b"hello world").await.unwrap();
    let png = c
        .proxy()
        .add("image/png", &[0x89, b'P', b'N', b'G'])
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), changes.next())
        .await
        .unwrap()
        .unwrap();

    let page = c.list("", &[], Scope::All, 0, 50).await.unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.clips[0].id, png, "most recent first");
    assert!(page.clips[0].is_image());
    assert_eq!(page.clips[1].preview, "hello world");
    assert_eq!(c.get(png).await.unwrap().0, "image/png");

    // Tagging pins (documented in the UI).
    let tags = c
        .set_tags(text, &["Work".to_owned(), " rust ".to_owned()])
        .await
        .unwrap();
    assert_eq!(tags, vec!["rust", "work"]);
    let pinned = c.list("", &[], Scope::Pinned, 0, 50).await.unwrap();
    assert_eq!(pinned.clips.len(), 1);
    assert!(pinned.clips[0].pinned);
    assert_eq!(
        c.tags().await.unwrap(),
        vec![("rust".into(), 1), ("work".into(), 1)]
    );
    let by_tag = c.list("", &["work".to_owned()], Scope::All, 0, 50).await.unwrap();
    assert_eq!(by_tag.total, 1);
    let search = c.list("WORLD", &[], Scope::All, 0, 50).await.unwrap();
    assert_eq!(search.total, 1);

    c.set_pinned(png, true).await.unwrap();
    c.set_pinned(png, false).await.unwrap();
    assert_eq!(c.clear_unpinned().await.unwrap(), 1);
    assert_eq!(c.delete(&[text, 999]).await.unwrap(), 1);
    assert_eq!(c.list("", &[], Scope::All, 0, 50).await.unwrap().total, 0);

    assert_eq!(c.get(text).await, Err(AppError::NotFound));
    assert!(matches!(
        c.proxy()
            .add("text/plain", b"")
            .await
            .map_err(|e| AppError::from_zbus(&e, msi_center::errors::Service::Clipd)),
        Err(AppError::Rejected(_))
    ));
}
