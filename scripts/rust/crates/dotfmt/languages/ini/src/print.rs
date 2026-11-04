use crate::config::{Config, IndentType};
use crate::dialect::Dialect;
use crate::parse::{Document, Entry, Line, LineKind, Separator, comment_body, trim_end_layout};

pub fn print(document: &Document<'_>, config: &Config, dialect: Dialect) -> String {
    let widths = alignment_widths(&document.lines, config, dialect);
    let mut output: Vec<String> = Vec::new();
    let mut in_block = false;
    let mut pending_blank = 0;
    for (index, line) in document.lines.iter().enumerate() {
        if matches!(line.kind, LineKind::Blank) {
            if !output.is_empty() {
                pending_blank += 1;
            }
            continue;
        }
        if !output.is_empty() && pending_blank > 0 {
            // Blank lines inside a configparser continuation are part of the
            // value when empty_lines_in_values is enabled, so preserve their
            // exact count there instead of applying the layout limit.
            let blanks =
                if dialect == Dialect::Generic && matches!(line.kind, LineKind::Continuation(_)) {
                    pending_blank
                } else {
                    pending_blank.min(config.blank_lines)
                };
            for _ in 0..blanks {
                output.push(String::new());
            }
        }
        pending_blank = 0;
        let text = match &line.kind {
            LineKind::Blank => unreachable!("blank lines are handled above"),
            LineKind::Comment(comment) => {
                let prefix = if dialect == Dialect::Ssh {
                    String::new()
                } else {
                    indent(dialect, config, in_block)
                };
                format!(
                    "{prefix}{}{}",
                    comment.marker,
                    comment_body(comment.body, config.format_comments)
                )
            }
            LineKind::Section(header) => {
                in_block = true;
                (*header).to_owned()
            }
            LineKind::Entry(entry) => render_entry(dialect, config, in_block, entry, widths[index]),
            LineKind::Continuation(raw) => (*raw).to_owned(),
            LineKind::Raw(raw) => trim_end_layout(raw).to_owned(),
        };
        output.push(text);
    }
    output.join("\n")
}

fn alignment_widths(lines: &[Line<'_>], config: &Config, dialect: Dialect) -> Vec<usize> {
    let mut widths = vec![0; lines.len()];
    if !config.align || dialect == Dialect::Systemd {
        return widths;
    }
    let mut index = 0;
    while index < lines.len() {
        if !matches!(lines[index].kind, LineKind::Entry(_)) {
            index += 1;
            continue;
        }
        let mut group = Vec::new();
        let mut next = index;
        while next < lines.len() {
            match &lines[next].kind {
                LineKind::Entry(entry) => {
                    if includes_in_alignment(entry, dialect) {
                        group.push((next, key_width(entry.key)));
                    }
                    next += 1;
                }
                // Dropped blank lines merge the entry groups they separated.
                LineKind::Blank if config.blank_lines == 0 => next += 1,
                _ => break,
            }
        }
        let maximum = group.iter().map(|(_, width)| *width).max();
        if let Some(maximum) = maximum
            && maximum <= config.align_max
        {
            for (entry, _) in group {
                widths[entry] = maximum;
            }
        }
        index = next.max(index + 1);
    }
    widths
}

fn includes_in_alignment(entry: &Entry<'_>, dialect: Dialect) -> bool {
    dialect != Dialect::Ssh || entry.separator == Separator::Space
}

fn render_entry(
    dialect: Dialect,
    config: &Config,
    in_block: bool,
    entry: &Entry<'_>,
    max_width: usize,
) -> String {
    let mut output = indent(dialect, config, in_block);
    match dialect {
        Dialect::Systemd => {
            output.push_str(entry.key);
            output.push('=');
            output.push_str(entry.value);
        }
        Dialect::Ssh => {
            output.push_str(entry.key);
            if entry.separator == Separator::Equals {
                output.push('=');
            } else if !entry.value.is_empty() {
                let padding = max_width.saturating_sub(key_width(entry.key));
                output.push_str(&" ".repeat(padding + 1));
                output.push_str(entry.value);
            }
        }
        _ => {
            output.push_str(entry.key);
            let padding = max_width.saturating_sub(key_width(entry.key));
            output.push_str(&" ".repeat(padding + 1));
            output.push('=');
            if !entry.value.is_empty() {
                output.push(' ');
                output.push_str(entry.value);
            }
        }
    }
    if entry.value.ends_with('\\') {
        output.push_str(entry.trailing);
    }
    output
}

fn indent(dialect: Dialect, config: &Config, in_block: bool) -> String {
    if !in_block {
        return String::new();
    }
    let default = match dialect {
        Dialect::Git => 2,
        Dialect::Ssh => 4,
        // Their entries always start at column zero, whatever the settings say.
        Dialect::Generic | Dialect::EditorConfig | Dialect::Systemd | Dialect::Auto => {
            return String::new();
        }
    };
    match config.indent_type {
        Some(IndentType::Tabs) => "\t".to_owned(),
        Some(IndentType::Spaces) | None => " ".repeat(config.indent.unwrap_or(default)),
    }
}

fn key_width(text: &str) -> usize {
    text.chars().count()
}
