use std::fs::File;
use std::io::{self, IsTerminal, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use ui_progress::{Reporter, Spinner};
use ui_terminal::reduced_motion_requested;

use crate::env::Sysfs;
use crate::gpu;
use crate::hwmon::{self, Hwmon};
use crate::journal::{self, Counts};
use crate::paths;
use crate::time;

pub const CSV_HEADER: &str = "t_iso,elapsed_s,tctl_c,vrm_c,gpu_c,radiator_pwm,radiator_rpm,pump_pwm,pump_rpm,case_pwm,case_rpm,vrm_pwm,vrm_rpm,journal_new";
const INTERVAL: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(250);
const PROGRESS_EVERY: usize = 12;

pub struct Sources {
    cpu: Option<Hwmon>,
    chip: Option<Hwmon>,
    gpu: bool,
}

impl Sources {
    pub fn discover(sys: &Sysfs) -> Self {
        Self {
            cpu: Hwmon::find(sys, hwmon::CPU_SENSOR).ok(),
            chip: Hwmon::find(sys, hwmon::CHIP).ok(),
            gpu: gpu::query().is_ok(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub elapsed: u64,
    pub tctl: Option<f64>,
    pub vrm: Option<f64>,
    pub gpu: Option<f64>,
    pub channels: [(Option<u8>, Option<u32>); 4],
    pub journal_new: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Peaks {
    pub tctl: Option<f64>,
    pub vrm: Option<f64>,
    pub gpu: Option<f64>,
    pub pwm: [Option<u8>; 4],
    pub rpm: [Option<u32>; 4],
    pub samples: usize,
}

fn max_f64(current: Option<f64>, next: Option<f64>) -> Option<f64> {
    match (current, next) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

impl Peaks {
    pub fn fold(&mut self, sample: &Sample) {
        self.samples += 1;
        self.tctl = max_f64(self.tctl, sample.tctl);
        self.vrm = max_f64(self.vrm, sample.vrm);
        self.gpu = max_f64(self.gpu, sample.gpu);
        for (index, (pwm, rpm)) in sample.channels.iter().enumerate() {
            self.pwm[index] = self.pwm[index].max(*pwm);
            self.rpm[index] = self.rpm[index].max(*rpm);
        }
    }

    pub fn keys(&self) -> Vec<(String, String)> {
        let degrees =
            |value: Option<f64>| value.map_or("n/a".to_string(), |value| format!("{value:.1}"));
        let rpm = |value: Option<u32>| value.map_or("n/a".to_string(), |value| value.to_string());
        vec![
            ("peak_tctl".into(), degrees(self.tctl)),
            ("peak_vrm".into(), degrees(self.vrm)),
            ("peak_gpu".into(), degrees(self.gpu)),
            ("peak_radiator".into(), rpm(self.rpm[0])),
            ("peak_pump".into(), rpm(self.rpm[1])),
            ("peak_case".into(), rpm(self.rpm[2])),
            ("peak_vrm_fan".into(), rpm(self.rpm[3])),
        ]
    }

    pub fn rows(&self) -> Vec<Vec<String>> {
        self.keys()
            .into_iter()
            .map(|(key, value)| vec![key.trim_start_matches("peak_").replace('_', " "), value])
            .collect()
    }
}

pub struct Finish {
    pub status: Option<ExitStatus>,
    pub elapsed: Duration,
    pub timed_out: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Evidence {
    pub journal_visible: bool,
    pub journal_broken: bool,
    pub samples: usize,
    pub cpu_samples: usize,
    pub gpu_samples: usize,
}

impl Evidence {
    pub fn missing(&self, profile: &str) -> Vec<String> {
        let mut missing = Vec::new();
        if !self.journal_visible || self.journal_broken {
            missing.push("kernel journal unavailable or incomplete".into());
        }
        let measured = if profile == "gpu" {
            self.gpu_samples
        } else {
            self.cpu_samples
        };
        if self.samples == 0 || measured != self.samples {
            missing.push(format!(
                "{} temperature sampling unavailable or incomplete",
                if profile == "gpu" { "GPU" } else { "CPU" }
            ));
        }
        missing
    }

    pub fn verdict(&self, profile: &str, workload_ok: bool, journal_errors: usize) -> &'static str {
        if !workload_ok || journal_errors > 0 {
            "fail"
        } else if self.missing(profile).is_empty() {
            "pass"
        } else {
            "unknown"
        }
    }
}

pub struct Monitor {
    sources: Sources,
    csv: File,
    path: PathBuf,
    pub peaks: Peaks,
    pub journal: Counts,
    journal_lines: usize,
    pub evidence: Evidence,
    start_epoch: u64,
    quiet: bool,
    live: Option<Reporter<Stdout>>,
    motion: bool,
}

pub fn csv_row(iso: &str, sample: &Sample) -> String {
    let degrees = |value: Option<f64>| value.map_or(String::new(), |value| format!("{value:.1}"));
    let mut fields = vec![
        iso.to_string(),
        sample.elapsed.to_string(),
        degrees(sample.tctl),
        degrees(sample.vrm),
        degrees(sample.gpu),
    ];
    for (pwm, rpm) in &sample.channels {
        fields.push(pwm.map_or(String::new(), |value| value.to_string()));
        fields.push(rpm.map_or(String::new(), |value| value.to_string()));
    }
    fields.push(sample.journal_new.to_string());
    fields.join(",")
}

impl Monitor {
    pub fn start(session: &str, sys: &Sysfs, quiet: bool) -> Result<Self, String> {
        let dir = paths::cache_dir()?;
        let path = dir.join(format!("{session}.csv"));
        let mut csv = super::touch(&path)?;
        writeln!(csv, "{CSV_HEADER}").map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self {
            sources: Sources::discover(sys),
            csv,
            path,
            peaks: Peaks::default(),
            journal: Counts::default(),
            journal_lines: 0,
            evidence: Evidence {
                journal_visible: journal::since(None).is_ok_and(|lines| !lines.is_empty()),
                ..Evidence::default()
            },
            start_epoch: time::epoch_now(),
            quiet,
            live: (!quiet && io::stdout().is_terminal()).then(|| Reporter::new(io::stdout())),
            motion: !reduced_motion_requested("HWTUNE_REDUCED_MOTION"),
        })
    }

    pub fn csv_path(&self) -> &Path {
        &self.path
    }

    fn poll_journal(&mut self) -> usize {
        if self.evidence.journal_broken {
            return 0;
        }
        match journal::errors_since(Some(self.start_epoch)) {
            Ok(lines) => {
                let fresh = lines.len().saturating_sub(self.journal_lines);
                self.journal_lines = lines.len();
                self.journal = journal::counts(&lines);
                fresh
            }
            Err(e) => {
                self.evidence.journal_broken = true;
                eprintln!("  journal polling stopped: {e}");
                0
            }
        }
    }

    fn sample(&mut self, elapsed: Duration) -> Result<Sample, String> {
        let mut sample = Sample {
            elapsed: elapsed.as_secs(),
            ..Sample::default()
        };
        if let Some(cpu) = &self.sources.cpu {
            sample.tctl = cpu.temp_c(1).ok();
        }
        if let Some(chip) = &self.sources.chip {
            sample.vrm = chip.temp_c(hwmon::VRM_TEMP_CHANNEL).ok();
            for (index, (_, channel)) in hwmon::CHANNELS.iter().enumerate() {
                sample.channels[index] = (chip.pwm(*channel).ok(), chip.rpm(*channel).ok());
            }
        }
        if self.sources.gpu {
            sample.gpu = gpu::query().ok().map(|stats| stats.temp_c);
        }
        sample.journal_new = self.poll_journal();
        self.evidence.samples += 1;
        self.evidence.cpu_samples += usize::from(sample.tctl.is_some_and(f64::is_finite));
        self.evidence.gpu_samples += usize::from(sample.gpu.is_some_and(f64::is_finite));
        self.peaks.fold(&sample);
        writeln!(self.csv, "{}", csv_row(&time::now_iso(), &sample))
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        Ok(sample)
    }

    fn progress(&mut self, sample: &Sample) {
        if self.quiet {
            return;
        }
        let degrees =
            |value: Option<f64>| value.map_or("n/a".into(), |value| format!("{value:.0}°C"));
        let line = format!(
            "{:>5}s  tctl {}  vrm {}  gpu {}  radiator {} rpm  journal {}",
            sample.elapsed,
            degrees(sample.tctl),
            degrees(sample.vrm),
            degrees(sample.gpu),
            sample.channels[0]
                .1
                .map_or("n/a".into(), |rpm| rpm.to_string()),
            self.journal.summary()
        );
        if let Some(reporter) = &mut self.live {
            let frame = Spinner::default().frame(self.peaks.samples as u64, self.motion);
            let _ = reporter.update(&format!("  {frame} {line}"));
        } else if self.peaks.samples % PROGRESS_EVERY == 1 {
            println!("  {line}");
        }
    }

    fn clear_progress(&mut self) {
        if let Some(reporter) = &mut self.live {
            let _ = reporter.finish();
        }
    }

    pub fn run(
        &mut self,
        budget: Duration,
        mut child: Option<&mut Child>,
    ) -> Result<Finish, String> {
        let start = Instant::now();
        let mut next = start;
        loop {
            let elapsed = start.elapsed();
            if Instant::now() >= next {
                let sample = self.sample(elapsed)?;
                self.progress(&sample);
                next += INTERVAL;
            }
            if let Some(child) = child.as_deref_mut()
                && let Some(status) = child.try_wait().map_err(|e| format!("wait: {e}"))?
            {
                self.sample(start.elapsed())?;
                self.clear_progress();
                return Ok(Finish {
                    status: Some(status),
                    elapsed: start.elapsed(),
                    timed_out: false,
                });
            }
            if elapsed >= budget {
                if let Some(child) = child.as_deref_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                self.sample(start.elapsed())?;
                self.clear_progress();
                return Ok(Finish {
                    status: None,
                    elapsed: start.elapsed(),
                    timed_out: child.is_some(),
                });
            }
            thread::sleep(TICK);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/stress/monitor_tests.rs"]
mod tests;
