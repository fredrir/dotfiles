use ui_terminal::text::width;
use ui_theme::Role;
use workstation::Style;

/// Column alignment; numeric columns read best right-aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// A run of text with one presentation, part of a styled cell.
#[derive(Clone, Debug)]
pub struct Span {
    text: String,
    role: Role,
    bold: bool,
}

impl Span {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: Role::Plain,
            bold: false,
        }
    }

    pub fn role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
}

/// A cell: one or more styled spans laid out as a single column value.
#[derive(Clone, Debug, Default)]
pub struct Cell {
    spans: Vec<Span>,
}

impl Cell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new().push(Span::new(text))
    }

    pub fn paint(role: Role, text: impl Into<String>) -> Self {
        Self::new().push(Span::new(text).role(role))
    }

    pub fn push(mut self, span: Span) -> Self {
        self.spans.push(span);
        self
    }

    fn width(&self) -> usize {
        self.spans.iter().map(|span| width(&span.text)).sum()
    }

    fn painted(&self, style: &Style) -> String {
        self.spans
            .iter()
            .map(|span| {
                let painted = style.paint(span.role, &span.text);
                if span.bold {
                    style.bold(&painted)
                } else {
                    painted
                }
            })
            .collect()
    }
}

/// A table column: a muted header and an alignment for its data.
pub struct Column<'a> {
    title: &'a str,
    align: Align,
}

impl<'a> Column<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            align: Align::Left,
        }
    }

    pub fn right(title: &'a str) -> Self {
        Self {
            title,
            align: Align::Right,
        }
    }
}

fn header(style: &Style, columns: &[Column], widths: &[usize]) -> String {
    let cells = columns
        .iter()
        .enumerate()
        .map(|(column, spec)| {
            let padding = widths[column].saturating_sub(width(spec.title));
            let title = style.paint(Role::Muted, spec.title);
            match spec.align {
                Align::Left => format!("{title}{}", " ".repeat(padding)),
                Align::Right => format!("{}{title}", " ".repeat(padding)),
            }
        })
        .collect::<Vec<_>>();
    format!("{}\n", cells.join("  ").trim_end())
}

fn widths(columns: &[Column], rows: &[Vec<Cell>]) -> Vec<usize> {
    columns
        .iter()
        .enumerate()
        .map(|(column, spec)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(Cell::width)
                .max()
                .unwrap_or(0)
                .max(width(spec.title))
        })
        .collect()
}

/// Render a table with muted headers and per-cell semantic color.
pub fn styled(style: &Style, columns: &[Column], rows: &[Vec<Cell>]) -> String {
    let widths = widths(columns, rows);
    let mut out = header(style, columns, &widths);
    for row in rows {
        let cells = columns
            .iter()
            .enumerate()
            .map(|(column, spec)| {
                let cell = row.get(column);
                let padding = widths[column].saturating_sub(cell.map_or(0, Cell::width));
                let body = cell.map_or_else(String::new, |cell| cell.painted(style));
                match spec.align {
                    Align::Left => format!("{body}{}", " ".repeat(padding)),
                    Align::Right => format!("{}{body}", " ".repeat(padding)),
                }
            })
            .collect::<Vec<_>>();
        out.push_str(&format!("{}\n", cells.join("  ").trim_end()));
    }
    out
}

/// Render a plain, uncolored table; kept for callers without a `Style`.
pub fn render(headers: &[&str], rows: &[Vec<String>]) -> String {
    let columns = headers
        .iter()
        .map(|title| Column::new(title))
        .collect::<Vec<_>>();
    let rows = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| Cell::text(cell.as_str()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    styled(&Style::plain(), &columns, &rows)
}

#[cfg(test)]
#[path = "../tests/unit/table_tests.rs"]
mod tests;
