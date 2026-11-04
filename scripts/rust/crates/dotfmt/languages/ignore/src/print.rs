use crate::config::Config;
use crate::dialect::Dialect;
use crate::parse::{Document, Line};

pub fn print(document: &Document, config: &Config, dialect: Dialect) -> String {
    let dialect = dialect.base();
    let align_width = attribute_width(document, config, dialect);
    let mut rendered = Vec::new();
    let mut pending_blanks = 0usize;
    let mut started = false;
    for line in &document.lines {
        if matches!(line, Line::Blank) {
            if started {
                pending_blanks += 1;
            }
            continue;
        }
        if started {
            for _ in 0..pending_blanks.min(config.blank_lines) {
                rendered.push(String::new());
            }
        }
        pending_blanks = 0;
        started = true;
        rendered.push(render(line, config, dialect, align_width));
    }
    rendered.join("\n")
}

fn render(line: &Line, config: &Config, dialect: Dialect, align_width: usize) -> String {
    match line {
        Line::Blank => String::new(),
        Line::Comment {
            indent,
            body,
            trailing,
        } => {
            let mut output = String::new();
            if dialect != Dialect::Attributes || !config.trim_leading_whitespace {
                output.push_str(indent);
            }
            if config.format_comments {
                output.push_str(&format_comment(body));
            } else {
                output.push_str(body);
            }
            if !config.trim_trailing_whitespace {
                output.push_str(trailing);
            }
            output
        }
        Line::Pattern { body, trailing } => {
            let mut output = String::from(*body);
            if !config.trim_trailing_whitespace {
                output.push_str(trailing);
            }
            output
        }
        Line::Attributes {
            indent,
            pattern,
            attrs,
            trailing,
        } => {
            let mut output = String::new();
            if !config.trim_leading_whitespace {
                output.push_str(indent);
            }
            output.push_str(pattern);
            if !attrs.is_empty() {
                if align_width > 0 {
                    let padding = align_width.saturating_sub(pattern.chars().count()) + 1;
                    output.push_str(&" ".repeat(padding));
                } else {
                    output.push(' ');
                }
                output.push_str(&attrs.join(" "));
            }
            if !config.trim_trailing_whitespace {
                output.push_str(trailing);
            }
            output
        }
        Line::Opaque(line) => (*line).to_owned(),
    }
}

fn attribute_width(document: &Document, config: &Config, dialect: Dialect) -> usize {
    if dialect != Dialect::Attributes || !config.align {
        return 0;
    }
    let widest = document
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::Attributes { pattern, attrs, .. } if !attrs.is_empty() => {
                Some(pattern.chars().count())
            }
            _ => None,
        })
        .max();
    match widest {
        Some(width) if config.align_max == 0 || width <= config.align_max => width,
        _ => 0,
    }
}

fn format_comment(body: &str) -> String {
    let rest = &body[1..];
    if rest.is_empty() {
        return body.to_owned();
    }
    if matches!(rest.as_bytes()[0], b'#' | b'!') {
        return body.to_owned();
    }
    let text = rest.trim_start_matches([' ', '\t']);
    if text.is_empty() {
        return "#".to_owned();
    }
    format!("# {text}")
}
