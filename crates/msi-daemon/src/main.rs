//! `msi-daemon`: system service exposing `org.blazorde.MsiCenter1` on the system bus (run by systemd as root,
//! `systemd/msi-daemon.service`).
//!
//! - no argument: run the daemon (`--state PATH` overrides `/var/lib/msi-center/state.toml`);
//! - `--probe`: **read-only** dump of the dGPU power state, its holders and a few telemetry samples;
//! - `--restore-factory`: hands the fans back to the firmware (factory tables, then fan mode `auto`) and exits.
//!   No D-Bus, idempotent. Run by systemd `ExecStopPost` after every stop (S5), including crashes.
//!
//! Log level: `MSI_DAEMON_LOG` (`EnvFilter` syntax, default `info`).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use futures_util::StreamExt as _;
use msi_core::ec::{CONF_G2_5, FactoryTables};
use msi_daemon::controller::Controller;
use msi_daemon::ec::{EcBackend as _, SysfsEc, restore_factory};
use msi_daemon::fs::FsRoot;
use msi_daemon::gpu::{Dgpu, GpuManager};
use msi_daemon::notify::Notifier;
use msi_daemon::polkit::PolkitAuthorizer;
use msi_daemon::power::PowerSupply;
use msi_daemon::rapl::Rapl;
use msi_daemon::service::{BUS_NAME, MsiCenter, OBJECT_PATH};
use msi_daemon::state::{DEFAULT_STATE_PATH, StateStore};
use msi_daemon::telemetry::dgpu::{DgpuPolicy, NvmlBackend};
use msi_daemon::telemetry::{Clock, Sample, Sampler};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: msi-daemon [--state PATH]
       msi-daemon --probe [--samples N] [--interval-ms MS] [--no-nvml]
       msi-daemon --restore-factory
  (no option)      run the daemon (system bus, root, started by systemd)
  --state PATH     settings file (default /var/lib/msi-center/state.toml)
  --restore-factory  factory fan tables then fan mode auto, and exit (systemd ExecStopPost)
  --probe          read-only dump of dGPU state, holders and telemetry (never writes)
  --samples N      telemetry samples to print (default 3)
  --interval-ms MS delay between samples (default 1000)
  --no-nvml        never query NVML, even if the dGPU is active";

#[derive(Debug, PartialEq)]
enum Command {
    Run { state: PathBuf },
    Probe(ProbeArgs),
    RestoreFactory,
}

#[derive(Debug, PartialEq)]
struct ProbeArgs {
    samples: u32,
    interval: Duration,
    nvml: bool,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut probe = false;
    let mut state = PathBuf::from(DEFAULT_STATE_PATH);
    let mut parsed = ProbeArgs {
        samples: 3,
        interval: Duration::from_secs(1),
        nvml: true,
    };
    if args.first().is_some_and(|a| a == "--restore-factory") {
        return if args.len() == 1 {
            Ok(Command::RestoreFactory)
        } else {
            Err("--restore-factory takes no other argument".into())
        };
    }
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--probe" => probe = true,
            "--no-nvml" => parsed.nvml = false,
            "--samples" => {
                parsed.samples = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--samples needs a number")?;
            }
            "--interval-ms" => {
                let ms = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--interval-ms needs a number")?;
                parsed.interval = Duration::from_millis(ms);
            }
            "--state" => state = it.next().ok_or("--state needs a path")?.into(),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if probe {
        Ok(Command::Probe(parsed))
    } else if parsed
        != (ProbeArgs {
            samples: 3,
            interval: Duration::from_secs(1),
            nvml: true,
        })
    {
        Err("probe options need --probe".into())
    } else {
        Ok(Command::Run { state })
    }
}

fn opt<T: std::fmt::Display>(value: Option<T>, unit: &str) -> String {
    value.map_or_else(|| "-".to_owned(), |v| format!("{v:.1}{unit}"))
}

