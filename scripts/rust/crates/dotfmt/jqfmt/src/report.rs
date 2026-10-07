use std::path::Path;

use dotfmt_core::report::{Report as SharedReport, Unreadable};
pub use dotfmt_core::report::{Tally, label};

use crate::PROGRAM;
use crate::repair::Repairs;

pub struct Report {
    shared: SharedReport,
}

impl Report {
    pub fn new(verbose: bool, quiet: bool, check: bool) -> Self {
        Self {
            shared: SharedReport::new(PROGRAM, verbose, quiet, check, Unreadable::UnlessQuiet),
        }
    }

    pub fn changed(&self, label: &str) {
        self.shared.changed(label);
    }

    pub fn unchanged(&self, label: &str) {
        self.shared.unchanged(label);
    }

    /// What `--editor` fixed, named rather than counted in a summary: the value
    /// that was written is not the value that was there, and the whole run's
    /// report is the wrong place to find that out.
    pub fn repaired(&self, label: &str, repairs: Repairs) {
        if self.shared.quiet() || repairs.is_empty() {
            return;
        }
        let verb = if self.shared.check() {
            "would fix"
        } else {
            "fixed"
        };
        eprintln!(
            "{PROGRAM}: {}: {verb} {}",
            self.shared.style().teal(label),
            repairs.describe()
        );
    }

    pub fn settings(&self, source: Option<&Path>, warnings: &[String]) {
        if self.shared.quiet() {
            return;
        }
        for warning in warnings {
            eprintln!("{PROGRAM}: {}", self.shared.style().dim(warning));
        }
        self.shared.settings(source);
    }

    pub fn failed(&self, message: &str) {
        self.shared.failed(message);
    }

    pub fn unreadable(&self, count: usize) {
        self.shared.unreadable(count);
    }

    pub fn summary(&self, tally: &Tally) {
        self.shared.summary(tally);
    }
}
