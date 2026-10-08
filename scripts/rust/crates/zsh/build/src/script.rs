//! Parsed source text, and the line ranges each statement owns.
//!
//! The parser records the starting line of every pipeline but no byte spans,
//! so a statement owns the lines that, parsed alone, reproduce exactly that
//! statement. Edits are only ever applied to owned lines.

use std::cell::Cell;
use std::ops::RangeInclusive;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

use serde_json::Value;
use zshrs_parse::lexer::untokenize_preserve_quotes;
use zshrs_parse::parser::{ZshAssignValue, ZshCommand, ZshList, ZshParser, ZshProgram, ZshSimple};

pub struct Script {
    pub text: String,
    line_starts: Vec<usize>,
    pub program: ZshProgram,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub lists: std::ops::Range<usize>,
    pub lines: RangeInclusive<usize>,
}

impl Script {
    pub fn parse(text: String) -> Result<Self, String> {
        let program = parse(&text)?;
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index + 1),
        );
        if line_starts.last() == Some(&text.len()) && !text.is_empty() {
            line_starts.pop();
        }
        Ok(Self {
            text,
            line_starts,
            program,
        })
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte range of 1-based, inclusive lines, including the final newline.
    pub fn byte_range(&self, lines: &RangeInclusive<usize>) -> std::ops::Range<usize> {
        let start = self.line_starts[lines.start() - 1];
        let end = self
            .line_starts
            .get(*lines.end())
            .copied()
            .unwrap_or(self.text.len());
        start..end
    }

    pub fn lines(&self, lines: &RangeInclusive<usize>) -> &str {
        &self.text[self.byte_range(lines)]
    }

    /// Owned line ranges for `lists`, which end by line `last`; statements
    /// sharing lines form one segment.
    pub fn segments(&self, lists: &[ZshList], last: usize) -> Vec<Option<Segment>> {
        let starts: Vec<usize> = lists.iter().map(start_line).collect();
        let mut segments = Vec::new();
        let mut index = 0;
        while index < lists.len() {
            let first = next_group(&starts, index);
            let mut group = first;
            let mut found = None;
            for _ in 0..MAX_JOINED {
                let lines = if group < lists.len() {
                    Some(starts[index]..=starts[group].saturating_sub(1).max(starts[index]))
                        .filter(|lines| self.reproduces(lines, &lists[index..group]))
                } else {
                    (starts[index]..=last)
                        .map(|end| starts[index]..=end)
                        .find(|lines| self.reproduces(lines, &lists[index..group]))
                };
                if lines.is_some() || group >= lists.len() {
                    found = lines;
                    break;
                }
                group = next_group(&starts, group);
            }
            match found {
                Some(lines) => {
                    segments.push(Some(Segment {
                        lists: index..group,
                        lines,
                    }));
                    index = group;
                }
                None => {
                    segments.extend((index..first).map(|_| None));
                    index = first;
                }
            }
        }
        segments
    }

    fn reproduces(&self, lines: &RangeInclusive<usize>, lists: &[ZshList]) -> bool {
        if *lines.end() > self.line_count() || lines.start() > lines.end() {
            return false;
        }
        match parse(self.lines(lines)) {
            Ok(program) => same_lists(&program.lists, lists),
            Err(_) => false,
        }
    }
}

fn next_group(starts: &[usize], index: usize) -> usize {
    let mut next = index + 1;
    while next < starts.len() && starts[next] == starts[index] {
        next += 1;
    }
    next
}

pub fn start_line(list: &ZshList) -> usize {
    list.sublist.pipe.lineno.max(1) as usize
}

/// The parser names anonymous functions from a process-wide counter.
pub const ANONYMOUS: &str = "_zshrs_anon_";

/// Statements that may share lines with the one before them.
const MAX_JOINED: usize = 4;

thread_local! {
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// Parses complete input only. The parser accepts unterminated constructs,
/// so a sentinel command must still parse as its own final statement.
pub fn parse(text: &str) -> Result<ZshProgram, String> {
    let mut program = parse_raw(&format!("{text}\n:\n"))?;
    let sentinel = program.lists.pop().is_some_and(|last| {
        !last.flags.async_
            && last.sublist.next.is_none()
            && last.sublist.pipe.next.is_none()
            && matches!(&last.sublist.pipe.cmd, ZshCommand::Simple(simple)
                if simple.assigns.is_empty() && simple.redirs.is_empty()
                    && simple.words.len() == 1 && untokenize_preserve_quotes(&simple.words[0]) == ":")
    });
    if sentinel {
        Ok(program)
    } else {
        Err("incomplete input".to_string())
    }
}

/// Parses with the parser's panics on malformed input turned into errors.
fn parse_raw(text: &str) -> Result<ZshProgram, String> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !QUIET.with(Cell::get) {
                previous(info);
            }
        }));
    });
    QUIET.with(|quiet| quiet.set(true));
    let result = panic::catch_unwind(AssertUnwindSafe(|| ZshParser::new(text).parse()));
    QUIET.with(|quiet| quiet.set(false));
    match result {
        Ok(Ok(program)) => Ok(program),
        Ok(Err(errors)) => Err(errors
            .first()
            .map(ToString::to_string)
            .unwrap_or_else(|| "parse error".to_string())),
        Err(_) => Err("parser failure".to_string()),
    }
}

/// FNV-1a: stable across builds and platforms, unlike `std`'s hasher.
pub fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

pub fn same_lists(left: &[ZshList], right: &[ZshList]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| structure(left) == structure(right))
}

/// The list without source positions.
pub fn structure(list: &ZshList) -> Value {
    let mut value = serde_json::to_value(list).unwrap_or(Value::Null);
    strip(&mut value, false);
    value
}

/// The list without positions or words: what an in-place word rewrite keeps.
pub fn shape(list: &ZshList) -> Value {
    let mut value = serde_json::to_value(list).unwrap_or(Value::Null);
    strip(&mut value, true);
    value
}

fn strip(value: &mut Value, text: bool) {
    match value {
        Value::Object(map) => {
            map.remove("lineno");
            if text {
                map.remove("body_source");
                map.remove("words");
            }
            map.values_mut().for_each(|value| strip(value, text));
        }
        Value::Array(items) => items.iter_mut().for_each(|value| strip(value, text)),
        Value::String(string) if text => string.clear(),
        Value::String(string) if string.starts_with(ANONYMOUS) => *string = ANONYMOUS.to_string(),
        _ => {}
    }
}

/// Source text for a simple command without redirections.
pub fn reprint(simple: &ZshSimple) -> Option<String> {
    if !simple.redirs.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for assign in &simple.assigns {
        let operator = if assign.append { "+=" } else { "=" };
        let value = match &assign.value {
            ZshAssignValue::Scalar(value) => untokenize_preserve_quotes(value),
            ZshAssignValue::Array(values) => format!(
                "({})",
                values
                    .iter()
                    .map(|value| untokenize_preserve_quotes(value))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        };
        parts.push(format!("{}{operator}{value}", assign.name));
    }
    parts.extend(
        simple
            .words
            .iter()
            .map(|word| untokenize_preserve_quotes(word)),
    );
    let text = parts.join(" ");
    let program = parse(&text).ok()?;
    let [list] = program.lists.as_slice() else {
        return None;
    };
    let original = serde_json::to_value(simple).ok()?;
    let reparsed = match &list.sublist.pipe.cmd {
        ZshCommand::Simple(simple) => serde_json::to_value(simple).ok()?,
        _ => return None,
    };
    (original == reparsed).then_some(text)
}

#[cfg(test)]
#[path = "../tests/unit/script_tests.rs"]
mod tests;