fn print_dgpu(gpu: &GpuManager, dgpu: &Dgpu) {
    let attr = |name: &str| {
        std::fs::read_to_string(dgpu.gpu.path.join(name)).map_or("?".into(), |s| s.trim().to_owned())
    };
    println!("dgpu.address      {}", dgpu.gpu.address);
    println!(
        "dgpu.companions   {}",
        dgpu.companions
            .iter()
            .map(|f| f.address.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("dgpu.drm_nodes    {}", dgpu.drm_nodes.join(", "));
    println!("dgpu.state        {}", dgpu.state());
    println!(
        "dgpu.power        control={} runtime_status={} power_state={}",
        attr("power/control"),
        attr("power/runtime_status"),
        attr("power_state")
    );
    println!("dgpu.outputs      {}", dgpu.connected_outputs().join(", "));
    let scan = gpu.holders(dgpu);
    for holder in &scan.holders {
        println!("dgpu.holder       {} {}", holder.pid, holder.name);
    }
    println!(
        "dgpu.unreadable   {} process(es) could not be inspected",
        scan.unreadable
    );
    match gpu.check_removable(dgpu) {
        Ok(()) => println!("dgpu.removable    yes"),
        Err(err) => println!("dgpu.removable    no: {err}"),
    }
}

fn print_sample(i: u32, s: &Sample) {
    println!(
        "sample {i}: cpu={} ram={} igpu={} dgpu={} npu={} cpu_temp={} gpu_temp={} fan1={} fan2={} dgpu_power={} dgpu_state={}",
        opt(s.cpu_pct, "%"),
        opt(s.ram_pct, "%"),
        opt(s.igpu_pct, "%"),
        opt(s.dgpu_pct, "%"),
        opt(s.npu_pct, "%"),
        opt(s.thermals.cpu_temp_c, "C"),
        opt(s.thermals.gpu_temp_c, "C"),
        s.thermals.fan1_rpm.map_or("-".into(), |r| r.to_string()),
        s.thermals.fan2_rpm.map_or("-".into(), |r| r.to_string()),
        opt(s.dgpu_power_w, "W"),
        s.dgpu_state,
    );
}

fn probe(args: &ProbeArgs) {
    let root = FsRoot::real();
    let gpu = GpuManager::new(root.clone());
    let dgpu = gpu.discover();
    match &dgpu {
        Some(dgpu) => print_dgpu(&gpu, dgpu),
        None => println!("dgpu.state        removed (no NVIDIA display device on the bus)"),
    }
    let bus_id = dgpu.map_or_else(|| "0000:01:00.0".to_owned(), |d| d.gpu.address);
    let policy = if args.nvml {
        DgpuPolicy::WhenActive
    } else {
        DgpuPolicy::Off
    };
    let mut sampler = Sampler::new(root, NvmlBackend::new(bus_id), policy);
    let start = Instant::now();
    // One extra warm-up sample: delta-based values need two readings.
    for i in 0..=args.samples {
        let now = Clock(u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX));
        let sample = sampler.sample(now);
        if i > 0 {
            print_sample(i, &sample);
        }
        if i < args.samples {
            thread::sleep(args.interval);
        }
    }
    sampler.release_dgpu();
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("MSI_DAEMON_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // Under systemd, journald already timestamps lines and does not render colours.
    let journald = std::env::var_os("JOURNAL_STREAM").is_some();
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(!journald);
    if journald {
        builder.without_time().init();
    } else {
        builder.init();
    }
}

/// S5 on panic: the release profile aborts on panic, so no destructor runs. The hook restores the factory
/// tables and fan mode `auto` through its own [`SysfsEc`] (no lock: the panic may hold the controller's; the
/// kernel serialises curve writes), then runs the default hook.
fn install_panic_hook(ec: SysfsEc, factory: Option<FactoryTables>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut ec = ec.clone();
        let restored = restore_factory(&mut ec, factory.as_ref());
        eprintln!("msi-daemon panicked; EC factory tables + fan mode auto restored: {restored:?}");
        previous(info);
    }));
}

