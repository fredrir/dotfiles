use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::parse::{self, Options};
use crate::render;
use crate::repair::Repairs;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Done {
    Unchanged,
    Changed,
}

pub struct Formatted {
    pub text: String,
    pub repairs: Repairs,
}

pub struct Outcome {
    pub done: Done,
    pub repairs: Repairs,
}

/// Lays one body out. `editor` is the flag of the same name: it takes the
/// repairs jq would refuse, and counts them rather than hiding them.
pub fn format(label: &str, input: &[u8], config: &Config, editor: bool) -> Result<Formatted, String> {
    let shaped = shape(label, input, config, editor)?;
    guard(label, &shaped.text, config)?;
    Ok(shaped)
}

fn shape(
    label: &str,
    input: &[u8],
    config: &Config,
    editor: bool,
) -> Result<Formatted, String> {
    let parsed = parse::parse(input, Options { editor })
        .map_err(|problem| format!("{label}:{}", problem.said()))?;
    let Some(value) = parsed.value else {
        // jq makes nothing of an empty input, so nothing is what an empty file
        // becomes. Anything else that holds no value is refused instead: a file
        // of comments is a file somebody wrote, and a formatter that answered
        // it with zero bytes is one nobody could leave on save.
        if input.is_empty() {
            return Ok(Formatted {
                text: String::new(),
                repairs: parsed.repairs,
            });
        }
        return Err(format!("{label}:1:1: expected a value"));
    };
    Ok(Formatted {
        text: render::write(&value, config.layout()),
        repairs: parsed.repairs,
    })
}

/// Two things have to hold before a byte is written: what was just laid out has
/// to read back as strict JSON, and it has to lay out again as itself. The
/// first is what stops this from writing a file jq cannot read, and the second
/// is what stops a second run from finding more to do.
fn guard(label: &str, text: &str, config: &Config) -> Result<(), String> {
    let again = shape(label, text.as_bytes(), config, false)?;
    if again.text != text {
        return Err(broken(label, "laying it out again does not settle"));
    }
    Ok(())
}

pub fn apply(
    path: &Path,
    label: &str,
    config: &Config,
    editor: bool,
    write: bool,
) -> Result<Outcome, String> {
    let raw = fs::read(path).map_err(|error| format!("{label}: {error}"))?;
    let formatted = format(label, &raw, config, editor)?;
    if formatted.text.as_bytes() == raw {
        return Ok(Outcome {
            done: Done::Unchanged,
            repairs: formatted.repairs,
        });
    }
    if write {
        replace(path, &formatted.text).map_err(|error| format!("{label}: {error}"))?;
    }
    Ok(Outcome {
        done: Done::Changed,
        repairs: formatted.repairs,
    })
}

fn broken(label: &str, why: &str) -> String {
    format!("{label}: internal error: {why}, so nothing was written")
}

/// Written beside the target and moved over it, so an interrupted run leaves
/// the file it was writing either as it was or as it should be. A rename swaps
/// the inode, so the mode travels with the contents.
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
        let temporary = parent.join(format!(".{name}.jqfmt-{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((file, temporary)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}
