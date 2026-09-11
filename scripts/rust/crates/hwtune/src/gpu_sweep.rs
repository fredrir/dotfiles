use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command as Process, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use hostkit::process::{self, CaptureLimits};
use serde::Serialize;

use crate::bench::report::format_value;
use crate::bench::{provenance, record, runner, store};
use crate::env::Sysfs;
use crate::gpu;
use crate::table;

const CAP_TOLERANCE_W: f64 = 0.5;
const VERIFY_ATTEMPTS: usize = 40;
const TICK: Duration = Duration::from_millis(250);

#[derive(Subcommand)]
pub enum Command {
    /// Measure GPU metrics at each power cap, then restore the original cap.
    Sweep(SweepOptions),
}

#[derive(Args)]
pub struct SweepOptions {
    /// Power caps in watts, comma separated, within the range LACT reports.
    #[arg(long, value_delimiter = ',', required = true, value_name = "WATTS", value_parser = clap::value_parser!(u32).range(1..))]
    pub caps: Vec<u32>,
    /// Families measured at each cap.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "ai",
        value_name = "FAMILIES"
    )]
    pub only: Vec<String>,
    /// Seconds to wait after applying a cap before measuring.
    #[arg(long, default_value = "15", value_parser = clap::value_parser!(u64).range(0..=600))]
    pub settle: u64,
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limit {
    pub current: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Trial {
    pub cap_w: u32,
    pub run_id: Option<String>,
    pub grade: Option<String>,
    pub medians: BTreeMap<String, f64>,
    pub error: Option<String>,
}

#[derive(Serialize)]
struct Session {
    schema: u32,
    kind: &'static str,
    session: String,
    host: String,
    gpu: String,
    started: String,
    finished: Option<String>,
    original_cap_w: f64,
    caps_w: Vec<u32>,
    families: Vec<String>,
    settle_seconds: u64,
    trials: Vec<Trial>,
    status: String,
    error: Option<String>,
    restored: bool,
}

pub fn parse_list(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once(':')?;
            let rest = rest.trim();
            let (id, rest) = rest.split_once(" (")?;
            let (name, kind) = rest.rsplit_once(") [")?;
            Some(Gpu {
                id: id.trim().to_string(),
                name: name.trim().to_string(),
                kind: kind.trim_end_matches(']').trim().to_string(),
            })
        })
        .collect()
}

