//! Read, lay out, check the result against itself, and only then write.
//!
//! All three modes come through here — a tree walk, `--check`, and `--stdin` —
//! so none of them can drift into formatting a file differently from the
//! others. The formatters themselves are text in and text out; this is the
//! only module that knows a file exists.
//!
//! Writing is a sibling temp file, an fsync and a rename, with the original's
//! mode carried over. `format.py` truncates the file and then writes into it,
//! so a crash between the two leaves a truncated `hyprland.conf` — and a
//! formatter that runs on save is exactly the program that must never be able
//! to do that.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::block;
use crate::conf::{self, Mode};
use crate::config::Config;

/// Which formatter owns a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `.conf` and `.config`.
    Conf,
    /// `.dotfile`.
    Block,
}

/// What formatting one file came to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Done {
    Unchanged,
    Changed,
}

/// What happened to one file, and the one thing worth saying about how.
pub struct Outcome {
    pub done: Done,
    /// Which `.conf` mode was chosen, for `--verbose`. The pattern that picked
    /// it is the thing people get wrong, and this is where they find out.
    pub mode: Option<Mode>,
}

/// Which formatter a path belongs to, decided by the only thing that decides
/// it. `None` is a file dotfmt does not own.
pub fn kind(path: &Path) -> Option<Kind> {
    match path.extension()?.to_str()? {
        "conf" | "config" => Some(Kind::Conf),
        "dotfile" => Some(Kind::Block),
        _ => None,
    }
}

/// Lay out a body, guarded. A file dotfmt does not own comes back untouched,
/// which is what makes `--stdin` safe to point at anything.
pub fn format(path: &Path, label: &str, text: &str, config: &Config) -> Result<String, String> {
    let Some(kind) = kind(path) else {
        return Ok(text.to_string());
    };
    let formatted = shape(path, label, text, kind, config)?;
    guard(path, label, text, &formatted, kind, config)?;
    Ok(formatted)
}

/// Format one file in place, or under `--check` only work out whether it
/// would change.
pub fn apply(path: &Path, label: &str, config: &Config, write: bool) -> Result<Outcome, String> {
    let raw = fs::read(path).map_err(|error| format!("{label}: {error}"))?;
    let text = String::from_utf8(raw).map_err(|_| format!("{label}: not UTF-8"))?;
    let formatted = format(path, label, &text, config)?;
    let mode = (kind(path) == Some(Kind::Conf)).then(|| config.mode_for(&shown(path)));
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
        Kind::Conf => Ok(conf::format(text, config.mode_for(&shown(path)))),
        Kind::Block => block::format(text, config)
            .map_err(|problem| format!("{label}:{}: {}", problem.line, problem.message)),
    }
}

/// Check the output against itself before it is allowed anywhere near the disk.
///
/// Two questions. Does laying the output out again produce the same bytes —
/// which catches a rule that grows a line a little on every run, as an
/// unbalanced quote in kitty mode does. And, for a `.dotfile`, does the output
/// still parse into the same entries in the same blocks — which catches a rule
/// that moved data rather than moving whitespace. A formatter that fails
/// either has a bug, and the only safe thing a buggy formatter can do is
/// refuse to write.
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

/// The path a glob is matched against: the one that was given, the way
/// `fnmatch` saw it, so `*/hypr/*` still recognises a file named from the walk
/// root down.
fn shown(path: &Path) -> String {
    path.display().to_string()
}

fn broken(label: &str, why: &str) -> String {
    format!("{label}: internal error: {why}, so nothing was written")
}

/// Put `text` where `path` is, atomically.
///
/// The temp file is a sibling so the rename stays on one filesystem, is
/// created exclusively so two dotfmts on one tree cannot share it, and is
/// removed again if anything at all goes wrong. The rename itself either
/// happened or did not; there is no state in between for a crash to find.
///
/// It renames over the file the path *resolves* to, which is the one thing a
/// rename gets wrong that a truncating write does not: half the configs this
/// repository owns are reached through a symlink in `~/.config`, and renaming
/// over the link would replace it with a regular file and quietly strand the
/// copy under version control.
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

/// An empty file next to `path` that nothing else is holding.
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
