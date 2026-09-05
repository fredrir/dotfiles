use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::UNIX_EPOCH;

use crate::backend;
use crate::context::Context;
use crate::event::{Action, Event, EventSink, Phase};

pub fn synchronize(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
) -> Result<usize, String> {
    synchronize_with_backend(context, dry_run, events, &backend::path())
}

fn synchronize_with_backend(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
    program: &Path,
) -> Result<usize, String> {
    let stamp = context.state.join("sync/docs.fingerprint");
    let before = fingerprint(context)?;
    if fs::read_to_string(&stamp).ok().as_deref() == Some(before.as_str()) {
        return Ok(0);
    }
    events.emit(Event::PhaseStarted {
        phase: Phase::Artifacts,
        total: None,
    });
    let output = generate_with_backend(program, dry_run)?;
    let accepted = output.status.success() || dry_run && output.status.code() == Some(1);
    if !accepted {
        return Err(
            first_error(&output).unwrap_or_else(|| "documentation generation failed".into())
        );
    }
    let verb = if dry_run { "drifted" } else { "updated" };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut paths = stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix(verb).map(str::trim))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    paths.extend(doc_keybinds::generate(&context.root, dry_run)?);
    for path in &paths {
        events.emit(Event::Item {
            action: Action::Generate,
            path: path.clone(),
            detail: if dry_run { "would update" } else { "updated" }.into(),
            changed: true,
        });
    }
    if !dry_run {
        let after = fingerprint(context)?;
        write_stamp(&stamp, &after)?;
    }
    Ok(paths.len())
}

fn generate_with_backend(program: &Path, check: bool) -> Result<Output, String> {
    let mut command = Command::new(program);
    command.arg("__reference");
    if check {
        command.arg("--check");
    }
    command
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .map_err(|error| format!("cannot run documentation generator: {error}"))
}

fn fingerprint(context: &Context) -> Result<String, String> {
    let mut hasher = DefaultHasher::new();
    let inputs = [
        context.root.join("scripts/python/pyproject.toml"),
        context.root.join("scripts/python/src"),
        context.root.join("scripts/rust/Cargo.toml"),
        context.root.join("scripts/rust/Cargo.lock"),
        context.root.join("scripts/rust/crates"),
        context.root.join("docs/cli"),
        context.root.join("docs/keybinds"),
    ];
    for input in inputs {
        hash_path(&input, &mut hasher)?;
    }
    for input in doc_keybinds::INPUTS {
        hash_path(&context.root.join(input), &mut hasher)?;
    }
    Ok(format!("{:016x}\n", hasher.finish()))
}

fn hash_path(path: &Path, hasher: &mut DefaultHasher) -> Result<(), String> {
    if matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("target" | ".venv" | "__pycache__")
    ) {
        return Ok(());
    }
    path.to_string_lossy().hash(hasher);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    metadata.len().hash(hasher);
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .hash(hasher);
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .map(|entry| {
                entry
                    .map(|entry| entry.path())
                    .map_err(|error| format!("{}: {error}", path.display()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort();
        for entry in entries {
            hash_path(&entry, hasher)?;
        }
    }
    Ok(())
}

fn write_stamp(path: &Path, value: &str) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    use std::io::Write;
    file.write_all(value.as_bytes())
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.persist(path)
        .map_err(|error| format!("{}: {}", path.display(), error.error))?;
    Ok(())
}

fn first_error(output: &Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

#[cfg(all(test, unix))]
#[path = "../../tests/unit/artifacts/docs_tests.rs"]
mod tests;
