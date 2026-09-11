use std::fs;
use std::process::{Command, ExitCode};

use workstation::native::Resolver;

use crate::bios::export;
use crate::paths::{self, Paths};

pub fn tags(export_text: Option<&str>, lact_yaml: Option<&str>) -> Vec<String> {
    let mut tags = Vec::new();
    if let Some(text) = export_text {
        tags.push(format!("bios:{}", export::sha8(text)));
    }
    if let Some(text) = lact_yaml {
        tags.push(format!("lact:{}", export::sha8(text)));
    }
    tags
}

pub fn current_tags(paths: &Paths) -> Result<Vec<String>, String> {
    let export_text = match export::latest(&paths.exports_dir(), &paths.host)? {
        Some(path) => Some(export::load(&path)?.0),
        None => None,
    };
    let lact = fs::read_to_string(paths::lact_config()).ok();
    Ok(tags(export_text.as_deref(), lact.as_deref()))
}

fn sysinfo(paths: &Paths) -> String {
    Resolver::discover(paths.root.clone())
        .resolve("sysinfo")
        .ok()
        .flatten()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "sysinfo".into())
}

pub fn run(paths: &Paths, note: Option<&str>, extra: &[String]) -> Result<ExitCode, String> {
    let mut command = Command::new(sysinfo(paths));
    command.args(["bench", "run"]);
    for tag in current_tags(paths)? {
        command.arg("--tag").arg(tag);
    }
    if let Some(note) = note {
        command.arg("--note").arg(note);
    }
    command.args(extra);
    let status = command.status().map_err(|e| format!("sysinfo: {e}"))?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        workstation::exit_code(status.code().unwrap_or(1))
    })
}

#[cfg(test)]
#[path = "../tests/unit/bench_tests.rs"]
mod tests;