/// Factory tables for the EC under `ec`, if it is supported (S8) and they are known.
fn factory_for(ec: &SysfsEc) -> Option<FactoryTables> {
    let fw = ec.fw_version().ok()?;
    if !CONF_G2_5.supports_firmware(&fw) || !ec.curve_supported() {
        return None;
    }
    CONF_G2_5.factory_tables(&fw)
}

/// `--restore-factory` (systemd `ExecStopPost`): factory tables when they are known and the EC is supported,
/// then fan mode `auto` in every case (as the former inline shell did). msi-ec absent → nothing to do.
fn restore_factory_cli() -> ExitCode {
    let mut ec = SysfsEc::new(&FsRoot::real());
    if let Err(e) = ec.fw_version() {
        println!("msi-ec not available ({e}): nothing to restore");
        return ExitCode::SUCCESS;
    }
    let factory = factory_for(&ec);
    match restore_factory(&mut ec, factory.as_ref()) {
        Ok(()) if factory.is_some() => {
            println!("EC back on its factory tables, fan mode auto");
            ExitCode::SUCCESS
        }
        Ok(()) => {
            println!("fan mode auto (factory tables unknown or EC unsupported: tables left as they are)");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("msi-daemon --restore-factory: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Waits for SIGTERM (systemd stop) or SIGINT (Ctrl+C).
async fn shutdown_signal() -> anyhow::Result<&'static str> {
    let mut term = signal(SignalKind::terminate()).context("installing SIGTERM handler")?;
    let mut int = signal(SignalKind::interrupt()).context("installing SIGINT handler")?;
    Ok(tokio::select! {
        _ = term.recv() => "SIGTERM",
        _ = int.recv() => "SIGINT",
    })
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// Re-applies the profile after each resume: the EC may have reset its tables during suspend.
async fn watch_resume(conn: zbus::Connection, iface: zbus::object_server::InterfaceRef<MsiCenter>) {
    let proxy = match Login1ManagerProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "logind unavailable: no re-apply after resume (drift check still runs)");
            return;
        }
    };
    let mut stream = match proxy.receive_prepare_for_sleep().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "cannot watch PrepareForSleep");
            return;
        }
    };
    while let Some(sig) = stream.next().await {
        if sig.args().is_ok_and(|a| !a.start) {
            tracing::info!("resumed from suspend: re-applying the profile");
            iface.get().await.resume(iface.signal_emitter()).await;
        }
    }
}

/// 1 Hz supervision: S4, AC switch, drift check, telemetry. `WATCHDOG=1` only after a completed tick (S6).
async fn tick_loop(iface: zbus::object_server::InterfaceRef<MsiCenter>, notifier: Notifier) {
    let start = Instant::now();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let now = Clock(u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX));
        match iface.get().await.tick(iface.signal_emitter(), now).await {
            Ok(()) => {
                if let Err(e) = notifier.notify("WATCHDOG=1") {
                    tracing::warn!(error = %e, "watchdog notification failed");
                }
            }
            Err(e) => tracing::error!(error = %e, "supervision tick failed"),
        }
    }
}

