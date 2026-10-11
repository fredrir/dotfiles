//! The state each shell reached, as `assets/verify.zsh` dumps it, and where
//! the bundle's differs from the sources' beyond what the build changes on
//! purpose.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use similar::{Algorithm, DiffTag, capture_diff_slices};

use crate::compile::eval_statement;
use crate::native;
use crate::scan::{self, Substitution};

/// Parameters whose value is new in each shell.
const SESSION: &[&str] = &["ATUIN_SESSION"];

/// Table, then name, to value.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Dump(pub BTreeMap<String, BTreeMap<String, String>>);

impl Dump {
    /// NUL-separated table, name and value records, up to an `end` record.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut fields = bytes
            .split(|byte| *byte == 0)
            .map(|field| String::from_utf8_lossy(field).into_owned());
        let mut tables: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        loop {
            let (Some(table), Some(name), Some(value)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err("dump incomplete".to_string());
            };
            if table == "end" {
                return Ok(Self(tables));
            }
            tables.entry(table).or_default().insert(name, value);
        }
    }

    pub fn function(&self, name: &str) -> Option<&String> {
        self.0.get("functions")?.get(name)
    }
}

/// What the build adds to the bundle's state on purpose.
#[derive(Debug, Default)]
pub struct Expected {
    /// Directories of completion scripts the bundle puts first on `fpath`.
    pub completions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub table: String,
    pub name: String,
    pub bundle: Option<String>,
    pub sources: Option<String>,
}

/// Characters kept ahead of where two excerpts part.
const CONTEXT: usize = 20;

impl Difference {
    /// The first line where the values part, from shortly before the first
    /// character that differs, each cut to `width` characters.
    pub fn excerpt(&self, width: usize) -> (String, String) {
        let (Some(bundle), Some(sources)) = (&self.bundle, &self.sources) else {
            let side = |value: &Option<String>| {
                value.as_deref().map_or("(none)".to_string(), |value| {
                    cut(&visible(value.lines().next().unwrap_or("").trim()), width)
                })
            };
            return (side(&self.bundle), side(&self.sources));
        };
        let (bundle, sources): (Vec<&str>, Vec<&str>) =
            (bundle.lines().collect(), sources.lines().collect());
        let at = (0..bundle.len().max(sources.len()))
            .find(|index| bundle.get(*index) != sources.get(*index))
            .unwrap_or(0);
        let (raw_left, raw_right) = (
            bundle.get(at).copied().unwrap_or("(end)"),
            sources.get(at).copied().unwrap_or("(end)"),
        );
        let (mut left, mut right) = (raw_left.trim(), raw_right.trim());
        if left == right {
            (left, right) = (raw_left, raw_right);
        }
        let (left, right) = (visible(left), visible(right));
        let common = left
            .chars()
            .zip(right.chars())
            .take_while(|(left, right)| left == right)
            .count();
        let skip = common.saturating_sub(CONTEXT);
        let from = |text: &str| match skip {
            0 => cut(text, width),
            _ => format!(
                "...{}",
                cut(&text.chars().skip(skip).collect::<String>(), width)
            ),
        };
        (from(&left), from(&right))
    }
}

/// Control characters but tabs as escapes, so they print as text.
pub fn visible(text: &str) -> String {
    text.chars()
        .flat_map(|character| match character {
            '\t' => vec![character],
            character if character.is_control() => character.escape_default().collect(),
            character => vec![character],
        })
        .collect()
}

fn cut(text: &str, width: usize) -> String {
    match text.char_indices().nth(width) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_string(),
    }
}

