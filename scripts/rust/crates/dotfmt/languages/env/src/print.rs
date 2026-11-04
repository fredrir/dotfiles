use unicode_width::UnicodeWidthStr;

use crate::comment;
use crate::config::{Config, LineEndings};
use crate::model::{Document, Entry, Item};

#[derive(Clone, Copy)]
enum Arranged<'a> {
    Blank,
    Item(&'a Item<'a>),
}

pub(crate) fn print(document: &Document<'_>, config: &Config, bom: bool) -> String {
    let arranged = arrange(&document.items, config, bom);
    let widths = widths(&arranged, config);
    let mut output = String::new();
    for (index, line) in arranged.iter().enumerate() {
        match line {
            Arranged::Blank => {}
            Arranged::Item(Item::Blank { .. }) => {}
            Arranged::Item(Item::Comment { text }) => {
                output.push_str(&comment::format(text, config));
            }
            Arranged::Item(Item::Entry(entry)) => {
                output.push_str(&render_entry(entry, widths[index], config));
            }
            Arranged::Item(Item::Verbatim { raw }) => output.push_str(raw),
        }
        line_break(&mut output, config);
    }
    if !config.final_newline && !output.is_empty() {
        remove_line_break(&mut output);
    }
    output
}

/// Drops leading and trailing blank runs and caps the ones that remain, so
/// alignment sees the groups the reader will see.
fn arrange<'a>(items: &'a [Item<'a>], config: &Config, bom: bool) -> Vec<Arranged<'a>> {
    let mut arranged = Vec::new();
    let mut pending = 0;
    for item in items {
        if let Item::Blank { count, protected } = item {
            if *protected {
                for _ in 0..pending.min(config.blank_lines) {
                    arranged.push(Arranged::Blank);
                }
                pending = 0;
                for _ in 0..*count {
                    arranged.push(Arranged::Blank);
                }
            } else if !arranged.is_empty() {
                pending += count;
            }
            continue;
        }
        if arranged.is_empty() {
            // A BOM in the middle of the file is content. If dropping leading
            // blanks would move it to byte zero, the next run would read it as
            // the file BOM and strip a value byte; keep one blank as a pin.
            if !bom && starts_with_bom(item) {
                arranged.push(Arranged::Blank);
            }
        } else {
            for _ in 0..pending.min(config.blank_lines) {
                arranged.push(Arranged::Blank);
            }
        }
        pending = 0;
        arranged.push(Arranged::Item(item));
    }
    arranged
}

fn starts_with_bom(item: &Item<'_>) -> bool {
    match item {
        Item::Verbatim { raw } => raw.starts_with('\u{feff}'),
        Item::Entry(entry) => {
            entry.lead_ws.is_empty() && entry.export.is_none() && entry.key.starts_with('\u{feff}')
        }
        Item::Blank { .. } | Item::Comment { .. } => false,
    }
}

fn widths(arranged: &[Arranged<'_>], config: &Config) -> Vec<Option<usize>> {
    let mut widths = vec![None; arranged.len()];
    if !config.align {
        return widths;
    }
    let mut start = 0;
    while start < arranged.len() {
        if !is_entry(arranged[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < arranged.len() && is_entry(arranged[end]) {
            end += 1;
        }
        let width = arranged[start..end]
            .iter()
            .filter_map(|line| match line {
                Arranged::Item(Item::Entry(entry)) => {
                    Some(entry_width(entry).min(config.align_max))
                }
                _ => None,
            })
            .max();
        widths[start..end].fill(width);
        start = end;
    }
    widths
}

fn is_entry(line: Arranged<'_>) -> bool {
    matches!(line, Arranged::Item(Item::Entry(_)))
}

fn entry_width(entry: &Entry<'_>) -> usize {
    let export = entry.export.map_or(0, |export| {
        UnicodeWidthStr::width(export) + UnicodeWidthStr::width(entry.export_gap)
    });
    UnicodeWidthStr::width(entry.lead_ws) + export + UnicodeWidthStr::width(entry.key)
}

fn render_entry(entry: &Entry<'_>, width: Option<usize>, config: &Config) -> String {
    let mut prefix = String::new();
    prefix.push_str(entry.lead_ws);
    if let Some(export) = entry.export {
        prefix.push_str(export);
        prefix.push_str(entry.export_gap);
    }
    prefix.push_str(entry.key);

    let mut text = String::new();
    match width {
        Some(target) => {
            let prefix_width = UnicodeWidthStr::width(prefix.as_str()).min(config.align_max);
            let pad = target.saturating_sub(prefix_width).max(1);
            text.push_str(&prefix);
            text.push_str(&" ".repeat(pad));
            text.push('=');
            if !entry.value.is_empty() {
                // A space before a bare `#` would turn the value into a
                // comment; alignment must not change what the value is.
                if !entry.value.raw().starts_with('#') {
                    text.push(' ');
                }
                text.push_str(entry.value.raw());
            }
        }
        None => {
            text.push_str(&prefix);
            text.push_str(entry.before_eq_gap);
            text.push('=');
            text.push_str(entry.after_eq_gap);
            text.push_str(entry.value.raw());
        }
    }

    if let Some(comment) = entry.comment {
        if config.trim_trailing_whitespace {
            text.push(' ');
        } else {
            text.push_str(entry.trailing_gap);
        }
        text.push_str(&comment::format(comment, config));
    } else if !config.trim_trailing_whitespace {
        text.push_str(entry.trailing_gap);
    }
    text
}

fn line_break(output: &mut String, config: &Config) {
    match config.line_endings {
        LineEndings::Unix => output.push('\n'),
        LineEndings::Windows => output.push_str("\r\n"),
    }
}

fn remove_line_break(output: &mut String) {
    if output.ends_with("\r\n") {
        output.truncate(output.len() - 2);
    } else if output.ends_with('\n') {
        output.pop();
    }
}
