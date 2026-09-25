use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use crate::context::Context;

pub struct Unit {
    pub name: String,
    pub state: String,
}

impl Unit {
    pub fn disabled(&self) -> bool {
        matches!(
            self.state.as_str(),
            "disabled" | "linked" | "linked-runtime"
        )
    }

    pub fn missing(&self) -> bool {
        self.state == "not-found"
    }
}

/// `enable` lines of tracked `systemd/system-preset/*.preset` files, applied one unit at a
/// time: `systemctl preset-all` would also apply Arch's `disable *` to everything else.
pub fn wanted<'a>(files: impl Iterator<Item = (&'a Path, &'a [u8])>) -> Vec<String> {
    let mut units = BTreeSet::new();
    for (_, content) in files.filter(|(destination, _)| is_preset(destination)) {
        for line in String::from_utf8_lossy(content).lines() {
            let mut words = line.split_whitespace();
            if let (Some("enable"), Some(unit)) = (words.next(), words.next())
                && valid(unit)
            {
                units.insert(unit.to_string());
            }
        }
    }
    units.into_iter().collect()
}

pub fn inspect(context: &Context, names: Vec<String>) -> Vec<Unit> {
    names
        .into_iter()
        .map(|name| Unit {
            state: state(context, &name),
            name,
        })
        .collect()
}

pub fn show(unit: &Unit) {
    println!("  {:<10} {}", unit.state, unit.name);
}

pub fn counted(units: &[Unit]) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    for unit in units {
        *counts.entry(unit.state.as_str()).or_default() += 1;
    }
    let counts = counts
        .into_iter()
        .map(|(state, count)| format!("{count} {state}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{counts} unit(s)")
}

/// A unit whose file is about to be installed is not known to systemd yet.
pub fn provided_by(unit: &Unit, destination: &Path) -> bool {
    unit.missing()
        && destination
            .file_name()
            .is_some_and(|name| name == unit.name.as_str())
        && destination
            .parent()
            .is_some_and(|parent| parent.ends_with("systemd/system"))
}

/// Runs `prepare` first: units read what the same install wrote, such as modules to load.
pub fn enable(context: &Context, names: &[&str], prepare: &[&[&str]]) -> Result<bool, String> {
    for arguments in prepare {
        if !sudo_systemctl(context, arguments)? {
            return Ok(false);
        }
    }
    let mut arguments = vec!["enable", "--now", "--"];
    arguments.extend_from_slice(names);
    sudo_systemctl(context, &arguments)
}

fn sudo_systemctl(context: &Context, arguments: &[&str]) -> Result<bool, String> {
    let mut command = context.command("sudo");
    command.arg("systemctl").args(arguments);
    crate::process::status(&mut command)
        .map(|status| status.success())
        .map_err(|error| format!("systemctl {}: {error}", arguments.join(" ")))
}

fn is_preset(destination: &Path) -> bool {
    destination.extension().is_some_and(|ext| ext == "preset")
        && destination
            .parent()
            .is_some_and(|parent| parent.ends_with("systemd/system-preset"))
}

pub(crate) fn valid(unit: &str) -> bool {
    unit.rsplit_once('.').is_some_and(|(name, kind)| {
        !name.is_empty()
            && matches!(
                kind,
                "service" | "socket" | "timer" | "path" | "mount" | "target"
            )
    }) && unit
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"@._-:".contains(&byte))
}

fn state(context: &Context, unit: &str) -> String {
    let mut command = context.command("systemctl");
    command.args(["is-enabled", "--", unit]);
    crate::process::output(
        &mut command,
        hostkit::process::CaptureLimits::default(),
        Duration::from_secs(5),
    )
    .ok()
    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    .filter(|state| !state.is_empty())
    .unwrap_or_else(|| "unknown".into())
}