/// Entries the two dumps disagree on, by table and name.
pub fn differences(bundle: &Dump, sources: &Dump, expected: &Expected) -> Vec<Difference> {
    let bundle = normalize(bundle, Some(expected));
    let sources = normalize(sources, None);
    let empty = BTreeMap::new();
    let tables: BTreeSet<&String> = bundle.0.keys().chain(sources.0.keys()).collect();
    let mut found = Vec::new();
    for table in tables {
        let (left, right) = (
            bundle.0.get(table).unwrap_or(&empty),
            sources.0.get(table).unwrap_or(&empty),
        );
        let names: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
        for name in names {
            let (left, right) = (left.get(name), right.get(name));
            if !same(table, name, left, right) {
                found.push(Difference {
                    table: table.clone(),
                    name: name.clone(),
                    bundle: left.cloned(),
                    sources: right.cloned(),
                });
            }
        }
    }
    found
}

/// The dump without what only one side has by design; `expected` for the bundle's.
fn normalize(dump: &Dump, expected: Option<&Expected>) -> Dump {
    let mut tables = BTreeMap::new();
    for (table, entries) in &dump.0 {
        let mut kept = BTreeMap::new();
        for (name, value) in entries.iter().filter(|(name, _)| !own(name)) {
            let value = match (table.as_str(), expected) {
                // The bundle's `type` and `whence` that report where functions came from
                ("functions", Some(_))
                    if matches!(name.as_str(), "type" | "whence")
                        && value.contains("_zsh_build_whence") =>
                {
                    continue;
                }
                // The switch that makes `~/.zshrc` load the sources
                ("parameters" | "types", None) if name == "ZSH_BUILD_SKIP" => continue,
                // The bundle's completion scripts come first on `fpath`
                ("parameters", Some(expected)) if name == "fpath" || name == "FPATH" => {
                    let separator = if name == "fpath" { '\n' } else { ':' };
                    value
                        .split(separator)
                        .filter(|dir| !expected.completions.iter().any(|kept| kept == dir))
                        .collect::<Vec<_>>()
                        .join(&separator.to_string())
                }
                // Hook indices follow what was added and removed before; the order counts
                ("zstyles", _) if name.starts_with("zle-") && name.ends_with(" widgets") => {
                    hook_order(value)
                }
                _ => value.clone(),
            };
            kept.insert(name.clone(), value);
        }
        tables.insert(table.clone(), kept);
    }
    Dump(tables)
}

/// zsh-build's helpers, folded values and saved `$0`s, and the verify hook;
/// leftover deferred units are not its own.
fn own(name: &str) -> bool {
    name.starts_with("_zsh_build_")
        || name.starts_with("__zbv_")
        || name == "__zb_head"
        || name.strip_prefix("__zb").is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'_')
        })
}

fn hook_order(value: &str) -> String {
    let mut hooks: Vec<(u64, &str)> = value
        .lines()
        .map(|hook| {
            hook.split_once(':')
                .and_then(|(index, widget)| Some((index.parse().ok()?, widget)))
                .unwrap_or((u64::MAX, hook))
        })
        .collect();
    hooks.sort_by_key(|(index, _)| *index);
    hooks
        .into_iter()
        .map(|(_, widget)| widget)
        .collect::<Vec<_>>()
        .join("\n")
}

fn same(table: &str, name: &str, bundle: Option<&String>, sources: Option<&String>) -> bool {
    match (bundle, sources) {
        (Some(bundle), Some(sources)) if bundle == sources => true,
        // A missing parameter shows under parameters
        (None, _) | (_, None) if table == "types" => true,
        // Present in both, new in each shell
        (Some(_), Some(_)) if table == "parameters" && SESSION.contains(&name) => true,
        (Some(bundle), None) if queue_residue(table, name, bundle) => true,
        // Deferred compinit runs after `zsh/complist` loads, so it wraps `menu-select` too
        _ if complist_wrapped(table, name, bundle, sources) => true,
        (Some(bundle), Some(sources)) if table == "functions" => rewritten(bundle, sources),
        _ => false,
    }
}

