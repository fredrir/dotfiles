use std::path::{Path, PathBuf};

use crate::fold::Constants;
use crate::quote;

/// `type` and `whence` that report where bundled functions were defined.
const WHENCE: &str = include_str!("../assets/whence.zsh");

/// The bundle: a background `zcompile` when its wordcode is stale, kept only
/// if the bundle it compiled is still current, folded values, then the code.
pub fn bundle(
    path: &Path,
    source: &Path,
    origins: &Path,
    constants: &Constants,
    body: &str,
) -> Result<String, String> {
    let bundle = path
        .to_str()
        .filter(|path| quote::is_plain_path(path))
        .ok_or_else(|| format!("{}: unsupported characters in output path", path.display()))?;
    let wordcode = format!("{bundle}.zwc");
    let staging = format!("{bundle}.$$.zwc");
    let origins = origins
        .to_str()
        .filter(|path| quote::is_plain_path(path))
        .ok_or_else(|| {
            format!(
                "{}: unsupported characters in output path",
                origins.display()
            )
        })?;
    let wrappers = WHENCE
        .replace("@BUNDLE@", bundle)
        .replace("@ORIGINS@", origins);
    let declarations = constants.declarations() + &wrappers;
    let id = crate::script::hash(&format!("{declarations}{body}"));
    let header = format!("# zsh-build: {id:016x} generated from {}", source.display());
    let expected = quote::word(&header);
    let prelude = format!(
        "{header}\n\
         [[ {wordcode} -nt {bundle} ]] || {{ zcompile -U {staging} {bundle} && IFS= read -r __zb_head < {bundle} && [[ $__zb_head == {expected} ]] && command mv -f {staging} {wordcode} || command rm -f {staging} }} &!\n\
         {declarations}",
    );
    crate::script::parse(&prelude).map_err(|error| format!("prelude does not parse: {error}"))?;
    Ok(prelude + body)
}

/// Sourced by `.zshenv` in interactive shells: skips the global startup
/// files the bundle runs, unless anything they depend on changed since.
pub fn guard(
    path: &Path,
    bundle: &Path,
    sources: &[PathBuf],
    profile: Option<&Path>,
) -> Result<String, String> {
    let word = |path: &Path| {
        path.to_str()
            .map(quote::word)
            .ok_or_else(|| format!("{}: not UTF-8", path.display()))
    };
    let guard = word(path)?;
    let link = "${ZDOTDIR:-$HOME}/.zprofile";
    let profile = match profile {
        Some(profile) => format!("{link} -ef {}", word(profile)?),
        None => format!("! -e {link}"),
    };
    let stale = sources
        .iter()
        .map(|source| Ok(format!("{} -nt {guard}", word(source)?)))
        .collect::<Result<Vec<_>, String>>()?
        .join(" ||\n  ");
    let text = format!(
        "# zsh-build: global startup files compiled into {bundle}\n\
         [[ -r {bundle_word} && {profile} ]] || return 0\n\
         [[ {stale} ]] && return 0\n\
         setopt no_global_rcs\n\
         typeset -g {flag}=1\n",
        bundle = bundle.display(),
        bundle_word = word(bundle)?,
        flag = crate::compile::system::FLAG,
    );
    crate::script::parse(&text).map_err(|error| format!("guard does not parse: {error}"))?;
    Ok(text)
}

/// Removes a guard whose section is gone; returns whether one existed.
pub fn remove_guard(path: &Path, dry_run: bool) -> Result<bool, String> {
    if path.symlink_metadata().is_err() {
        return Ok(false);
    }
    if !dry_run {
        std::fs::remove_file(path)
            .map_err(|error| format!("remove {}: {error}", path.display()))?;
    }
    Ok(true)
}

/// Writes the guard when its text changed or a source is newer than it.
pub fn write_guard(
    path: &Path,
    text: &str,
    sources: &[PathBuf],
    dry_run: bool,
) -> Result<bool, String> {
    let modified = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    };
    let current = std::fs::read_to_string(path).ok();
    let stale = match (current.as_deref(), modified(path)) {
        (Some(current), Some(written)) if current == text => sources
            .iter()
            .filter_map(|source| modified(source))
            .any(|source| source > written),
        _ => true,
    };
    if stale && !dry_run {
        write_atomic(path, text)?;
    }
    Ok(stale)
}

pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("{}: no parent", path.display()))?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let staging = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&staging, text)
        .map_err(|error| format!("write {}: {error}", staging.display()))?;
    std::fs::rename(&staging, path).map_err(|error| {
        let _ = std::fs::remove_file(&staging);
        format!("write {}: {error}", path.display())
    })
}

/// `name<TAB>file:line` per bundled function; returns whether it changed.
pub fn write_origins(
    path: &Path,
    origins: &std::collections::BTreeMap<String, String>,
    dry_run: bool,
) -> Result<bool, String> {
    let text: String = origins
        .iter()
        .filter(|(name, location)| !format!("{name}{location}").contains(['\t', '\n']))
        .map(|(name, location)| format!("{name}\t{location}\n"))
        .collect();
    let changed = std::fs::read_to_string(path).map_or(true, |current| current != text);
    if changed && !dry_run {
        write_atomic(path, &text)?;
    }
    Ok(changed)
}

pub fn read_origins(path: &Path) -> Result<std::collections::BTreeMap<String, String>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(name, location)| (name.to_string(), location.to_string()))
        .collect())
}
