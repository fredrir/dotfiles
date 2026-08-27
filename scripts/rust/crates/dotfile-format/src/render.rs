//! What a run looks like once it is finished.
//!
//! Everything here is written to stderr. stdout carries data — the completion
//! script, the command dump, the help — and the prompts, which are the one
//! thing a person answers rather than reads.

use std::path::{Path, PathBuf};

use workstation::Style;

use crate::configs::{Placement, Source};
use crate::lang::Mode;
use crate::run::Ran;

/// The name of the run, and what it is being run over.
pub fn heading(program: &str, root: &Path, what: &str, style: &Style) -> Vec<String> {
    let mut line = format!("  {}  {}", style.bold(program), style.teal(&shorten(root)));
    if !what.is_empty() {
        line += &format!("  {}", style.dim(what));
    }
    vec![String::new(), line, String::new()]
}

/// One row per language, then whatever the tools had to say.
pub fn report(done: &[Ran], mode: Mode, style: &Style) -> Vec<String> {
    let name = done
        .iter()
        .map(|ran| ran.lang.name().chars().count())
        .max()
        .unwrap_or(0);
    let count = done
        .iter()
        .map(|ran| ran.files.to_string().len())
        .max()
        .unwrap_or(0);

    let mut lines: Vec<String> = done
        .iter()
        .map(|ran| {
            format!(
                "  {:name$}  {:>count$} {:<5}  {}",
                ran.lang.name(),
                ran.files,
                files(ran.files),
                status(ran, mode, style),
            )
        })
        .collect();
    for ran in done.iter().filter(|ran| !ran.output.is_empty()) {
        lines.push(String::new());
        lines.push(format!("  {}", style.bold(ran.lang.name())));
        lines.extend(ran.output.trim_end().lines().map(str::to_string));
    }
    lines
}

/// What became of one language, in as few words as it takes.
fn status(ran: &Ran, mode: Mode, style: &Style) -> String {
    let missing = ran
        .missing
        .iter()
        .map(|program| format!("{program} not installed"))
        .collect::<Vec<_>>()
        .join(", ");
    if ran.failed {
        return with(&style.red("failed"), &missing, style);
    }
    if ran.findings {
        return with(&style.red("findings"), &missing, style);
    }
    if let Some(note) = &ran.note {
        return with(&style.dim(note), &missing, style);
    }
    // Nothing ran at all, so the missing tools are the whole story rather
    // than a footnote to it.
    if ran.ran == 0 {
        return style.dim(&missing);
    }
    let done = match mode {
        Mode::Write => "formatted",
        Mode::Check => "ok",
    };
    with(&style.green(done), &missing, style)
}

fn with(word: &str, missing: &str, style: &Style) -> String {
    if missing.is_empty() {
        return word.to_string();
    }
    format!("{word}  {}", style.dim(missing))
}

/// What the whole run came to, in one line.
pub fn tally(done: &[Ran], mode: Mode) -> String {
    let total: usize = done.iter().map(|ran| ran.files).sum();
    let verb = match mode {
        Mode::Write => "formatted",
        Mode::Check => "checked",
    };
    let mut parts = vec![format!("{total} {} {verb}", files(total))];
    let findings = done.iter().filter(|ran| ran.findings).count();
    if findings > 0 {
        parts.push(format!("{findings} with findings"));
    }
    let failed = done.iter().filter(|ran| ran.failed).count();
    if failed > 0 {
        parts.push(format!("{failed} that could not be run"));
    }
    let missing: usize = done.iter().map(|ran| ran.missing.len()).sum();
    if missing > 0 {
        parts.push(format!("{missing} not installed"));
    }
    parts.join(", ")
}

/// What `--add` or `--sync` did, one line per file.
///
/// `.editorconfig` is the one file in the set a project plausibly already has
/// with unrelated content in it, so replacing that one is worth seeing.
pub fn placed(done: &[&Placement], style: &Style) -> Vec<String> {
    done.iter()
        .map(|placement| {
            let verb = if placement.exists {
                "replaced"
            } else {
                "copied"
            };
            let name = if placement.exists && placement.name == ".editorconfig" {
                style.red(placement.name)
            } else {
                placement.name.to_string()
            };
            format!("  {verb} {name}")
        })
        .collect()
}

/// Where the configs came from, named only when it is not the repository —
/// copies compiled into a binary can be older than the repository they came
/// from, and a person deserves to know which they just took.
pub fn provenance(source: &Source, style: &Style) -> Option<String> {
    match source {
        Source::Repo(_) => None,
        Source::Embedded => Some(format!(
            "  {}",
            style.dim("from the copies built into this binary; no checkout was found")
        )),
    }
}

/// A path as a person would write it.
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
