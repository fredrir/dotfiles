use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use hostkit::process::{self, CaptureLimits};
use serde::{Deserialize, Serialize};

use crate::env::{Sysfs, read_text};

static UNSAFE_TRIAL: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
const INTERVAL: Duration = Duration::from_millis(250);

pub fn cancelled() -> bool {
    UNSAFE_TRIAL.load(Ordering::Acquire)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Evidence {
    pub passed: bool,
    pub reason: Option<String>,
    pub peak_temp_c: f64,
    pub max_temp_c: f64,
    pub samples: usize,
    pub journal_checked: bool,
    pub elapsed_seconds: f64,
}

#[derive(Clone, Debug)]
struct Sensor {
    input: PathBuf,
    label: String,
    limit: f64,
}

fn temperature(path: &Path) -> Result<f64, String> {
    let value = read_text(path)?
        .parse::<f64>()
        .map_err(|_| format!("invalid temperature: {}", path.display()))?
        / 1000.0;
    if !value.is_finite() || !(-30.0..=150.0).contains(&value) {
        return Err(format!("invalid temperature: {}", path.display()));
    }
    Ok(value)
}

fn sensors(sys: &Sysfs, requested_limit: Option<f64>) -> Result<Vec<Sensor>, String> {
    if requested_limit.is_some_and(|value| !value.is_finite() || !(40.0..=110.0).contains(&value)) {
        return Err("maximum CPU temperature must be between 40 and 110 C".into());
    }
    let root = sys.sys.join("class/hwmon");
    let mut found = Vec::new();
    for entry in fs::read_dir(&root)
        .map_err(|error| format!("CPU temperature telemetry unavailable: {error}"))?
    {
        let directory = entry.map_err(|error| error.to_string())?.path();
        let name = read_text(&directory.join("name"))?;
        if !matches!(
            name.as_str(),
            "coretemp" | "k10temp" | "k8temp" | "zenpower" | "cpu_thermal"
        ) {
            continue;
        }
        for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let Some(prefix) = name.to_str().and_then(|name| name.strip_suffix("_input")) else {
                continue;
            };
            if !prefix.strip_prefix("temp").is_some_and(|index| {
                !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            }) {
                continue;
            }
            let mut hardware_limit: Option<f64> = None;
            for suffix in ["max", "crit"] {
                let path = directory.join(format!("{prefix}_{suffix}"));
                if path.exists() {
                    let value = temperature(&path)?;
                    if value > 40.0 {
                        hardware_limit = Some(
                            hardware_limit
                                .map_or(value - 5.0, |previous| previous.min(value - 5.0)),
                        );
                    }
                }
            }
            let limit = match (requested_limit, hardware_limit) {
                (Some(requested), Some(hardware)) => requested.min(hardware),
                (Some(requested), None) => requested,
                (None, Some(hardware)) => hardware,
                (None, None) => {
                    return Err(format!(
                        "{} has no temperature limit; set --max-temp",
                        entry.path().display()
                    ));
                }
            };
            found.push(Sensor {
                input: entry.path(),
                label: read_text(&directory.join(format!("{prefix}_label")))
                    .unwrap_or_else(|_| prefix.to_owned()),
                limit,
            });
        }
    }
    found.sort_by(|left, right| left.input.cmp(&right.input));
    if found.is_empty() {
        return Err("supported CPU temperature telemetry unavailable".into());
    }
    Ok(found)
}

fn sample(sensors: &[Sensor], evidence: &mut Evidence) -> Result<(), String> {
    for sensor in sensors {
        let current = temperature(&sensor.input)?;
        evidence.peak_temp_c = evidence.peak_temp_c.max(current);
        if current >= sensor.limit {
            return Err(format!(
                "{} reached {current:.1} C (limit {:.1} C)",
                sensor.label, sensor.limit
            ));
        }
    }
    evidence.samples += 1;
    Ok(())
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "__CURSOR")]
    cursor: String,
    #[serde(rename = "MESSAGE")]
    message: String,
}

