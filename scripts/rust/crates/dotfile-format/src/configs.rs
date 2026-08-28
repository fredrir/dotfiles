//! Where this repository's tool configuration comes from, and how it is put
//! into another project.
//!
//! `--add` offers one config per language the project appears to use and asks
//! about each; `--sync` refreshes the ones already there and introduces
//! nothing. The asymmetry is the point: `--add` is how a project gets these
//! settings, `--sync` is how a project that already agreed to them keeps up,
//! and neither should ever be the other by accident.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use workstation::Answer;

use crate::lang::{LANGS, Lang};

/// The files `--add` copies, compiled in so that a binary which cannot find
/// the repository still has something true to say. `setup.sh` rebuilds on a
/// change under `shared/tools`, so these are the same bytes as the repository
/// held when this binary was built.
pub const EMBEDDED: [(&str, &str); 9] = [
    (
        "dotfmt.dotfile",
        include_str!("../../../../../shared/tools/dotfmt.dotfile"),
    ),
    (
        "ruff.toml",
        include_str!("../../../../../shared/tools/ruff.toml"),
    ),
    (
        "biome.global.json",
        include_str!("../../../../../shared/tools/biome.global.json"),
    ),
    (
        "stylua.toml",
        include_str!("../../../../../shared/tools/stylua.toml"),
    ),
    (
        "rustfmt.toml",
        include_str!("../../../../../shared/tools/rustfmt.toml"),
    ),
    (
        ".taplo.toml",
        include_str!("../../../../../shared/tools/.taplo.toml"),
    ),
    (
        ".yamllint.yaml",
        include_str!("../../../../../shared/tools/.yamllint.yaml"),
    ),
    (
        ".sqlfluff",
        include_str!("../../../../../shared/tools/.sqlfluff"),
    ),
    (
        ".editorconfig",
        include_str!("../../../../../shared/tools/.editorconfig"),
    ),
];

/// Where the live copies live, under whichever root was found.
const TOOLS: &str = "shared/tools";

/// The environment variable that names the repository outright.
pub const OVERRIDE: &str = "DOTFILE_ROOT";

/// Where the configs are being read from.
#[derive(Debug)]
pub enum Source {
    /// A checkout of this repository, read live.
    Repo(PathBuf),
    /// No checkout was found, so the copies inside this binary are it.
    Embedded,
}

/// A directory is this repository when it holds both of these — the same
/// predicate `core/paths.py:repo_root` uses. Probing for `.git` instead would
/// happily name whatever repository the run happens to be standing in.
pub fn is_root(directory: &Path) -> bool {
    directory.join("environment").is_dir() && directory.join("config/targets.dotfile").is_file()
}

