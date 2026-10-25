//! `msictl`: command-line client of `org.blazorde.MsiCenter1` (system bus). Thin: every decision is the
//! daemon's; polkit authorizes the mutating commands (no sudo needed for the active local user).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::process::ExitCode;

use futures_util::StreamExt as _;
use msi_daemon::client::{MsiCenter1Proxy, RaisedWire, SpecWire, TableWire};
use zbus::OwnedVariant;

const USAGE: &str = "usage: msictl [--session] <command>
  status                         profile, fan mode, Cooler Boost, battery, dGPU, firmware
  profile <eco|balanced|gaming>  apply a profile
  curve show [cpu|gpu]           curve parameters and the resulting EC table
  charge-limit [50..100]         show or set the battery charge limit
  telemetry [--watch]            one telemetry sample, or one per second until Ctrl+C
  --session                      talk to a daemon on the session bus (development)";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cmd {
    Status,
    Profile(String),
    CurveShow(Vec<String>),
    ChargeLimit(Option<u8>),
    Telemetry { watch: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cli {
    session: bool,
    cmd: Cmd,
}

fn parse(args: &[String]) -> Result<Cli, String> {
    let mut session = false;
    let mut rest: Vec<&str> = Vec::new();
    for a in args {
        if a == "--session" {
            session = true;
        } else {
            rest.push(a);
        }
    }
    let cmd = match rest.as_slice() {
        ["status"] => Cmd::Status,
        ["profile", p] => Cmd::Profile((*p).to_owned()),
        ["curve", "show"] => Cmd::CurveShow(vec!["cpu".into(), "gpu".into()]),
        ["curve", "show", fan @ ("cpu" | "gpu")] => Cmd::CurveShow(vec![(*fan).to_owned()]),
        ["charge-limit"] => Cmd::ChargeLimit(None),
        ["charge-limit", n] => Cmd::ChargeLimit(Some(
            n.parse()
                .map_err(|_| format!("charge limit must be a number in 50..100, got {n:?}"))?,
        )),
        ["telemetry"] => Cmd::Telemetry { watch: false },
        ["telemetry", "--watch"] => Cmd::Telemetry { watch: true },
        [] => return Err("missing command".into()),
        other => return Err(format!("unknown command: {}", other.join(" "))),
    };
    Ok(Cli { session, cmd })
}

fn join(v: &[u8]) -> String {
    v.iter().fold(String::new(), |mut out, x| {
        let _ = write!(out, "{x:>4}");
        out
    })
}

fn format_curve(
    fan: &str,
    spec: &SpecWire,
    table: &TableWire,
    raised: &[RaisedWire],
    custom: bool,
) -> String {
    let (idle, t0, t1, min, max, k, up, hysteresis) = spec;
    let mut out = String::new();
    let origin = if custom { "custom" } else { "preset" };
    let _ = writeln!(
        out,
        "{fan} ({origin}): idle {idle} %, {min}→{max} % from {t0} to {t1} °C, k {k}, hysteresis {hysteresis} °C, up {up:?}"
    );
    let _ = writeln!(out, "  step          {}", join(&[0, 1, 2, 3, 4, 5, 6]));
    let _ = writeln!(out, "  speed %       {}", join(&table.0));
    let _ = writeln!(out, "  up ≥ °C           {}", join(&table.1));
    let _ = writeln!(out, "  down < °C         {}", join(&table.2));
    for (step, temp, curve, applied, floor_t, floor_s) in raised {
        let _ = writeln!(
            out,
            "  ! step {step} (from {temp} °C) raised {curve} → {applied} % by the safety floor {floor_s} % at {floor_t} °C"
        );
    }
    out
}

fn value_str(v: &OwnedVariant) -> String {
    if let Ok(d) = f64::try_from(&**v) {
        format!("{d:.1}")
    } else if let Ok(u) = u32::try_from(&**v) {
        u.to_string()
    } else if let Ok(y) = u8::try_from(&**v) {
        y.to_string()
    } else if let Ok(b) = bool::try_from(&**v) {
        b.to_string()
    } else if let Ok(s) = <&str>::try_from(&**v) {
        s.to_owned()
    } else {
        "?".to_owned()
    }
}

fn format_telemetry(data: &HashMap<String, OwnedVariant>) -> String {
    let mut keys: Vec<&String> = data.keys().collect();
    keys.sort();
    keys.iter()
        .map(|k| format!("{k}={}", value_str(&data[*k])))
        .collect::<Vec<_>>()
        .join(" ")
}

fn describe(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, msg, _) => {
            let short = name.as_str().rsplit('.').next().unwrap_or(name.as_str());
            format!("{short}: {}", msg.as_deref().unwrap_or(""))
        }
        other => other.to_string(),
    }
}

