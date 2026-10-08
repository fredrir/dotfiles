use std::path::Path;

use crate::fold::Constants;
use crate::quote;

/// The bundle: a background `zcompile` when its wordcode is stale, folded
/// values, then the compiled startup code.
pub fn bundle(
    path: &Path,
    source: &Path,
    constants: &Constants,
    body: &str,
) -> Result<String, String> {
    let bundle = path
        .to_str()
        .filter(|path| quote::is_plain_path(path))
        .ok_or_else(|| format!("{}: unsupported characters in output path", path.display()))?;
    let wordcode = format!("{bundle}.zwc");
    let staging = format!("{bundle}.$$.zwc");
    let prelude = format!(
        "# zsh-build: generated from {}\n\
         [[ {wordcode} -nt {bundle} ]] || {{ zcompile -U {staging} {bundle} && command mv -f {staging} {wordcode} }} &!\n\
         {}",
        source.display(),
        constants.declarations(),
    );
    crate::script::parse(&prelude).map_err(|error| format!("prelude does not parse: {error}"))?;
    Ok(prelude + body)
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
