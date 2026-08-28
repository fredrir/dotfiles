//! Showing a survey before anything is done about it.
//!
//! Three sections, because there are three fates, and the destructive one is
//! the one to read: a restored file is still in `HEAD`, while a deleted
//! untracked file is nowhere. Columns are measured over the rows that are
//! actually shown, so a long path in the tail of a long section does not
//! stretch the ones above it.

use std::path::Path;

use gix::bstr::BString;
use workstation::Style;

use crate::survey::{Counts, Entry, Fate, Survey};

/// How a plan is shown: the palette, how many rows a section gets, and how
/// much width there is to spend on paths.
pub struct View {
    pub program: &'static str,
    pub style: Style,
    pub rows: usize,
    pub width: usize,
}

/// A row's five columns, already turned into text.
struct Row {
    label: &'static str,
    path: String,
    added: String,
    removed: String,
    note: String,
}

#[derive(Default)]
struct Widths {
    label: usize,
    path: usize,
    added: usize,
    removed: usize,
}

enum Align {
    Left,
    Right,
}

impl View {
    /// Print what would be discarded, and what that would cost.
    pub fn plan(&self, survey: &Survey, paths: &[BString]) {
        let sections = [
            (Fate::Restore, "restore to HEAD".to_string()),
            (Fate::Delete, self.style.red("delete permanently")),
            (Fate::Keep, "kept".to_string()),
        ];
        let listed: Vec<Vec<Row>> = sections
            .iter()
            .map(|(fate, _)| survey.with(*fate).map(|entry| self.row(entry)).collect())
            .collect();

        let mut widths = Widths::default();
        for row in listed.iter().flat_map(|rows| rows.iter().take(self.rows)) {
            widths.label = widths.label.max(row.label.chars().count());
            widths.path = widths.path.max(row.path.chars().count());
            widths.added = widths.added.max(row.added.chars().count());
            widths.removed = widths.removed.max(row.removed.chars().count());
        }

        println!();
        let mut heading = format!(
            "  {}  {}",
            self.style.bold(self.program),
            self.style.teal(&shorten_home(&survey.root))
        );
        if !paths.is_empty() {
            let paths: Vec<String> = paths.iter().map(BString::to_string).collect();
            heading += &format!("  {}", self.style.dim(&paths.join(" ")));
        }
        println!("{heading}");

        for ((_, header), rows) in sections.iter().zip(&listed) {
            if rows.is_empty() {
                continue;
            }
            println!();
            println!("  {}", self.style.bold(header));
            for row in rows.iter().take(self.rows) {
                println!("{}", self.line(row, &widths));
            }
            if rows.len() > self.rows {
                let more = format!("… and {} more", rows.len() - self.rows);
                println!("    {}", self.style.dim(&more));
            }
        }

        let mut summary: Vec<String> = Vec::new();
        for ((fate, _), rows) in sections.iter().zip(&listed) {
            if !rows.is_empty() {
                let what = match fate {
                    Fate::Restore => "restored",
                    Fate::Delete => "deleted",
                    Fate::Keep => "kept",
                };
                summary.push(format!("{} {what}", rows.len()));
            }
        }
        let (added, removed) = survey.totals();
        let mut line = format!("  {}", summary.join(", "));
        if added > 0 {
            line += &format!("   {}", self.style.green(&format!("+{added}")));
        }
        if removed > 0 {
            line += &format!("  {}", self.style.red(&format!("-{removed}")));
        }
        println!();
        println!("{line}");
        println!();
    }

    fn row(&self, entry: &Entry) -> Row {
        // A long path loses its front, because the end of it is the part that
        // says which file this is.
        let room = self.width.saturating_sub(36).max(24);
        let (added, removed) = match entry.counts {
            Counts::Lines { added, removed } => (count(added, '+'), count(removed, '-')),
            _ => (String::new(), String::new()),
        };
        Row {
            label: entry.label,
            path: shorten_front(&entry.shown(), room),
            added,
            removed,
            note: entry.note().unwrap_or_default(),
        }
    }

    fn line(&self, row: &Row, widths: &Widths) -> String {
        let line = format!(
            "    {}  {}  {} {}  {}",
            cell(row.label, widths.label, Align::Left, |text| self
                .style
                .dim(text)),
            cell(&row.path, widths.path, Align::Left, str::to_string),
            cell(&row.added, widths.added, Align::Right, |text| self
                .style
                .green(text)),
            cell(&row.removed, widths.removed, Align::Right, |text| self
                .style
                .red(text)),
            self.style.dim(&row.note),
        );
        line.trim_end().to_string()
    }
}

/// A column, padded outside its colour so that the escapes never count as
/// width and an empty column is plain spaces the line can shed.
fn cell(text: &str, width: usize, align: Align, paint: impl Fn(&str) -> String) -> String {
    let padding = " ".repeat(width.saturating_sub(text.chars().count()));
    if text.is_empty() {
        return padding;
    }
    match align {
        Align::Left => paint(text) + &padding,
        Align::Right => padding + &paint(text),
    }
}

fn count(lines: u32, sign: char) -> String {
    if lines == 0 {
        return String::new();
    }
    format!("{sign}{lines}")
}

fn shorten_front(path: &str, room: usize) -> String {
    let length = path.chars().count();
    if length <= room {
        return path.to_string();
    }
    let tail: String = path.chars().skip(length + 1 - room).collect();
    format!("…{tail}")
}

fn shorten_home(path: &Path) -> String {
    let path = path.display().to_string();
    let Some(home) = std::env::var_os("HOME") else {
        return path;
    };
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some(inside) if inside.is_empty() || inside.starts_with('/') => format!("~{inside}"),
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_pads_outside_its_colour() {
        assert_eq!(cell("ab", 5, Align::Left, str::to_string), "ab   ");
        assert_eq!(cell("ab", 5, Align::Right, str::to_string), "   ab");
        assert_eq!(cell("", 3, Align::Right, |_| "painted".into()), "   ");
        assert_eq!(cell("toolong", 3, Align::Left, str::to_string), "toolong");
    }

    #[test]
    fn a_long_path_keeps_its_end() {
        assert_eq!(shorten_front("short", 10), "short");
        assert_eq!(shorten_front("0123456789", 5), "…6789");
        assert_eq!(shorten_front("0123456789", 10), "0123456789");
    }

    #[test]
    fn counts_of_nothing_leave_their_column_empty() {
        assert_eq!(count(0, '+'), "");
        assert_eq!(count(12, '+'), "+12");
        assert_eq!(count(3, '-'), "-3");
    }

    #[test]
    fn home_becomes_a_tilde_only_at_a_boundary() {
        // SAFETY: the tests in this module do not read HOME concurrently.
        unsafe { std::env::set_var("HOME", "/home/someone") };
        assert_eq!(shorten_home(Path::new("/home/someone/work")), "~/work");
        assert_eq!(shorten_home(Path::new("/home/someone")), "~");
        assert_eq!(
            shorten_home(Path::new("/home/someone-else/work")),
            "/home/someone-else/work"
        );
    }
}
