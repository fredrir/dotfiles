use std::path::{Path, PathBuf};

use workstation::Style;

use crate::PROGRAM;
use crate::conf::Mode;

#[derive(Default)]
pub struct Tally {
    pub total: usize,
    pub changed: usize,
    pub failed: usize,
}

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

    fn note(&self, mode: Option<Mode>) -> String {
        match mode.filter(|_| self.verbose) {
            Some(mode) => format!("  {}", self.style.dim(mode.name())),
            None => String::new(),
        }
    }

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

    pub fn failed(&self, message: &str) {
        eprintln!("{PROGRAM}: {message}");
    }

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

pub fn label(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.display().to_string(),
        _ => shorten(path),
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
