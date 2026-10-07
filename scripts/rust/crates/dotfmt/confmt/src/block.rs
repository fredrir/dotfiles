use crate::conf::lines;
use crate::config::Config;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Blank,
    Comment,
    Open,
    Close,
    Entry,
    Bare,
}

pub struct Line<'a> {
    pub class: Class,
    #[cfg_attr(not(test), allow(dead_code))]
    pub number: usize,
    pub body: &'a str,
    pub block: &'a str,
    pub depth: usize,
    pub key: &'a str,
    pub value: &'a str,
}

impl Line<'_> {
    fn signature(&self) -> (Class, &str, &str, &str) {
        (self.class, self.block, self.key, self.value)
    }
}

#[derive(Debug)]
pub struct Problem {
    pub line: usize,
    pub message: String,
}

pub fn parse(text: &str) -> Result<Vec<Line<'_>>, Problem> {
    let mut found = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    let mut number = 0;
    for raw in lines(text) {
        number += 1;
        let mut rest = raw;
        loop {
            let patterns = open.last().is_some_and(|name| {
                matches!(
                    *name,
                    "include"
                        | "exclude"
                        | "included_files"
                        | "included-files"
                        | "excluded_files"
                        | "excluded-files"
                        | "whitelist"
                        | "blacklist"
                )
            });
            let body = rest.trim_start_matches([' ', '\t']);
            let body = if patterns { body } else { trim(body) };
            let structural = body.split_once('#').map_or(body, |(head, _)| trim(head));
            let (class, key, value, tail) = if body.starts_with('#') || body.is_empty() {
                let (class, key, value) = classify(body);
                (class, key, value, "")
            } else if structural.starts_with('}') {
                let tail = &body[1..];
                let (comment, tail) = trailing_comment(tail);
                (Class::Close, "", comment, tail)
            } else if !patterns
                && let Some(at) = structural.find('{')
                && !structural[..at].contains(['=', '\'', '"'])
            {
                let name = trim(&structural[..at]);
                let (comment, tail) = trailing_comment(&body[at + 1..]);
                (Class::Open, name, comment, tail)
            } else {
                let at = closing_brace(body, patterns);
                let (part, tail) = at.map_or((body, ""), |at| body.split_at(at));
                let (class, key, value) = if patterns {
                    (Class::Bare, part, "")
                } else {
                    classify(trim(part))
                };
                let depth = open.len();
                found.push(Line {
                    class,
                    number,
                    body: if patterns { part } else { trim(part) },
                    block: open.last().copied().unwrap_or(""),
                    depth,
                    key,
                    value,
                });
                if tail.is_empty() {
                    break;
                }
                rest = tail;
                continue;
            };
            let (block, depth) = match class {
                Class::Open => {
                    let depth = open.len();
                    open.push(key);
                    (key, depth)
                }
                Class::Close => {
                    let Some(name) = open.pop() else {
                        return Err(Problem::at(number, "unexpected }"));
                    };
                    (name, open.len())
                }
                _ => (open.last().copied().unwrap_or(""), open.len()),
            };
            found.push(Line {
                class,
                number,
                body,
                block,
                depth,
                key,
                value,
            });
            if tail.trim().is_empty() {
                break;
            }
            rest = tail;
        }
    }
    if let Some(name) = open.last() {
        return Err(Problem::at(number, format!("missing }} for {name}")));
    }
    Ok(found)
}

fn trailing_comment(tail: &str) -> (&str, &str) {
    if tail.trim_start().starts_with('#') {
        (trim(tail), "")
    } else {
        ("", tail)
    }
}

// A structural close is outside quoted values and balanced literal braces.
// Pattern blocks additionally require whitespace before an inline close,
// preserving braces that are part of a filename and escaped trailing spaces.
fn closing_brace(text: &str, patterns: bool) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    let mut braces = 0usize;
    let mut separated = false;
    for (at, ch) in text.char_indices() {
        let separator = ch.is_whitespace() && !escaped && quote.is_none();
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() {
            if matches!(ch, '\'' | '"') && (!patterns || at == 0) {
                quote = Some(ch);
            } else if ch == '#' && !patterns {
                break;
            } else if ch == '{' && !patterns {
                braces += 1;
            } else if ch == '}' {
                if braces > 0 {
                    braces -= 1;
                } else if !patterns || separated {
                    return Some(at);
                }
            }
        }
        separated = separator;
    }
    None
}

pub fn format(text: &str, config: &Config) -> Result<String, Problem> {
    let parsed = parse(text)?;
    // A file of nothing but blank lines is left exactly as it is. There is no
    // layout to impose on it, and a formatter that can turn a file into zero
    // bytes is a formatter nobody can leave on save.
    if parsed.iter().all(|line| line.class == Class::Blank) {
        return Ok(text.to_string());
    }
    let arranged = arrange(&parsed, config);
    let widths = columns(&arranged, config);
    let mut out = String::with_capacity(text.len());
    for (item, width) in arranged.iter().zip(&widths) {
        if let Item::Text(line) = item {
            out.push_str(&render(line, *width, config));
        }
        out.push('\n');
    }
    if !config.final_newline {
        out.pop();
    }
    Ok(out)
}