fn entries(text: &str, previous: Option<&str>) -> Result<Vec<Entry>, String> {
    let entries = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Entry>(line)
                .map_err(|error| format!("invalid kernel journal record: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let first = entries
        .first()
        .ok_or("kernel journal visibility unavailable")?;
    if entries.iter().any(|entry| entry.cursor.is_empty()) {
        return Err("kernel journal cursor unavailable".into());
    }
    if previous.is_some_and(|cursor| cursor != first.cursor) {
        return Err("kernel journal continuity lost during tuning".into());
    }
    Ok(entries)
}

fn journal(previous: Option<&str>, stopped: &AtomicBool) -> Result<Vec<Entry>, String> {
    let mut command = Command::new("journalctl");
    command.args([
        "--kernel",
        "--boot",
        "--no-pager",
        "--output=json",
        "--output-fields=MESSAGE",
    ]);
    match previous {
        Some(cursor) => {
            command.arg("--cursor").arg(cursor);
        }
        None => {
            command.args(["--lines", "1"]);
        }
    }
    let output = process::output_cancellable(
        &mut command,
        CaptureLimits {
            stdout: 4 * 1024 * 1024,
            stderr: 16 * 1024,
        },
        Duration::from_secs(2),
        &|| stopped.load(Ordering::Acquire) || ui_terminal::termination_requested(),
    )
    .map_err(|error| format!("kernel journal unavailable: {error}"))?;
    if !output.status.success()
        || output.stdout_truncated
        || output.stderr_truncated
        || !output.stderr.is_empty()
    {
        return Err(format!(
            "kernel journal visibility incomplete: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    entries(&String::from_utf8_lossy(&output.stdout), previous)
}

fn assess_journal(entries: &[Entry]) -> Result<(), String> {
    for entry in entries.iter().skip(1) {
        if crate::journal::classify(&entry.message).is_some() {
            return Err(format!("new hardware error: {}", entry.message));
        }
    }
    Ok(())
}

pub struct Monitor {
    finish: mpsc::Sender<()>,
    stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<Evidence>>,
}

impl Monitor {
    pub fn start(sys: &Sysfs, max_temp: Option<f64>) -> Result<Self, String> {
        let sensors = sensors(sys, max_temp)?;
        let mut evidence = Evidence {
            passed: true,
            reason: None,
            peak_temp_c: -30.0,
            max_temp_c: sensors
                .iter()
                .map(|sensor| sensor.limit)
                .fold(f64::INFINITY, f64::min),
            samples: 0,
            journal_checked: false,
            elapsed_seconds: 0.0,
        };
        sample(&sensors, &mut evidence)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let initial = journal(None, &stopped)?;
        let mut cursor = initial
            .last()
            .ok_or("kernel journal cursor unavailable")?
            .cursor
            .clone();
        evidence.journal_checked = true;
        if ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("another tuning monitor is active".into());
        }
        UNSAFE_TRIAL.store(false, Ordering::Release);
        let (finish, receiver) = mpsc::channel();
        let worker_stopped = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            let started = Instant::now();
            let mut next_journal = Instant::now();
            loop {
                let finishing =
                    receiver.recv_timeout(INTERVAL) != Err(mpsc::RecvTimeoutError::Timeout);
                let result = (|| {
                    if ui_terminal::termination_requested() {
                        return Err("tuning interrupted".to_owned());
                    }
                    sample(&sensors, &mut evidence)?;
                    if finishing || Instant::now() >= next_journal {
                        let batch = journal(Some(&cursor), &worker_stopped)?;
                        assess_journal(&batch)?;
                        cursor = batch
                            .last()
                            .ok_or("kernel journal cursor unavailable")?
                            .cursor
                            .clone();
                        next_journal = Instant::now() + Duration::from_secs(1);
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    evidence.passed = false;
                    evidence.reason = Some(error);
                    UNSAFE_TRIAL.store(true, Ordering::Release);
                    break;
                }
                if finishing {
                    break;
                }
            }
            evidence.elapsed_seconds = started.elapsed().as_secs_f64();
            evidence
        });
        Ok(Self {
            finish,
            stopped,
            worker: Some(worker),
        })
    }

    pub fn finish(mut self) -> Result<Evidence, String> {
        let _ = self.finish.send(());
        let result = self
            .worker
            .take()
            .ok_or("tuning monitor already finished")?
            .join()
            .map_err(|_| "tuning monitor panicked".to_owned());
        ACTIVE.store(false, Ordering::Release);
        result
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.stopped.store(true, Ordering::Release);
            let _ = self.finish.send(());
            if worker.join().is_err() {
                UNSAFE_TRIAL.store(true, Ordering::Release);
            }
            ACTIVE.store(false, Ordering::Release);
        }
    }
}

pub fn run_stress(sys: &Sysfs, seconds: u64, max_temp: Option<f64>) -> Result<Evidence, String> {
    if !(1..=3600).contains(&seconds) {
        return Err("stress duration must be between 1 and 3600 seconds".into());
    }
    let monitor = Monitor::start(sys, max_temp)?;
    let mut command = Command::new("stress-ng");
    command.args([
        "--cpu",
        "0",
        "--cpu-method",
        "all",
        "--verify",
        "--timeout",
        &format!("{seconds}s"),
        "--metrics-brief",
    ]);
    let started = Instant::now();
    let output = process::output_cancellable(
        &mut command,
        CaptureLimits::default(),
        Duration::from_secs(seconds + 10),
        &|| cancelled() || ui_terminal::termination_requested(),
    );
    let workload_elapsed = started.elapsed();
    let mut evidence = monitor.finish()?;
    if !evidence.passed {
        return Ok(evidence);
    }
    let failure = match output {
        Err(error) => Some(format!("stability test failed: {error}")),
        Ok(output) if !output.status.success() => Some(format!(
            "stability test failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Ok(output) if output.stdout_truncated || output.stderr_truncated => {
            Some("stability output truncated".into())
        }
        Ok(_) if workload_elapsed.as_secs_f64() < seconds as f64 * 0.9 => {
            Some("stability test ended before its validation interval".into())
        }
        Ok(_) => None,
    };
    if let Some(reason) = failure {
        evidence.passed = false;
        evidence.reason = Some(reason);
    }
    Ok(evidence)
}

#[cfg(test)]
#[path = "../../tests/unit/tune/monitor_tests.rs"]
mod tests;
