use std::collections::BTreeMap;
use std::path::Path;

use super::metadata;
use crate::context::Context;
use clap_complete::Shell;

pub fn emit(shell: Shell) -> Result<(), String> {
    if shell != Shell::Zsh {
        return Err(format!("no {shell} completions; available: zsh"));
    }
    print!("{}", script(&metadata::native()));
    Ok(())
}
pub fn write_all(context: &Context, directory: &Path) -> Result<usize, String> {
    let mut scripts = metadata::declared(context)?
        .commands
        .into_iter()
        .map(|(name, tree)| (name, script(&tree)))
        .collect::<BTreeMap<_, _>>();
    scripts.insert("dotfile".into(), script(&metadata::native()));
    let count = scripts.len();
    let body = scripts.into_values().collect::<Vec<_>>().join("\n");
    crate::fs::write_generated(&directory.join("tools-completion.zsh"), body.as_bytes())?;
    Ok(count)
}
pub fn emit_program(context: &Context, program: &str, shell: &str) -> Result<(), String> {
    if shell != "zsh" {
        return Err(format!("no {shell} completions; available: zsh"));
    }
    let tree = if program == "dotfile" {
        metadata::native()
    } else {
        metadata::declared(context)?
            .commands
            .remove(program)
            .ok_or_else(|| format!("{program}: command metadata unavailable"))?
    };
    print!("{}", script(&tree));
    Ok(())
}

pub use workstation::surface::zsh::script;
