use super::{record::Metric, runner::Setting};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
pub mod disk;
pub mod gpu;
pub mod thermal;
pub mod workload;

pub const WRITTEN: &str = "__bytes_written";
static NUMBERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-?\d+(?:\.\d+)?").expect("numeric regex"));
pub type Samples = BTreeMap<String, Vec<f64>>;
pub struct Measurement {
    pub values: Samples,
    pub detail: Value,
}
impl Measurement {
    pub fn values(values: impl IntoIterator<Item = (String, Vec<f64>)>) -> Self {
        Self {
            values: values.into_iter().collect(),
            detail: json!({}),
        }
    }
}
pub struct Job {
    pub name: String,
    pub tool: String,
    pub version: String,
    pub method: String,
    pub outputs: Vec<Metric>,
    pub writes: u64,
    pub repeat: bool,
    pub detail: Value,
    pub measure: Box<dyn FnMut() -> Result<Measurement, String> + Send>,
}
pub fn output(key: &str, scale: &str, proportion: &str, comparable: &str) -> Metric {
    Metric {
        key: key.into(),
        scale: scale.into(),
        proportion: proportion.into(),
        comparable: comparable.into(),
        ..Metric::default()
    }
}
pub fn job(
    name: &str,
    tool: &str,
    version: &str,
    method: &str,
    outputs: Vec<Metric>,
    detail: Value,
    measure: impl FnMut() -> Result<Measurement, String> + Send + 'static,
) -> Job {
    Job {
        name: name.into(),
        tool: tool.into(),
        version: version.into(),
        method: method.into(),
        outputs,
        writes: 0,
        repeat: true,
        detail,
        measure: Box::new(measure),
    }
}
pub fn tool_path(names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|name| {
        let path = Path::new(name);
        if path.components().count() > 1 {
            return executable(path).then(|| path.to_path_buf());
        }
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join(name))
            .find(|path| executable(path))
    })
}
pub fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
pub fn capture(
    command: &mut Command,
    seconds: u64,
) -> Result<hostkit::process::CapturedOutput, String> {
    capture_for(command, Duration::from_secs(seconds))
}
pub fn capture_for(
    command: &mut Command,
    timeout: Duration,
) -> Result<hostkit::process::CapturedOutput, String> {
    let result = hostkit::process::output_cancellable(
        command.stdin(Stdio::null()),
        hostkit::process::CaptureLimits {
            stdout: 16 * 1024 * 1024,
            stderr: 128 * 1024,
        },
        timeout,
        &crate::bench::runner::cancelled,
    )
    .map_err(|e| format!("{}: {e}", command.get_program().to_string_lossy()))?;
    if result.stdout_truncated {
        return Err(format!(
            "{} output exceeds 16 MiB",
            command.get_program().to_string_lossy()
        ));
    }
    Ok(result)
}
pub fn require(command: &mut Command, seconds: u64) -> Result<String, String> {
    let result = capture(command, seconds)?;
    if !result.status.success() {
        return Err(format!(
            "{} exited {}",
            command.get_program().to_string_lossy(),
            result.status
        ));
    }
    String::from_utf8(result.stdout).map_err(|_| "command produced non-UTF-8 output".into())
}
pub fn version(path: &Path, args: &[&str], pattern: &str) -> String {
    let Ok(result) = capture(Command::new(path).args(args), 15) else {
        return String::new();
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    first_number(&text, pattern).unwrap_or_default()
}
pub fn first_number(text: &str, pattern: &str) -> Option<String> {
    Regex::new(pattern)
        .ok()?
        .captures(text)?
        .get(1)
        .map(|m| m.as_str().into())
}
pub fn number(text: &str, pattern: &str, error: &str) -> Result<f64, String> {
    first_number(text, pattern)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .ok_or_else(|| error.into())
}
pub fn parse_size(value: &str) -> u64 {
    let Some(last) = value.chars().last() else {
        return 0;
    };
    let multiplier = match last.to_ascii_lowercase() {
        'k' => 1024.0,
        'm' => 1048576.0,
        'g' => 1073741824.0,
        _ => return value.parse().unwrap_or(0),
    };
    value[..value.len() - 1]
        .parse::<f64>()
        .map_or(0, |n| (n * multiplier) as u64)
}
pub fn scalar(key: &str, value: f64) -> Measurement {
    Measurement::values([(key.to_string(), vec![value])])
}
pub fn collect_jobs(setting: &Setting) -> (Vec<Job>, Vec<String>) {
    let mut jobs = Vec::new();
    let mut failures = Vec::new();
    for (name, builder) in [
        ("cpu", cpu as fn(&Setting) -> Result<Vec<Job>, String>),
        ("native", native),
        ("memory", memory),
        ("disk", disk::jobs),
        ("gpu", gpu::jobs),
        ("workload", workload::jobs),
        ("thermal", thermal::jobs),
    ] {
        match builder(setting) {
            Ok(found) => jobs.extend(found.into_iter().filter(|job| {
                job.outputs
                    .first()
                    .is_some_and(|o| setting.accepts(o.family()))
            })),
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    (jobs, failures)
}

fn cpu(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("cpu") {
        return Ok(Vec::new());
    }
    let mut jobs = Vec::new();
    if let Some(path) = tool_path(&["7z", "7zz"]) {
        let ver = version(
            &path,
            &[],
            r"7-Zip\s*(?:\([^)]*\)|\[[^\]]*\])?\s*(\d[\d.]*)",
        );
        let tool = path.file_name().unwrap_or_default().to_string_lossy();
        for (key, single) in [("cpu.single", true), ("cpu.multi", false)] {
            let binary = path.clone();
            jobs.push(job(
                key,
                &tool,
                &ver,
                &format!("{key}/1.0.0"),
                vec![output(key, "MIPS", "HIB", "world")],
                json!({"dictionary":"-md22","passes":"1"}),
                move || {
                    let mut command = Command::new(&binary);
                    command.args(["b", "1", "-md22"]);
                    if single {
                        command.arg("-mmt1");
                    }
                    let text = require(&mut command, 300)?;
                    let line = text
                        .lines()
                        .find_map(|line| line.strip_prefix("Tot:"))
                        .ok_or("7z produced no total rating")?;
                    let values = NUMBERS
                        .find_iter(line)
                        .filter_map(|m| m.as_str().parse::<f64>().ok())
                        .collect::<Vec<_>>();
                    if values.len() < 3 {
                        return Err("7z total rating is incomplete".into());
                    }
                    Ok(scalar(key, *values.last().unwrap()))
                },
            ));
        }
    }
    if let Some(path) = tool_path(&[
        "/opt/homebrew/opt/openssl@3/bin/openssl",
        "/usr/local/opt/openssl@3/bin/openssl",
        "openssl",
    ]) {
        let implementation = require(Command::new(&path).arg("version"), 15)
            .unwrap_or_else(|_| "unknown".into())
            .split_whitespace()
            .next()
            .unwrap_or("unknown")
            .to_string();
        let ver = version(&path, &["version"], r"(\d[\d.]*)");
        jobs.push(job(
            "cpu.crypto",
            &implementation.to_lowercase(),
            &ver,
            "cpu.crypto/1.0.0",
            vec![output("cpu.crypto", "MB/s", "HIB", "world")],
            json!({"cipher":"aes-256-gcm","block":"16384","implementation":implementation}),
            move || {
                let text = require(
                    Command::new(&path).args([
                        "speed",
                        "-evp",
                        "aes-256-gcm",
                        "-seconds",
                        "1",
                        "-bytes",
                        "16384",
                    ]),
                    180,
                )?;
                let line = text
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("aes-256-gcm"))
                    .ok_or("openssl reported no throughput")?;
                let value = number(
                    line.split_whitespace().last().unwrap_or(""),
                    r"([\d.]+)",
                    "openssl reported no throughput",
                )?;
                Ok(scalar("cpu.crypto", value / 1000.0))
            },
        ));
    }
    Ok(jobs)
}
fn memory(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("mem") && !setting.accepts("cache") {
        return Ok(Vec::new());
    }
    let Some(path) = tool_path(&["sysbench"]) else {
        return Ok(Vec::new());
    };
    let ver = version(&path, &["--version"], r"(\d[\d.]*)");
    let block = if setting.memory_bytes > 0 && setting.memory_bytes < 8 * 1024_u64.pow(3) {
        "256M"
    } else {
        "1G"
    };
    let mut jobs = Vec::new();
    for (name, method, block, mode, scope, keys) in [
        (
            "mem.bandwidth",
            "mem.bandwidth/3.0.0",
            block,
            "seq",
            "world",
            vec![("mem.write", "write"), ("mem.read", "read")],
        ),
        (
            "mem.random",
            "mem.random/3.0.0",
            block,
            "rnd",
            "world",
            vec![("mem.random", "read")],
        ),
        (
            "cache.bandwidth",
            "cache.bandwidth/1.0.0",
            "1M",
            "seq",
            "host",
            vec![("cache.write", "write"), ("cache.read", "read")],
        ),
    ] {
        let binary = path.clone();
        let outputs = keys
            .iter()
            .map(|(key, _)| output(key, "MiB/s", "HIB", scope))
            .collect();
        jobs.push(job(
            name,
            "sysbench",
            &ver,
            method,
            outputs,
            json!({ "block": block, "seconds": "2", "mode": mode, "threads": 1, "working_set": parse_size(block) }),
            move || {
                let mut values = Vec::new();
                for (key, operation) in &keys {
                    let text = require(Command::new(&binary).args([
                        "memory",
                        &format!("--memory-block-size={block}"),
                        "--memory-total-size=64G",
                        &format!("--memory-oper={operation}"),
                        &format!("--memory-access-mode={mode}"),
                        "--threads=1",
                        "--time=2",
                        "run",
                    ]), 120)?;
                    let throughput = number(&text, r"\(([\d.]+)\s*MiB/sec\)", &format!("sysbench reported no {mode} {operation} throughput"))?;
                    values.push((key.to_string(), vec![throughput]));
                }
                Ok(Measurement::values(values))
            },
        ));
    }
    Ok(jobs)
}
pub fn native_path() -> Result<Option<PathBuf>, String> {
    workstation::native::Resolver::discover(sysinfo::inventory::repo_root())
        .resolve("bench-workloads")
}
fn native(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("cpu") && !setting.accepts("mem") {
        return Ok(Vec::new());
    }
    let Some(path) = native_path()? else {
        return Ok(Vec::new());
    };
    let ver = version(&path, &["--version"], r"(\d[\d.]*)");
    let mut jobs = Vec::new();
    for (name, scale, detail, measurements) in [
        (
            "cpu.native",
            "Mops/s",
            json!({"iterations":800000000}),
            vec![
                (
                    "cpu.native_single",
                    vec!["cpu", "--threads", "1", "--iterations", "800000000"],
                ),
                (
                    "cpu.native_multi",
                    vec!["cpu", "--threads", "0", "--iterations", "800000000"],
                ),
            ],
        ),
        (
            "mem.native",
            "GiB/s",
            json!({"buffer_mib":256,"passes":128,"threads":1}),
            vec![
                (
                    "mem.native_read",
                    vec!["memory", "--op", "read", "--mib", "256", "--passes", "128"],
                ),
                (
                    "mem.native_write",
                    vec!["memory", "--op", "write", "--mib", "256", "--passes", "128"],
                ),
            ],
        ),
    ] {
        let binary = path.clone();
        let outputs = measurements
            .iter()
            .map(|(key, _)| output(key, scale, "HIB", "world"))
            .collect();
        jobs.push(job(
            name,
            "bench-workloads",
            &ver,
            &format!("{name}/1.0.0"),
            outputs,
            detail,
            move || {
                let mut values = Vec::new();
                for (key, args) in &measurements {
                    let text = require(Command::new(&binary).args(args), 300)?;
                    let payload: Value = serde_json::from_str(&text)
                        .map_err(|_| "bench-workloads produced unreadable output")?;
                    let value = payload["value"]
                        .as_f64()
                        .filter(|n| n.is_finite() && *n > 0.0)
                        .ok_or("bench-workloads reported no value")?;
                    values.push((key.to_string(), vec![value]));
                }
                Ok(Measurement::values(values))
            },
        ));
    }
    Ok(jobs)
}
