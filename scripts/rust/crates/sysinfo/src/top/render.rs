//! The process table: shares, cores, and sizes each in their own color.

use super::{Report, Row, Sort};
use ui_terminal::text::sanitize;
use ui_theme::Role;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use workstation::Style;

const GAP: &str = "  ";
const USER_WIDTH: usize = 16;
const COMMAND_WIDTH: usize = 12;
const SHARE: Role = Role::Accent;
const CORES: Role = Role::Info;
const SIZE: Role = Role::Theirs;
const NONE: &str = "—";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Cpu,
    Memory,
    Gpu,
}

impl Metric {
    fn of(sort: Sort) -> Option<Self> {
        match sort {
            Sort::Total => None,
            Sort::Cpu => Some(Self::Cpu),
            Sort::Memory => Some(Self::Memory),
            Sort::Gpu => Some(Self::Gpu),
        }
    }
}

/// The metric a row ranks by: the sorted one, else the row's largest share.
pub fn emphasis(row: &Row, sort: Sort) -> Option<Metric> {
    Metric::of(sort).or_else(|| {
        [
            (Metric::Cpu, row.cpu),
            (Metric::Memory, row.memory_share),
            (Metric::Gpu, row.gpu.unwrap_or(0.0)),
        ]
        .into_iter()
        .filter(|(_, share)| *share > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(metric, _)| metric)
    })
}

pub fn percent(value: f64) -> String {
    if value >= 99.95 {
        "100%".into()
    } else {
        format!("{:.1}%", value.max(0.0))
    }
}

pub fn cores(value: f64) -> String {
    if value >= 9.95 {
        format!("{value:.0}c")
    } else {
        format!("{:.1}c", value.max(0.0))
    }
}

pub fn size(bytes: u64) -> String {
    let kib = bytes as f64 / 1024.0;
    let mib = kib / 1024.0;
    let gib = mib / 1024.0;
    if kib < 999.5 {
        format!("{kib:.0}K")
    } else if mib < 999.5 {
        format!("{mib:.0}M")
    } else if gib < 9.95 {
        format!("{gib:.1}G")
    } else if gib < 999.5 {
        format!("{gib:.0}G")
    } else {
        format!("{:.1}T", gib / 1024.0)
    }
}

pub fn age(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const YEAR: u64 = 365 * DAY;
    match seconds {
        s if s < MINUTE => format!("{s}s"),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < WEEK => format!("{}d", s / DAY),
        s if s < YEAR => format!("{}w", s / WEEK),
        s => format!("{}y", s / YEAR),
    }
}

/// Cut `text` to `width` terminal cells, marking the cut with an ellipsis.
pub fn truncate(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.into();
    }
    let mut kept = String::new();
    let mut used = 0;
    for character in text.chars() {
        let cells = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + cells + 1 > width {
            break;
        }
        kept.push(character);
        used += cells;
    }
    kept.push('…');
    kept
}

struct Cells {
    user: String,
    pid: String,
    cpu: String,
    cores: String,
    memory: String,
    size: String,
    gpu: String,
    age: String,
    command: String,
    count: String,
    emphasis: Option<Metric>,
}

impl Cells {
    fn of(row: &Row, sort: Sort) -> Self {
        Self {
            user: truncate(&sanitize(&row.user), USER_WIDTH),
            pid: row.pid.to_string(),
            cpu: percent(row.cpu),
            cores: cores(row.cores),
            memory: percent(row.memory_share),
            size: size(row.memory),
            gpu: row.gpu.map_or_else(|| NONE.into(), percent),
            age: age(row.age),
            command: sanitize(&row.command),
            count: if row.count > 1 {
                format!(" ×{}", row.count)
            } else {
                String::new()
            },
            emphasis: emphasis(row, sort),
        }
    }
}

struct Widths {
    user: usize,
    pid: usize,
    cpu: usize,
    cores: usize,
    memory: usize,
    size: usize,
    gpu: usize,
    age: usize,
}