/// Find the source of truth.
pub fn source() -> Result<Source, String> {
    resolve(
        std::env::var_os(OVERRIDE).map(PathBuf::from),
        std::env::current_dir().ok(),
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf)),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// The order the four places are tried in, taking each as an argument so that
/// it can be settled without an environment to set.
///
/// `DOTFILE_ROOT` is first and is a hard error when it does not hold the
/// repository: a typo there must not quietly fall through to some other
/// checkout and copy the wrong settings into a project.
///
/// Then the working directory, because a run inside a checkout means that
/// checkout; then the directory the binary is in, which is what finds the root
/// under `cargo run` and `cargo test`; then the conventional place. Only when
/// all four come up empty are the compiled-in copies used.
pub fn resolve(
    named: Option<PathBuf>,
    cwd: Option<PathBuf>,
    exe: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<Source, String> {
    if let Some(named) = named {
        if !is_root(&named) {
            return Err(format!(
                "{OVERRIDE} does not name this repository: {} \
                 (wanted environment/ and config/targets.dotfile in it)",
                named.display()
            ));
        }
        return Ok(Source::Repo(named));
    }
    let found = cwd
        .as_deref()
        .and_then(climb)
        .or_else(|| exe.as_deref().and_then(climb))
        .or_else(|| {
            let here = home?.join("dotfiles");
            is_root(&here).then_some(here)
        });
    Ok(match found {
        Some(root) => Source::Repo(root),
        None => Source::Embedded,
    })
}

fn climb(from: &Path) -> Option<PathBuf> {
    let mut at = from;
    loop {
        if is_root(at) {
            return Some(at.to_path_buf());
        }
        at = at.parent()?;
    }
}

/// The text of one source-of-truth config.
pub fn read(source: &Source, name: &str) -> Result<String, String> {
    match source {
        Source::Repo(root) => {
            let path = root.join(TOOLS).join(name);
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
        }
        Source::Embedded => EMBEDDED
            .iter()
            .find(|(embedded, _)| *embedded == name)
            .map(|(_, text)| (*text).to_string())
            .ok_or_else(|| format!("no copy of {name} is compiled in")),
    }
}

/// Which languages a project uses.
///
/// A marker file at the top settles it on its own, because a project can be a
/// Rust project on the strength of its `Cargo.toml` before it has a single
/// `.rs` file. Otherwise the same walk a format run does answers it: one file
/// of a language is enough, and a false positive costs one `n`.
pub fn detect(root: &Path, files: &[PathBuf]) -> Vec<Lang> {
    LANGS
        .into_iter()
        .filter(|lang| {
            lang.markers()
                .iter()
                .any(|marker| root.join(marker).exists())
                || files.iter().any(|file| Lang::of(file) == Some(*lang))
        })
        .collect()
}

/// One config, and where it would land.
pub struct Placement {
    /// Its name in `shared/tools/`.
    pub from: &'static str,
    /// The name it has to land under, which is only different for biome.
    pub name: &'static str,
    pub to: PathBuf,
    /// Whether the project already has a file by that name.
    pub exists: bool,
}

pub fn placements(root: &Path, langs: &[Lang]) -> Vec<Placement> {
    langs
        .iter()
        .filter_map(|lang| {
            let (from, name) = lang.config()?;
            let to = root.join(name);
            Some(Placement {
                from,
                name,
                exists: to.exists(),
                to,
            })
        })
        .collect()
}

/// Ask about each config and copy the ones agreed to.
///
/// The wording says which of the two things is about to happen, so answering
/// `a` is never a blind overwrite. Answers running out is a cancellation:
/// a non-interactive `--add` copies nothing and is not a failure.
pub fn add<'a>(
    source: &Source,
    placements: &'a [Placement],
    ask: &mut dyn FnMut(&str) -> Option<Answer>,
) -> Result<Vec<&'a Placement>, String> {
    let mut done = Vec::new();
    let mut everything = false;
    for placement in placements {
        if !everything {
            let verb = if placement.exists { "replace" } else { "copy" };
            match ask(&format!("  {verb} {}? [Y/n/a] ", placement.name)) {
                Some(Answer::Yes) => {}
                Some(Answer::All) => everything = true,
                Some(Answer::No) => continue,
                None => return Ok(done),
            }
        }
        place(source, placement)?;
        done.push(placement);
    }
    Ok(done)
}

/// Replace the configs a project already has, and introduce none.
pub fn sync<'a>(
    source: &Source,
    placements: &'a [Placement],
) -> Result<Vec<&'a Placement>, String> {
    let mut done = Vec::new();
    for placement in placements.iter().filter(|placement| placement.exists) {
        place(source, placement)?;
        done.push(placement);
    }
    Ok(done)
}

fn place(source: &Source, placement: &Placement) -> Result<(), String> {
    let text = read(source, placement.from)?;
    fs::write(&placement.to, text).map_err(|error| format!("{}: {error}", placement.to.display()))
}

// ------------------------------------------------- pointing a tool at a config

/// Which of a row's files a set of arguments applies to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// All of them. The program resolves once, from the directory it is run
    /// in, so a config deeper in the tree is one it would never have found and
    /// naming this one takes nothing away.
    Row,
    /// Only the files with no config of their own between them and the root.
    ///
    /// The program resolves per file, and a config named on the command line
    /// outranks a nearer one — measured: `stylua --config-path` overrides a
    /// `.stylua.toml` one directory down, and `biome --config-path` turns off
    /// the nested resolution it would otherwise do. So the files that have
    /// their own are run in a second invocation without the flag, and
    /// `shared/nvim/.stylua.toml` and `shared/wezterm/.stylua.toml` keep
    /// winning inside their subtrees.
    Unclaimed(&'static [&'static str]),
}

/// A program that has to be told where this repository's config is, and the
/// names it would look for on its own.
///
/// Keyed by program rather than by language, which is not a detail: the YAML
/// row runs two of them, and `yamlfmt -c` is not a flag — it exits printing
/// the usage text, exactly the way `yamlfmt -w` did. A config belongs to a
/// tool, not to a language.
///
/// Every one of these searches for a config and would find nothing of this
/// repository's, because the configs live under `shared/tools/` rather than at
/// the root. Two of them — taplo and stylua — really did format this whole
/// tree at their own defaults and report it clean. The other two are saved on
/// this machine only by a symlink in `$HOME` that a fresh checkout, a CI
/// runner, or a target outside `$HOME` would not have; naming the config is
/// what stops the settings depending on where the run happens to stand.
pub struct Injection {
    pub program: &'static str,
    /// The flag that names a config outright.
    pub flag: &'static str,
    /// Its name in `shared/tools/`.
    pub file: &'static str,
    /// What the program finds by itself. A target that has one keeps it.
    pub own: &'static [&'static str],
    pub scope: Scope,
}

