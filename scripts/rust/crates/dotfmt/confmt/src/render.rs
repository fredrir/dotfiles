use std::path::Path;

use dotfmt_core::report::{Report as SharedReport, Unreadable};
pub use dotfmt_core::report::{Tally, label};

use crate::PROGRAM;
use crate::conf::Mode;

pub struct Report {
    shared: SharedReport,
}

impl Report {
    pub fn new(verbose: bool, quiet: bool, check: bool) -> Self {
        Self {
            shared: SharedReport::new(PROGRAM, verbose, quiet, check, Unreadable::UnlessQuiet),
        }
    }

    pub fn changed(&self, label: &str, mode: Option<Mode>) {
        self.shared.changed_with_note(label, mode.map(Mode::name));
    }

    pub fn unchanged(&self, label: &str, mode: Option<Mode>) {
        self.shared.unchanged_with_note(label, mode.map(Mode::name));
    }

    pub fn settings(&self, source: Option<&Path>) {
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
