//! Parsed source text, and the line ranges each statement owns.
//!
//! The parser records the starting line of every pipeline but no byte spans,
//! so a statement owns the lines that, parsed alone, reproduce exactly that
//! statement. Edits are only ever applied to owned lines.

use std::borrow::Cow;
use std::cell::Cell;
use std::ops::RangeInclusive;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

use serde_json::Value;
use zshrs_parse::lexer::untokenize_preserve_quotes;
use zshrs_parse::parser::{ZshAssignValue, ZshCommand, ZshList, ZshParser, ZshProgram, ZshSimple};

use crate::scan::scan;

pub struct Script {
    pub text: String,
    line_starts: Vec<usize>,
    pub program: ZshProgram,
    reading: Reading,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub lists: std::ops::Range<usize>,
    pub lines: RangeInclusive<usize>,
}

impl Script {
    pub fn parse(text: String) -> Result<Self, String> {
        let (program, reading) = parse_either(&text)?;
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
            reading,
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
        parse_as(self.lines(lines), self.reading)
            .is_ok_and(|program| same_lists(&program.lists, lists))
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

/// Parses complete input only, as written or else read around the parser's misreadings.
pub fn parse(text: &str) -> Result<ZshProgram, String> {
    parse_either(text).map(|(program, _)| program)
}

/// What the parser needed to read a text as zsh does.
#[derive(Clone, Copy, Default)]
struct Reading {
    /// The parser lexes a line inside `name=(...)` from command position, so
    /// a reserved word or `name=` there ends the array; line continuations don't.
    joined: bool,
    /// The parser reads `else {` as the brace form, which zsh only takes
    /// after `if ... {`; a command marked in between keeps `{` in the body.
    else_blocks: bool,
}

const READINGS: [Reading; 3] = [
    Reading {
        joined: true,
        else_blocks: false,
    },
    Reading {
        joined: false,
        else_blocks: true,
    },
    Reading {
        joined: true,
        else_blocks: true,
    },
];

/// Stands between `else` and a `{` block while the parser reads them.
const ELSE_MARK: &str = "__zb_else";

fn parse_either(text: &str) -> Result<(ZshProgram, Reading), String> {
    let error = match parse_complete(text) {
        Ok(program) => return Ok((program, Reading::default())),
        Err(error) => error,
    };
    READINGS
        .into_iter()
        .find_map(|reading| {
            parse_as(text, reading)
                .ok()
                .map(|program| (program, reading))
        })
        .ok_or(error)
}

fn parse_as(text: &str, reading: Reading) -> Result<ZshProgram, String> {
    let text = if reading.joined {
        join_array_lines(text)
    } else {
        Cow::Borrowed(text)
    };
    match parse_complete(&text) {
        Err(error) if reading.else_blocks => parse_else_blocks(&text).ok_or(error),
        parsed => parsed,
    }
}

/// Ambiguous `else`s past which only marking all or none of them is tried.
const MAX_UNSURE: usize = 8;

/// Where an `else` in reserved-word position stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ElseAfter {
    /// First on its line or after `;`, where zsh reads only the then form.
    Separator,
    /// Right after `}` on its line: the brace form, or a then body ending in a block.
    Brace,
}

/// `text` parsed with `ELSE_MARK` after each then-form `else` a `{` block
/// follows, then without the marks; an `else` right after `}` is marked as
/// its `if` reads, then as the parser needs, fewest marks first.
fn parse_else_blocks(text: &str) -> Option<ZshProgram> {
    let bytes = text.as_bytes();
    let reserved: Vec<(&str, usize)> = scan(text)
        .words
        .into_iter()
        .map(|word| (&text[word.range.clone()], word.range))
        .filter(|(word, range)| {
            matches!(*word, "if" | "then" | "elif" | "else" | "fi")
                && command_position(bytes, range.start)
        })
        .map(|(word, range)| (word, range.end))
        .collect();
    let mut sure = Vec::new();
    let mut unsure = Vec::new();
    let mut guess = Vec::new();
    for (index, &(word, end)) in reserved.iter().enumerate() {
        if word != "else" || !opens_block(bytes, end) {
            continue;
        }
        match else_after(bytes, end - word.len()) {
            Some(ElseAfter::Separator) => sure.push(end),
            Some(ElseAfter::Brace) => {
                unsure.push(end);
                guess.push(takes_then(&reserved[..index]));
            }
            None => {}
        }
    }
    let rest = mark_choices(unsure.len())
        .into_iter()
        .filter(|chosen| *chosen != guess);
    std::iter::once(guess.clone())
        .chain(rest)
        .find_map(|chosen| {
            let mut ends: Vec<usize> = unsure
                .iter()
                .zip(chosen)
                .filter_map(|(end, chosen)| chosen.then_some(*end))
                .chain(sure.iter().copied())
                .collect();
            ends.sort_unstable();
            parse_marked(text, &ends)
        })
}

/// `text` parsed with `ELSE_MARK` after each of `ends`, then without the marks.
fn parse_marked(text: &str, ends: &[usize]) -> Option<ZshProgram> {
    if ends.is_empty() {
        return None;
    }
    let mark = format!(" {ELSE_MARK};");
    let mut marked = String::with_capacity(text.len() + ends.len() * mark.len());
    let mut position = 0;
    for &end in ends {
        marked.push_str(&text[position..end]);
        marked.push_str(&mark);
        position = end;
    }
    marked.push_str(&text[position..]);
    let program = parse_complete(&marked).ok()?;
    unmark(program, &mark, ends.len())
}

/// The `else` at `start` by what precedes it on its line; `None` when it is a word
/// after others, not the reserved word.
fn else_after(bytes: &[u8], start: usize) -> Option<ElseAfter> {
    match bytes[..start]
        .iter()
        .rev()
        .find(|byte| !matches!(byte, b' ' | b'\t'))
    {
        None | Some(b'\n' | b';') => Some(ElseAfter::Separator),
        Some(b'}') => Some(ElseAfter::Brace),
        Some(_) => None,
    }
}

/// Whether a word at `start` may be a reserved word: first on its line, after
/// an operator or brace, or after a reserved word that a command follows.
fn command_position(bytes: &[u8], start: usize) -> bool {
    let before = bytes[..start]
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'));
    let Some(last) = before else {
        return true;
    };
    if matches!(
        bytes[last],
        b'\n' | b';' | b'&' | b'|' | b'!' | b'{' | b'}' | b'(' | b')'
    ) {
        return true;
    }
    let word_start = bytes[..=last]
        .iter()
        .rposition(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
        .map_or(0, |at| at + 1);
    matches!(
        &bytes[word_start..=last],
        b"if" | b"then" | b"elif" | b"else" | b"do" | b"while" | b"until"
    )
}

/// Whether the `if` an `else` after `reserved` belongs to took `then`: a
/// `then` before that `if`, walking back past whole nested `if ... fi`s.
fn takes_then(reserved: &[(&str, usize)]) -> bool {
    let mut depth = 0usize;
    for &(word, _) in reserved.iter().rev() {
        match word {
            "fi" => depth += 1,
            "if" if depth == 0 => return false,
            "if" => depth -= 1,
            "then" if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

/// Which of `count` ambiguous `else`s to mark, per attempt: fewest marks first,
/// or past `MAX_UNSURE` none and then all.
fn mark_choices(count: usize) -> Vec<Vec<bool>> {
    if count > MAX_UNSURE {
        return vec![vec![false; count], vec![true; count]];
    }
    let mut masks: Vec<u32> = (0..1u32 << count).collect();
    masks.sort_by_key(|mask| mask.count_ones());
    masks
        .into_iter()
        .map(|mask| (0..count).map(|index| mask & (1 << index) != 0).collect())
        .collect()
}

/// Whether a `{` token follows `end` past blanks, separators and comments.
fn opens_block(bytes: &[u8], end: usize) -> bool {
    let mut position = end;
    while let Some(byte) = bytes.get(position) {
        match byte {
            b' ' | b'\t' | b'\n' | b';' => position += 1,
            b'#' => {
                while bytes.get(position).is_some_and(|byte| *byte != b'\n') {
                    position += 1;
                }
            }
            b'{' => {
                return bytes
                    .get(position + 1)
                    .is_none_or(|next| matches!(next, b' ' | b'\t' | b'\n'));
            }
            _ => return false,
        }
    }
    false
}

/// `program` without its `marks` mark commands, each the first of a body.
fn unmark(program: ZshProgram, mark: &str, marks: usize) -> Option<ZshProgram> {
    fn walk(value: &mut Value, mark: &str, removed: &mut usize) {
        match value {
            Value::Object(map) => {
                if let Some(Value::Array(lists)) = map.get_mut("lists")
                    && lists.first().is_some_and(|first| {
                        serde_json::from_value::<ZshList>(first.clone())
                            .is_ok_and(|list| sole_word(&list, ELSE_MARK))
                    })
                {
                    lists.remove(0);
                    *removed += 1;
                }
                if let Some(Value::String(source)) = map.get_mut("body_source") {
                    *source = source.replace(mark, "");
                }
                map.values_mut()
                    .for_each(|value| walk(value, mark, removed));
            }
            Value::Array(items) => items
                .iter_mut()
                .for_each(|value| walk(value, mark, removed)),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(program).ok()?;
    let mut removed = 0;
    walk(&mut value, mark, &mut removed);
    if removed != marks {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// Same lines and array elements; zsh reads ` \<newline>` there as a newline.
fn join_array_lines(text: &str) -> Cow<'_, str> {
    let newlines = scan(text).array_newlines;
    if newlines.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut joined = String::with_capacity(text.len() + 2 * newlines.len());
    let mut position = 0;
    for newline in newlines {
        joined.push_str(&text[position..newline]);
        joined.push_str(" \\");
        position = newline;
    }
    joined.push_str(&text[position..]);
    Cow::Owned(joined)
}

/// The parser accepts unterminated constructs, so a sentinel command must
/// still parse as its own final statement.
fn parse_complete(text: &str) -> Result<ZshProgram, String> {
    let mut program = parse_raw(&format!("{text}\n:\n"))?;
    let sentinel = program
        .lists
        .pop()
        .is_some_and(|last| sole_word(&last, ":"));
    if sentinel {
        Ok(program)
    } else {
        Err("incomplete input".to_string())
    }
}

/// Whether `list` is the command `word` alone.
fn sole_word(list: &ZshList, word: &str) -> bool {
    !list.flags.async_
        && list.sublist.next.is_none()
        && list.sublist.pipe.next.is_none()
        && matches!(&list.sublist.pipe.cmd, ZshCommand::Simple(simple)
            if simple.assigns.is_empty() && simple.redirs.is_empty()
                && simple.words.len() == 1 && untokenize_preserve_quotes(&simple.words[0]) == word)
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

/// `text` with a space after each `(` zsh reads as a subshell around `((`, a
/// spelling the parser rejects; `None` when there is none.
pub fn respace(text: &str) -> Option<String> {
    let opens = scan(text).subshell_parens;
    if opens.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len() + opens.len());
    let mut position = 0;
    for open in opens {
        out.push_str(&text[position..=open]);
        out.push(' ');
        position = open + 1;
    }
    out.push_str(&text[position..]);
    Some(out)
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