async fn status(p: &MsiCenter1Proxy<'_>) -> zbus::Result<()> {
    println!("profile          {}", p.profile().await?);
    println!("auto on AC       {}", p.auto_profile_on_power().await?);
    println!("on AC power      {}", p.on_ac_power().await?);
    println!("fan mode         {}", p.fan_mode().await?);
    let forced = if p.boost_forced().await? {
        " (forced by S4)"
    } else {
        ""
    };
    println!("cooler boost     {}{forced}", p.cooler_boost().await?);
    let limit = p.charge_limit().await?;
    println!(
        "charge limit     {}",
        if limit == 0 {
            "unsupported".to_owned()
        } else {
            format!("{limit} %")
        }
    );
    println!("dGPU             {}", p.gpu_state().await?);
    println!("EC firmware      {}", p.firmware_version().await?);
    if p.read_only().await? {
        println!("READ-ONLY        {}", p.read_only_reason().await?);
    }
    Ok(())
}

async fn run(cli: Cli) -> zbus::Result<()> {
    let conn = if cli.session {
        zbus::Connection::session().await?
    } else {
        zbus::Connection::system().await?
    };
    let p = MsiCenter1Proxy::new(&conn).await?;
    match cli.cmd {
        Cmd::Status => status(&p).await?,
        Cmd::Profile(name) => {
            p.set_profile(&name).await?;
            println!("profile {name} applied");
        }
        Cmd::CurveShow(fans) => {
            for fan in fans {
                let (spec, table, raised, custom) = p.fan_curve(&fan).await?;
                print!("{}", format_curve(&fan, &spec, &table, &raised, custom));
            }
        }
        Cmd::ChargeLimit(None) => println!("{} %", p.charge_limit().await?),
        Cmd::ChargeLimit(Some(n)) => {
            p.set_charge_limit(n).await?;
            println!("charge limit set to {n} %");
        }
        Cmd::Telemetry { watch } => {
            let mut stream = p.receive_telemetry().await?;
            while let Some(sig) = stream.next().await {
                println!("{}", format_telemetry(&sig.args()?.data));
                if !watch {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("msictl: {e}");
            return ExitCode::FAILURE;
        }
    };
    match rt.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("msictl: {}", describe(&e));
            ExitCode::FAILURE
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
    fn parses_every_command() {
        let p = |a: &[&str]| parse(&args(a)).map(|c| c.cmd);
        assert_eq!(p(&["status"]), Ok(Cmd::Status));
        assert_eq!(p(&["profile", "eco"]), Ok(Cmd::Profile("eco".into())));
        assert_eq!(
            p(&["curve", "show"]),
            Ok(Cmd::CurveShow(vec!["cpu".into(), "gpu".into()]))
        );
        assert_eq!(
            p(&["curve", "show", "gpu"]),
            Ok(Cmd::CurveShow(vec!["gpu".into()]))
        );
        assert_eq!(p(&["charge-limit"]), Ok(Cmd::ChargeLimit(None)));
        assert_eq!(p(&["charge-limit", "80"]), Ok(Cmd::ChargeLimit(Some(80))));
        assert_eq!(p(&["telemetry"]), Ok(Cmd::Telemetry { watch: false }));
        assert_eq!(p(&["telemetry", "--watch"]), Ok(Cmd::Telemetry { watch: true }));
        assert!(parse(&args(&["--session", "status"])).unwrap().session);
    }

    #[test]
    fn rejects_bad_commands() {
        for bad in [
            &[][..],
            &["profile"],
            &["curve", "show", "fan3"],
            &["charge-limit", "x"],
            &["reboot"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn formats_a_curve_with_its_safety_report() {
        let spec = (0, 50, 82, 30, 100, 2.0, vec![50, 57, 64, 70, 76, 82], 4);
        let table = (
            vec![0, 30, 33, 43, 57, 76, 100],
            vec![50, 57, 64, 70, 76, 82],
            vec![46, 53, 60, 66, 72, 78],
        );
        let out = format_curve("cpu", &spec, &table, &[(1, 55, 0, 30, 60, 30)], false);
        assert!(
            out.starts_with("cpu (preset): idle 0 %, 30→100 % from 50 to 82 °C"),
            "{out}"
        );
        assert!(out.contains("   0  30  33  43  57  76 100"), "{out}");
        assert!(out.contains("step 1 (from 55 °C) raised 0 → 30 %"), "{out}");
    }

    #[test]
    fn formats_telemetry_sorted() {
        let mut m = HashMap::new();
        m.insert("fan1_pct".to_owned(), OwnedVariant::from(33u8));
        m.insert("cpu_temp".to_owned(), OwnedVariant::from(55.0));
        m.insert("cooler_boost".to_owned(), OwnedVariant::from(false));
        assert_eq!(
            format_telemetry(&m),
            "cooler_boost=false cpu_temp=55.0 fan1_pct=33"
        );
    }
}
