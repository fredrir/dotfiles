use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::env::{Sysfs, read_text};

pub const CPUIDLE_GOVERNOR: &str = "devices/system/cpu/cpuidle/current_governor";

#[derive(Clone, Debug, Serialize)]
pub struct Control {
    /// Path relative to the captured sysfs root.
    pub path: PathBuf,
    pub original: String,
    pub choices: Vec<String>,
    pub driver: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Profile {
    pub name: String,
    pub values: BTreeMap<PathBuf, String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub controls: Vec<Control>,
    pub candidates: Vec<Profile>,
    pub unavailable: Vec<String>,
}

fn add_control(
    root: &Path,
    path: PathBuf,
    choices_path: &Path,
    driver: Option<String>,
    plan: &mut Plan,
) {
    if !root.join(&path).exists() {
        return;
    }
    let discovered = (|| {
        let original = read_text(&root.join(&path))?;
        let choices = read_text(choices_path)?
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !choices.contains(&original) || original == "custom" {
            return Err(format!(
                "current value {original:?} cannot be restored through this interface"
            ));
        }
        checked_path(root, &path)?;
        Ok(Control {
            path: path.clone(),
            original,
            choices,
            driver,
        })
    })();
    match discovered {
        Ok(control) => plan.controls.push(control),
        Err(error) => plan
            .unavailable
            .push(format!("{}: {error}", path.display())),
    }
}

pub fn discover(sys: &Sysfs) -> Result<Plan, String> {
    let mut plan = Plan {
        controls: Vec::new(),
        candidates: Vec::new(),
        unavailable: Vec::new(),
    };
    let policies = Path::new("devices/system/cpu/cpufreq");
    match fs::read_dir(sys.sys.join(policies)) {
        Ok(entries) => {
            let mut entries = entries
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let name = entry.file_name();
                let Some(name) = name.to_str().filter(|name| policy_name(name)) else {
                    continue;
                };
                let policy = policies.join(name);
                let driver = read_text(&sys.sys.join(&policy).join("scaling_driver")).ok();
                let start = plan.controls.len();
                for (attribute, available) in [
                    ("scaling_governor", "scaling_available_governors"),
                    (
                        "energy_performance_preference",
                        "energy_performance_available_preferences",
                    ),
                ] {
                    add_control(
                        &sys.sys,
                        policy.join(attribute),
                        &sys.sys.join(&policy).join(available),
                        driver.clone(),
                        &mut plan,
                    );
                }
                if sys
                    .sys
                    .join(&policy)
                    .join("energy_performance_preference")
                    .exists()
                    && !plan.controls[start..]
                        .iter()
                        .any(|control| control.path.ends_with("energy_performance_preference"))
                {
                    plan.controls.truncate(start);
                    plan.unavailable.push(format!(
                        "{}: governor changes could alter an EPP value that cannot be restored",
                        policy.display()
                    ));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("CPU policy discovery: {error}")),
    }
    add_control(
        &sys.sys,
        PathBuf::from("firmware/acpi/platform_profile"),
        &sys.sys.join("firmware/acpi/platform_profile_choices"),
        None,
        &mut plan,
    );
    add_control(
        &sys.sys,
        PathBuf::from(CPUIDLE_GOVERNOR),
        &sys.sys
            .join("devices/system/cpu/cpuidle/available_governors"),
        None,
        &mut plan,
    );
    plan.controls
        .sort_by_key(|control| (order(&control.path), control.path.clone()));
    plan.candidates = candidates(&plan.controls);
    if plan.controls.is_empty() {
        plan.unavailable
            .push("no restorable CPU or platform profile controls are available".into());
    }
    Ok(plan)
}

fn policy_name(value: &str) -> bool {
    value.strip_prefix("policy").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

pub fn allowed_path(path: &Path) -> bool {
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return false;
    }
    if path == Path::new("firmware/acpi/platform_profile") || path == Path::new(CPUIDLE_GOVERNOR) {
        return true;
    }
    let parts = path.iter().map(|part| part.to_str()).collect::<Vec<_>>();
    matches!(parts.as_slice(), [Some("devices"), Some("system"), Some("cpu"), Some("cpufreq"), Some(policy), Some(attribute)]
        if policy_name(policy) && matches!(*attribute, "scaling_governor" | "energy_performance_preference"))
}

pub(crate) fn checked_path(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if !allowed_path(relative) {
        return Err(format!(
            "unrecognized tuning control: {}",
            relative.display()
        ));
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let resolved = root
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("{}: {error}", relative.display()))?;
    if !resolved.starts_with(&root) {
        return Err(format!(
            "tuning control escapes sysfs: {}",
            relative.display()
        ));
    }
    Ok(resolved)
}

pub(crate) fn order(path: &Path) -> u8 {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("platform_profile") => 0,
        Some("scaling_governor") => 1,
        _ => 2,
    }
}

pub fn original_profile(controls: &[Control]) -> Profile {
    Profile {
        name: "original".into(),
        values: controls
            .iter()
            .map(|control| (control.path.clone(), control.original.clone()))
            .collect(),
    }
}

fn candidates(controls: &[Control]) -> Vec<Profile> {
    let original = original_profile(controls);
    let mut result = Vec::<Profile>::new();
    for name in ["performance", "balanced", "efficient"] {
        let mut candidate = original.clone();
        candidate.name = name.into();
        for control in controls {
            let pstate = control
                .driver
                .as_deref()
                .is_some_and(|driver| matches!(driver, "intel_pstate" | "amd-pstate-epp"));
            let preferred: &[&str] = match (
                control.path.file_name().and_then(|part| part.to_str()),
                name,
            ) {
                (Some("scaling_governor"), "performance") => &["performance"],
                (Some("scaling_governor"), _) if pstate => &["powersave"],
                (Some("scaling_governor"), _) => &["schedutil", "ondemand"],
                (Some("energy_performance_preference"), "performance") => &["performance"],
                (Some("energy_performance_preference"), "balanced") => {
                    &["balance_performance", "balance_power"]
                }
                (Some("energy_performance_preference"), _) => &["power", "balance_power"],
                (Some("platform_profile"), "performance") => {
                    &["performance", "balanced-performance"]
                }
                (Some("platform_profile"), "balanced") => &["balanced"],
                (Some("platform_profile"), _) => &["low-power", "quiet", "cool"],
                _ => &[],
            };
            if let Some(value) = preferred
                .iter()
                .find(|value| control.choices.iter().any(|choice| choice == **value))
            {
                candidate
                    .values
                    .insert(control.path.clone(), (*value).into());
            }
        }
        // Performance governors may force EPP=performance. Do not propose an
        // EPP value that the active P-state driver is documented to reject.
        for control in controls
            .iter()
            .filter(|control| control.path.ends_with("energy_performance_preference"))
        {
            let governor = control.path.with_file_name("scaling_governor");
            if candidate
                .values
                .get(&governor)
                .is_some_and(|value| value == "performance")
            {
                if control.choices.iter().any(|choice| choice == "performance") {
                    candidate
                        .values
                        .insert(control.path.clone(), "performance".into());
                } else {
                    candidate.values = original.values.clone();
                    break;
                }
            }
        }
        if candidate.values != original.values
            && !result
                .iter()
                .any(|previous| previous.values == candidate.values)
        {
            result.push(candidate);
        }
    }
    for control in controls
        .iter()
        .filter(|control| control.path == Path::new(CPUIDLE_GOVERNOR))
    {
        for choice in control
            .choices
            .iter()
            .filter(|choice| **choice != control.original)
        {
            let mut candidate = original.clone();
            candidate.name = format!("cpuidle-{choice}");
            candidate
                .values
                .insert(control.path.clone(), choice.clone());
            result.push(candidate);
        }
    }
    result
}

pub(crate) fn read(root: &Path, path: &Path) -> Result<String, String> {
    read_text(&checked_path(root, path)?)
}

pub(crate) fn write(root: &Path, path: &Path, value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains(char::is_whitespace) || value.contains('\0') {
        return Err("invalid tuning control value".into());
    }
    let resolved = checked_path(root, path)?;
    match OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&resolved)
    {
        Ok(mut file) => file
            .write_all(format!("{value}\n").as_bytes())
            .map_err(|error| format!("{}: {error}", resolved.display())),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            privileged_write(&resolved, value)
        }
        Err(error) => Err(format!("{}: {error}", resolved.display())),
    }
}

fn privileged_write(resolved: &Path, value: &str) -> Result<(), String> {
    let mut child = Command::new("sudo")
        .args(["-n", "tee"])
        .arg(resolved)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("sudo: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("{value}\n").as_bytes())
            .map_err(|error| format!("{}: {error}", resolved.display()))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("sudo: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}: sudo tee: {}",
            resolved.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tune/controls_tests.rs"]
mod tests;
