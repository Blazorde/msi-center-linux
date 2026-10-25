//! `msi-daemon`: system service exposing `org.blazorde.MsiCenter1` (D-Bus wiring comes in phase 4).
//!
//! For now only `--probe` exists: a **read-only** dump of the dGPU power state, its holders and a few telemetry
//! samples. It never writes to sysfs.

use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use msi_daemon::fs::FsRoot;
use msi_daemon::gpu::{Dgpu, GpuManager};
use msi_daemon::telemetry::dgpu::{DgpuPolicy, NvmlBackend};
use msi_daemon::telemetry::{Clock, Sample, Sampler};

const USAGE: &str = "usage: msi-daemon --probe [--samples N] [--interval-ms MS] [--no-nvml]
  --probe          read-only dump of dGPU state, holders and telemetry (never writes)
  --samples N      telemetry samples to print (default 3)
  --interval-ms MS delay between samples (default 1000)
  --no-nvml        never query NVML, even if the dGPU is active";

#[derive(Debug)]
struct ProbeArgs {
    samples: u32,
    interval: Duration,
    nvml: bool,
}

fn parse_args(args: &[String]) -> Result<ProbeArgs, String> {
    let mut probe = false;
    let mut parsed = ProbeArgs {
        samples: 3,
        interval: Duration::from_secs(1),
        nvml: true,
    };
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
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if probe {
        Ok(parsed)
    } else {
        Err("nothing to do".into())
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(parsed) => {
            probe(&parsed);
            ExitCode::SUCCESS
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
        let p = parse_args(&args(&[
            "--probe",
            "--samples",
            "5",
            "--interval-ms",
            "200",
            "--no-nvml",
        ]))
        .unwrap();
        assert_eq!(
            (p.samples, p.interval, p.nvml),
            (5, Duration::from_millis(200), false)
        );
        let d = parse_args(&args(&["--probe"])).unwrap();
        assert_eq!((d.samples, d.nvml), (3, true));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["--probe", "--samples"])).is_err());
        assert!(parse_args(&args(&["--probe", "--remove"])).is_err());
    }

    #[test]
    fn formats_optional_values() {
        assert_eq!(opt(Some(12.345), "%"), "12.3%");
        assert_eq!(opt(None::<f64>, "%"), "-");
    }
}
