use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use super::go_patches;
use super::install::run;
use crate::context::Context;
use crate::event::{Action, Event, EventSink};
use crate::process::CaptureLimits;

const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
// A slow network costs a sync at most this, once a day.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

pub fn due(context: &Context) -> bool {
    let patched = go_patches::modules(&source(context)).is_ok_and(|modules| !modules.is_empty());
    patched
        && fs::metadata(stamp(context))
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|checked| SystemTime::now().duration_since(checked).ok())
            .is_none_or(|age| age >= INTERVAL)
}

/// Moves each patched module to its newest release, unless its patch no longer applies.
pub fn upgrade_patched(context: &Context, events: &dyn EventSink) -> Result<(), String> {
    let source = source(context);
    for module in go_patches::modules(&source)? {
        crate::cancel::check()?;
        if let Some(version) = newer(context, &source, &module) {
            upgrade(context, &source, &module, &version, events)?;
        }
    }
    let stamp = stamp(context);
    if let Some(parent) = stamp.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    fs::write(&stamp, "").map_err(|e| format!("write {}: {e}", stamp.display()))
}

fn source(context: &Context) -> PathBuf {
    context.root.join("scripts/go")
}

fn stamp(context: &Context) -> PathBuf {
    context.root_config.join("sync/upgrades")
}

// An unreachable proxy is skipped quietly; the next check is a day away.
fn newer(context: &Context, source: &Path, module: &str) -> Option<String> {
    let output = crate::process::output(
        context
            .command("go")
            .current_dir(source)
            .args(["list", "-m", "-u", "-json", module])
            .stdin(Stdio::null()),
        CaptureLimits::default(),
        LOOKUP_TIMEOUT,
    )
    .ok()
    .filter(|output| output.status.success())?;
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    listed["Update"]["Version"].as_str().map(str::to_string)
}

fn upgrade(
    context: &Context,
    source: &Path,
    module: &str,
    version: &str,
    events: &dyn EventSink,
) -> Result<(), String> {
    let manifests = [source.join("go.mod"), source.join("go.sum")];
    let saved = manifests
        .iter()
        .map(|path| fs::read(path).ok())
        .collect::<Vec<_>>();
    let upgraded = run(
        context
            .command("go")
            .current_dir(source)
            .arg("get")
            .arg(format!("{module}@{version}")),
        "go get",
    )
    .and_then(|()| {
        run(
            context
                .command("go")
                .current_dir(source)
                .args(["mod", "tidy"]),
            "go mod tidy",
        )
    })
    .and_then(|()| go_patches::prepare(context, source).map(drop));
    if let Err(error) = upgraded {
        for (path, contents) in manifests.iter().zip(saved) {
            if let Some(contents) = contents {
                fs::write(path, contents).map_err(|e| format!("write {}: {e}", path.display()))?;
            }
        }
        events.emit(Event::Warning {
            message: format!("{module} {version} kept back: {error}"),
            hint: Some(format!(
                "update {}",
                go_patches::patch_path(source, module).display()
            )),
        });
        return Ok(());
    }
    events.emit(Event::Item {
        action: Action::Generate,
        path: manifests[0].clone(),
        detail: format!("{module} {version}"),
        changed: true,
    });
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/tooling/go_upgrades_tests.rs"]
mod tests;
