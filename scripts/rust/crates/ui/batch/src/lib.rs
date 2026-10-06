#![forbid(unsafe_code)]

//! The pipeline a batch command shares: walk results are listed under a
//! heading, bounded to a screenful, summarized, confirmed, then applied one by
//! one with failures reported. The walk and the work stay with the command.

use ui_terminal::text;
use ui_theme::Style;

pub const ROWS: usize = 12;
pub const WIDTH: usize = 100;

/// The width to lay rows out for: the terminal's, when it is wide enough to
/// hold a row, and a comfortable default otherwise.
pub fn width() -> usize {
    ui_terminal::terminal_width()
        .filter(|columns| *columns >= 40)
        .unwrap_or(WIDTH)
}

/// One line of a listing: a label on the left and an optional, already styled
/// detail on the right.
pub struct Row {
    label: String,
    detail: String,
}

impl Row {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: String::new(),
        }
    }

    pub fn detailed(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
        }
    }
}

/// What a command should do after the prompt: go on, stop cleanly (a dry run
/// or a refusal), or stop because the answers ran out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Proceed,
    Stop,
    Interrupted,
}

pub struct Options {
    pub every_row: bool,
    pub dry: bool,
    pub yes: bool,
}

/// A run of a batch command, bound to the command's name, targets and style.
pub struct Run<'a> {
    program: &'a str,
    targets: &'a [String],
    style: &'a Style,
    options: Options,
}

impl<'a> Run<'a> {
    pub fn new(
        program: &'a str,
        targets: &'a [String],
        style: &'a Style,
        options: Options,
    ) -> Self {
        Self {
            program,
            targets,
            style,
            options,
        }
    }

    pub fn heading(&self) {
        println!();
        println!(
            "  {}  {}",
            self.style.bold(self.program),
            self.style.teal(&self.targets.join(" "))
        );
    }

    /// Prints a blank line, a bold header and the bounded rows below it.
    pub fn section(&self, header: &str, rows: &[Row]) {
        if rows.is_empty() {
            return;
        }
        println!();
        println!("  {}", self.style.bold(header));
        self.list(rows);
    }

    /// Prints the bounded rows under the heading, with no section header.
    pub fn list(&self, rows: &[Row]) {
        if rows.is_empty() {
            return;
        }
        let limit = if self.options.every_row {
            usize::MAX
        } else {
            ROWS
        };
        let shown = rows.len().min(limit);
        let detail = rows[..shown]
            .iter()
            .map(|row| text::width(&row.detail))
            .max()
            .unwrap_or(0);
        let gap = if detail > 0 { 2 } else { 0 };
        let room = width().saturating_sub(4 + gap + detail).max(16);
        let labels: Vec<String> = rows[..shown]
            .iter()
            .map(|row| text::truncate_front(&row.label, room))
            .collect();
        let left = labels
            .iter()
            .map(|label| text::width(label))
            .max()
            .unwrap_or(0);
        for (row, label) in rows[..shown].iter().zip(&labels) {
            if row.detail.is_empty() {
                println!("    {label}");
            } else {
                let pad = " ".repeat(left.saturating_sub(text::width(label)));
                println!("    {label}{pad}  {}", row.detail);
            }
        }
        if rows.len() > shown {
            let more = format!("\u{2026} and {} more", rows.len() - shown);
            println!("    {}", self.style.dim(&more));
        }
    }

    /// Prints a blank line and a summary line.
    pub fn summary(&self, text: &str) {
        println!();
        println!("  {text}");
    }

    /// Prints a header and one muted line beneath it.
    pub fn note(&self, header: &str, text: &str) {
        println!();
        println!("  {}", self.style.bold(header));
        println!("    {}", self.style.dim(text));
    }

    /// Asks to go on unless `--dry` or `--yes` already answered.
    pub fn decide(&self) -> Decision {
        if self.options.dry {
            return Decision::Stop;
        }
        if self.options.yes {
            return Decision::Proceed;
        }
        println!();
        match ui_cli::confirm("  Continue? [Y/n] ") {
            Some(true) => Decision::Proceed,
            Some(false) => {
                println!("{}: cancelled", self.program);
                Decision::Stop
            }
            // The answers ran out; leave the prompt's line and stop.
            None => {
                println!();
                Decision::Interrupted
            }
        }
    }

    /// Runs `act` over every item, reporting each failure, then prints `done`.
    /// The returned count is the failures.
    pub fn apply<T>(&self, items: &[T], mut act: impl FnMut(&T) -> Result<(), String>) -> usize {
        let mut failures = 0;
        for item in items {
            if let Err(error) = act(item) {
                eprintln!("{}: {error}", self.program);
                failures += 1;
            }
        }
        println!();
        println!("  {}", self.style.dim("done"));
        failures
    }
}
