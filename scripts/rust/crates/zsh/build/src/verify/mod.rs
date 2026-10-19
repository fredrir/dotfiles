//! `zsh-build verify`: a login zsh from the bundle and one from the sources,
//! on a pty, compared once each has run its deferred queue after the first
//! prompt. Uses the bundle as built and runs no command line.

pub mod compare;
pub(crate) mod shell;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::cache::Cache;
use crate::config::{self, Config};
use crate::state::State;
use crate::{Options, wordcode};
use compare::{Difference, Dump, Expected};
use shell::Startup;

const HOOK: &str = include_str!("../../assets/verify.zsh");

#[derive(Debug)]
pub struct Verified {
    /// Bundles a build would rewrite.
    pub stale: Vec<PathBuf>,
    pub differences: Vec<Difference>,
}

/// Verifies the bundles under `root`; `direct` sources the first one in place
/// of `~/.zshrc`, for a bundle built elsewhere.
pub fn verify(root: &Path, direct: bool) -> Result<Verified, String> {
    let config = Config::load(&root.join(config::RELATIVE_PATH))?;
    let output = root.join(&config.output);
    let bundles: Vec<PathBuf> = config
        .targets
        .iter()
        .map(|target| output.join(format!("{}.zsh", target.name)))
        .collect();
    let completions: Vec<String> = config
        .targets
        .iter()
        .map(|target| {
            output
                .join(format!("{}.completions", target.name))
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let zsh = shell_path()?;
    let version = wordcode::shell_version(&zsh, &mut Cache::default())?;
    for bundle in &bundles {
        let wordcode = wordcode::path(bundle);
        if !bundle.is_file() {
            return Err(format!("{}: not built", bundle.display()));
        }
        // Older wordcode would have the bundle recompile it in place
        if !wordcode::current(bundle, &wordcode, &version) {
            return Err(format!("{}: stale", wordcode.display()));
        }
    }
    let stale = crate::build(&Options {
        root: root.to_path_buf(),
        dry_run: true,
    })?
    .into_iter()
    .filter(|built| built.changed)
    .map(|built| built.path)
    .collect();
    let before = stamps(&bundles);
    let scratch = tempfile::Builder::new()
        .prefix("zsh-build-verify.")
        .tempdir()
        .map_err(|error| format!("scratch directory: {error}"))?;
    let mut ambient = config.ambient.clone();
    ambient.extend(config.system.ambient.iter().cloned());
    let env = shell::environment(&ambient);
    let sourced = bundles.first().filter(|_| direct).map(PathBuf::as_path);
    let dump = |startup: &Startup| {
        shell::run(&zsh, scratch.path(), &env, startup, HOOK).and_then(|dump| {
            Dump::parse(&dump).map_err(|error| format!("{}: {error}", startup.label()))
        })
    };
    let bundle = dump(&Startup::Bundle(&completions, sourced))?;
    let sources = dump(&Startup::Sources)?;
    if stamps(&bundles) != before {
        return Err("bundle changed while verifying".to_string());
    }
    if !loaded(&bundle) {
        return Err("~/.zshrc did not load the bundle".to_string());
    }
    if loaded(&sources) {
        return Err("~/.zshrc loaded the bundle with ZSH_BUILD_SKIP=1".to_string());
    }
    Ok(Verified {
        stale,
        differences: compare::differences(&bundle, &sources, &Expected { completions }),
    })
}

/// Every bundle defines the helper behind its `type` and `whence`.
fn loaded(dump: &Dump) -> bool {
    dump.function("_zsh_build_whence").is_some()
}

/// The zsh a terminal starts, as the build picks it to compile wordcode.
fn shell_path() -> Result<PathBuf, String> {
    let path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    wordcode::shell(&State::new(BTreeMap::new(), String::new(), path))
        .ok_or_else(|| "zsh not found".to_string())
}

/// Modification time and size of each bundle and its wordcode.
fn stamps(bundles: &[PathBuf]) -> Vec<Option<(SystemTime, u64)>> {
    bundles
        .iter()
        .flat_map(|bundle| [bundle.clone(), wordcode::path(bundle)])
        .map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            Some((meta.modified().ok()?, meta.len()))
        })
        .collect()
}
