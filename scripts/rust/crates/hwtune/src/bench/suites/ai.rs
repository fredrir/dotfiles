use super::{Job, Measurement, capture, first_number, job, output, tool_path, version};
use crate::bench::runner::Setting;
use crate::gpu;
use crate::paths::Paths;
use regex::Regex;
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use workstation::blocks::{self, Comments};

pub const BLOCK: &str = "ai";
pub const DEFAULT_PREDICT: u32 = 128;
const TIMEOUT_SECONDS: u64 = 600;
const PROMPT: &str = "Summarize the following engineering notes into a clear plan with numbered steps. \
The workstation runs a rolling Linux distribution on an eight core processor with simultaneous \
multithreading, thirty two gigabytes of memory in two modules, a discrete graphics card with sixteen \
gigabytes of video memory, and a two terabyte solid state drive. Most of the time the machine sits \
idle in a text console while a laptop connects over the network to run containers, compile large \
projects, and forward ports for remote development sessions. When a build starts, every core should \
reach its highest sustained clock without exceeding the thermal limit set in firmware, and the fans \
should ramp smoothly rather than oscillate. When the build finishes, the processor should return to \
its lowest idle state quickly so the room stays quiet. The graphics card serves language model \
inference; prompt processing is bound by compute throughput while token generation is bound by \
memory bandwidth, so the power cap can drop noticeably before generation speed suffers. Memory \
timings beyond the primary latencies were never tuned, the fabric clock is automatic, and the curve \
optimizer uses one conservative offset for every core although the cores differ in quality. The \
benchmark suite records compile time, idle power, fan speed, memory latency, scheduler wake latency, \
and tokens per second, and it refuses to compare runs whose measurement method, tool version, or \
hardware identity changed. Firmware settings are imported from exports and checked against a \
tracked specification, while operating system controls are applied through a transaction that \
restores the original values on failure. Write the plan for a careful engineer who wants measurable \
gains, reversible changes, and a quiet machine at idle, and finish with the single most valuable \
next experiment.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub model: PathBuf,
    pub predict: u32,
}

pub fn settings_path(paths: &Paths) -> PathBuf {
    paths
        .root
        .join("config/hwtune")
        .join(format!("{}.bench.dotfile", paths.host))
}

pub fn expand_home(value: &str, home: Option<&Path>) -> PathBuf {
    match (value.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(value),
    }
}

pub fn parse_settings(text: &str, home: Option<&Path>) -> Result<Option<Settings>, String> {
    let mut model = None;
    let mut predict = DEFAULT_PREDICT;
    for entry in blocks::parse_with_comments(text, Comments::Lines)? {
        if entry.opens || entry.block != BLOCK {
            continue;
        }
        let (key, value) = entry.split();
        if value.is_empty() {
            return Err(format!("line {}: {key} has no value", entry.number));
        }
        match key {
            "model" => model = Some(expand_home(value, home)),
            "predict" => {
                predict = value
                    .parse::<u32>()
                    .ok()
                    .filter(|count| *count > 0)
                    .ok_or_else(|| format!("line {}: predict is not a count", entry.number))?;
            }
            _ => return Err(format!("line {}: unknown {BLOCK} key {key}", entry.number)),
        }
    }
    Ok(model.map(|model| Settings { model, predict }))
}

