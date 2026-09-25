use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::catalog::{Crate, Language, Stage, Toolchain, profile_directory};
use crate::context::Context;
use crate::event::{Event, EventSink, Phase};
use crate::process::CaptureLimits;

const BUILD_TIMEOUT: Duration = Duration::from_secs(20 * 60);

pub struct Options {
    pub languages: Vec<Language>,
    /// `uv sync` of the project venv; command installation does not need it.
    pub venv: bool,
    pub rebuild: bool,
}

impl Options {
    pub fn everything() -> Self {
        Self {
            languages: Language::ALL.to_vec(),
            venv: true,
            rebuild: false,
        }
    }

    /// Compiled binaries only: no interpreter, no project venv.
    pub fn native() -> Self {
        Self {
            languages: vec![Language::Rust, Language::Go],
            venv: false,
            rebuild: false,
        }
    }

    pub fn commands() -> Self {
        Self {
            venv: false,
            ..Self::everything()
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub rebuilt: Vec<Language>,
    pub installed: Vec<String>,
    pub dotfile_changed: bool,
    pub pruned: usize,
    pub completions: usize,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.rebuilt.is_empty() && self.installed.is_empty()
    }
}

pub fn binary_dir(context: &Context) -> PathBuf {
    context.root.join(".bin")
}

/// Builds every stale language in parallel, then installs all of their binaries
/// and staleness stamps in one transaction: either this machine has the whole
/// new toolchain or it still has the whole old one.
pub fn ensure(
    context: &Context,
    options: &Options,
    events: &dyn EventSink,
) -> Result<Report, String> {
    let toolchain = Toolchain::read(&context.root)?;
    let bin = binary_dir(context);
    let stamps = context.root_config.join("sync");
    fs::create_dir_all(&bin).map_err(|e| format!("create {}: {e}", bin.display()))?;

    let mut pending = Vec::new();
    for stage in &toolchain.stages {
        if !options.languages.contains(&stage.language) {
            continue;
        }
        let digest = super::stage_digest(&stamps, stage)?;
        if !options.rebuild && current(&stamps, stage, &digest, &bin) {
            continue;
        }
        if context.program(stage.language.driver()).is_none() {
            missing_driver(stage.language)?;
            continue;
        }
        pending.push((stage, digest));
    }
    let mut report = if pending.is_empty() {
        Report::default()
    } else {
        events.emit(Event::PhaseStarted {
            phase: Phase::Tooling,
            total: Some(pending.len()),
        });
        let staging = tempfile::Builder::new()
            .prefix(".dotfile-build-")
            .tempdir_in(scratch(context)?)
            .map_err(|e| format!("create build directory: {e}"))?;
        let built = build_all(context, options, &pending, staging.path(), events)?;
        install(context, &bin, &stamps, &pending, &built, events)?
    };
    report.pruned = prune(context)?;
    if !report.is_empty() || !completions_current(context) {
        report.completions = completions(context)?;
    }
    Ok(report)
}

/// Completions describe the installed commands, so they are rewritten whenever
/// those change and whenever the cache they live in has gone missing.
pub fn completions_current(context: &Context) -> bool {
    completions_dir(context).is_dir()
}

fn completions_dir(context: &Context) -> PathBuf {
    context.root.join(".cache/zsh")
}

fn completions(context: &Context) -> Result<usize, String> {
    let directory = completions_dir(context);
    crate::surface::completions::write_all(context, &directory).map_err(|error| {
        format!(
            "cannot write shell completions to {}: {error}",
            directory.display()
        )
    })
}

fn scratch(context: &Context) -> Result<PathBuf, String> {
    let cache = context.root.join(".cache");
    fs::create_dir_all(&cache).map_err(|e| format!("create {}: {e}", cache.display()))?;
    Ok(cache)
}

fn current(stamps: &Path, stage: &Stage, digest: &str, bin: &Path) -> bool {
    stage
        .binaries
        .iter()
        .all(|name| is_executable(&bin.join(name)))
        && fs::read_to_string(stamps.join(stage.language.key()))
            .is_ok_and(|saved| saved.trim() == digest)
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    path.is_file()
}

/// A missing Rust or Python toolchain is fatal; Go is newer than most of the
/// machines this runs on, so its absence only costs the Go commands.
fn missing_driver(language: Language) -> Result<(), String> {
    if language == Language::Go {
        eprintln!("dotfile: go is not on PATH; skipping {}", language.source());
        return Ok(());
    }
    Err(format!(
        "{} is required to build {}",
        language.driver(),
        language.source()
    ))
}

/// Where each binary was built, by name.
type Built = BTreeMap<String, PathBuf>;
type Artifacts = BTreeMap<Language, Built>;

fn build_all(
    context: &Context,
    options: &Options,
    pending: &[(&Stage, String)],
    staging: &Path,
    events: &dyn EventSink,
) -> Result<Artifacts, String> {
    let total = pending.len();
    for (index, (stage, _)) in pending.iter().enumerate() {
        events.emit(Event::Progress {
            phase: Phase::Tooling,
            completed: index,
            total: Some(total),
            label: format!("building {}", stage.language.source()),
        });
    }
    let results: Vec<(Language, Result<Built, String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = pending
            .iter()
            .map(|(stage, _)| {
                let output = staging.join(stage.language.key());
                scope.spawn(move || (stage.language, build(context, options, stage, &output)))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_else(|_| panic!("build thread")))
            .collect()
    });
    let mut built = BTreeMap::new();
    for (language, result) in results {
        built.insert(
            language,
            result.map_err(|error| format!("{language}: {error}"))?,
        );
    }
    events.emit(Event::Progress {
        phase: Phase::Tooling,
        completed: total,
        total: Some(total),
        label: "installing workstation commands".to_string(),
    });
    Ok(built)
}

fn build(
    context: &Context,
    options: &Options,
    stage: &Stage,
    output: &Path,
) -> Result<Built, String> {
    crate::cancel::check()?;
    let directory = match stage.language {
        Language::Rust => return build_rust(context, &stage.crates),
        Language::Go => build_go(context, output)?,
        Language::Python => build_python(context, options, output)?,
    };
    Ok(stage
        .binaries
        .iter()
        .map(|name| (name.clone(), directory.join(name)))
        .collect())
}

/// One Cargo invocation per profile, side by side: fat-LTO links overlap the rest of the build.
fn build_rust(context: &Context, crates: &[Crate]) -> Result<Built, String> {
    let workspace = context.root.join("scripts/rust");
    let mut profiles: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for krate in crates {
        profiles
            .entry(krate.profile.as_str())
            .or_default()
            .push(krate.package.as_str());
    }
    std::thread::scope(|scope| {
        let builds: Vec<_> = profiles
            .iter()
            .map(|(profile, packages)| {
                let manifest = workspace.join("Cargo.toml");
                scope.spawn(move || {
                    let mut command = context.command("cargo");
                    command
                        .args(["build", "--locked", "--quiet", "--profile", profile])
                        .arg("--manifest-path")
                        .arg(&manifest);
                    for package in packages {
                        command.args(["--package", package]);
                    }
                    run(&mut command, "cargo build")
                })
            })
            .collect();
        builds.into_iter().try_for_each(|build| {
            build
                .join()
                .unwrap_or_else(|_| panic!("cargo build thread"))
        })
    })?;
    let target = workspace.join("target");
    Ok(crates
        .iter()
        .flat_map(|krate| {
            let directory = target.join(profile_directory(&krate.profile));
            krate
                .binaries
                .iter()
                .map(move |name| (name.clone(), directory.join(name)))
        })
        .collect())
}

fn build_go(context: &Context, output: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(output).map_err(|e| format!("create {}: {e}", output.display()))?;
    run(
        context
            .command("go")
            .current_dir(context.root.join("scripts/go"))
            .arg("build")
            .arg("-o")
            .arg(format!("{}/", output.display()))
            .arg("./cmd/..."),
        "go build",
    )?;
    Ok(output.to_path_buf())
}

/// `uv tool install` writes launchers with absolute interpreter paths, so they
/// are built in a staging directory and moved into `.bin` with everything else.
fn build_python(context: &Context, options: &Options, output: &Path) -> Result<PathBuf, String> {
    let project = context.root.join("scripts/python");
    if options.venv {
        run(
            context
                .command("uv")
                .args(["sync", "--locked", "--compile-bytecode", "--quiet"])
                .arg("--project")
                .arg(&project),
            "uv sync",
        )?;
    }
    let constraints = export_constraints(context, &project)?;
    fs::create_dir_all(output).map_err(|e| format!("create {}: {e}", output.display()))?;
    run(
        context
            .command("uv")
            .env("UV_TOOL_BIN_DIR", output)
            .env("UV_TOOL_DIR", context.root.join(".uv"))
            .args(["tool", "install", "--compile-bytecode", "--quiet"])
            .arg("--constraints")
            .arg(constraints.path())
            .args(["--editable", "--reinstall"])
            .arg(&project),
        "uv tool install",
    )?;
    Ok(output.to_path_buf())
}

fn export_constraints(
    context: &Context,
    project: &Path,
) -> Result<tempfile::NamedTempFile, String> {
    let exported = crate::process::output(
        context
            .command("uv")
            .args(["export", "--locked", "--no-dev", "--no-emit-project"])
            .args(["--no-header", "--no-annotate", "--no-hashes", "--quiet"])
            .arg("--project")
            .arg(project),
        CaptureLimits::default(),
        BUILD_TIMEOUT,
    )
    .map_err(|e| format!("uv export: {e}"))?;
    if !exported.status.success() {
        return Err(failure("uv export", &exported.stderr));
    }
    let mut file = tempfile::NamedTempFile::new().map_err(|e| format!("uv export: {e}"))?;
    std::io::Write::write_all(&mut file, &exported.stdout)
        .map_err(|e| format!("uv export: {e}"))?;
    Ok(file)
}

fn run(command: &mut Command, label: &str) -> Result<(), String> {
    let result = crate::process::output(
        command.stdin(Stdio::null()),
        CaptureLimits::default(),
        BUILD_TIMEOUT,
    )
    .map_err(|e| format!("{label}: {e}"))?;
    if result.status.success() {
        return Ok(());
    }
    Err(failure(label, &result.stderr))
}

fn failure(label: &str, stderr: &[u8]) -> String {
    let reason = String::from_utf8_lossy(stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("build failed")
        .to_string();
    format!("{label}: {reason}")
}

fn install(
    context: &Context,
    bin: &Path,
    stamps: &Path,
    pending: &[(&Stage, String)],
    built: &Artifacts,
    events: &dyn EventSink,
) -> Result<Report, String> {
    crate::cancel::check()?;
    let installed_dotfile = bin.join("dotfile");
    let mut report = Report::default();
    let mut transaction = crate::fs::transaction::Transaction::new(context)?;
    for (stage, _) in pending {
        let Some(artifacts) = built.get(&stage.language) else {
            continue;
        };
        for name in &stage.binaries {
            let from = artifacts
                .get(name)
                .ok_or_else(|| format!("{}: no build produced {name}", stage.language))?;
            let bytes = fs::read(from).map_err(|e| format!("{}: {e}", from.display()))?;
            let destination = bin.join(name);
            if crate::fs::content_matches(&destination, &bytes)? && is_executable(&destination) {
                continue;
            }
            transaction.write_executable(&destination, &bytes)?;
            if destination == installed_dotfile {
                report.dotfile_changed = true;
            }
            report.installed.push(name.clone());
        }
        report.rebuilt.push(stage.language);
    }
    for (stage, digest) in pending {
        transaction.write(
            &stamps.join(stage.language.key()),
            format!("{digest}\n").as_bytes(),
        )?;
    }
    transaction.commit()?;
    events.emit(Event::Progress {
        phase: Phase::Tooling,
        completed: pending.len(),
        total: Some(pending.len()),
        label: "workstation commands ready".to_string(),
    });
    Ok(report)
}

/// Binaries and entry points this repository used to install.
const RETIRED: [&str; 11] = [
    "doc-keybinds",
    "sysinfo-collect",
    "tardirs",
    "cpa",
    "cpas",
    "acp",
    "update-readme-fastfetch",
    "gdd",
    "power-menu",
    "confirm-exit",
    "clean-copy",
];

fn prune(context: &Context) -> Result<usize, String> {
    let bin = binary_dir(context);
    let completions = context.root.join(".cache/zsh");
    let toolchain = Toolchain::read(&context.root)?;
    let mut removed = 0;
    for name in RETIRED {
        if toolchain.binaries().any(|binary| binary == name) {
            continue;
        }
        for path in [
            bin.join(name),
            completions.join(format!("{name}-completion.zsh")),
        ] {
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("remove {}: {error}", path.display())),
            }
        }
    }
    Ok(removed)
}
