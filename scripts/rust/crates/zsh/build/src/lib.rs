#![forbid(unsafe_code)]

pub mod compile;
pub mod config;
pub mod emit;
pub mod expand;
pub mod fold;
pub mod quote;
pub mod scan;
pub mod script;
pub mod state;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use compile::Compiler;
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
    let mut built = Vec::new();
    for target in &config.targets {
        let state = State::new(ambient.clone(), host.clone(), path.clone());
        let folder = Folder::new(
            config.fold_commands(),
            Duration::from_millis(config.fold.timeout_ms),
        );
        let mut compiler = Compiler::new(state, folder, options.root.clone());
        for env in &target.env {
            compiler.analyze(&options.root.join(env))?;
        }
        let output = options
            .root
            .join(&config.output)
            .join(format!("{}.zsh", target.name));
        let body = compiler.root(&options.root.join(&target.source))?;
        let text = emit::bundle(&output, &target.source, &compiler.constants, &body)?;
        let changed = std::fs::read_to_string(&output).map_or(true, |current| current != text);
        if changed && !options.dry_run {
            emit::write_atomic(&output, &text)?;
        }
        built.push(Built {
            name: target.name.clone(),
            path: output,
            changed,
            inlined: compiler.inlined,
            folded: compiler.folded,
            warnings: compiler.warnings,
            skipped: compiler.skipped,
        });
    }
    Ok(built)
}

/// The dotfiles checkout: `$DOTFILES`, else `~/dotfiles`.
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("DOTFILES")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join("dotfiles")))
}
