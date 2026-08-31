use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use workstation::Answer;

use crate::lang::{LANGS, Lang};

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

const TOOLS: &str = "shared/tools";

pub const OVERRIDE: &str = "DOTFILE_ROOT";

#[derive(Debug)]
pub enum Source {
    Repo(PathBuf),
    Embedded,
}

pub fn is_root(directory: &Path) -> bool {
    directory.join("environment").is_dir() && directory.join("config/targets.dotfile").is_file()
}

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

pub struct Placement {
    pub from: &'static str,
    pub name: &'static str,
    pub to: PathBuf,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Row,
    Unclaimed(&'static [&'static str]),
}

pub struct Injection {
    pub program: &'static str,
    pub flag: &'static str,
    pub file: &'static str,
    pub own: &'static [&'static str],
    pub scope: Scope,
}

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

#[derive(Debug, PartialEq, Eq)]
pub struct Injected {
    pub program: &'static str,
    pub args: Vec<OsString>,
    pub scope: Scope,
}

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
