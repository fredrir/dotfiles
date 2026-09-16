use std::path::Path;

use workstation::Style;
use workstation::path::home_relative;
use workstation::text::plural;

use crate::PROGRAM;
use crate::repair::Repairs;

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

    pub fn changed(&self, label: &str) {
        if self.quiet {
            return;
        }
        let verb = if self.check {
            self.style.red("needs format")
        } else {
            self.style.green("format")
        };
        eprintln!("  {verb} {}", self.style.teal(label));
    }

    pub fn unchanged(&self, label: &str) {
        if self.quiet || !self.verbose {
            return;
        }
        eprintln!("  {} {}", self.style.dim("ok"), self.style.dim(label));
    }

    /// What `--editor` fixed, named rather than counted in a summary: the value
    /// that was written is not the value that was there, and the whole run's
    /// report is the wrong place to find that out.
    pub fn repaired(&self, label: &str, repairs: Repairs) {
        if self.quiet || repairs.is_empty() {
            return;
        }
        let verb = if self.check { "would fix" } else { "fixed" };
        eprintln!(
            "{PROGRAM}: {}: {verb} {}",
            self.style.teal(label),
            repairs.describe()
        );
    }

    pub fn settings(&self, source: Option<&Path>, warnings: &[String]) {
        if self.quiet {
            return;
        }
        for warning in warnings {
            eprintln!("{PROGRAM}: {}", self.style.dim(warning));
        }
        if !self.verbose {
            return;
        }
        let from = match source {
            Some(path) => home_relative(path),
            None => "built-in defaults".to_string(),
        };
        eprintln!("  {} {}", self.style.dim("config"), self.style.dim(&from));
    }

    pub fn failed(&self, message: &str) {
        eprintln!("{PROGRAM}: {message}");
    }

    pub fn unreadable(&self, count: usize) {
        if self.quiet || count == 0 {
            return;
        }
        eprintln!(
            "{PROGRAM}: {count} {} could not be read",
            plural(count, "directory", "directories")
        );
    }

    pub fn summary(&self, tally: &Tally) {
        if self.quiet {
            return;
        }
        let files = plural(tally.total, "file", "files");
        let line = if self.check {
            if tally.changed == 0 {
                format!("{} {files} already formatted", tally.total)
            } else {
                format!("{} of {} {files} need formatting", tally.changed, tally.total)
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
        _ => home_relative(path),
    }
}