impl Widths {
    fn of(cells: &[Cells]) -> Self {
        let widest = |title: &str, cell: fn(&Cells) -> &str| {
            cells
                .iter()
                .map(|cells| UnicodeWidthStr::width(cell(cells)))
                .chain([title.len()])
                .max()
                .unwrap_or(0)
        };
        Self {
            user: widest("USER", |c| &c.user),
            pid: widest("PID", |c| &c.pid),
            cpu: widest("CPU", |c| &c.cpu),
            cores: widest("", |c| &c.cores),
            memory: widest("MEM", |c| &c.memory),
            size: widest("", |c| &c.size),
            gpu: widest("GPU", |c| &c.gpu),
            age: widest("TIME", |c| &c.age),
        }
    }

    /// Cells before COMMAND, gaps included.
    fn fixed(&self) -> usize {
        self.user
            + self.pid
            + self.cpu
            + 1
            + self.cores
            + self.memory
            + 1
            + self.size
            + self.gpu
            + self.age
            + 6 * GAP.len()
    }
}

fn left(style: &Style, role: Role, text: &str, width: usize) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(text));
    format!("{}{}", style.paint(role, text), " ".repeat(padding))
}

fn right(style: &Style, role: Role, text: &str, width: usize) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(text));
    format!("{}{}", " ".repeat(padding), style.paint(role, text))
}

pub fn render(report: &Report, sort: Sort, style: &Style, width: Option<usize>) -> String {
    let cells: Vec<Cells> = report.rows.iter().map(|row| Cells::of(row, sort)).collect();
    let widths = Widths::of(&cells);
    let command_width = width.map(|width| width.saturating_sub(widths.fixed()).max(COMMAND_WIDTH));
    let mut lines = vec![header(style, sort, &widths)];
    for cells in &cells {
        let share = |metric: Metric, text: &str, width: usize| {
            let role = if text == NONE { Role::Muted } else { SHARE };
            let painted = right(style, role, text, width);
            if cells.emphasis == Some(metric) {
                style.bold(&painted)
            } else {
                painted
            }
        };
        let command = match command_width {
            Some(width) => truncate(
                &cells.command,
                width.saturating_sub(UnicodeWidthStr::width(cells.count.as_str())),
            ),
            None => cells.command.clone(),
        };
        let line = [
            left(style, Role::Plain, &cells.user, widths.user),
            right(style, Role::Plain, &cells.pid, widths.pid),
            format!(
                "{} {}",
                share(Metric::Cpu, &cells.cpu, widths.cpu),
                right(style, CORES, &cells.cores, widths.cores)
            ),
            format!(
                "{} {}",
                share(Metric::Memory, &cells.memory, widths.memory),
                right(style, SIZE, &cells.size, widths.size)
            ),
            share(Metric::Gpu, &cells.gpu, widths.gpu),
            right(style, Role::Plain, &cells.age, widths.age),
            format!("{command}{}", cells.count),
        ]
        .join(GAP);
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n") + "\n"
}

fn header(style: &Style, sort: Sort, widths: &Widths) -> String {
    let title = |metric: Option<Metric>, text: &str, width: usize| {
        let role = if metric.is_some() && metric == Metric::of(sort) {
            Role::Strong
        } else {
            Role::Muted
        };
        right(style, role, text, width)
    };
    [
        left(style, Role::Muted, "USER", widths.user),
        title(None, "PID", widths.pid),
        format!(
            "{}{}",
            title(Some(Metric::Cpu), "CPU", widths.cpu),
            " ".repeat(widths.cores + 1)
        ),
        format!(
            "{}{}",
            title(Some(Metric::Memory), "MEM", widths.memory),
            " ".repeat(widths.size + 1)
        ),
        title(Some(Metric::Gpu), "GPU", widths.gpu),
        title(None, "TIME", widths.age),
        style.paint(Role::Muted, "COMMAND"),
    ]
    .join(GAP)
}

#[cfg(test)]
#[path = "../../tests/unit/top/render.rs"]
mod tests;
