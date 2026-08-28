//! The `.dotfile` grammar, and the one place its layout is decided.
//!
//! The grammar is `blocks.py`'s, read here rather than reimplemented: a line
//! is classified by its trimmed body alone, there is exactly one level of
//! nesting, and the three structural failures keep `blocks.py`'s wording so a
//! stray brace reads the same whichever tool found it.
//!
//! Three things are read differently on purpose, and each is pinned by a test
//! so nobody later "fixes" it back:
//!
//! - **A comment is kept.** `blocks.scan` drops comments because its callers
//!   want values; a formatter that dropped them would delete the header of
//!   every file in `config/`.
//! - **An entry at top level is legal.** `blocks.scan` raises `OUTSIDE`, but
//!   `config/targets.dotfile` is nothing but top-level entries.
//! - **A missing file is a failure.** `blocks.read` returns `[]` for a file it
//!   cannot open, which for a reader is a sane default and for a formatter
//!   would mean silently formatting nothing.
//!
//! Alignment is the reason this module exists. Three Python generators used to
//! each have their own idea of the `=` column; they now pipe through this one.

use crate::conf::lines;
use crate::config::Config;

/// What a line is, decided by its trimmed body and nothing else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Blank,
    Comment,
    Open,
    Close,
    Entry,
    Bare,
}

/// One classified line, borrowing the text it came from.
pub struct Line<'a> {
    pub class: Class,
    /// Which line of the file this was, counting from one.
    pub number: usize,
    /// The body: one trailing `\r` off, then `[ \t]` off both ends.
    pub body: &'a str,
    /// The block this line belongs to. `Open` and `Close` carry the block they
    /// delimit; a line at top level carries the empty string.
    pub block: &'a str,
    /// How deep the line sits: `0` at top level and on the braces themselves,
    /// `1` for the lines a block holds.
    pub depth: usize,
    /// `Open`: the block's name. `Entry`: the text before the first `=`.
    /// `Bare` and `Comment`: the whole body. `Close`: empty.
    pub key: &'a str,
    /// `Entry`: the text after the first `=`, trimmed. Everything else: empty.
    pub value: &'a str,
}

impl Line<'_> {
    /// What the round-trip guard compares: everything about a line except
    /// where it sits and how it is spaced.
    fn signature(&self) -> (Class, &str, &str, &str) {
        (self.class, self.block, self.key, self.value)
    }
}

/// A structural failure, at the line that caused it.
#[derive(Debug)]
pub struct Problem {
    pub line: usize,
    pub message: String,
}

/// Classify every line and check the structure around them.
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

/// Lay out a whole file.
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

/// The sequence the round-trip guard compares, with the blank lines left out
/// because collapsing a run of them is the whole point of formatting.
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

/// What the file comes to once the blank lines are settled: either a gap or a
/// line to write.
enum Item<'a> {
    Blank,
    Text(&'a Line<'a>),
}

fn trim(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

/// The classification, in the order the grammar tests it. `Open` is decided
/// before `Entry`, so a block header holding an `=` is still a block header.
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

/// Settle the blank lines: the ones above the first real line go, a run
/// collapses to `blank_lines`, the one just inside a closing brace goes, and
/// the ones at the end go.
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

/// The `=` column for each line, or `None` where there is nothing to align to.
///
/// A group is a maximal run of entry, bare and comment lines inside one block.
/// A blank line resets it and so does a brace, but a comment does not: a
/// comment above three entries is a label for them, and splitting the group
/// there would step the column in the middle of a list.
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

/// Whether a line can sit in an alignment group.
///
/// Top-level entries deliberately cannot. `add.py:targets_has_line` tests
/// `config/targets.dotfile` for the exact string `src = dst`; pad that line
/// and `dotfile add` appends a duplicate mapping every single time.
fn groupable(item: &Item) -> bool {
    match item {
        Item::Blank => false,
        Item::Text(line) => {
            line.depth > 0 && matches!(line.class, Class::Entry | Class::Bare | Class::Comment)
        }
    }
}

/// One line, written out.
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
