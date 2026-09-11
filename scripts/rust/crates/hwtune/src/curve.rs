use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};
use std::sync::LazyLock;

use clap::Subcommand;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bench::provenance::StabilitySession;
use crate::bench::store::{self, Store};
use crate::bench::suites::{native, require, tool_path};
use crate::bios::export::{self, Export};
use crate::cpu;
use crate::env::{self, Sysfs};
use crate::paths::Paths;
use crate::table;

pub const STEP: i32 = 5;
pub const FLOOR: i32 = -50;
pub const MIN_PASS_MINUTES: u64 = 10;
pub const STRETCH_DROP_PCT: f64 = 3.0;
const DEFAULT_ITERATIONS: u64 = 200_000_000;

static PER_CORE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^core\s+(\d+)\s+curve optimizer\s+(sign|magnitude)$").expect("core regex")
});

#[derive(Subcommand)]
pub enum Command {
    /// Per-core offsets, stress evidence, throughput drift, and the next step.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Per-core single-thread throughput, stored as clock-stretch evidence.
    Bench {
        /// Workload iterations per core.
        #[arg(long, default_value_t = DEFAULT_ITERATIONS)]
        iterations: u64,
    },
}

pub type Offsets = BTreeMap<u32, i32>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CoreEvidence {
    pub passed: BTreeSet<i32>,
    pub failed: BTreeSet<i32>,
}

impl CoreEvidence {
    pub fn best_passed(&self) -> Option<i32> {
        self.passed.iter().next().copied()
    }