pub fn signature(text: &str) -> Result<Vec<(Class, String, String, String)>, Problem> {
    Ok(parse(text)?
        .iter()
        .filter(|line| line.class != Class::Blank)
        .map(|line| {
            let (class, block, key, value) = line.signature();
            (class, block.to_string(), key.to_string(), value.to_string())
        })
        .collect())
}

impl Problem {
    fn at(line: usize, message: impl Into<String>) -> Problem {
        Problem {
            line,
            message: message.into(),
        }
    }
}

enum Item<'a> {
    Blank,
    Text(&'a Line<'a>),
}

fn trim(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

fn classify(body: &str) -> (Class, &str, &str) {
    if body.is_empty() {
        return (Class::Blank, "", "");
    }
    if body.starts_with('#') {
        return (Class::Comment, body, "");
    }
    if body == "}" {
        return (Class::Close, "", "");
    }
    if let Some(name) = body.strip_suffix('{') {
        return (Class::Open, trim(name), "");
    }
    // A trailing `# ...` belongs to the value: `config/hosts.dotfile` holds
    // part numbers with a `#` in them, and there is no way to tell the two
    // apart from here.
    if let Some((key, value)) = body.split_once('=') {
        return (Class::Entry, trim(key), trim(value));
    }
    (Class::Bare, body, "")
}

fn arrange<'a>(parsed: &'a [Line<'a>], config: &Config) -> Vec<Item<'a>> {
    let mut out = Vec::new();
    let mut pending = 0;
    for line in parsed {
        if line.class == Class::Blank {
            // A blank with nothing above it separates nothing, so it is only
            // counted once something has been written.
            if !out.is_empty() {
                pending += 1;
            }
            continue;
        }
        // The gap above a `}` is the gap at the end of a block, which is not a
        // gap between two things.
        if line.class != Class::Close {
            for _ in 0..pending.min(config.blank_lines) {
                out.push(Item::Blank);
            }
        }
        pending = 0;
        out.push(Item::Text(line));
    }
    out
}

fn columns(arranged: &[Item], config: &Config) -> Vec<Option<usize>> {
    let mut widths = vec![None; arranged.len()];
    if !config.align {
        return widths;
    }
    let mut start = 0;
    while start < arranged.len() {
        if !groupable(&arranged[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < arranged.len() && groupable(&arranged[end]) {
            end += 1;
        }
        // Only entries have a key, so only entries set the width — a group of
        // bare lines is written out as it stands.
        let width = arranged[start..end]
            .iter()
            .filter_map(|item| match item {
                Item::Text(line) if line.class == Class::Entry => {
                    Some(line.key.chars().count().min(config.align_max))
                }
                _ => None,
            })
            .max();
        widths[start..end].fill(width);
        start = end;
    }
    widths
}

fn groupable(item: &Item) -> bool {
    match item {
        Item::Blank => false,
        Item::Text(line) => {
            line.depth > 0 && matches!(line.class, Class::Entry | Class::Bare | Class::Comment)
        }
    }
}

fn render(line: &Line, width: Option<usize>, config: &Config) -> String {
    let indent = if line.depth == 0 {
        String::new()
    } else {
        " ".repeat(config.indent * line.depth)
    };
    match line.class {
        // Always the spaced form. `blocks.py` is read with `open_suffix="{"`
        // everywhere except `packages.py`, which uses `" {"`, and `name {` is
        // the only spelling both readers parse the same way.
        Class::Open if line.key.is_empty() => with_comment(format!("{indent}{{"), line.value),
        Class::Open => with_comment(format!("{indent}{} {{", line.key), line.value),
        Class::Close => with_comment(format!("{indent}}}"), line.value),
        Class::Entry => {
            // The `=` sits two columns past the widest key in the group, and a
            // key at or past the cap takes its one space and overflows rather
            // than dragging the column out after it.
            let pad = match width {
                Some(width) => (width + 2).saturating_sub(line.key.chars().count()).max(1),
                None => 1,
            };
            let mut text = format!("{indent}{}{}=", line.key, " ".repeat(pad));
            if !line.value.is_empty() {
                text.push(' ');
                text.push_str(line.value);
            }
            text
        }
        // Interior whitespace is never edited: `config/hosts.dotfile` holds
        // values like `32 GB (2×16 GB) DDR5-6000 CL30`, and collapsing the
        // runs in one would be editing data rather than laying it out.
        _ => format!("{indent}{}", line.body),
    }
}

fn with_comment(mut text: String, comment: &str) -> String {
    if !comment.is_empty() {
        text.push(' ');
        text.push_str(comment);
    }
    text
}
