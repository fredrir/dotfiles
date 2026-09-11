use crate::{Item, Outcome, Picker};
use ui_terminal::text::{fit, width};
use ui_terminal::{Event, Key, Screen, Surface};
use ui_theme::{Role, Style};
use ui_widgets::{KeyHint, Line, hints};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub kind: String,
    pub option: String,
}

#[derive(Clone, Debug)]
pub struct Column {
    pub kind: String,
    pub title: String,
    pub options: Vec<(String, String)>,
    pub index: usize,
}

impl Column {
    pub fn new(kind: &str, title: &str, options: Vec<(String, String)>) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            options,
            index: 0,
        }
    }
    pub fn picked(&self) -> Pick {
        Pick {
            kind: self.kind.clone(),
            option: self
                .options
                .get(self.index)
                .map(|option| option.0.clone())
                .unwrap_or_default(),
        }
    }
}

pub fn choose(title: &str, options: &[String]) -> Result<Option<usize>, String> {
    let style = Style::for_stdout();
    match Picker::new(
        title,
        options
            .iter()
            .enumerate()
            .map(|(index, label)| Item::new(index, label)),
        &style,
    )
    .run()
    .map_err(|error| error.to_string())?
    {
        Outcome::Selected(selected) => Ok(selected.first().copied()),
        Outcome::Unavailable => Err("terminal unavailable".into()),
        Outcome::Cancelled | Outcome::Interrupted => Ok(None),
    }
}

pub fn cascade(
    title: &str,
    expand: impl Fn(&[Pick]) -> Option<Column>,
) -> Result<Option<Vec<Pick>>, String> {
    match cascade_outcome(title, expand).map_err(|error| error.to_string())? {
        Outcome::Selected(picks) => Ok(Some(picks)),
        Outcome::Unavailable => Err("terminal unavailable".into()),
        Outcome::Cancelled | Outcome::Interrupted => Ok(None),
    }
}

pub fn cascade_outcome(
    title: &str,
    expand: impl Fn(&[Pick]) -> Option<Column>,
) -> std::io::Result<Outcome<Pick>> {
    let Some(mut screen) = Screen::open()? else {
        return Ok(Outcome::Unavailable);
    };
    let style = Style::for_stdout();
    cascade_in(title, expand, &style, &mut screen)
}

pub fn cascade_in<T: Surface>(
    title: &str,
    expand: impl Fn(&[Pick]) -> Option<Column>,
    style: &Style,
    terminal: &mut T,
) -> Result<Outcome<Pick>, T::Error> {
    let result = interact(title, expand, style, terminal);
    let cleared = terminal.clear();
    match (result, cleared) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}

fn interact<T: Surface>(
    title: &str,
    expand: impl Fn(&[Pick]) -> Option<Column>,
    style: &Style,
    terminal: &mut T,
) -> Result<Outcome<Pick>, T::Error> {
    let Some(mut root) = expand(&[]).filter(|column| !column.options.is_empty()) else {
        return Ok(Outcome::Cancelled);
    };
    root.index = root.index.min(root.options.len() - 1);
    let mut columns = vec![root];
    let mut widths = vec![column_width(&columns[0])];
    let mut style = ui_theme::LiveStyle::new(style);
    let mut dirty = true;
    loop {
        dirty |= style.poll();
        let (width, height) = terminal.size();
        if dirty {
            terminal.draw(&frame(
                title,
                &columns,
                &widths,
                width,
                height,
                style.style(),
            ))?;
            dirty = false;
        }
        let Some(event) = terminal.poll_event(std::time::Duration::from_secs(1))? else {
            continue;
        };
        dirty = true;
        let Event::Key(key) = event else {
            continue;
        };
        let column = columns.last_mut().expect("root column");
        match key {
            Key::Interrupt => return Ok(Outcome::Interrupted),
            Key::Escape | Key::Char('q') => return Ok(Outcome::Cancelled),
            Key::Left | Key::Backspace | Key::Char('h') => {
                if columns.len() > 1 {
                    columns.pop();
                    widths.pop();
                }
            }
            Key::Up | Key::Char('k') => {
                column.index = (column.index + column.options.len() - 1) % column.options.len()
            }
            Key::Down | Key::Char('j') => column.index = (column.index + 1) % column.options.len(),
            Key::Home => column.index = 0,
            Key::End => column.index = column.options.len() - 1,
            Key::PageUp => {
                column.index = column.index.saturating_sub(height.saturating_sub(4).max(1))
            }
            Key::PageDown => {
                column.index = column
                    .index
                    .saturating_add(height.saturating_sub(4).max(1))
                    .min(column.options.len() - 1)
            }
            Key::Enter | Key::Right | Key::Char('l') => {
                let picks: Vec<_> = columns.iter().map(Column::picked).collect();
                if let Some(mut child) = expand(&picks).filter(|column| !column.options.is_empty())
                {
                    child.index = child.index.min(child.options.len() - 1);
                    widths.push(column_width(&child));
                    columns.push(child);
                } else if key == Key::Enter {
                    return Ok(Outcome::Selected(picks));
                }
            }
            Key::Char(digit) if digit.is_ascii_digit() && digit != '0' => {
                let index = (digit as u8 - b'1') as usize;
                if index < column.options.len() {
                    column.index = index;
                }
            }
            _ => {}
        }
    }
}

