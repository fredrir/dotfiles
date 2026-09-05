use std::path::PathBuf;

use workstation::{Style, path, text};

pub const ROWS: usize = 12;
pub const WIDTH: usize = 100;

pub struct Row {
    pub path: String,
    pub minus: usize,
    pub plus: usize,
}

pub fn width() -> usize {
    workstation::terminal_width()
        .filter(|columns| *columns >= 40)
        .unwrap_or(WIDTH)
}

pub fn heading(targets: &[PathBuf], style: &Style) {
    let shown: Vec<String> = targets.iter().map(|target| path::shorten(target)).collect();
    println!();
    println!(
        "  {}  {}",
        style.bold("doc-purge"),
        style.teal(&shown.join(" "))
    );
}

pub fn purged(rows: &[Row], all: bool, style: &Style) {
    if rows.is_empty() {
        return;
    }
    let limit = if all { usize::MAX } else { ROWS };
    let shown = rows.len().min(limit);
    let counts: Vec<(String, String)> = rows[..shown]
        .iter()
        .map(|row| {
            (
                format!("-{}", row.minus),
                if row.plus > 0 {
                    format!("+{}", row.plus)
                } else {
                    String::new()
                },
            )
        })
        .collect();
    let minus_room = counts
        .iter()
        .map(|(minus, _)| minus.len())
        .max()
        .unwrap_or(0);
    let room = width().saturating_sub(10 + minus_room).max(16);
    let paths: Vec<String> = rows[..shown]
        .iter()
        .map(|row| text::truncate_front(&row.path, room))
        .collect();
    let left = paths
        .iter()
        .map(|path| path.chars().count())
        .max()
        .unwrap_or(0);

    println!();
    println!("  {}", style.bold("purge"));
    for (path, (minus, plus)) in paths.iter().zip(counts.iter()) {
        let gap = " ".repeat(left - path.chars().count());
        let lead = " ".repeat(minus_room - minus.len());
        println!(
            "    {path}{gap}  {lead}{}  {}",
            style.red(minus),
            style.green(plus)
        );
    }
    if rows.len() > shown {
        let more = format!("\u{2026} and {} more", rows.len() - shown);
        println!("    {}", style.dim(&more));
    }
}

pub fn listed(header: &str, rows: &[(String, String)], all: bool, style: &Style) {
    if rows.is_empty() {
        return;
    }
    let limit = if all { usize::MAX } else { ROWS };
    let shown = rows.len().min(limit);
    let room = width().saturating_sub(40).max(16);
    let paths: Vec<String> = rows[..shown]
        .iter()
        .map(|(path, _)| text::truncate_front(path, room))
        .collect();
    let left = paths
        .iter()
        .map(|path| path.chars().count())
        .max()
        .unwrap_or(0);
    println!();
    println!("  {}", style.bold(header));
    for (path, (_, reason)) in paths.iter().zip(rows[..shown].iter()) {
        let gap = " ".repeat(left - path.chars().count());
        println!("    {path}{gap}  {}", style.dim(reason));
    }
    if rows.len() > shown {
        let more = format!("\u{2026} and {} more", rows.len() - shown);
        println!("    {}", style.dim(&more));
    }
}
