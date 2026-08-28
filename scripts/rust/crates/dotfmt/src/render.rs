//! Everything a person reads, and the one stream it is allowed on.
//!
//! stdout carries data and nothing else — the formatted body under `--stdin`,
//! a completion script, the command dump, `--help`. conform.nvim makes stdout
//! the replacement buffer, so a stray progress line there is a line pasted
//! into somebody's file. Every function here writes to stderr, in every mode,
//! `--quiet` or not, which is also what makes `dotfmt --check . 2>&1 >/dev/null`
//! a thing worth piping.

use std::path::{Path, PathBuf};

use workstation::Style;

use crate::PROGRAM;
use crate::conf::Mode;

/// What a run came to.
#[derive(Default)]
pub struct Tally {
    /// Files looked at.
    pub total: usize,
    /// Files whose bytes were not what formatting them produces.
    pub changed: usize,
    /// Files that could not be read, written, or made sense of.
    pub failed: usize,
}

/// Who says what, and in what colour.
pub struct Report {
    style: Style,
    verbose: bool,
    quiet: bool,
    check: bool,
}

impl Report {
    pub fn new(verbose: bool, quiet: bool, check: bool) -> Report {
        Report {
            style: Style::for_stderr(),
            verbose,
            quiet,
            check,
        }
    }

    /// A file that was rewritten, or under `--check` one that would be.
    pub fn changed(&self, label: &str, mode: Option<Mode>) {
        if self.quiet {
            return;
        }
        let verb = if self.check {
            self.style.red("needs format")
        } else {
            self.style.green("format")
        };
        eprintln!("  {verb} {}{}", self.style.teal(label), self.note(mode));
    }

    /// A file already in the shape it would be written in.
    pub fn unchanged(&self, label: &str, mode: Option<Mode>) {
        if self.quiet || !self.verbose {
            return;
        }
        eprintln!(
            "  {} {}{}",
            self.style.dim("ok"),
            self.style.dim(label),
            self.note(mode)
        );
    }

    /// Which mode a `.conf` file was laid out in, said only when asked: it is
    /// the answer to why a pattern did or did not match.
    fn note(&self, mode: Option<Mode>) -> String {
        match mode.filter(|_| self.verbose) {
            Some(mode) => format!("  {}", self.style.dim(mode.name())),
            None => String::new(),
        }
    }

    /// Which settings the run is using, and where they came from.
    pub fn settings(&self, source: Option<&Path>) {
        if self.quiet || !self.verbose {
            return;
        }
        let from = match source {
            Some(path) => shorten(path),
            None => "built-in defaults".to_string(),
        };
        eprintln!("  {} {}", self.style.dim("config"), self.style.dim(&from));
    }

    /// Something that did not work. Always said, `--quiet` included: it is
    /// what the exit code is about.
    pub fn failed(&self, message: &str) {
        eprintln!("{PROGRAM}: {message}");
    }

    /// What the whole run came to, in one line.
    ///
    /// A check run never writes, so it never says it formatted anything. The
    /// verb is the one thing a summary has to get right: "1 file formatted"
    /// after `--check` reads as though the tree had just been rewritten.
    pub fn summary(&self, tally: &Tally) {
        if self.quiet {
            return;
        }
        let files = files(tally.total);
        let line = if self.check {
            if tally.changed == 0 {
                format!("{} {files} already formatted", tally.total)
            } else {
                format!(
                    "{} of {} {files} need formatting",
                    tally.changed, tally.total
                )
            }
        } else {
            format!("formatted {} of {} {files}", tally.changed, tally.total)
        };
        eprintln!("{line}");
    }
}

/// A path as the report should name it: relative to the root the run was
/// pointed at, and `~`-shortened when it sits outside that root.
///
/// `format.py` assumed every file lived under the repository, and printed
/// nonsense for anything else. `dotfmt` runs wherever it is pointed.
pub fn label(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.display().to_string(),
        _ => shorten(path),
    }
}

/// A path with `$HOME` written as `~`.
pub fn shorten(path: &Path) -> String {
    let shown = path.display().to_string();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return shown;
    };
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => shown,
    }
}

fn files(count: usize) -> &'static str {
    if count == 1 { "file" } else { "files" }
}