fn column_width(column: &Column) -> usize {
    column
        .options
        .iter()
        .map(|(label, detail)| {
            width(label)
                + if detail.is_empty() {
                    2
                } else {
                    4 + width(detail)
                }
        })
        .max()
        .unwrap_or(0)
        .max(width(&column.title))
}

pub fn cascade_frame(
    title: &str,
    columns: &[Column],
    width: usize,
    height: usize,
    style: &Style,
) -> Vec<String> {
    let widths: Vec<_> = columns.iter().map(column_width).collect();
    frame(title, columns, &widths, width, height, style)
}

fn frame(
    title: &str,
    columns: &[Column],
    widths: &[usize],
    width: usize,
    height: usize,
    style: &Style,
) -> Vec<String> {
    if columns.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }
    let width = width.saturating_sub(1);
    let active = columns.len() - 1;
    let mut first = 0;
    while first < active && widths[first..].iter().sum::<usize>() + (active - first) * 3 > width {
        first += 1;
    }
    let heading = if first == 0 {
        title.to_owned()
    } else {
        format!(
            "{title}  ‹ {}",
            columns[..first]
                .iter()
                .map(|column| column.picked().option)
                .collect::<Vec<_>>()
                .join(" › ")
        )
    };
    let mut lines = vec![Line::styled(heading, Role::Strong).paint(style, width)];
    let headers = columns[first..]
        .iter()
        .enumerate()
        .map(|(slot, column)| {
            let shown = fit(&column.title, widths[first + slot]);
            format!(
                "{}{}",
                shown,
                " ".repeat(widths[first + slot].saturating_sub(ui_terminal::text::width(&shown)))
            )
        })
        .collect::<Vec<_>>()
        .join("   ");
    lines.push(Line::styled(headers, Role::Muted).paint(style, width));
    let room = height.saturating_sub(3);
    let shown = columns[first..]
        .iter()
        .map(|column| column.options.len().min(room))
        .max()
        .unwrap_or(0);
    for row in 0..shown {
        let mut text = String::new();
        for (index, column) in columns.iter().enumerate().skip(first) {
            if index > first {
                text.push_str("   ");
            }
            let item = column.index.saturating_sub(room.saturating_sub(1)) + row;
            let cell = column
                .options
                .get(item)
                .map(|(label, detail)| {
                    format!(
                        "{}{}{}",
                        if item == column.index { "▸ " } else { "  " },
                        label,
                        if index == active && !detail.is_empty() {
                            format!("  {detail}")
                        } else {
                            String::new()
                        }
                    )
                })
                .unwrap_or_default();
            text.push_str(&cell);
            if index < active {
                text.push_str(
                    &" ".repeat(widths[index].saturating_sub(ui_terminal::text::width(&cell))),
                );
            }
        }
        lines.push(Line::plain(text).paint(style, width));
    }
    lines.push(
        hints(&[
            KeyHint::new("↑↓/jk", "move"),
            KeyHint::new("←→/hl", "level"),
            KeyHint::new("enter", "select"),
            KeyHint::new("esc", "cancel"),
        ])
        .paint(style, width),
    );
    lines.truncate(height);
    lines
}