/// The names each of them looks for, written once so the guard and the scope
/// cannot drift apart.
const OWN_TAPLO: &[&str] = &[".taplo.toml", "taplo.toml"];
const OWN_BIOME: &[&str] = &["biome.json", "biome.jsonc"];
const OWN_STYLUA: &[&str] = &[".stylua.toml", "stylua.toml"];
const OWN_YAMLLINT: &[&str] = &[".yamllint", ".yamllint.yaml", ".yamllint.yml"];

pub const INJECTED: [Injection; 4] = [
    Injection {
        program: "taplo",
        flag: "--config",
        file: ".taplo.toml",
        own: OWN_TAPLO,
        // Resolved once from the working directory: a `.taplo.toml` in a
        // subdirectory is one taplo would never have read.
        scope: Scope::Row,
    },
    Injection {
        program: "biome",
        // biome takes either a directory or a file here, but a directory is
        // searched for `biome.json`/`biome.jsonc` only — handed
        // `shared/tools/`, biome exits 1 with "couldn't find a configuration
        // in the directory", because the copy there is `biome.global.json`.
        // Naming the file itself is what makes the repository's own copy
        // usable without renaming it.
        flag: "--config-path",
        file: "biome.global.json",
        own: OWN_BIOME,
        scope: Scope::Unclaimed(OWN_BIOME),
    },
    Injection {
        program: "stylua",
        flag: "--config-path",
        file: "stylua.toml",
        own: OWN_STYLUA,
        scope: Scope::Unclaimed(OWN_STYLUA),
    },
    Injection {
        program: "yamllint",
        // Measured on 1.38: yamllint does search upward, so on a machine
        // where `~/.yamllint.yaml` is linked it already lints this repository
        // at the shipped settings and nothing is injected. It is here for the
        // machine where that link is absent — a fresh checkout, CI, or any
        // target outside `$HOME` — where it would otherwise fall back to its
        // own defaults: line length 80 as an error, against the 100 as a
        // warning the shipped config asks for.
        flag: "-c",
        file: ".yamllint.yaml",
        own: OWN_YAMLLINT,
        scope: Scope::Row,
    },
];

/// One program's arguments, and which of a row's files they are for.
#[derive(Debug, PartialEq, Eq)]
pub struct Injected {
    pub program: &'static str,
    pub args: Vec<OsString>,
    pub scope: Scope,
}

/// The extra arguments each program needs to run with this repository's
/// settings, for the programs that would otherwise use their own defaults.
///
/// Nothing is injected when the target already has a config the program would
/// find for itself: a project's own settings always win. Nothing is injected
/// either when no checkout was found, because the copies compiled into this
/// binary have no path to name.
pub fn injections(source: &Source, root: &Path) -> Vec<Injected> {
    let Source::Repo(repo) = source else {
        return Vec::new();
    };
    INJECTED
        .iter()
        .filter(|injection| !brings_its_own(root, injection.own))
        .map(|injection| {
            let config = repo.join(TOOLS).join(injection.file);
            Injected {
                program: injection.program,
                args: vec![OsString::from(injection.flag), config.into_os_string()],
                scope: injection.scope,
            }
        })
        .collect()
}

/// The same upward search the programs themselves do from the directory every
/// child is run in, so the answer to "would it find one?" is theirs and not a
/// second opinion.
fn brings_its_own(root: &Path, names: &[&str]) -> bool {
    let mut at = Some(root);
    while let Some(directory) = at {
        if names.iter().any(|name| directory.join(name).is_file()) {
            return true;
        }
        at = directory.parent();
    }
    false
}

/// Split a row's files by whether the program would find a config of its own
/// above them, for the programs that resolve per file.
///
/// Only the directories below the root are looked at. Root and above have
/// already been searched — finding nothing there is why there is anything to
/// inject — so a second walk up to the filesystem root would ask a question
/// that has been answered.
///
/// Memoised per directory: walking up from each of forty `.lua` files under
/// `shared/nvim/lua/plugins/` reads the same chain forty times otherwise.
pub fn partition(root: &Path, own: &[&str], files: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut known: HashMap<PathBuf, bool> = HashMap::new();
    let mut theirs = Vec::new();
    let mut ours = Vec::new();
    for file in files {
        let holder = file.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let has = *known
            .entry(holder)
            .or_insert_with_key(|holder| below(root, holder, own));
        if has { &mut theirs } else { &mut ours }.push(file.clone());
    }
    (theirs, ours)
}

fn below(root: &Path, holder: &Path, own: &[&str]) -> bool {
    let mut at = root.join(holder);
    while at.as_path() != root {
        if own.iter().any(|name| at.join(name).is_file()) {
            return true;
        }
        if !at.pop() {
            return false;
        }
    }
    false
}
