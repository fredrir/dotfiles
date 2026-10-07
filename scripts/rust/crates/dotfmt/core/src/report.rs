use std::path::Path;

use workstation::Style;
use workstation::path::home_relative;
use workstation::text::plural;

#[derive(Default)]
pub struct Tally {
    pub total: usize,
    pub changed: usize,
    pub failed: usize,
}

#[derive(Clone, Copy)]
pub enum Unreadable {
    Always,
    UnlessQuiet,
}

pub struct Report {
    program: &'static str,
    style: Style,
    verbose: bool,
    quiet: bool,
    check: bool,
    unreadable: Unreadable,
}

impl Report {
    pub fn new(
        program: &'static str,
        verbose: bool,
        quiet: bool,
        check: bool,
        unreadable: Unreadable,
    ) -> Self {
        Self {
            program,
            style: Style::for_stderr(),
            verbose,
            quiet,
            check,
            unreadable,
        }
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    pub fn quiet(&self) -> bool {
        self.quiet
    }

    pub fn check(&self) -> bool {
        self.check
    }

    pub fn changed(&self, label: &str) {
        self.changed_with_note(label, None);
    }

    pub fn changed_with_note(&self, label: &str, note: Option<&str>) {
        if self.quiet {
            return;
        }
        let verb = if self.check {
            self.style.red("needs format")
        } else {
            self.style.green("format")
        };
        eprintln!("  {verb} {}{}", self.style.teal(label), self.note(note));
    }

    pub fn unchanged(&self, label: &str) {
        self.unchanged_with_note(label, None);
    }

    pub fn unchanged_with_note(&self, label: &str, note: Option<&str>) {
        if self.quiet || !self.verbose {
            return;
        }
        eprintln!(
            "  {} {}{}",
            self.style.dim("ok"),
            self.style.dim(label),
            self.note(note)
        );
    }

    fn note(&self, note: Option<&str>) -> String {
        match note.filter(|_| self.verbose) {
            Some(note) => format!("  {}", self.style.dim(note)),
            None => String::new(),
        }
    }

    pub fn settings(&self, source: Option<&Path>) {
        if self.quiet || !self.verbose {
            return;
        }
        let from = match source {
            Some(path) => home_relative(path),
            None => "built-in defaults".to_string(),
        };
        eprintln!("  {} {}", self.style.dim("config"), self.style.dim(&from));
    }

    pub fn skipped(&self, label: &str) {
        if self.verbose && !self.quiet {
            eprintln!("  {} {}", self.style.dim("skip"), self.style.dim(label));
        }
    }

    pub fn failed(&self, message: &str) {
        eprintln!("{}: {message}", self.program);
    }

    pub fn unreadable(&self, count: usize) {
        if count == 0 || (self.quiet && matches!(self.unreadable, Unreadable::UnlessQuiet)) {
            return;
        }
        eprintln!(
            "{}: {count} {} could not be read",
            self.program,
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
        _ => home_relative(path),
    }
}
