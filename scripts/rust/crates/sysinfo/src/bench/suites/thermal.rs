use super::{Job, Measurement, capture_for, job, output, tool_path, version};
use crate::bench::runner::Setting;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};
pub fn sensor_temperature(timeout: Duration) -> Option<f64> {
    if cfg!(target_os = "macos") {
        return None;
    }
    let result = capture_for(
        Command::new("sensors").arg("-j"),
        timeout.min(Duration::from_secs(10)),
    )
    .ok()?;
    if !result.status.success() {
        return None;
    }
    let payload: Value = serde_json::from_slice(&result.stdout).ok()?;
    let mut best: Option<f64> = None;
    for (chip, readings) in payload.as_object()? {
        if !["k10temp", "coretemp", "zenpower"]
            .iter()
            .any(|mark| chip.to_lowercase().contains(mark))
        {
            continue;
        }
        for entries in readings
            .as_object()
            .into_iter()
            .flat_map(|object| object.values())
        {
            for (name, value) in entries.as_object().into_iter().flatten() {
                if name.contains("input")
                    && let Some(value) = value.as_f64()
                {
                    best = Some(best.map_or(value, |before| before.max(value)));
                }
            }
        }
    }
    best
}
pub fn package_clock() -> Option<f64> {
    let text = fs::read_to_string("/proc/cpuinfo").ok()?;
    let values = text
        .lines()
        .filter(|line| line.to_lowercase().starts_with("cpu mhz"))
        .filter_map(|line| line.split_once(':')?.1.trim().parse::<f64>().ok())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}
fn sample_before_deadline<T>(
    deadline: Instant,
    probe: impl FnOnce(Duration) -> Option<T>,
) -> Option<T> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    let value = probe(remaining)?;
    (Instant::now() < deadline).then_some(value)
}
pub fn sustained(path: &Path, seconds: u64) -> Result<Measurement, String> {
    let path = path.to_path_buf();
    let workers = std::thread::available_parallelism().map_or(2, usize::from);
    let (sender, receiver) = mpsc::channel();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let worker = std::thread::spawn(move || {
        let reply = hostkit::process::output_terminating(
            Command::new(path).args([
                "--matrix",
                &workers.to_string(),
                "--timeout",
                &format!("{seconds}s"),
                "--metrics-brief",
            ]),
            hostkit::process::CaptureLimits {
                stdout: 0,
                stderr: 128 * 1024,
            },
            Duration::from_secs(seconds),
            Duration::from_secs(30),
            &ui_terminal::termination_requested,
        )
        .map_err(|error| error.to_string());
        let _ = sender.send(reply);
    });
    let mut temperatures = Vec::new();
    let mut clocks = Vec::new();
    let result = loop {
        match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => break result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(value) = sample_before_deadline(deadline, sensor_temperature) {
                    temperatures.push(value);
                }
                if let Some(value) = sample_before_deadline(deadline, |_| package_clock()) {
                    clocks.push(value);
                }
            }
            Err(error) => break Err(error.to_string()),
        }
    };
    worker.join().map_err(|_| "thermal worker failed")?;
    let result = result?;
    if !result.deadline_reached && !result.output.status.success() {
        let text = String::from_utf8_lossy(&result.output.stderr);
        let detail = text
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        return Err(format!(
            "stress-ng exited {}: {detail}",
            result.output.status
        ));
    }
    if temperatures.is_empty() && clocks.is_empty() {
        return Err("no thermal telemetry was available during the load".into());
    }
    let mut values = Vec::new();
    if !temperatures.is_empty() {
        values.push((
            "thermal.peak".into(),
            vec![
                temperatures
                    .iter()
                    .copied()
                    .fold(f64::NEG_INFINITY, f64::max),
            ],
        ));
        let tail = &temperatures[temperatures.len() / 2..];
        values.push((
            "thermal.steady".into(),
            vec![tail.iter().sum::<f64>() / tail.len() as f64],
        ));
    }
    if !clocks.is_empty() {
        let tail = &clocks[clocks.len() / 2..];
        values.push((
            "cpu.sustained_clock".into(),
            vec![tail.iter().sum::<f64>() / tail.len() as f64],
        ));
    }
    Ok(Measurement::values(values))
}

pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("thermal") || setting.tier == "quick" {
        return Ok(Vec::new());
    }
    let Some(path) = tool_path(&["stress-ng"]) else {
        return Ok(Vec::new());
    };
    let seconds = if setting.tier == "heavy" { 120 } else { 60 };
    let ver = version(&path, &["--version"], r"(\d[\d.]*)");
    let mut result = job(
        "thermal",
        "stress-ng",
        &ver,
        "thermal/1.0.0",
        vec![
            output("thermal.peak", "°C", "LIB", "host"),
            output("thermal.steady", "°C", "LIB", "host"),
            output("cpu.sustained_clock", "MHz", "HIB", "host"),
        ],
        json!({"seconds":seconds,"stressor":"matrix","interval":2.0}),
        move || sustained(&path, seconds),
    );
    result.repeat = false;
    Ok(vec![result])
}

#[cfg(test)]
#[path = "../../../tests/thermal_tests.rs"]
mod tests;
