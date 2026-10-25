use crate::config::Config;
use crate::parse::{Assign, Block, Document, Entry, Preamble, Trivia};
use crate::verify::case;

pub fn print(document: &Document, text: &str, config: &Config) -> String {
    let mut segments = Vec::new();
    for item in &document.items {
        let block = block(&item.block, text, config);
        segments.push(match trivia(&item.leading, text, config) {
            Some(leading) => format!("{leading}\n{block}"),
            None => block,
        });
    }
    if let Some(trailer) = trivia(&document.trailer, text, config) {
        segments.push(trailer);
    }
    segments.join(&"\n".repeat(1 + config.blank_lines))
}

fn block(block: &Block, text: &str, config: &Config) -> String {
    match block {
        Block::Entry(value) => print_entry(value, text, config),
        Block::Assign(value) => print_assign(value, text, config),
        Block::Preamble(value) => print_preamble(value, text),
        Block::Comment(comment) => comment.span.slice(text).to_owned(),
    }
}

fn print_entry(entry: &Entry, text: &str, config: &Config) -> String {
    let indent = config.indent_unit();
    let kind = case(entry.kind.slice(text), config.entry_type_case);
    let key = entry.key.slice(text).trim();
    if entry.fields.is_empty() && entry.trailing.is_empty() {
        return format!("@{kind}{}{key}{}", entry.open as char, entry.close as char);
    }
    let names: Vec<String> = entry
        .fields
        .iter()
        .map(|field| case(field.name.slice(text), config.field_name_case))
        .collect();
    let width = if config.align {
        names
            .iter()
            .map(|name| name.chars().count())
            .max()
            .filter(|width| config.align_max == 0 || *width <= config.align_max)
            .unwrap_or(0)
    } else {
        0
    };
    let mut output = format!("@{kind}{}{key},\n", entry.open as char);
    for (index, field) in entry.fields.iter().enumerate() {
        for comment in &field.leading {
            output.push_str(&indent);
            output.push_str(comment.text(text));
            output.push('\n');
        }
        let name = &names[index];
        let padding = " ".repeat(width.saturating_sub(name.chars().count()));
        output.push_str(&indent);
        output.push_str(name);
        output.push_str(&padding);
        output.push_str(" = ");
        let raw = field.value.span.slice(text).trim();
        let start = indent.chars().count() + name.chars().count() + padding.len() + 3;
        if config.wrap {
            output.push_str(&wrap_value(raw, start, config, &indent));
        } else {
            output.push_str(raw);
        }
        let last = index + 1 == entry.fields.len();
        let comma = if last && !config.trailing_comma {
            ""
        } else {
            ","
        };
        if let Some(trailing) = field.trailing {
            output.push_str(comma);
            output.push(' ');
            output.push_str(trailing.text(text));
        } else {
            output.push_str(comma);
        }
        output.push('\n');
    }
    for comment in &entry.trailing {
        output.push_str(&indent);
        output.push_str(comment.text(text));
        output.push('\n');
    }
    output.push(entry.close as char);
    output
}

fn print_assign(assign: &Assign, text: &str, config: &Config) -> String {
    let indent = config.indent_unit();
    let name = case(assign.name.slice(text), config.field_name_case);
    let value = assign.value.span.slice(text).trim();
    if assign.leading.is_empty() && assign.trailing.is_empty() {
        return format!(
            "@string{}{name} = {value}{}",
            assign.open as char, assign.close as char
        );
    }
    let mut output = format!("@string{}\n", assign.open as char);
    for comment in &assign.leading {
        output.push_str(&indent);
        output.push_str(comment.text(text));
        output.push('\n');
    }
    output.push_str(&indent);
    output.push_str(&name);
    output.push_str(" = ");
    output.push_str(value);
    output.push_str(",\n");
    for comment in &assign.trailing {
        output.push_str(&indent);
        output.push_str(comment.text(text));
        output.push('\n');
    }
    output.push(assign.close as char);
    output
}

fn print_preamble(preamble: &Preamble, text: &str) -> String {
    format!(
        "@preamble{}{}{}",
        preamble.open as char,
        preamble.inner.slice(text).trim(),
        preamble.close as char
    )
}

fn trivia(trivia: &Trivia, text: &str, config: &Config) -> Option<String> {
    let mut lines: Vec<&str> = Vec::new();
    let mut pending = 0usize;
    for line in trivia.span.slice(text).lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            pending += 1;
            continue;
        }
        if !lines.is_empty() {
            lines.extend(std::iter::repeat_n("", pending.min(config.blank_lines)));
        }
        pending = 0;
        lines.push(line.trim_start());
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn wrap_value(raw: &str, start: usize, config: &Config, continuation: &str) -> String {
    if raw.contains('\n') {
        return raw.to_owned();
    }
    let unit = config.indent_unit();
    let mut output = String::with_capacity(raw.len() + 8);
    let mut rest = raw;
    let mut column = start;
    loop {
        if column + rest.chars().count() <= config.width {
            output.push_str(rest);
            break;
        }
        let limit = config.width.saturating_sub(column).max(1);
        let Some((prefix, suffix)) = break_point(rest, limit, config.wrap_min()) else {
            output.push_str(rest);
            break;
        };
        output.push_str(prefix.trim_end());
        output.push('\n');
        output.push_str(continuation);
        output.push_str(&unit);
        column = continuation.chars().count() + unit.chars().count();
        rest = suffix.trim_start();
        if rest.is_empty() {
            break;
        }
    }
    output
}

fn break_point(text: &str, limit: usize, minimum: usize) -> Option<(&str, &str)> {
    let minimum = minimum.min(limit);
    let mut previous = None;
    let mut candidate = None;
    let mut preferred = None;
    for (index, (byte, character)) in text.char_indices().enumerate() {
        if index >= limit {
            break;
        }
        if character == ' ' && index > 0 && previous != Some('\\') && previous != Some('%') {
            candidate = Some(byte);
            if index >= minimum {
                preferred = Some(byte);
            }
        }
        previous = Some(character);
    }
    let index = preferred.or(candidate)?;
    Some((&text[..index], &text[index + 1..]))
}