pub fn load_settings(path: &Path, home: Option<&Path>) -> Result<Option<Settings>, String> {
    match fs::read_to_string(path) {
        Ok(text) => parse_settings(&text, home).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub fn parse_summary(text: &str) -> Result<(f64, f64), String> {
    let pattern =
        Regex::new(r"\[\s*Prompt:\s*([\d.]+)\s*t/s\s*\|\s*Generation:\s*([\d.]+)\s*t/s\s*\]")
            .map_err(|e| e.to_string())?;
    let found = pattern
        .captures_iter(text)
        .last()
        .ok_or("llama-cli reported no throughput summary")?;
    let rate = |index: usize| {
        found[index]
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value > 0.0)
            .ok_or_else(|| format!("llama-cli throughput is not a rate: {}", &found[index]))
    };
    Ok((rate(1)?, rate(2)?))
}

pub fn parse_version(text: &str) -> String {
    first_number(text, r"build\s+(\d+)").unwrap_or_default()
}

pub fn arguments(settings: &Settings) -> Vec<String> {
    vec![
        "-m".into(),
        settings.model.display().to_string(),
        "-p".into(),
        PROMPT.into(),
        "-n".into(),
        settings.predict.to_string(),
        "-st".into(),
        "--no-display-prompt".into(),
        "-ngl".into(),
        "99".into(),
        "--temp".into(),
        "0".into(),
        "-s".into(),
        "1".into(),
        "--perf".into(),
    ]
}

fn sample_gpu(stop: Arc<AtomicBool>) -> JoinHandle<Vec<f64>> {
    thread::spawn(move || {
        let mut samples = Vec::new();
        while !stop.load(Ordering::Acquire) {
            if let Ok(stats) = gpu::query()
                && stats.power_w.is_finite()
            {
                samples.push(stats.power_w);
            }
            thread::sleep(Duration::from_millis(100));
        }
        samples
    })
}

pub fn active_mean(samples: &[f64]) -> Option<f64> {
    let low = samples.iter().copied().fold(f64::INFINITY, f64::min);
    let high = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let active = samples
        .iter()
        .filter(|value| **value >= (low + high) / 2.0)
        .collect::<Vec<_>>();
    (!active.is_empty()).then(|| active.iter().copied().sum::<f64>() / active.len() as f64)
}

pub fn measure(binary: &Path, settings: &Settings) -> Result<Measurement, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = sample_gpu(stop.clone());
    let result = capture(
        Command::new(binary).args(arguments(settings)),
        TIMEOUT_SECONDS,
    );
    stop.store(true, Ordering::Release);
    let samples = sampler.join().map_err(|_| "GPU sampler panicked")?;
    let result = result?;
    if !result.status.success() {
        return Err(format!("llama-cli exited {}", result.status));
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let (prompt, generate) = parse_summary(&text)?;
    let mut values = vec![
        ("ai.prompt_tps".to_string(), vec![prompt]),
        ("ai.generate_tps".to_string(), vec![generate]),
    ];
    if let Some(watts) = active_mean(&samples) {
        values.push(("ai.gpu_w".to_string(), vec![watts]));
    }
    Ok(Measurement::values(values))
}

pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts(BLOCK) {
        return Ok(Vec::new());
    }
    let Some(binary) = tool_path(&["llama-cli"]) else {
        return Ok(Vec::new());
    };
    let Ok(paths) = Paths::discover(None) else {
        return Ok(Vec::new());
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let Some(settings) = load_settings(&settings_path(&paths), home.as_deref())? else {
        return Ok(Vec::new());
    };
    let Ok(model) = fs::metadata(&settings.model) else {
        return Ok(Vec::new());
    };
    if !model.is_file() {
        return Ok(Vec::new());
    }
    let ver = version(&binary, &["--version"], r"build\s+(\d+)");
    let detail = json!({
        "model": settings.model.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default(),
        "model_bytes": model.len(),
        "predict": settings.predict,
        "prompt_words": PROMPT.split_whitespace().count(),
        "gpu_w": "mean of samples in the upper half of the observed range",
    });
    Ok(vec![job(
        BLOCK,
        "llama-cli",
        &ver,
        "ai.llama/1.0.0",
        vec![
            output("ai.prompt_tps", "t/s", "HIB", "host"),
            output("ai.generate_tps", "t/s", "HIB", "host"),
            output("ai.gpu_w", "W", "LIB", "host"),
        ],
        detail,
        move || measure(&binary, &settings),
    )])
}

#[cfg(test)]
#[path = "../../../tests/unit/bench/ai_tests.rs"]
mod tests;