pub fn choose_gpu(gpus: &[Gpu]) -> Result<&Gpu, String> {
    let nvidia = |gpu: &&Gpu| {
        let name = gpu.name.to_ascii_lowercase();
        name.contains("nvidia") || name.contains("geforce") || gpu.id.starts_with("10DE:")
    };
    if let Some(gpu) = gpus.iter().find(nvidia) {
        return Ok(gpu);
    }
    let mut dedicated = gpus.iter().filter(|gpu| gpu.kind == "Dedicated");
    match (dedicated.next(), dedicated.next()) {
        (Some(gpu), None) => Ok(gpu),
        _ => Err(format!(
            "no NVIDIA GPU in lact list: {}",
            gpus.iter()
                .map(|gpu| format!("{} ({})", gpu.id, gpu.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn watts(field: &str) -> Option<f64> {
    field
        .trim()
        .trim_end_matches('W')
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

pub fn parse_limit(text: &str) -> Result<Limit, String> {
    let line = text
        .lines()
        .find(|line| line.contains("power limit"))
        .ok_or("lact reported no power limit")?;
    let (current, range) = line
        .split_once("power limit:")
        .map(|(_, rest)| rest.split_once('(').unwrap_or((rest, "")))
        .ok_or("lact reported no power limit")?;
    let current = watts(current).ok_or("lact reported no current power limit")?;
    let range = range
        .split_once("Range:")
        .map(|(_, rest)| rest.trim_end_matches(')'))
        .ok_or("lact reported no configurable range")?;
    let (min, max) = range
        .split_once(" to ")
        .ok_or("lact reported no configurable range")?;
    let min = watts(min).ok_or("lact reported no minimum power limit")?;
    let max = watts(max).ok_or("lact reported no maximum power limit")?;
    if min > max || current < min - CAP_TOLERANCE_W || current > max + CAP_TOLERANCE_W {
        return Err(format!(
            "lact power limit {current} W outside {min}..{max} W"
        ));
    }
    Ok(Limit { current, min, max })
}

pub fn validate_caps(caps: &[u32], limit: &Limit) -> Result<Vec<u32>, String> {
    let mut unique = Vec::new();
    for cap in caps {
        let value = f64::from(*cap);
        if value < limit.min || value > limit.max {
            return Err(format!(
                "cap {cap} W outside the configurable range {:.0}..{:.0} W",
                limit.min, limit.max
            ));
        }
        if !unique.contains(cap) {
            unique.push(*cap);
        }
    }
    if unique.is_empty() {
        return Err("no power caps given".into());
    }
    Ok(unique)
}

pub fn per_watt(rate: Option<f64>, power_w: Option<f64>) -> Option<f64> {
    match (rate, power_w) {
        (Some(rate), Some(power)) if power > 0.0 && rate.is_finite() => Some(rate / power),
        _ => None,
    }
}

pub fn format_cap(cap: f64) -> String {
    if cap.fract() == 0.0 {
        format!("{cap:.0}")
    } else {
        format!("{cap:.1}")
    }
}

pub fn medians(run: &record::Run) -> BTreeMap<String, f64> {
    run.metrics
        .iter()
        .filter_map(|metric| Some((metric.key.clone(), metric.median()?)))
        .collect()
}

pub fn rows(trials: &[Trial]) -> (Vec<&'static str>, Vec<Vec<String>>) {
    let fans = trials
        .iter()
        .any(|trial| trial.medians.contains_key("idle.fan_rpm"));
    let mut headers = vec!["cap W", "prompt t/s", "generate t/s", "gpu W", "t/s per W"];
    if fans {
        headers.push("fan rpm");
    }
    headers.push("run");
    let rows = trials
        .iter()
        .map(|trial| {
            let value = |key: &str| trial.medians.get(key).copied();
            let mut row = vec![
                trial.cap_w.to_string(),
                format_value(value("ai.prompt_tps"), ""),
                format_value(value("ai.generate_tps"), ""),
                format_value(value("ai.gpu_w"), ""),
                format_value(per_watt(value("ai.generate_tps"), value("ai.gpu_w")), ""),
            ];
            if fans {
                row.push(format_value(value("idle.fan_rpm"), ""));
            }
            row.push(match (&trial.run_id, &trial.error) {
                (Some(id), _) => id.clone(),
                (None, Some(error)) => format!("failed: {error}"),
                (None, None) => "skipped".into(),
            });
            row
        })
        .collect();
    (headers, rows)
}

fn lact(args: &[&str]) -> Result<String, String> {
    let mut command = Process::new("lact");
    command.arg("cli").args(args);
    let captured = process::output(
        &mut command,
        CaptureLimits::default(),
        Duration::from_secs(30),
    )
    .map_err(|e| format!("lact: {e}"))?;
    if !captured.status.success() {
        return Err(format!(
            "lact cli {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&captured.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&captured.stdout).into_owned())
}

fn limit_of(id: &str) -> Result<Limit, String> {
    parse_limit(&lact(&["-g", id, "power-limit", "get"])?)
}

fn verify_cap(expected: f64) -> Result<(), String> {
    let mut last = None;
    for _ in 0..VERIFY_ATTEMPTS {
        let stats = gpu::query()?;
        if (stats.power_cap_w - expected).abs() <= CAP_TOLERANCE_W {
            return Ok(());
        }
        last = Some(stats.power_cap_w);
        thread::sleep(TICK);
    }
    Err(format!(
        "power cap {expected} W not active; nvidia-smi reports {}",
        last.map_or("nothing".into(), |cap| format!("{cap} W"))
    ))
}

fn set_cap(id: &str, cap: f64) -> Result<(), String> {
    lact(&["-g", id, "power-limit", "set", &format_cap(cap)])?;
    verify_cap(cap)
}

struct CapGuard {
    id: String,
    original: f64,
    armed: bool,
}

impl CapGuard {
    fn restore(&mut self) -> Result<(), String> {
        set_cap(&self.id, self.original)?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for CapGuard {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = self.restore()
        {
            eprintln!(
                "gpu cap restoration failed: {error}; run: lact cli -g {} power-limit set {}",
                self.id,
                format_cap(self.original)
            );
        }
    }
}

fn local_host(explicit: Option<&str>) -> Result<String, String> {
    let local = crate::paths::host_name(None)?;
    if let Some(requested) = explicit.filter(|host| !host.is_empty())
        && requested != local
    {
        return Err(format!(
            "GPU sweeps can only target the local machine {local:?}"
        ));
    }
    Ok(local)
}

fn families(only: &[String]) -> Result<Vec<String>, String> {
    let mut families = Vec::new();
    for family in only.iter().map(|value| value.trim()) {
        if family.is_empty() {
            continue;
        }
        if !runner::FAMILIES.contains(&family) {
            return Err(format!(
                "unknown family '{family}'; expected one of {}",
                runner::FAMILIES.join(", ")
            ));
        }
        if !families.iter().any(|known| known == family) {
            families.push(family.to_string());
        }
    }
    if families.is_empty() {
        return Err("no families to measure".into());
    }
    Ok(families)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn save_session(store: &store::Store, session: &Session) -> Result<PathBuf, String> {
    let _lock = store.exclusive()?;
    let path = store.tuning_path(&session.host, &session.session)?;
    store::atomic_write(
        &path,
        &serde_json::to_vec_pretty(session).map_err(|error| error.to_string())?,
    )?;
    Ok(path)
}

fn settle(seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if runner::cancelled() {
            return Err("sweep interrupted".into());
        }
        thread::sleep(TICK.min(deadline.saturating_duration_since(Instant::now())));
    }
    Ok(())
}

fn measure(
    store: &store::Store,
    session: &Session,
    cap: u32,
    quiet: bool,
) -> Result<record::Run, String> {
    let mut context = provenance::current(&session.host);
    context.tuning_session = Some(session.session.clone());
    let options = runner::Options {
        context: Some(context),
        host: session.host.clone(),
        tier: "quick".into(),
        families: session.families.clone(),
        note: format!("gpu-sweep {} cap {cap}", session.session),
        tags: vec!["gpu-sweep".into()],
        ..runner::Options::default()
    };
    let run = runner::execute(&options, &mut |event, job, detail| {
        if !quiet {
            eprintln!("cap {cap} W: {event} {job} {detail}");
        }
    })?;
    if run.metrics.is_empty() {
        return Err("no benchmark produced a result".into());
    }
    let _lock = store.exclusive()?;
    store.save_run(&run)?;
    Ok(run)
}

fn sweep(options: SweepOptions, host: Option<&str>, _sys: &Sysfs) -> Result<ExitCode, String> {
    let host = local_host(host)?;
    let families = families(&options.only)?;
    let _signals = ui_terminal::SignalGuard::new().map_err(|error| error.to_string())?;
    let _measurement = store::measurement_lock()?;
    let gpus = parse_list(&lact(&["list"])?);
    let gpu = choose_gpu(&gpus)?.clone();
    let limit = limit_of(&gpu.id)?;
    let caps = validate_caps(&options.caps, &limit)?;
    let store = store::Store::discover();
    let mut session = Session {
        schema: 1,
        kind: "gpu-sweep",
        session: format!(
            "gpu-sweep-{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ"),
            std::process::id()
        ),
        host,
        gpu: gpu.id.clone(),
        started: now(),
        finished: None,
        original_cap_w: limit.current,
        caps_w: caps.clone(),
        families,
        settle_seconds: options.settle,
        trials: Vec::new(),
        status: "running".into(),
        error: None,
        restored: false,
    };
    save_session(&store, &session)?;
    if !options.json {
        println!(
            "{} ({}): original cap {} W, range {}..{} W",
            gpu.name,
            gpu.id,
            format_cap(limit.current),
            format_cap(limit.min),
            format_cap(limit.max)
        );
    }
    let mut guard = CapGuard {
        id: gpu.id.clone(),
        original: limit.current,
        armed: true,
    };
    let mut outcome: Result<(), String> = Ok(());
    for cap in &caps {
        if runner::cancelled() {
            outcome = Err("sweep interrupted".into());
            break;
        }
        let trial = (|| {
            set_cap(&gpu.id, f64::from(*cap))?;
            settle(options.settle)?;
            measure(&store, &session, *cap, options.json)
        })();
        let recorded = match trial {
            Ok(run) => Trial {
                cap_w: *cap,
                run_id: Some(run.run_id.clone()),
                grade: Some(run.grade.clone()),
                medians: medians(&run),
                error: None,
            },
            Err(error) => Trial {
                cap_w: *cap,
                run_id: None,
                grade: None,
                medians: BTreeMap::new(),
                error: Some(error.clone()),
            },
        };
        let interrupted = runner::cancelled();
        session.trials.push(recorded);
        save_session(&store, &session)?;
        if interrupted {
            outcome = Err("sweep interrupted".into());
            break;
        }
    }
    let restored = guard.restore();
    session.restored = restored.is_ok();
    session.finished = Some(now());
    let failed = session
        .trials
        .iter()
        .filter(|trial| trial.error.is_some())
        .count();
    session.status = match (&outcome, &restored) {
        (Err(_), _) => "aborted",
        (Ok(()), Err(_)) => "failed",
        (Ok(()), Ok(())) if failed > 0 => "partial",
        (Ok(()), Ok(())) => "completed",
    }
    .into();
    session.error = match (&outcome, &restored) {
        (Err(error), Ok(())) => Some(error.clone()),
        (Err(error), Err(restore)) => {
            Some(format!("{error}; original cap not restored: {restore}"))
        }
        (Ok(()), Err(restore)) => Some(format!("original cap not restored: {restore}")),
        (Ok(()), Ok(())) if failed > 0 => Some(format!("{failed} cap(s) failed to measure")),
        (Ok(()), Ok(())) => None,
    };
    let path = save_session(&store, &session)?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&session).map_err(|error| error.to_string())?
        );
    } else {
        let (headers, rows) = rows(&session.trials);
        print!("{}", table::render(&headers, &rows));
        println!(
            "\n  {}  cap {}  session {}",
            session.status,
            if session.restored {
                format!("restored to {} W", format_cap(session.original_cap_w))
            } else {
                "not restored".into()
            },
            path.display()
        );
        if let Some(error) = &session.error {
            println!("  {error}");
        }
    }
    if session.status == "completed" {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

pub fn run(command: Command, host: Option<&str>, sys: &Sysfs) -> Result<ExitCode, String> {
    match command {
        Command::Sweep(options) => sweep(options, host, sys),
    }
}

#[cfg(test)]
#[path = "../tests/unit/gpu_sweep_tests.rs"]
mod tests;
