use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};
use std::time::Duration;

use clap::Subcommand;
use hostkit::process::{self, CaptureLimits};
use serde_yaml_ng::Value;
use workstation::Style;

use crate::env::{Sysfs, read_text};
use crate::rows::{self, Row};
use crate::table;

pub const DEFAULT: &str = "balanced";
const LACT_DEFAULT: &str = "Default";
const UNITS: [&str; 2] = ["fan2go.service", "cpu-power.service"];

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "List installed profiles")]
    List,
    #[command(about = "Switch fans, CPU, and GPU to a profile")]
    Set { name: String },
}

pub struct Roots {
    pub etc: PathBuf,
    pub state: PathBuf,
    pub proc: PathBuf,
    pub lact: PathBuf,
}

impl Roots {
    pub fn from_env() -> Self {
        let root = |variable: &str, default: &str| {
            std::env::var_os(variable).map_or_else(|| PathBuf::from(default), PathBuf::from)
        };
        Self {
            etc: root("HWTUNE_ETC_ROOT", "/etc"),
            state: root("HWTUNE_PROFILE_STATE", "/var/lib/hwtune/profile"),
            proc: root("HWTUNE_PROC_ROOT", "/proc"),
            lact: crate::paths::lact_config(),
        }
    }

    fn fans_dir(&self) -> PathBuf {
        self.etc.join("fan2go/profiles")
    }

    fn cpu_file(&self, name: &str) -> PathBuf {
        self.etc.join("cpu-power").join(format!("{name}.env"))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub cpu: Option<BTreeMap<String, String>>,
    pub gpu: Option<Gpu>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Gpu {
    pub lact: String,
    pub power_cap: Option<String>,
}

impl Profile {
    fn missing(&self) -> Vec<&'static str> {
        [("cpu", self.cpu.is_none()), ("gpu", self.gpu.is_none())]
            .into_iter()
            .filter_map(|(part, missing)| missing.then_some(part))
            .collect()
    }

    fn cpu_summary(&self) -> String {
        self.cpu.as_ref().map_or("missing".into(), cpu_summary)
    }

    fn gpu_summary(&self) -> String {
        self.gpu.as_ref().map_or("missing".into(), |gpu| {
            gpu.power_cap
                .as_ref()
                .map_or(gpu.lact.clone(), |cap| format!("{} {cap} W", gpu.lact))
        })
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub fn assignments(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().into(), value.trim().trim_matches('"').into()))
        .collect()
}

fn cpu_summary(values: &BTreeMap<String, String>) -> String {
    let get = |key: &str| values.get(key).map_or("?", String::as_str);
    format!(
        "{} {} boost {}",
        get("CPU_GOVERNOR"),
        get("CPU_EPP"),
        match get("CPU_BOOST") {
            "1" => "on",
            "0" => "off",
            other => other,
        }
    )
}

pub fn lact_gpus(config: &Value, lact: &str) -> Option<Gpu> {
    let gpus = if lact == LACT_DEFAULT {
        config.get("gpus")?
    } else {
        config.get("profiles")?.get(lact)?.get("gpus")?
    };
    let power_cap = gpus
        .as_mapping()?
        .values()
        .find_map(|gpu| gpu.get("power_cap"))
        .and_then(|cap| cap.as_f64())
        .map(|cap| format!("{cap:.0}"));
    Some(Gpu {
        lact: lact.into(),
        power_cap,
    })
}

pub fn lact_profile(config: &Value, name: &str) -> Option<Gpu> {
    lact_gpus(config, name).or_else(|| {
        (name == DEFAULT)
            .then(|| lact_gpus(config, LACT_DEFAULT))
            .flatten()
    })
}

pub fn installed(roots: &Roots) -> Result<Vec<Profile>, String> {
    let dir = roots.fans_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "{} not found; run dotfile system install",
                dir.display()
            ));
        }
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    let names = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension()? == "yaml")
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .filter(|name| valid_name(name))
        .collect::<BTreeSet<_>>();
    let lact = fs::read(&roots.lact)
        .ok()
        .and_then(|bytes| serde_yaml_ng::from_slice::<Value>(&bytes).ok())
        .unwrap_or(Value::Null);
    Ok(names
        .into_iter()
        .map(|name| Profile {
            cpu: fs::read_to_string(roots.cpu_file(&name))
                .ok()
                .map(|text| assignments(&text)),
            gpu: lact_profile(&lact, &name),
            name,
        })
        .collect())
}