/// What `_defer_schedule` leaves once the queue drained: `add-zle-hook-widget`,
/// its dispatchers and the queue's widgets, only in the bundle when the sources
/// queue nothing.
fn queue_residue(table: &str, name: &str, value: &str) -> bool {
    match table {
        "functions" => name == "add-zle-hook-widget" || name.starts_with("azhw:"),
        "widgets" => {
            matches!(name, "_defer_drain" | "_defer_typeahead")
                || (name.starts_with("zle-") && value.starts_with("user:azhw:"))
        }
        "zstyles" => name == "zle-hook types",
        _ => false,
    }
}

/// `menu-select`, or zsh-autosuggestions' copy of it, as the completion widget
/// compinit makes in the bundle and as complist's builtin in the sources.
fn complist_wrapped(
    table: &str,
    name: &str,
    bundle: Option<&String>,
    sources: Option<&String>,
) -> bool {
    let copy = name
        .strip_prefix("autosuggest-orig-")
        .and_then(|rest| rest.strip_suffix("-menu-select"))
        .is_some_and(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()));
    match (
        table,
        bundle.map(String::as_str),
        sources.map(String::as_str),
    ) {
        ("widgets", Some("completion:.menu-select:_main_complete"), Some(sources)) => {
            (name == "menu-select" && sources == "builtin")
                || (copy && sources == "user:_zsh_autosuggest_orig_menu-select")
        }
        ("functions", None, Some("\tzle .menu-select")) => {
            name == "_zsh_autosuggest_orig_menu-select"
        }
        _ => false,
    }
}

/// Whether a function body from the bundle is the sources' as the build
/// rewrites it: native expansions, folded `$(cmd)` and `eval "$(cmd)"`
/// inlined as a block. Both are zsh's own listing, so layout and quoting
/// alike; what zsh read as aliases in the sources stays a difference.
fn rewritten(bundle: &str, sources: &str) -> bool {
    let (bundle, sources) = (unquoted(&unwrap_natives(bundle)), unquoted(sources));
    let sources = sources.as_str();
    let substitutions = scan::scan(sources).substitutions;
    let old: Vec<&str> = lines(&bundle).iter().map(|line| line.text).collect();
    let new: Vec<Line> = lines(sources);
    let texts: Vec<&str> = new.iter().map(|line| line.text).collect();
    hunks(&old, &texts)
        .into_iter()
        .all(|(old_range, new_range)| {
            aligned(&old[old_range], &new[new_range], sources, &substitutions)
        })
}

/// A line of the sources' body and where it starts.
struct Line<'a> {
    text: &'a str,
    start: usize,
}

fn lines(text: &str) -> Vec<Line<'_>> {
    let mut start = 0;
    text.split('\n')
        .map(|line| {
            let line = Line { text: line, start };
            start += line.text.len() + 1;
            line
        })
        .filter(|line| line.start < text.len())
        .collect()
}

/// Line ranges where the two bodies differ, adjacent changes merged.
fn hunks(old: &[&str], new: &[&str]) -> Vec<(Range<usize>, Range<usize>)> {
    let mut hunks: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, old, new) {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        match hunks.last_mut() {
            Some((last_old, last_new))
                if last_old.end == old_range.start && last_new.end == new_range.start =>
            {
                last_old.end = old_range.end;
                last_new.end = new_range.end;
            }
            _ => hunks.push((old_range, new_range)),
        }
    }
    hunks
}

fn aligned(old: &[&str], new: &[Line], text: &str, substitutions: &[Substitution]) -> bool {
    let (mut at_old, mut at_new) = (0, 0);
    while at_old < old.len() && at_new < new.len() {
        let line = &new[at_new];
        if old[at_old] == line.text || folded(old[at_old], line, substitutions) {
            at_old += 1;
        } else if let Some(taken) = inlined(&old[at_old..], line, text, substitutions) {
            at_old += taken;
        } else {
            return false;
        }
        at_new += 1;
    }
    at_old == old.len() && at_new == new.len()
}