    pub fn shallowest_failed(&self) -> Option<i32> {
        self.failed.iter().next_back().copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Try,
    Hold,
    Stress,
    BackOff,
    NoExport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub action: Action,
    pub offset: Option<i32>,
}

impl Suggestion {
    pub fn text(&self) -> String {
        match (self.action, self.offset) {
            (Action::Try, Some(offset)) => format!("try {offset}"),
            (Action::Hold, _) => "hold".into(),
            (Action::Stress, Some(offset)) => format!("stress {offset} first"),
            (Action::BackOff, Some(offset)) => format!("back off to {offset}"),
            _ => "no offset in export".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CoreStatus {
    pub core: u32,
    pub prefcore: Option<u64>,
    pub offset: Option<i32>,
    pub best_passed: Option<i32>,
    pub shallowest_failed: Option<i32>,
    pub mops: Option<f64>,
    pub previous_mops: Option<f64>,
    pub drift_pct: Option<f64>,
    pub stretching: bool,
    pub next: Suggestion,
}

#[derive(Clone, Debug, Serialize)]
pub struct RyzenSmu {
    pub version: String,
    pub codename: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub schema: u32,
    pub host: String,
    pub mode: Option<String>,
    pub export: Option<String>,
    pub ryzen_smu: Option<RyzenSmu>,
    pub cores: Vec<CoreStatus>,
    pub bios_changes: Vec<String>,
    pub commands: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CoreSample {
    pub mops: f64,
    pub prefcore: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Sample {
    pub schema: u32,
    pub host: String,
    pub taken: String,
    pub bios: String,
    pub iterations: u64,
    #[serde(default)]
    pub worker: String,
    #[serde(default)]
    pub worker_path: String,
    pub cores: BTreeMap<u32, CoreSample>,
}

pub fn worker_identity(binary: &Path) -> Result<String, String> {
    let bytes = fs::read(binary).map_err(|e| format!("{}: {e}", binary.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn comparable(previous: &Sample, latest: &Sample) -> bool {
    !latest.worker.is_empty()
        && previous.worker == latest.worker
        && previous.iterations == latest.iterations
}

pub fn mode(export: &Export) -> Option<String> {
    export.value("Curve Optimizer").map(str::to_owned)
}

fn signed(sign: Option<&str>, magnitude: Option<&str>) -> Option<i32> {
    let magnitude = magnitude?.parse::<i32>().ok()?;
    let sign = match sign?.to_ascii_lowercase().as_str() {
        "negative" => -1,
        "positive" => 1,
        _ => return None,
    };
    Some(sign * magnitude)
}

pub fn parse_offsets(export: &Export, cores: &[u32]) -> Offsets {
    let all = |offset: i32| {
        cores
            .iter()
            .map(|core| (*core, offset))
            .collect::<Offsets>()
    };
    match mode(export).as_deref() {
        Some("All Cores") => {
            let offset = signed(
                export.value("All Core Curve Optimizer Sign"),
                export.value("All Core Curve Optimizer Magnitude"),
            );
            offset.map_or_else(Offsets::new, all)
        }
        Some("Per Core") => {
            let mut signs = BTreeMap::new();
            let mut magnitudes = BTreeMap::new();
            for setting in &export.settings {
                let Some(found) = PER_CORE.captures(&setting.name) else {
                    continue;
                };
                let Ok(core) = found[1].parse::<u32>() else {
                    continue;
                };
                let target = if found[2].eq_ignore_ascii_case("sign") {
                    &mut signs
                } else {
                    &mut magnitudes
                };
                target.entry(core).or_insert(setting.value.as_str());
            }
            cores
                .iter()
                .filter_map(|core| {
                    signed(signs.get(core).copied(), magnitudes.get(core).copied())
                        .map(|offset| (*core, offset))
                })
                .collect()
        }
        Some(_) => all(0),
        None => Offsets::new(),
    }
}

fn verdict_passed(value: &str) -> Option<bool> {
    if value.starts_with("pass") {
        Some(true)
    } else if value.starts_with("fail") {
        Some(false)
    } else {
        None
    }
}

pub fn session_offsets(
    session: &StabilitySession,
    exports: &BTreeMap<String, Offsets>,
    cores: &[u32],
) -> Offsets {
    if let Some(found) = session.details.get("bios").and_then(|sha| exports.get(sha))
        && !found.is_empty()
    {
        return found.clone();
    }
    session
        .details
        .get("offset")
        .and_then(|value| value.parse::<i32>().ok())
        .map(|offset| cores.iter().map(|core| (*core, offset)).collect())
        .unwrap_or_default()
}

pub fn evidence(
    sessions: &[StabilitySession],
    exports: &BTreeMap<String, Offsets>,
    cores: &[u32],
) -> BTreeMap<u32, CoreEvidence> {
    let mut found = cores
        .iter()
        .map(|core| (*core, CoreEvidence::default()))
        .collect::<BTreeMap<_, _>>();
    for session in sessions {
        if session.details.get("profile").map(String::as_str) != Some("per-core") {
            continue;
        }
        let long_enough = session
            .details
            .get("minutes")
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|minutes| minutes >= MIN_PASS_MINUTES);
        let offsets = session_offsets(session, exports, cores);
        for (core, offset) in offsets {
            let Some(entry) = found.get_mut(&core) else {
                continue;
            };
            match session
                .details
                .get(&format!("core{core}"))
                .and_then(|value| verdict_passed(value))
            {
                Some(true) if long_enough => {
                    entry.passed.insert(offset);
                }
                Some(false) => {
                    entry.failed.insert(offset);
                }
                _ => {}
            }
        }
    }
    found
}

pub fn suggest(current: Option<i32>, evidence: &CoreEvidence) -> Suggestion {
    let Some(current) = current else {
        return Suggestion {
            action: Action::NoExport,
            offset: None,
        };
    };
    if evidence.failed.contains(&current) {
        let target = evidence
            .passed
            .iter()
            .find(|offset| **offset > current)
            .copied()
            .unwrap_or((current + STEP).min(0));
        return Suggestion {
            action: Action::BackOff,
            offset: Some(target),
        };
    }
    if !evidence.passed.contains(&current) {
        return Suggestion {
            action: Action::Stress,
            offset: Some(current),
        };
    }
    let deeper_failure = evidence.failed.iter().any(|offset| *offset < current);
    if deeper_failure || current - STEP < FLOOR {
        return Suggestion {
            action: Action::Hold,
            offset: Some(current),
        };
    }
    Suggestion {
        action: Action::Try,
        offset: Some(current - STEP),
    }
}

pub fn drift_pct(previous: f64, current: f64) -> Option<f64> {
    (previous > 0.0 && current.is_finite()).then(|| (current - previous) / previous * 100.0)
}

pub fn stretching(drift: Option<f64>) -> bool {
    drift.is_some_and(|value| value < -STRETCH_DROP_PCT)
}

fn command_line(cores: &[u32], offset: i32) -> String {
    format!(
        "hwtune stress cpu --profile per-core --cores {} --offset {offset} --minutes {MIN_PASS_MINUTES}",
        cores
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

pub fn plan(cores: &[CoreStatus]) -> (Vec<String>, Vec<String>) {
    let mut changes = Vec::new();
    let mut targets = BTreeMap::<i32, Vec<u32>>::new();
    for core in cores {
        let Some(offset) = core.next.offset else {
            continue;
        };
        if matches!(core.next.action, Action::Hold | Action::NoExport) {
            continue;
        }
        if core.offset != Some(offset) {
            changes.push(format!("core {} {offset}", core.core));
        }
        targets.entry(offset).or_default().push(core.core);
    }
    let commands = targets
        .iter()
        .rev()
        .map(|(offset, cores)| command_line(cores, *offset))
        .collect();
    (changes, commands)
}

fn host_of(paths: Option<&Paths>) -> Result<String, String> {
    match paths {
        Some(paths) => Ok(paths.host.clone()),
        None => crate::paths::host_name(None),
    }
}

fn host_dir(store: &Store, host: &str, kind: &str) -> Result<PathBuf, String> {
    if !sysinfo::inventory::valid_name(host) {
        return Err(format!("invalid host name: {host}"));
    }
    Ok(store.root.join("hosts").join(host).join(kind))
}

fn json_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

pub fn load_sessions(store: &Store, host: &str) -> Result<Vec<StabilitySession>, String> {
    json_files(&host_dir(store, host, "stability")?)?
        .iter()
        .map(|path| {
            let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_slice::<StabilitySession>(&bytes)
                .map_err(|e| format!("{}: {e}", path.display()))
        })
        .collect()
}

pub fn load_samples(store: &Store, host: &str) -> Result<Vec<Sample>, String> {
    let mut samples = json_files(&host_dir(store, host, "curve")?)?
        .iter()
        .map(|path| {
            let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_slice::<Sample>(&bytes).map_err(|e| format!("{}: {e}", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    samples.retain(|sample| sample.schema == 1 && sample.host == host);
    samples.sort_by(|a, b| a.taken.cmp(&b.taken));
    Ok(samples)
}

#[derive(Default)]
pub struct Exports {
    pub by_sha: BTreeMap<String, Offsets>,
    pub latest: Option<(String, Export)>,
}

pub fn load_exports(paths: Option<&Paths>, cores: &[u32]) -> Result<Exports, String> {
    let mut found = Exports::default();
    let Some(paths) = paths else {
        return Ok(found);
    };
    for path in export::list(&paths.exports_dir(), &paths.host)? {
        let (text, export) = export::load(&path)?;
        found
            .by_sha
            .insert(export::sha8(&text), parse_offsets(&export, cores));
        found.latest = Some((
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            export,
        ));
    }
    Ok(found)
}

pub fn ryzen_smu(sys: &Sysfs) -> Option<RyzenSmu> {
    let dir = sys.sys.join("kernel/ryzen_smu_drv");
    if !dir.is_dir() {
        return None;
    }
    Some(RyzenSmu {
        version: env::read_text(&dir.join("version")).unwrap_or_else(|_| "?".into()),
        codename: env::read_text(&dir.join("codename")).unwrap_or_else(|_| "?".into()),
    })
}

pub fn build_status(
    host: &str,
    cores: &[(u32, Option<u64>)],
    export: Option<(String, Export)>,
    exports: &BTreeMap<String, Offsets>,
    sessions: &[StabilitySession],
    samples: &[Sample],
    smu: Option<RyzenSmu>,
) -> Status {
    let ids = cores.iter().map(|(core, _)| *core).collect::<Vec<_>>();
    let current = export
        .as_ref()
        .map(|(_, export)| parse_offsets(export, &ids))
        .unwrap_or_default();
    let evidence = evidence(sessions, exports, &ids);
    let latest = samples.last();
    let previous = latest.and_then(|latest| {
        samples
            .iter()
            .rev()
            .skip(1)
            .find(|sample| comparable(sample, latest))
    });
    let rows = cores
        .iter()
        .map(|(core, prefcore)| {
            let offset = current.get(core).copied();
            let proof = evidence.get(core).cloned().unwrap_or_default();
            let mops = latest
                .and_then(|sample| sample.cores.get(core))
                .map(|s| s.mops);
            let previous_mops = previous
                .and_then(|sample| sample.cores.get(core))
                .map(|s| s.mops);
            let drift = previous_mops.zip(mops).and_then(|(a, b)| drift_pct(a, b));
            CoreStatus {
                core: *core,
                prefcore: *prefcore,
                offset,
                best_passed: proof.best_passed(),
                shallowest_failed: proof.shallowest_failed(),
                mops,
                previous_mops,
                drift_pct: drift,
                stretching: stretching(drift),
                next: suggest(offset, &proof),
            }
        })
        .collect::<Vec<_>>();
    let (bios_changes, commands) = plan(&rows);
    Status {
        schema: 1,
        host: host.into(),
        mode: export.as_ref().and_then(|(_, export)| mode(export)),
        export: export.map(|(name, _)| name),
        ryzen_smu: smu,
        cores: rows,
        bios_changes,
        commands,
    }
}

fn number(value: Option<i32>) -> String {
    value.map_or("-".into(), |value| value.to_string())
}

pub fn render(status: &Status) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "  curve optimizer  {}{}\n",
        status.mode.as_deref().unwrap_or("no export"),
        status
            .export
            .as_deref()
            .map(|name| format!("  ({name})"))
            .unwrap_or_default()
    ));
    out.push_str(&format!(
        "  ryzen_smu        {}\n",
        status.ryzen_smu.as_ref().map_or(
            "not loaded; ryzen_smu-dkms-git from the AUR adds SMU telemetry".to_string(),
            |smu| format!("{} {}", smu.codename, smu.version)
        )
    ));
    let rows = status
        .cores
        .iter()
        .map(|core| {
            vec![
                core.core.to_string(),
                core.prefcore.map_or("-".into(), |rank| rank.to_string()),
                core.offset.map_or("?".into(), |offset| offset.to_string()),
                number(core.best_passed),
                number(core.shallowest_failed),
                core.mops.map_or("-".into(), |mops| format!("{mops:.1}")),
                match core.drift_pct {
                    Some(drift) if core.stretching => format!("{drift:+.1}% stretching?"),
                    Some(drift) => format!("{drift:+.1}%"),
                    None => "-".into(),
                },
                core.next.text(),
            ]
        })
        .collect::<Vec<_>>();
    for line in table::render(
        &[
            "core", "rank", "offset", "passed", "failed", "mops", "drift", "next",
        ],
        &rows,
    )
    .lines()
    {
        out.push_str(&format!("  {line}\n"));
    }
    if !status.bios_changes.is_empty() {
        out.push_str(&format!("  bios    {}\n", status.bios_changes.join("  ")));
    }
    for command in &status.commands {
        out.push_str(&format!("  stress  {command}\n"));
    }
    out
}

fn status(paths: Option<&Paths>, sys: &Sysfs, json: bool) -> Result<ExitCode, String> {
    let host = host_of(paths)?;
    let store = Store::discover();
    let cores = cpu::physical_cores(sys)?;
    let ranked = cpu::prefcore_ranking(sys, &cores);
    let exports = load_exports(paths, &cores)?;
    let status = build_status(
        &host,
        &ranked,
        exports.latest,
        &exports.by_sha,
        &load_sessions(&store, &host)?,
        &load_samples(&store, &host)?,
        ryzen_smu(sys),
    );
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).map_err(|e| e.to_string())?
        );
    } else {
        print!("{}", render(&status));
    }
    Ok(ExitCode::SUCCESS)
}

pub fn parse_mops(text: &str) -> Result<f64, String> {
    let payload: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "bench-workloads produced unreadable output")?;
    payload["value"]
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| "bench-workloads reported no value".into())
}

fn measure_core(taskset: &Path, binary: &Path, core: u32, iterations: u64) -> Result<f64, String> {
    let mut values = Vec::new();
    for _ in 0..3 {
        if crate::bench::runner::cancelled() {
            return Err("curve bench interrupted".into());
        }
        let text = require(
            Process::new(taskset)
                .arg("-c")
                .arg(core.to_string())
                .arg(binary)
                .args(["cpu", "--threads", "1", "--iterations"])
                .arg(iterations.to_string()),
            600,
        )?;
        values.push(parse_mops(&text)?);
    }
    values
        .iter()
        .copied()
        .reduce(f64::max)
        .ok_or_else(|| "no samples".into())
}

fn bench(paths: Option<&Paths>, sys: &Sysfs, iterations: u64) -> Result<ExitCode, String> {
    if iterations == 0 {
        return Err("--iterations must be at least 1".into());
    }
    let host = host_of(paths)?;
    let taskset = tool_path(&["taskset"]).ok_or("taskset not found")?;
    let binary = native::native_path()?.ok_or("bench-workloads binary not found")?;
    let _measurement = store::measurement_lock()?;
    let store = Store::discover();
    let cores = cpu::physical_cores(sys)?;
    let worker = worker_identity(&binary)?;
    let previous = load_samples(&store, &host)?
        .into_iter()
        .rev()
        .find(|sample| sample.worker == worker && sample.iterations == iterations);
    let bios = paths
        .and_then(|paths| {
            export::latest(&paths.exports_dir(), &paths.host)
                .ok()
                .flatten()
        })
        .and_then(|path| export::load(&path).ok())
        .map_or_else(|| "none".into(), |(text, _)| export::sha8(&text));
    let mut sample = Sample {
        schema: 1,
        host: host.clone(),
        taken: crate::time::now_iso(),
        bios,
        iterations,
        worker,
        worker_path: binary.display().to_string(),
        cores: BTreeMap::new(),
    };
    let mut rows = Vec::new();
    for (core, prefcore) in cpu::prefcore_ranking(sys, &cores) {
        let mops = measure_core(&taskset, &binary, core, iterations)?;
        let before = previous
            .as_ref()
            .and_then(|sample| sample.cores.get(&core))
            .map(|s| s.mops);
        let drift = before.and_then(|before| drift_pct(before, mops));
        rows.push(vec![
            core.to_string(),
            prefcore.map_or("-".into(), |rank| rank.to_string()),
            format!("{mops:.1}"),
            before.map_or("-".into(), |before| format!("{before:.1}")),
            match drift {
                Some(drift) if stretching(Some(drift)) => format!("{drift:+.1}% stretching?"),
                Some(drift) => format!("{drift:+.1}%"),
                None => "-".into(),
            },
        ]);
        sample.cores.insert(core, CoreSample { mops, prefcore });
    }
    let id = format!(
        "{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        std::process::id()
    );
    let path = host_dir(&store, &host, "curve")?.join(format!("{id}.json"));
    let mut bytes = serde_json::to_vec_pretty(&sample).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    store::atomic_create(&path, &bytes)?;
    print!(
        "{}",
        table::render(&["core", "rank", "mops", "previous", "drift"], &rows)
    );
    println!(
        "  bios {}  worker {} {}  saved {}",
        sample.bios,
        sample.worker,
        sample.worker_path,
        path.display()
    );
    Ok(ExitCode::SUCCESS)
}

pub fn run(command: Command, paths: Option<&Paths>, sys: &Sysfs) -> Result<ExitCode, String> {
    match command {
        Command::Status { json } => status(paths, sys, json),
        Command::Bench { iterations } => bench(paths, sys, iterations),
    }
}

#[cfg(test)]
#[path = "../tests/unit/curve_tests.rs"]
mod tests;
