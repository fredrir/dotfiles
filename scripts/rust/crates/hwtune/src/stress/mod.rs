pub mod cpu;
pub mod gpu;
pub mod log;
pub mod mem;
pub mod monitor;
pub mod state;

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};

use clap::ValueEnum;
use workstation::Style;

use crate::bios::export;
use crate::env::Sysfs;
use crate::paths::{self, Paths};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Profile {
    AllCore,
    Light,
    PerCore,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Profile::AllCore => "all-core",
            Profile::Light => "light",
            Profile::PerCore => "per-core",
        }
    }
}

pub struct Context<'a> {
    pub sys: &'a Sysfs,
    pub paths: Option<&'a Paths>,
    pub style: &'a Style,
    pub log: bool,
}

impl Context<'_> {
    pub fn provenance(&self) -> crate::bench::provenance::RunContext {
        let mut context =
            crate::bench::provenance::capture_sources(self.paths, self.sys, &paths::lact_config());
        if let Ok(gpu) = crate::gpu::query() {
            context
                .observed
                .insert("gpu.power_cap_w".into(), gpu.power_cap_w.to_string());
        }
        context
    }

    pub fn record_session(
        &self,
        session: &str,
        keys: &[(String, String)],
        samples: &Path,
        before: &crate::bench::provenance::RunContext,
    ) -> Result<Option<PathBuf>, String> {
        if !self.log {
            return Ok(None);
        }
        let host = match self.paths {
            Some(paths) => paths.host.clone(),
            None => paths::host_name(None)?,
        };
        let details = keys
            .iter()
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>();
        let receipt = crate::bench::provenance::StabilitySession {
            schema: 1,
            host,
            session: session.into(),
            completed: crate::time::now_iso(),
            profile: details.get("profile").cloned().unwrap_or_default(),
            result: details
                .get("result")
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
            context: before.clone(),
            context_unchanged: crate::bench::provenance::same_settings(before, &self.provenance()),
            evidence_known: details
                .get("evidence_known")
                .is_some_and(|value| value == "true"),
            details,
            samples_path: samples.display().to_string(),
        };
        crate::bench::provenance::save_stability(&crate::bench::store::Store::discover(), &receipt)
            .map(Some)
    }

    pub fn bios_sha(&self) -> String {
        self.paths
            .and_then(|paths| {
                export::latest(&paths.exports_dir(), &paths.host)
                    .ok()
                    .flatten()
            })
            .and_then(|path| export::load(&path).ok())
            .map(|(text, _)| export::sha8(&text))
            .unwrap_or_else(|| "none".into())
    }

    pub fn record(&self, name: &str, keys: &[(String, String)]) -> Result<Option<PathBuf>, String> {
        let Some(paths) = self.paths.filter(|_| self.log) else {
            return Ok(None);
        };
        let file = paths.stability_file();
        log::append(&file, &log::render_block(name, keys))?;
        Ok(Some(file))
    }
}

pub fn session_log(session: &str) -> Result<PathBuf, String> {
    let dir = paths::cache_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir.join(format!("{session}.log")))
}

pub fn spawn(program: &str, args: &[String], log: &Path) -> Result<Child, String> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| format!("{}: {e}", log.display()))?;
    let errors = file
        .try_clone()
        .map_err(|e| format!("{}: {e}", log.display()))?;
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors))
        .spawn()
        .map_err(|e| format!("{program}: {e}"))
}

pub fn describe_status(status: Option<ExitStatus>, timed_out: bool) -> String {
    match status {
        Some(status) if status.success() => "exit 0".into(),
        Some(status) => match status.code() {
            Some(code) => format!("exit {code}"),
            None => "killed by signal".into(),
        },
        None if timed_out => "timed out".into(),
        None => "still running".into(),
    }
}

pub fn tail(log: &Path, lines: usize) -> Vec<String> {
    let Ok(text) = fs::read_to_string(log) else {
        return Vec::new();
    };
    let all = text.lines().collect::<Vec<_>>();
    all.iter()
        .skip(all.len().saturating_sub(lines))
        .map(|line| ui_terminal::text::sanitize(line))
        .collect()
}

pub fn keys(pairs: &[(&str, String)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

pub fn touch(path: &Path) -> Result<File, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    File::create(path).map_err(|e| format!("{}: {e}", path.display()))
}
