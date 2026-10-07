use std::path::Path;

use crate::block;
use crate::conf;
use crate::config::Config;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Conf,
    Block,
}

pub fn format(path: &Path, input: &[u8], config: &Config) -> Result<String, String> {
    let label = path.display().to_string();
    let text = std::str::from_utf8(input).map_err(|_| format!("{label}: not UTF-8"))?;
    let kind = if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("dotfile"))
    {
        Kind::Block
    } else {
        Kind::Conf
    };
    format_as(path, &label, text, kind, config)
}

pub fn format_as(
    path: &Path,
    label: &str,
    text: &str,
    kind: Kind,
    config: &Config,
) -> Result<String, String> {
    let formatted = shape(path, label, text, kind, config)?;
    guard(path, label, text, &formatted, kind, config)?;
    Ok(formatted)
}

fn shape(
    path: &Path,
    label: &str,
    text: &str,
    kind: Kind,
    config: &Config,
) -> Result<String, String> {
    match kind {
        Kind::Conf => Ok(conf::format(
            text,
            conf::mode(&shown(path)),
            config.final_newline,
        )),
        Kind::Block => block::format(text, config)
            .map_err(|problem| format!("{label}:{}: {}", problem.line, problem.message)),
    }
}

fn guard(
    path: &Path,
    label: &str,
    text: &str,
    formatted: &str,
    kind: Kind,
    config: &Config,
) -> Result<(), String> {
    if shape(path, label, formatted, kind, config)? != formatted {
        return Err(broken(label, "laying it out again does not settle"));
    }
    if kind == Kind::Block {
        let before = block::signature(text)
            .map_err(|problem| format!("{label}:{}: {}", problem.line, problem.message))?;
        let after = block::signature(formatted)
            .map_err(|problem| format!("{label}:{}: {}", problem.line, problem.message))?;
        if before != after {
            return Err(broken(label, "the entries it holds would change"));
        }
    }
    Ok(())
}

fn shown(path: &Path) -> String {
    path.display().to_string()
}

fn broken(label: &str, why: &str) -> String {
    format!("{label}: internal error: {why}, so nothing was written")
}