pub fn selected(roots: &Roots) -> Result<String, String> {
    match fs::read_to_string(&roots.state) {
        Ok(text) => assignments(&text)
            .remove("HWTUNE_PROFILE")
            .ok_or_else(|| format!("{}: HWTUNE_PROFILE missing", roots.state.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DEFAULT.into()),
        Err(error) => Err(format!("{}: {error}", roots.state.display())),
    }
}

fn capture(command: &mut Process) -> Result<String, String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let captured = process::output(command, CaptureLimits::default(), Duration::from_secs(10))
        .map_err(|error| format!("{program}: {error}"))?;
    if !captured.status.success() {
        return Err(format!(
            "{program}: {}",
            String::from_utf8_lossy(&captured.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&captured.stdout).trim().into())
}

pub fn config_profile(cmdline: &[u8]) -> Option<String> {
    let arguments = cmdline
        .split(|byte| *byte == 0)
        .map(String::from_utf8_lossy)
        .collect::<Vec<_>>();
    let config = arguments.iter().enumerate().find_map(|(index, argument)| {
        argument
            .strip_prefix("--config=")
            .map(str::to_owned)
            .or_else(|| {
                matches!(argument.as_ref(), "-c" | "--config")
                    .then(|| arguments.get(index + 1).map(|value| value.to_string()))
                    .flatten()
            })
    })?;
    let path = Path::new(&config);
    if path.parent()?.file_name()? == "profiles" {
        path.file_stem()?.to_str().map(str::to_owned)
    } else {
        Some(config)
    }
}

pub fn live_fans(roots: &Roots) -> Result<String, String> {
    let pid = capture(Process::new("systemctl").args([
        "show",
        "--property=MainPID",
        "--value",
        "fan2go.service",
    ]))?;
    if pid.is_empty() || pid == "0" {
        return Err("fan2go not running".into());
    }
    let path = roots.proc.join(&pid).join("cmdline");
    let cmdline = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    config_profile(&cmdline).ok_or_else(|| "fan2go config unknown".into())
}

pub fn live_cpu(sys: &Sysfs) -> Result<BTreeMap<String, String>, String> {
    let cpufreq = sys.sys.join("devices/system/cpu/cpufreq");
    let mut values = BTreeMap::new();
    values.insert("CPU_BOOST".into(), read_text(&cpufreq.join("boost"))?);
    let mut policies = fs::read_dir(&cpufreq)
        .map_err(|error| format!("{}: {error}", cpufreq.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("policy"))
        })
        .collect::<Vec<_>>();
    policies.sort();
    for (key, attribute) in [
        ("CPU_GOVERNOR", "scaling_governor"),
        ("CPU_EPP", "energy_performance_preference"),
    ] {
        let seen = policies
            .iter()
            .map(|policy| read_text(&policy.join(attribute)))
            .collect::<Result<BTreeSet<_>, _>>()?;
        values.insert(key.into(), seen.into_iter().collect::<Vec<_>>().join("+"));
    }
    Ok(values)
}

fn live_gpu() -> Result<String, String> {
    capture(Process::new("lact").args(["cli", "profile", "get"]))
}

fn compare(label: &str, live: Result<String, String>, want: &str) -> Row {
    match live {
        Ok(live) if live == want => Row::ok(label, live),
        Ok(live) => Row::bad(label, format!("{live}, want {want}")),
        Err(error) => Row::warn(label, error),
    }
}

pub fn check(roots: &Roots, sys: &Sysfs) -> Result<Vec<Row>, String> {
    let name = selected(roots)?;
    let profiles = installed(roots)?;
    let Some(profile) = profiles.iter().find(|profile| profile.name == name) else {
        return Ok(vec![Row::bad("profile", format!("{name} not installed"))]);
    };
    let mut rows = vec![Row::note("profile", &name)];
    rows.push(compare("fans", live_fans(roots), &name));
    rows.push(match &profile.cpu {
        Some(want) => compare(
            "cpu",
            live_cpu(sys).map(|live| cpu_summary(&live)),
            &cpu_summary(want),
        ),
        None => Row::bad(
            "cpu",
            format!("{} missing", roots.cpu_file(&name).display()),
        ),
    });
    rows.push(match &profile.gpu {
        Some(gpu) => compare("gpu", live_gpu(), &gpu.lact),
        None => Row::bad("gpu", format!("no LACT profile {name}")),
    });
    Ok(rows)
}

pub fn summary(roots: &Roots, sys: &Sysfs) -> String {
    match check(roots, sys) {
        Ok(rows) => {
            let name = rows.first().map_or("?", |row| row.summary.as_str());
            let drifted = rows
                .iter()
                .skip(1)
                .filter(|row| row.kind != rows::Kind::Ok)
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>();
            if drifted.is_empty() {
                name.into()
            } else {
                format!("{name}  mismatch: {}", drifted.join(", "))
            }
        }
        Err(error) => error,
    }
}

fn list(roots: &Roots) -> Result<ExitCode, String> {
    let active = selected(roots)?;
    let rows = installed(roots)?
        .iter()
        .map(|profile| {
            vec![
                if profile.name == active { "*" } else { "" }.into(),
                profile.name.clone(),
                profile.cpu_summary(),
                profile.gpu_summary(),
            ]
        })
        .collect::<Vec<_>>();
    print!("{}", table::render(&["", "profile", "cpu", "gpu"], &rows));
    Ok(ExitCode::SUCCESS)
}

fn privileged(program: &str) -> Process {
    if rustix::process::geteuid().is_root() {
        Process::new(program)
    } else {
        let mut command = Process::new("sudo");
        command.args(["-n", program]);
        command
    }
}

fn run(command: &mut Process) -> Result<(), String> {
    let text = format!("{command:?}");
    let status = command
        .status()
        .map_err(|error| format!("{text}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{text}: {status}"))
    }
}

