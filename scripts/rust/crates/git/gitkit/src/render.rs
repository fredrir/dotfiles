use gix::bstr::BString;
use workstation::{Style, path, text};

use crate::survey::{Counts, Entry, Fate, Survey};

pub struct View {
    pub program: &'static str,
    pub style: Style,
    pub rows: usize,
    pub width: usize,
}

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
            self.style.teal(&path::home_relative(&survey.root))
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
            path: text::truncate_front(&entry.shown(), room),
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

#[cfg(test)]
#[path = "../tests/unit/render_tests.rs"]
mod tests;
