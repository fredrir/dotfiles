use super::{Job, Measurement, job, output};
use crate::bench::runner::{Setting, cancelled};
use crate::env::Sysfs;
use crate::gpu;
use crate::hwmon::{self, Hwmon};
use crate::power::Rapl;
use serde_json::json;
use std::{
    thread,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(8);
const INTERVAL: Duration = Duration::from_secs(1);
pub const PACKAGE: &str = "idle.package_w";
pub const GPU: &str = "idle.gpu_w";
pub const FANS: &str = "idle.fan_rpm";
pub const TCTL: &str = "idle.tctl_c";

pub struct Sources {
    pub rapl: Option<Rapl>,
    pub chip: Option<Hwmon>,
    pub cpu: Option<Hwmon>,
    pub gpu: bool,
}

impl Sources {
    pub fn discover(sys: &Sysfs) -> Self {
        Self {
            rapl: Rapl::discover(sys).ok(),
            chip: Hwmon::find(sys, hwmon::CHIP).ok(),
            cpu: Hwmon::find(sys, hwmon::CPU_SENSOR).ok(),
            gpu: gpu::query().is_ok(),
        }
    }

    pub fn keys(&self) -> Vec<(&'static str, &'static str)> {
        let mut keys = Vec::new();
        if self.rapl.is_some() {
            keys.push((PACKAGE, "W"));
        }
        if self.gpu {
            keys.push((GPU, "W"));
        }
        if self.chip.is_some() {
            keys.push((FANS, "rpm"));
        }
        if self.cpu.is_some() {
            keys.push((TCTL, "C"));
        }
        keys
    }
}

fn fan_rpm(chip: &Hwmon) -> Option<f64> {
    let readings = hwmon::CHANNELS
        .iter()
        .filter_map(|(_, channel)| chip.rpm(*channel).ok())
        .collect::<Vec<_>>();
    (!readings.is_empty()).then(|| readings.iter().map(|rpm| f64::from(*rpm)).sum())
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

pub fn sample_window(
    sources: &Sources,
    window: Duration,
    interval: Duration,
) -> Result<Measurement, String> {
    let started = Instant::now();
    let energy_before = sources.rapl.as_ref().map(Rapl::energy_uj).transpose()?;
    let mut gpu_w = Vec::new();
    let mut fans = Vec::new();
    let mut tctl = Vec::new();
    let mut samples = 0usize;
    loop {
        if cancelled() {
            return Err("benchmark interrupted".into());
        }
        if sources.gpu
            && let Ok(stats) = gpu::query()
        {
            gpu_w.push(stats.power_w);
        }
        if let Some(chip) = &sources.chip
            && let Some(rpm) = fan_rpm(chip)
        {
            fans.push(rpm);
        }
        if let Some(cpu) = &sources.cpu
            && let Ok(degrees) = cpu.temp_c(1)
        {
            tctl.push(degrees);
        }
        samples += 1;
        let elapsed = started.elapsed();
        if elapsed >= window {
            break;
        }
        thread::sleep(interval.min(window - elapsed));
    }
    let mut values = Vec::new();
    if let (Some(rapl), Some(before)) = (&sources.rapl, energy_before) {
        let after = rapl.energy_uj()?;
        let seconds = started.elapsed().as_secs_f64();
        values.push((
            PACKAGE.to_string(),
            vec![rapl.joules(before, after) / seconds],
        ));
    }
    for (key, collected) in [(GPU, &gpu_w), (FANS, &fans), (TCTL, &tctl)] {
        if let Some(average) = mean(collected) {
            values.push((key.to_string(), vec![average]));
        }
    }
    if values.is_empty() {
        return Err("no idle sources produced samples".into());
    }
    let mut measurement = Measurement::values(values);
    measurement.detail = json!({"samples": samples});
    Ok(measurement)
}

pub fn jobs(setting: &Setting) -> Result<Vec<Job>, String> {
    if !setting.accepts("idle") {
        return Ok(Vec::new());
    }
    let sources = Sources::discover(&Sysfs::from_env());
    let keys = sources.keys();
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let outputs = keys
        .iter()
        .map(|(key, scale)| output(key, scale, "LIB", "host"))
        .collect();
    Ok(vec![job(
        "idle",
        "hwtune",
        env!("CARGO_PKG_VERSION"),
        "idle/1.0.0",
        outputs,
        json!({"window_s": WINDOW.as_secs(), "interval_s": INTERVAL.as_secs()}),
        move || sample_window(&sources, WINDOW, INTERVAL),
    )])
}

#[cfg(test)]
#[path = "../../../tests/unit/bench/idle_tests.rs"]
mod tests;