fn write_state(roots: &Roots, name: &str) -> Result<(), String> {
    let mut staged = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    staged
        .write_all(format!("HWTUNE_PROFILE={name}\n").as_bytes())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    run(privileged("install")
        .args(["-D", "-m", "0644"])
        .arg(staged.path())
        .arg(&roots.state))
}

fn restart_units() -> Result<(), String> {
    let reload = capture(
        Process::new("systemctl")
            .args(["show", "--property=NeedDaemonReload", "--value"])
            .args(UNITS),
    )?;
    if reload.lines().any(|line| line.trim() == "yes") {
        run(privileged("systemctl").arg("daemon-reload"))?;
    }
    run(privileged("systemctl").arg("restart").args(UNITS))
}

fn set(name: &str, roots: &Roots, sys: &Sysfs, style: &Style) -> Result<ExitCode, String> {
    let profiles = installed(roots)?;
    let profile = profiles
        .iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| {
            format!(
                "unknown profile {name}; available: {}",
                profiles
                    .iter()
                    .map(|profile| profile.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
    let missing = profile.missing();
    if !missing.is_empty() {
        return Err(format!(
            "{name}: {} missing; run dotfile system install",
            missing.join(", ")
        ));
    }
    let lact = profile.gpu.as_ref().map(|gpu| gpu.lact.clone());
    let _measurement = crate::bench::store::measurement_lock()?;
    let _credentials = crate::tune::Credentials::acquire()?;
    write_state(roots, name)?;
    restart_units()?;
    if let Some(lact) = lact {
        capture(Process::new("lact").args(["cli", "profile", "set", &lact]))?;
    }
    show(roots, sys, style)
}

fn show(roots: &Roots, sys: &Sysfs, style: &Style) -> Result<ExitCode, String> {
    let rows = check(roots, sys)?;
    print!("{}", rows::render(&rows, style));
    Ok(rows::exit_code(&rows))
}

pub fn complete() -> Vec<String> {
    installed(&Roots::from_env())
        .map(|profiles| profiles.into_iter().map(|profile| profile.name).collect())
        .unwrap_or_default()
}

pub fn command(command: Option<Command>, sys: &Sysfs, style: &Style) -> Result<ExitCode, String> {
    let roots = Roots::from_env();
    match command {
        None => show(&roots, sys, style),
        Some(Command::List) => list(&roots),
        Some(Command::Set { name }) => set(&name, &roots, sys, style),
    }
}

#[cfg(test)]
#[path = "../tests/unit/profile_tests.rs"]
mod tests;
