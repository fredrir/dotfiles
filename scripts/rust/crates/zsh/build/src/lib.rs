#![forbid(unsafe_code)]

pub mod compile;
pub mod config;
pub mod emit;
pub mod expand;
pub mod fold;
pub mod native;
pub mod quote;
pub mod scan;
pub mod script;
pub mod state;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use compile::Compiler;
use compile::system::{Outcome, Startup};
use config::Config;
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
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
    /// Whether the global startup files were compiled in; `None` when not asked.
    pub system: Option<bool>,
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
    let mut built = Vec::new();
    for target in &config.targets {
        let state = State::new(ambient.clone(), host.clone(), path.clone());
        let folder = Folder::new(
            config.fold_commands(),
            Duration::from_millis(config.fold.timeout_ms),
        );
        let mut compiler = Compiler::new(state, folder, options.root.clone());
        compiler.path_helper_root = config.system.path_helper_root.clone();
        let env: Vec<PathBuf> = target
            .env
            .iter()
            .map(|env| options.root.join(env))
            .collect();
        let output = options
            .root
            .join(&config.output)
            .join(format!("{}.zsh", target.name));
        let guard_path = output.with_file_name(format!("{}.rcs.zsh", target.name));
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
        let text = emit::bundle(
            &output,
            &target.source,
            &compiler.constants,
            &(section.to_string() + &body),
        )?;
        let mut changed = std::fs::read_to_string(&output).map_or(true, |current| current != text);
        if guard.is_none() {
            changed |= emit::remove_guard(&guard_path, options.dry_run)?;
        }
        if changed && !options.dry_run {
            emit::write_atomic(&output, &text)?;
        }
        if let Some((text, sources)) = guard {
            changed |= emit::write_guard(&guard_path, text, sources, options.dry_run)?;
        }
        built.push(Built {
            name: target.name.clone(),
            path: output,
            changed,
            inlined: compiler.inlined,
            folded: compiler.folded,
            warnings: compiler.warnings,
            skipped: compiler.skipped,
            system: system.map(|system| matches!(system, System::Compiled { .. })),
        });
    }
    Ok(built)
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
