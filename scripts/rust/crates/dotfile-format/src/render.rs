use std::path::{Path, PathBuf};

use workstation::Style;

use crate::configs::{Placement, Source};
use crate::lang::Mode;
use crate::run::Ran;

pub fn heading(program: &str, root: &Path, what: &str, style: &Style) -> Vec<String> {
    let mut line = format!("  {}  {}", style.bold(program), style.teal(&shorten(root)));
    if !what.is_empty() {
        line += &format!("  {}", style.dim(what));
    }
    vec![String::new(), line, String::new()]
}

const NAMED: usize = 5;

const CLIP: usize = 96;

pub fn summary(done: &[Ran], mode: Mode, style: &Style) -> Vec<String> {
    let counted: Vec<&Ran> = done
        .iter()
        .filter(|ran| ran.ran > 0 || ran.failed)
        .collect();
    if counted.is_empty() {
        return absent(done, style).into_iter().collect();
    }
    let reported: Vec<&&Ran> = counted
        .iter()
        .filter(|ran| ran.failed || ran.findings)
        .collect();
    let name = reported
        .iter()
        .map(|ran| ran.lang.name().chars().count())
        .max()
        .unwrap_or(0);
    let count = reported
        .iter()
        .map(|ran| ran.files.to_string().len())
        .max()
        .unwrap_or(0);

    let mut lines = Vec::new();
    for ran in &reported {
        lines.push(format!(
            "{:name$}  {:>count$} {}  {}",
            ran.lang.name(),
            ran.files,
            files(ran.files),
            if ran.failed {
                style.red("failed")
            } else {
                style.red("findings")
            },
        ));
        lines.extend(culprits(ran, style));
    }

    let total: usize = counted.iter().map(|ran| ran.files).sum();
    let made: usize = counted
        .iter()
        .filter(|ran| !ran.failed && !ran.findings)
        .map(|ran| ran.files)
        .sum();
    lines.push(format!(
        "{made} / {total} {} {}",
        files(total),
        match mode {
            Mode::Write => "formatted",
            Mode::Check => "clean",
        }
    ));
    lines
}

fn culprits(ran: &Ran, style: &Style) -> Vec<String> {
    if ran.blamed.is_empty() {
        // Nothing in the output looked like one of the files this row was
        // given — a tool that failed before it opened one, or that names them
        // in some way this does not read. Its first word is more use than
        // silence.
        return ran
            .output
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(|line| format!("  {}", style.dim(&clip(line.trim()))))
            .into_iter()
            .collect();
    }
    let mut lines: Vec<String> = ran
        .blamed
        .iter()
        .take(NAMED)
        .map(|name| format!("  {name}"))
        .collect();
    if ran.blamed.len() > NAMED {
        lines.push(format!(
            "  {}",
            style.dim(&format!("… and {} more", ran.blamed.len() - NAMED))
        ));
    }
    lines
}

fn absent(done: &[Ran], style: &Style) -> Option<String> {
    let mut named: Vec<&str> = Vec::new();
    for program in done.iter().flat_map(|ran| &ran.missing) {
        if !named.contains(program) {
            named.push(program);
        }
    }
    (!named.is_empty()).then(|| style.dim(&format!("{} not installed", named.join(", "))))
}

fn clip(line: &str) -> String {
    if line.chars().count() <= CLIP {
        return line.to_string();
    }
    format!("{}…", line.chars().take(CLIP - 1).collect::<String>())
}

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

pub fn provenance(source: &Source, style: &Style) -> Option<String> {
    match source {
        Source::Repo(_) => None,
        Source::Embedded => Some(format!(
            "  {}",
            style.dim("from the copies built into this binary; no checkout was found")
        )),
    }
}

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
