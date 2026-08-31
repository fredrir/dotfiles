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
    let mut open: Option<&str> = None;
    let mut number = 0;
    for raw in lines(text) {
        number += 1;
        let body = trim(raw);
        let (class, key, value) = classify(body);
        // The block name is tracked separately from whether a block is open,
        // so `{` on a line of its own nests and closes like any other block
        // rather than being read as top level for having an empty name.
        let (block, depth) = match class {
            Class::Open => {
                if open.is_some() {
                    return Err(Problem::at(number, "nested block"));
                }
                open = Some(key);
                (key, 0)
            }
            Class::Close => {
                let Some(name) = open.take() else {
                    return Err(Problem::at(number, "unexpected }"));
                };
                (name, 0)
            }
            _ => (open.unwrap_or(""), usize::from(open.is_some())),
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
    }
    if let Some(name) = open {
        return Err(Problem::at(number, format!("missing }} for {name}")));
    }
    Ok(found)
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
        " ".repeat(config.indent)
    };
    match line.class {
        // Always the spaced form. `blocks.py` is read with `open_suffix="{"`
        // everywhere except `packages.py`, which uses `" {"`, and `name {` is
        // the only spelling both readers parse the same way.
        Class::Open => format!("{} {{", line.key),
        Class::Close => "}".to_string(),
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