async fn run(state: &Path) -> anyhow::Result<()> {
    init_tracing();
    let root = FsRoot::real();
    let ec = SysfsEc::new(&root);
    let hook_ec = ec.clone();
    let factory = factory_for(&ec);
    let gpu = GpuManager::new(root.clone());
    let bus_id = gpu
        .discover()
        .map_or_else(|| "0000:01:00.0".to_owned(), |d| d.gpu.address);
    let sampler = Sampler::new(root.clone(), NvmlBackend::new(bus_id), DgpuPolicy::Off);
    let (ctl, warnings) = Controller::new(
        Box::new(ec),
        PowerSupply::new(root.clone()),
        gpu,
        Some(Box::new(sampler)),
        StateStore::new(state),
    );
    let mut ctl = ctl.with_rapl(Rapl::new(root));
    for w in warnings {
        tracing::warn!("settings: {w}");
    }
    match ctl.status().read_only {
        Some(reason) => tracing::warn!(reason = %reason, "read-only mode (S8): the EC will not be written"),
        None => install_panic_hook(hook_ec, factory),
    }
    if let Err(e) = ctl.start() {
        tracing::warn!(error = %e, "initial profile not fully applied");
    }
    let ctl = Arc::new(Mutex::new(ctl));

    let conn = zbus::connection::Builder::system()
        .build()
        .await
        .context("connecting to the system bus")?;
    let auth = Arc::new(PolkitAuthorizer::new(conn.clone()));
    conn.object_server()
        .at(OBJECT_PATH, MsiCenter::new(Arc::clone(&ctl), auth))
        .await
        .context("serving the interface")?;
    conn.request_name(BUS_NAME)
        .await
        .context("acquiring the bus name")?;
    let iface = conn
        .object_server()
        .interface::<_, MsiCenter>(OBJECT_PATH)
        .await
        .context("interface lookup")?;
    tracing::info!(name = BUS_NAME, path = OBJECT_PATH, "serving");

    let notifier = Notifier::from_env();
    if let Err(e) = notifier.notify("READY=1") {
        tracing::warn!(error = %e, "READY notification failed");
    }
    let ticker = tokio::spawn(tick_loop(iface.clone(), notifier.clone()));
    let resume = tokio::spawn(watch_resume(conn.clone(), iface));

    let sig = shutdown_signal().await?;
    tracing::info!(signal = sig, "shutting down");
    if let Err(e) = notifier.notify("STOPPING=1") {
        tracing::debug!(error = %e, "STOPPING notification failed");
    }
    ticker.abort();
    resume.abort();
    // S5: back to the factory tables and the firmware curve. ExecStopPost does it again in case this fails.
    tokio::task::spawn_blocking(move || {
        ctl.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown();
    })
    .await
    .context("restoring the factory tables and fan mode auto")?;
    conn.graceful_shutdown().await;
    tracing::info!("stopped");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Command::Probe(parsed)) => {
            probe(&parsed);
            ExitCode::SUCCESS
        }
        Ok(Command::RestoreFactory) => restore_factory_cli(),
        Ok(Command::Run { state }) => {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
            match runtime
                .map_err(anyhow::Error::from)
                .and_then(|rt| rt.block_on(run(&state)))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("msi-daemon: {e:#}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(err) => {
            eprintln!("{err}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_probe_options() {
        let Command::Probe(p) = parse_args(&args(&[
            "--probe",
            "--samples",
            "5",
            "--interval-ms",
            "200",
            "--no-nvml",
        ]))
        .unwrap() else {
            panic!("probe expected");
        };
        assert_eq!(
            (p.samples, p.interval, p.nvml),
            (5, Duration::from_millis(200), false)
        );
        let Command::Probe(d) = parse_args(&args(&["--probe"])).unwrap() else {
            panic!("probe expected");
        };
        assert_eq!((d.samples, d.nvml), (3, true));
    }

    #[test]
    fn no_argument_runs_the_daemon() {
        assert_eq!(
            parse_args(&args(&[])).unwrap(),
            Command::Run {
                state: PathBuf::from("/var/lib/msi-center/state.toml")
            }
        );
        assert_eq!(
            parse_args(&args(&["--state", "/tmp/s.toml"])).unwrap(),
            Command::Run {
                state: PathBuf::from("/tmp/s.toml")
            }
        );
    }

    #[test]
    fn parses_restore_factory() {
        assert_eq!(
            parse_args(&args(&["--restore-factory"])).unwrap(),
            Command::RestoreFactory
        );
        assert!(parse_args(&args(&["--restore-factory", "--probe"])).is_err());
        assert!(parse_args(&args(&["--probe", "--restore-factory"])).is_err());
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse_args(&args(&["--samples", "4"])).is_err());
        assert!(parse_args(&args(&["--state"])).is_err());
        assert!(parse_args(&args(&["--probe", "--samples"])).is_err());
        assert!(parse_args(&args(&["--probe", "--remove"])).is_err());
    }

    #[test]
    fn formats_optional_values() {
        assert_eq!(opt(Some(12.345), "%"), "12.3%");
        assert_eq!(opt(None::<f64>, "%"), "-");
    }
}
