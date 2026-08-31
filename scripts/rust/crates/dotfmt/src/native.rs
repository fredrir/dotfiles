use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::block;
use crate::conf::{self, Mode};
use crate::config::Config;
use crate::select::Token;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Conf,
    Block,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Done {
    Unchanged,
    Changed,
}

pub struct Outcome {
    pub done: Done,
    pub mode: Option<Mode>,
}

pub fn kind(path: &Path) -> Option<Kind> {
    match path.extension()?.to_str()? {
        "conf" | "config" => Some(Kind::Conf),
        "dotfile" => Some(Kind::Block),
        _ => None,
    }
}

pub fn formatter(token: Token) -> Kind {
    match token {
        Token::Conf | Token::Config | Token::Empty => Kind::Conf,
        Token::Dotfile => Kind::Block,
    }
}

pub fn format(path: &Path, label: &str, text: &str, config: &Config) -> Result<String, String> {
    let Some(kind) = kind(path) else {
        return Ok(text.to_string());
    };
    format_as(path, label, text, kind, config)
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

pub fn apply(
    path: &Path,
    label: &str,
    kind: Kind,
    config: &Config,
    write: bool,
) -> Result<Outcome, String> {
    let raw = fs::read(path).map_err(|error| format!("{label}: {error}"))?;
    let text = String::from_utf8(raw).map_err(|_| format!("{label}: not UTF-8"))?;
    let formatted = format_as(path, label, &text, kind, config)?;
    let mode = (kind == Kind::Conf).then(|| conf::mode(&shown(path)));
    if formatted == text {
        return Ok(Outcome {
            done: Done::Unchanged,
            mode,
        });
    }
    if write {
        replace(path, &formatted).map_err(|error| format!("{label}: {error}"))?;
    }
    Ok(Outcome {
        done: Done::Changed,
        mode,
    })
}

fn shape(
    path: &Path,
    label: &str,
    text: &str,
    kind: Kind,
    config: &Config,
) -> Result<String, String> {
    match kind {
        Kind::Conf => Ok(conf::format(text, conf::mode(&shown(path)))),
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

fn replace(path: &Path, text: &str) -> io::Result<()> {
    let path = &fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let permissions = fs::metadata(path)?.permissions();
    let (mut file, temporary) = sibling(path)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::set_permissions(&temporary, permissions))
        .and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        fs::remove_file(&temporary).ok();
    }
    written
}

fn sibling(path: &Path) -> io::Result<(File, PathBuf)> {
    let parent = path.parent().filter(|at| !at.as_os_str().is_empty());
    let parent = parent.unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default().display().to_string();
    let mut attempt = 0;
    loop {
        let temporary = parent.join(format!(".{name}.dotfmt-{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((file, temporary)),
            // Somebody else is already using that name. There is no reason to
            // keep trying forever, and a hundred collisions is a real problem
            // rather than bad luck.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}