/// Whether `line` is `old` with a command substitution at each `${__zbN}`.
fn folded(old: &str, line: &Line, substitutions: &[Substitution]) -> bool {
    let pieces = constant_pieces(old);
    if pieces.len() < 2 {
        return false;
    }
    let mut at = 0;
    for (index, piece) in pieces.iter().enumerate() {
        if !line.text[at..].starts_with(piece) {
            return false;
        }
        at += piece.len();
        if index + 1 == pieces.len() {
            return at == line.text.len();
        }
        let Some(end) = substitution_at(substitutions, line.start + at) else {
            return false;
        };
        at = end - line.start;
        if at > line.text.len() {
            return false;
        }
    }
    false
}

/// The text around each `${__zbN}` in `line`.
fn constant_pieces(line: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut search = 0;
    while let Some(found) = line[search..].find("${__zb") {
        let open = search + found;
        let digits = line[open + 6..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let close = open + 6 + digits;
        if digits > 0 && line[close..].starts_with('}') {
            pieces.push(&line[start..open]);
            start = close + 1;
        }
        search = close.max(open + 1);
    }
    pieces.push(&line[start..]);
    pieces
}

/// The end of the outermost command substitution starting at `start`.
fn substitution_at(substitutions: &[Substitution], start: usize) -> Option<usize> {
    substitutions
        .iter()
        .filter(|substitution| substitution.range.start == start)
        .map(|substitution| substitution.range.end)
        .max()
}

/// Lines of the `{ }` block opening `old` that stands for the `eval "$(cmd)"`
/// in `line`: it opens where the `eval` stood and closes at the same indent,
/// ahead of what followed it.
fn inlined(old: &[&str], line: &Line, text: &str, substitutions: &[Substitution]) -> Option<usize> {
    let end = line.start + line.text.len();
    substitutions
        .iter()
        .filter(|substitution| {
            line.start <= substitution.range.start && substitution.range.end <= end
        })
        .filter_map(|substitution| eval_statement(text, substitution))
        .find_map(|statement| {
            let head = &text[line.start..statement.start];
            let tail = &text[statement.end..end];
            let indent = &head[..head.len() - head.trim_start().len()];
            let (open, close) = (format!("{head}{{"), format!("{indent}}}{tail}"));
            (old.first().copied() == Some(open.as_str())).then_some(())?;
            let last = old
                .iter()
                .skip(1)
                .position(|candidate| *candidate == close)?;
            Some(last + 2)
        })
}

/// `body` without a backslash before a plain character starting a word, which
/// the build adds so a bundle read as text expands no alias twice; a parsed
/// body runs the same either way.
fn unquoted(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    for (index, character) in body.char_indices() {
        let starts = index == 0
            || matches!(
                bytes[index - 1],
                b' ' | b'\t' | b'\n' | b';' | b'|' | b'&' | b'(' | b'{'
            );
        let plain = bytes
            .get(index + 1)
            .is_some_and(|next| next.is_ascii_alphanumeric() || b"_-.+@:/%,".contains(next));
        if !(character == '\\' && starts && plain) {
            out.push(character);
        }
    }
    out
}

/// `body` with each native expansion back to the substitution it stands for.
fn unwrap_natives(body: &str) -> String {
    let mut body = body.to_string();
    for substitute in native::substitutes() {
        let wrapped = (substitute.expansion)("\0");
        let Some((head, tail)) = wrapped.split_once('\0') else {
            continue;
        };
        let mut search = 0;
        while let Some(found) = body[search..].find(head) {
            let start = search + found;
            let inner = start + head.len();
            let end = substitution_at(&scan::scan(&body).substitutions, inner)
                .filter(|end| body[*end..].starts_with(tail));
            match end {
                Some(end) => {
                    body.replace_range(end..end + tail.len(), "");
                    body.replace_range(start..inner, "");
                    search = start;
                }
                None => search = inner,
            }
        }
    }
    body
}

#[cfg(test)]
#[path = "../../tests/unit/verify/compare_tests.rs"]
mod tests;
