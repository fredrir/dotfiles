#![forbid(unsafe_code)]

pub mod cache;
pub mod compdump;
pub mod compile;
pub mod completions;
pub mod config;
pub mod emit;
pub mod expand;
pub mod fold;
pub mod git;
pub mod native;
pub mod quote;
pub mod scan;
pub mod script;
pub mod state;
pub mod verify;
pub mod wordcode;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cache::Cache;
use compile::Compiler;
use compile::defer::Deferral;
use compile::system::{Outcome, Startup};
use completions::Completions;
use config::Config;
use expand::Env;
use fold::Folder;
use state::State;

pub struct Options {
    pub root: PathBuf,
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct Built {
    pub name: String,
    pub path: PathBuf,
    pub changed: bool,
    pub inlined: usize,
    pub folded: usize,
    pub deferred: usize,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
    /// Whether the global startup files were compiled in; `None` when not asked.
    pub system: Option<bool>,
    /// Completion dumps removed because the completion functions changed.
    pub refreshed: Vec<PathBuf>,
}

pub fn build(options: &Options) -> Result<Vec<Built>, String> {
    let config = Config::load(&options.root.join(config::RELATIVE_PATH))?;
    let ambient: BTreeMap<String, Option<String>> = config
        .ambient
        .iter()
        .map(|name| (name.clone(), std::env::var(name).ok()))
        .collect();
    let host = nix::unistd::gethostname()
        .map(|name| name.to_string_lossy().into_owned())
        .map_err(|error| format!("hostname: {error}"))?;
    let path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let system_ambient: BTreeMap<String, Option<String>> = config
        .system
        .ambient
        .iter()
        .map(|name| (name.clone(), std::env::var(name).ok()))
        .collect();
    let mut cache = Cache::load(options.root.join(&config.output).join(cache::FILE));
    let zsh = wordcode::shell(&State::new(BTreeMap::new(), host.clone(), path.clone()))
        .ok_or_else(|| "zsh not found".to_string())
        .and_then(|zsh| wordcode::shell_version(&zsh, &mut cache).map(|version| (zsh, version)));
    let mut built = Vec::new();
    for target in &config.targets {
        let state = State::new(ambient.clone(), host.clone(), path.clone());
        let folder = Folder::new(
            config.fold_commands(),
            Duration::from_millis(config.fold.timeout_ms),
            std::mem::take(&mut cache),
        );
        let output = options
            .root
            .join(&config.output)
            .join(format!("{}.zsh", target.name));
        let guard_path = output.with_file_name(format!("{}.rcs.zsh", target.name));
        let origins = output.with_file_name(format!("{}.origins", target.name));
        let mut compiler = Compiler::new(state, folder, options.root.clone());
        compiler.path_helper_root = config.system.path_helper_root.clone();
        compiler.zsh = zsh.as_ref().ok().map(|(zsh, _)| zsh.clone());
        compiler.deferral = Deferral::new(&config.defer, &options.root)?;
        compiler.deferral.completions =
            Completions::new(output.with_file_name(format!("{}.completions", target.name)));
        let env: Vec<PathBuf> = target
            .env
            .iter()
            .map(|env| options.root.join(env))
            .collect();
        let system = if target.system {
            let startup = Startup {
                dir: &config.system.dir,
                link: zdotdir()?.join(".zprofile"),
                profile: target
                    .profile
                    .as_ref()
                    .map(|profile| options.root.join(profile)),
                env: &env,
            };
            let state = State::new(system_ambient.clone(), host.clone(), Vec::new());
            Some(system(
                &mut compiler,
                state,
                &startup,
                &output,
                &guard_path,
            )?)
        } else {
            None
        };
        let (section, guard) = match &system {
            Some(System::Compiled { section, guard }) => (section.as_str(), Some(guard)),
            _ => ("", None),
        };
        for env in &env {
            compiler.analyze(env)?;
        }
        let body = compiler.root(&options.root.join(&target.source))?;
        if !compiler.redefined.is_empty() {
            return Err(compiler.redefined.join("\n"));
        }
        let completions = std::mem::take(&mut compiler.deferral.completions);
        let body = place_completions(
            &completions,
            compiler.settle_deferral(section.to_string() + &body),
        );
        let text = emit::bundle(
            &output,
            &target.source,
            &origins,
            &compiler.constants,
            &compiler.helpers,
            &body,
        )?;
        let written = completions.write(options.dry_run)?;
        let mut changed = std::fs::read_to_string(&output).map_or(true, |current| current != text);
        if guard.is_none() {
            changed |= emit::remove_guard(&guard_path, options.dry_run)?;
        }
        if changed && !options.dry_run {
            emit::write_atomic(&output, &text)?;
        }
        changed |= written.changed;
        changed |= compile_wordcode(&zsh, &output, options.dry_run, &mut compiler.warnings);
        changed |= emit::write_origins(&origins, &compiler.origins, options.dry_run)?;
        if let Some((text, sources)) = guard {
            changed |= emit::write_guard(&guard_path, text, sources, options.dry_run)?;
        }
        let omz = match compiler.state.var("ZSH") {
            expand::Var::Scalar(omz) => Some(PathBuf::from(omz)),
            _ => None,
        };
        let rebound: Vec<&Path> = completions
            .dir()
            .filter(|_| written.bindings)
            .into_iter()
            .collect();
        let refreshed = compdump::refresh(&zdotdir()?, omz.as_deref(), &rebound, options.dry_run)?;
        changed |= !refreshed.is_empty();
        cache = std::mem::take(&mut compiler.folder.cache);
        built.push(Built {
            name: target.name.clone(),
            path: output,
            changed,
            inlined: compiler.inlined,
            folded: compiler.folded,
            deferred: compiler.deferred,
            warnings: compiler.warnings,
            skipped: compiler.skipped,
            system: system.map(|system| matches!(system, System::Compiled { .. })),
            refreshed,
        });
    }
    if !options.dry_run {
        cache.save()?;
    }
    Ok(built)
}

/// Keeps the completion `fpath` statement only when files were taken; one
/// that a failed compile attempt lost goes ahead of everything else.
fn place_completions(completions: &Completions, body: String) -> String {
    match completions.fpath() {
        Some(fpath) if completions.is_empty() => body.replace(&fpath, ""),
        Some(fpath) if !body.contains(&fpath) => fpath + &body,
        _ => body,
    }
}

/// Compiles the bundle's wordcode unless it is current; returns whether it changed.
fn compile_wordcode(
    zsh: &Result<(PathBuf, String), String>,
    bundle: &Path,
    dry_run: bool,
    warnings: &mut Vec<String>,
) -> bool {
    let compiled = match zsh {
        Err(error) => Err(error.clone()),
        Ok((_, version)) if wordcode::current(bundle, &wordcode::path(bundle), version) => {
            return false;
        }
        Ok(_) if dry_run => return true,
        Ok((zsh, _)) => wordcode::compile(zsh, bundle),
    };
    if let Err(error) = &compiled {
        warnings.push(format!("{error}; wordcode left to the shell"));
    }
    compiled.is_ok()
}

enum System {
    Compiled {
        section: String,
        guard: (String, Vec<PathBuf>),
    },
    Kept,
}

fn system(
    compiler: &mut Compiler,
    state: State,
    startup: &Startup,
    output: &Path,
    guard_path: &Path,
) -> Result<System, String> {
    Ok(match compiler.system(state, startup)? {
        Outcome::Compiled(compiled) => {
            let sources = compile::unique_paths(compiled.sources);
            let guard = emit::guard(guard_path, output, &sources, compiled.profile.as_deref())?;
            System::Compiled {
                section: compiled.text,
                guard: (guard, sources),
            }
        }
        Outcome::Empty => System::Kept,
        Outcome::Kept { reason, fixable } => {
            let note = format!("{reason}; global startup files left to zsh");
            if fixable {
                compiler.note(note);
            } else if !compiler.skipped.contains(&note) {
                compiler.skipped.push(note);
            }
            System::Kept
        }
    })
}

/// Where each bundled function was defined, across targets.
pub fn origins(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let config = Config::load(&root.join(config::RELATIVE_PATH))?;
    let mut origins = BTreeMap::new();
    for target in &config.targets {
        let path = root
            .join(&config.output)
            .join(format!("{}.origins", target.name));
        if path.is_file() {
            origins.extend(emit::read_origins(&path)?);
        }
    }
    Ok(origins)
}

/// Where zsh reads the user's startup files.
fn zdotdir() -> Result<PathBuf, String> {
    std::env::var_os("ZDOTDIR")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

/// The dotfiles checkout: `$DOTFILES`, else `~/dotfiles`.
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("DOTFILES")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join("dotfiles")))
}
