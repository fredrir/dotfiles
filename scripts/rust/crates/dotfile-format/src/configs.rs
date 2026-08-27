//! Where this repository's tool configuration comes from, and how it is put
//! into another project.
//!
//! `--add` offers one config per language the project appears to use and asks
//! about each; `--sync` refreshes the ones already there and introduces
//! nothing. The asymmetry is the point: `--add` is how a project gets these
//! settings, `--sync` is how a project that already agreed to them keeps up,
//! and neither should ever be the other by accident.

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
        "dotfile.dotfile",
        include_str!("../../../../../shared/tools/dotfile.dotfile"),
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
