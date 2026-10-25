//! Protect constructs the line formatter must not inspect, and translate
//! ConTeXt environment syntax into the LaTeX form the formatter understands.
//!
//! Every replacement is a private-use token that contains no whitespace,
//! braces, backslashes, or percent signs, so it cannot start an environment,
//! open a group, become a comment, or be wrapped. Tokens are restored after
//! layout, which makes verbatim and inline verbatim byte-preserving.

use std::collections::HashMap;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use crate::dialect::Dialect;

const PRIVATE_USE_START: u32 = 0xe000;
const PRIVATE_USE_END: u32 = 0xf8ff;

const DEPTH_LIMIT: usize = 100;

pub struct Prepared {
    pub text: String,
    pub marker: String,
    prefix: char,
    inline: HashMap<String, String>,
    blocks: HashMap<String, String>,
    context: Vec<(String, String, String)>,
}

impl Prepared {
    pub fn context(&self) -> &[(String, String, String)] {
        &self.context
    }

    /// Whether `text` still contains an internal token marker, which means a
    /// replacement was mangled by layout and could not be restored.
    pub fn has_placeholder(&self, text: &str) -> bool {
        text.contains(self.prefix)
    }

    pub fn restore(&self, formatted: &str) -> String {
        let mut text = formatted.to_owned();
        // A restored replacement may itself contain a token from an earlier
        // pass (an off region inside a verbatim body, inline verbatim inside a
        // frozen split line, ...), so restore until no marker is left.
        for _ in 0..8 {
            if !text.contains(self.prefix) {
                break;
            }
            text = self.restore_once(&text);
        }
        text
    }

    fn restore_once(&self, formatted: &str) -> String {
        let mut text = formatted.to_owned();
        for (placeholder, start, stop) in &self.context {
            text = text.replace(&format!("\\begin{{{placeholder}}}"), start.as_str());
            text = text.replace(&format!("\\end{{{placeholder}}}"), stop.as_str());
        }
        let mut restored = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let content = line.trim_end_matches(['\r', '\n']);
            if let Some(original) = self.blocks.get(content.trim()) {
                restored.push_str(original);
                if line.ends_with('\n') {
                    restored.push('\n');
                }
            } else {
                restored.push_str(line);
            }
        }
        restore_tokens(&restored, self.prefix, &self.inline)
    }
}

pub fn prepare(
    input: &str,
    dialect: Dialect,
    verbatims: &[String],
    lists: &[String],
) -> Result<Prepared, Diagnostic> {
    let prefix = choose_prefix(input).ok_or_else(|| {
        Diagnostic::new(
            DiagnosticKind::Syntax,
            "input uses every private-use character, so protected content cannot be marked",
        )
    })?;
    let mut inline = Vec::new();
    let mut blocks = Vec::new();
    let mut context = Vec::new();

    let text = protect_off_regions(input, prefix, dialect, verbatims, &mut blocks);
    let text = protect_inline(&text, prefix, &mut inline);
    let text = protect_blocks(&text, prefix, dialect, verbatims, &mut blocks);
    let text = translate_display_math(&text, prefix, &mut context);
    let text = if dialect == Dialect::Context {
        translate_context(&text, prefix, &mut context)
    } else {
        text
    };
    let text = protect_unicode_whitespace(&text, prefix, &mut inline);
    let text = guard_linebreak_comments(&text, prefix, &mut inline);
    let text = protect_escaped_splitters(&text, prefix, &mut inline);
    let text = split_commands(&text);
    let text = protect_escaped_braces(&text, prefix, &mut inline);
    let text = protect_delimiters(&text, prefix, &mut inline);

    let marker = format!("%{prefix}blank");
    let text = encode_blank_lines(&text, &marker);
    let mut effective_lists = lists.to_vec();
    for (placeholder, start, _) in &context {
        let name = start.trim_start_matches("\\start");
        if lists.iter().any(|candidate| candidate == name) {
            effective_lists.push(placeholder.clone());
        }
    }
    check_depth(&text, &effective_lists)?;

    Ok(Prepared {
        text,
        marker,
        prefix,
        inline: inline
            .into_iter()
            .map(|item| (item.token, item.original))
            .collect(),
        blocks: blocks
            .into_iter()
            .map(|item| (item.token, item.original))
            .collect(),
        context,
    })
}

struct Replacement {
    token: String,
    original: String,
}

fn make_token(prefix: char, kind: char, index: usize) -> String {
    format!("{prefix}{kind}{index}{prefix}")
}

/// A private-use character that the input does not contain, so a token cannot
/// collide with the user's own text.
fn choose_prefix(input: &str) -> Option<char> {
    let mut used = [false; (PRIVATE_USE_END - PRIVATE_USE_START + 1) as usize];
    for character in input.chars() {
        let code = character as u32;
        if (PRIVATE_USE_START..=PRIVATE_USE_END).contains(&code) {
            used[(code - PRIVATE_USE_START) as usize] = true;
        }
    }
    used.iter().position(|used| !*used).map(|index| {
        char::from_u32(PRIVATE_USE_START + index as u32).expect("private-use scalar value")
    })
}

fn protect_inline(input: &str, prefix: char, replacements: &mut Vec<Replacement>) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && !escaped(bytes, i) {
            i = line_end(bytes, i);
            continue;
        }
        if bytes[i] != b'\\' || !command_starts(bytes, i) {
            i += 1;
            continue;
        }
        let Some(end) = inline_end(input, i) else {
            i += 1;
            continue;
        };
        let index = replacements.len();
        let token = make_token(prefix, 'i', index);
        output.push_str(&input[last..i]);
        output.push_str(&token);
        replacements.push(Replacement {
            token,
            original: input[i..end].to_owned(),
        });
        last = end;
        i = end;
    }
    output.push_str(&input[last..]);
    output
}

/// End of an inline construct that must stay byte-identical, starting at `i`.
fn inline_end(input: &str, i: usize) -> Option<usize> {
    if input[i..].starts_with("\\verb") {
        let mut j = i + "\\verb".len();
        let after = input.as_bytes().get(j).copied();
        if after == Some(b'*') {
            j += 1;
        }
        let delimiter = input[j..].chars().next()?;
        if delimiter.is_ascii_alphabetic() || delimiter.is_whitespace() {
            return None;
        }
        return Some(find_delimiter(input, j + delimiter.len_utf8(), delimiter));
    }
    if let Some(mut j) = command_rest(input, i, "\\lstinline") {
        if input.as_bytes().get(j) == Some(&b'*') {
            j += 1;
        }
        j = skip_spaces(input, j);
        j = skip_optional_bracket(input, j);
        let start = j;
        let delimiter = input[j..].chars().next()?;
        if delimiter == '{' {
            return matching_brace(input, j)
                .map(|end| end + 1)
                .or(Some(line_end(input.as_bytes(), i)));
        }
        if delimiter.is_whitespace() || delimiter.is_ascii_alphabetic() {
            return None;
        }
        return Some(find_delimiter(
            input,
            start + delimiter.len_utf8(),
            delimiter,
        ));
    }
    if let Some(mut j) = command_rest(input, i, "\\mintinline") {
        if input.as_bytes().get(j) == Some(&b'*') {
            j += 1;
        }
        j = skip_spaces(input, j);
        j = skip_optional_bracket(input, j);
        if input.as_bytes().get(j) != Some(&b'{') {
            return None;
        }
        j = matching_brace(input, j)? + 1;
        j = skip_optional_bracket(input, j);
        let delimiter = input[j..].chars().next()?;
        if delimiter == '{' {
            return matching_brace(input, j).map(|end| end + 1);
        }
        if delimiter.is_whitespace() || delimiter.is_ascii_alphabetic() {
            return Some(line_end(input.as_bytes(), i));
        }
        return Some(find_delimiter(input, j + delimiter.len_utf8(), delimiter));
    }
    if let Some(j) = command_rest(input, i, "\\href") {
        return protect_arguments(input, j, 2);
    }
    for name in [
        "\\url",
        "\\path",
        "\\nolinkurl",
        "\\input",
        "\\include",
        "\\includegraphics",
        "\\bibliography",
        "\\addbibresource",
        "\\lstinputlisting",
        "\\directlua",
        "\\luadirect",
        "\\luaexec",
        "\\latelua",
    ] {
        if let Some(j) = command_rest(input, i, name) {
            return protect_arguments(input, j, 1);
        }
    }
    None
}

fn protect_arguments(input: &str, mut j: usize, wanted: usize) -> Option<usize> {
    if input.as_bytes().get(j) == Some(&b'*') {
        j += 1;
    }
    j = skip_spaces(input, j);
    j = skip_optional_bracket(input, j);
    j = skip_spaces(input, j);
    let mut end = j;
    let mut found = 0;
    while found < wanted && input.as_bytes().get(j) == Some(&b'{') {
        let close = matching_brace(input, j)?;
        end = close + 1;
        found += 1;
        j = skip_spaces(input, end);
    }
    (found > 0).then_some(end)
}

fn command_rest(input: &str, i: usize, command: &str) -> Option<usize> {
    input[i..].starts_with(command).then_some(i + command.len())
}

fn find_delimiter(input: &str, from: usize, delimiter: char) -> usize {
    input[from..].find(delimiter).map_or_else(
        || line_end(input.as_bytes(), from),
        |offset| from + offset + delimiter.len_utf8(),
    )
}

fn matching_brace(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;
    for (offset, character) in input[open..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == '{' {
            depth += 1;
        } else if character == '}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(open + offset);
            }
        }
    }
    None
}

fn skip_optional_bracket(input: &str, j: usize) -> usize {
    if input.as_bytes().get(j) != Some(&b'[') {
        return j;
    }
    let mut depth = 0usize;
    for (offset, character) in input[j..].char_indices() {
        match character {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return j + offset + 1;
                }
            }
            _ => {}
        }
    }
    j
}

fn skip_spaces(input: &str, mut j: usize) -> usize {
    while input
        .as_bytes()
        .get(j)
        .is_some_and(|byte| *byte == b' ' || *byte == b'\t')
    {
        j += 1;
    }
    j
}

/// Lines covered by `% tex-fmt: skip`, `% tex-fmt: off` and `% tex-fmt: on`.
///
/// The transformations in this module run before the layout pass that honours
/// those markers, so they consult this map instead of rewriting an ignored
/// region.
struct SkipRegions {
    lines: Vec<bool>,
    starts: Vec<usize>,
}

impl SkipRegions {
    fn new(input: &str) -> Self {
        let mut lines = Vec::new();
        let mut starts = Vec::new();
        let mut start = 0usize;
        let mut ignore = false;
        for line in input.split('\n') {
            starts.push(start);
            start += line.len() + 1;
            let tail = line.trim_end();
            let off = tail.ends_with("% tex-fmt: off");
            let on = tail.ends_with("% tex-fmt: on");
            let skip = tail.ends_with("% tex-fmt: skip");
            lines.push(ignore || off || on || skip);
            if off {
                ignore = true;
            } else if on {
                ignore = false;
            }
        }
        Self { lines, starts }
    }

    fn covers_line(&self, index: usize) -> bool {
        self.lines.get(index).copied().unwrap_or(false)
    }

    fn covers(&self, offset: usize) -> bool {
        let index = self.starts.partition_point(|start| *start <= offset);
        self.covers_line(index - 1)
    }
}

/// Regions delimited by `% tex-fmt: off` and `% tex-fmt: on` (or to the end of
/// the input) are replaced by a token and restored byte-for-byte, so neither
/// this adapter nor the layout engine can normalize them.
fn protect_off_regions(
    input: &str,
    prefix: char,
    dialect: Dialect,
    verbatims: &[String],
    replacements: &mut Vec<Replacement>,
) -> String {
    let verbatim = verbatim_ranges(input, dialect, verbatims);
    let inside_verbatim = |offset: usize| {
        verbatim
            .iter()
            .any(|(start, end)| *start <= offset && offset < *end)
    };
    let mut spans = Vec::new();
    let mut start = None;
    let mut offset = 0usize;
    for line in input.split('\n') {
        let end = offset + line.len();
        let trimmed = line.trim_end();
        if start.is_none() && !inside_verbatim(offset) && trimmed.ends_with("% tex-fmt: off") {
            start = Some(offset);
        } else if !inside_verbatim(offset)
            && trimmed.ends_with("% tex-fmt: on")
            && let Some(begin) = start.take()
        {
            spans.push((begin, end));
        }
        offset = end + 1;
    }
    if let Some(begin) = start {
        spans.push((begin, input.len()));
    }
    if spans.is_empty() {
        return input.to_owned();
    }
    let mut output = String::with_capacity(input.len());
    let mut last = 0usize;
    for (begin, end) in spans {
        output.push_str(&input[last..begin]);
        let token = make_token(prefix, 'o', replacements.len());
        output.push_str(&token);
        output.push('\n');
        replacements.push(Replacement {
            token,
            original: input[begin..end].to_owned(),
        });
        last = end;
        if input[last..].starts_with('\n') {
            last += 1;
        }
    }
    output.push_str(&input[last..]);
    output
}

/// Byte ranges of verbatim-like environments in the raw input, used to keep
/// `% tex-fmt: off` text inside such a body from opening a format region.
fn verbatim_ranges(input: &str, dialect: Dialect, verbatims: &[String]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && !escaped(bytes, i) {
            i = line_end(bytes, i);
            continue;
        }
        if bytes[i] != b'\\' || !command_starts(bytes, i) {
            i += 1;
            continue;
        }
        let Some((name, start_end, context)) = environment_start(input, i, dialect) else {
            i += 1;
            continue;
        };
        if !verbatims.iter().any(|verbatim| verbatim == &name) {
            i += 1;
            continue;
        }
        let end = environment_end(input, start_end, &name, context).unwrap_or(input.len());
        ranges.push((i, end));
        i = end;
    }
    ranges
}

fn protect_blocks(
    input: &str,
    prefix: char,
    dialect: Dialect,
    verbatims: &[String],
    replacements: &mut Vec<Replacement>,
) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && !escaped(bytes, i) {
            i = line_end(bytes, i);
            continue;
        }
        if bytes[i] != b'\\' || !command_starts(bytes, i) {
            i += 1;
            continue;
        }
        let Some((name, start_end, context)) = environment_start(input, i, dialect) else {
            i += 1;
            continue;
        };
        if !verbatims.iter().any(|verbatim| verbatim == &name) {
            i += 1;
            continue;
        }
        let end = environment_end(input, start_end, &name, context).unwrap_or(input.len());
        let index = replacements.len();
        let token = make_token(prefix, 'b', index);
        output.push_str(&input[last..i]);
        while output.ends_with([' ', '\t']) {
            output.pop();
        }
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&token);
        output.push('\n');
        replacements.push(Replacement {
            token,
            original: input[i..end].to_owned(),
        });
        let mut next = end;
        if input[next..].starts_with('\n') {
            next += 1;
        }
        last = next;
        i = next;
    }
    output.push_str(&input[last..]);
    output
}

fn environment_start(input: &str, i: usize, dialect: Dialect) -> Option<(String, usize, bool)> {
    if let Some(j) = command_rest(input, i, "\\begin") {
        let mut j = skip_spaces(input, j);
        if input.as_bytes().get(j) != Some(&b'{') {
            return None;
        }
        j += 1;
        let close = input[j..].find('}')? + j;
        return Some((input[j..close].to_owned(), close + 1, false));
    }
    if dialect == Dialect::Context
        && let Some(j) = command_rest(input, i, "\\start")
    {
        let name = read_context_name(input, j);
        if name.is_empty() {
            return None;
        }
        return Some((name.to_owned(), j + name.len(), true));
    }
    None
}

fn environment_end(input: &str, from: usize, name: &str, context: bool) -> Option<usize> {
    let mut depth = 1usize;
    let mut cursor = from;
    while cursor < input.len() {
        let begin = find_environment_command(input, cursor, name, true, context);
        let end = find_environment_command(input, cursor, name, false, context);
        let (position, is_begin) = match (begin, end) {
            (Some(begin), Some(end)) => {
                if begin <= end {
                    (begin, true)
                } else {
                    (end, false)
                }
            }
            (Some(begin), None) => (begin, true),
            (None, Some(end)) => (end, false),
            (None, None) => return None,
        };
        let close = if context {
            position
                + if is_begin {
                    "\\start".len()
                } else {
                    "\\stop".len()
                }
                + name.len()
        } else {
            input[position..].find('}')? + position + 1
        };
        if is_begin {
            depth += 1;
        } else {
            depth -= 1;
            if depth == 0 {
                return Some(close);
            }
        }
        cursor = close;
    }
    None
}

fn find_environment_command(
    input: &str,
    from: usize,
    name: &str,
    begin: bool,
    context: bool,
) -> Option<usize> {
    let needle = if context {
        if begin { "\\start" } else { "\\stop" }
    } else if begin {
        "\\begin"
    } else {
        "\\end"
    };
    let mut cursor = from;
    while let Some(offset) = input[cursor..].find(needle) {
        let position = cursor + offset;
        if command_starts(input.as_bytes(), position) {
            let rest = position + needle.len();
            if context {
                if read_context_name(input, rest) == name {
                    return Some(position);
                }
            } else {
                let mut j = skip_spaces(input, rest);
                if input.as_bytes().get(j) == Some(&b'{') {
                    j += 1;
                    if let Some(close) = input[j..].find('}')
                        && &input[j..j + close] == name
                    {
                        return Some(position);
                    }
                }
            }
        }
        cursor = position + needle.len();
    }
    None
}

fn read_context_name(input: &str, from: usize) -> &str {
    let end = input[from..]
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '*' | '-' | '_'))
        })
        .map_or(input.len(), |offset| from + offset);
    &input[from..end]
}

/// Translate a matched pair of `\[`/`\]` display-math delimiters into an
/// environment so the layout engine indents the body.
///
/// Only delimiters that are the first token of their line are considered: a
/// control symbol in the middle of a line is a character constant such as
/// ``\catcode`\[`` and must stay where it is.
fn translate_display_math(
    input: &str,
    prefix: char,
    context: &mut Vec<(String, String, String)>,
) -> String {
    let placeholder = format!("{prefix}m0");
    let skipped = SkipRegions::new(input);
    let mut delimiters: Vec<(usize, u8)> = Vec::new();
    let mut line_start = 0usize;
    for (index, line) in input.split('\n').enumerate() {
        let start = line_start;
        line_start += line.len() + 1;
        if skipped.covers_line(index) {
            continue;
        }
        let comment = comment_index(line.as_bytes());
        let code = &line[..comment];
        let trimmed = code.trim_start();
        let bytes = trimmed.as_bytes();
        if bytes.len() < 2 || bytes[0] != b'\\' || !matches!(bytes[1], b'[' | b']') {
            continue;
        }
        delimiters.push((start + code.len() - trimmed.len(), bytes[1]));
    }
    let mut matched = vec![false; delimiters.len()];
    let mut open: Vec<usize> = Vec::new();
    for (index, (_, byte)) in delimiters.iter().enumerate() {
        if *byte == b'[' {
            open.push(index);
        } else if let Some(open) = open.pop() {
            matched[open] = true;
            matched[index] = true;
        }
    }
    if !matched.iter().any(|matched| *matched) {
        return input.to_owned();
    }
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    for ((offset, byte), matched) in delimiters.iter().zip(&matched) {
        if !matched {
            continue;
        }
        output.push_str(&input[last..*offset]);
        if *byte == b'[' {
            output.push_str(&format!("\\begin{{{placeholder}}}"));
        } else {
            output.push_str(&format!("\\end{{{placeholder}}}"));
        }
        last = offset + 2;
    }
    output.push_str(&input[last..]);
    context.push((placeholder, "\\[".to_owned(), "\\]".to_owned()));
    output
}

fn translate_context(
    input: &str,
    prefix: char,
    context: &mut Vec<(String, String, String)>,
) -> String {
    let skipped = SkipRegions::new(input);
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    let mut i = 0;
    let mut known: HashMap<String, String> = HashMap::new();
    while i < bytes.len() {
        if bytes[i] == b'%' && !escaped(bytes, i) {
            i = line_end(bytes, i);
            continue;
        }
        if bytes[i] != b'\\' || !command_starts(bytes, i) || skipped.covers(i) {
            i += 1;
            continue;
        }
        let (start, name) = if let Some(j) = command_rest(input, i, "\\start") {
            (true, read_context_name(input, j))
        } else if let Some(j) = command_rest(input, i, "\\stop") {
            (false, read_context_name(input, j))
        } else {
            i += 1;
            continue;
        };
        if name.is_empty() {
            i += 1;
            continue;
        }
        let placeholder = known.entry(name.to_owned()).or_insert_with(|| {
            let placeholder = format!("{prefix}c{}", context.len());
            context.push((
                placeholder.clone(),
                format!("\\start{name}"),
                format!("\\stop{name}"),
            ));
            placeholder
        });
        output.push_str(&input[last..i]);
        if start {
            output.push_str(&format!("\\begin{{{}}}", placeholder.as_str()));
        } else {
            output.push_str(&format!("\\end{{{}}}", placeholder.as_str()));
        }
        last = i + 1 + if start { "start".len() } else { "stop".len() } + name.len();
        i = last;
    }
    output.push_str(&input[last..]);
    output
}

/// Replace non-ASCII whitespace with an inline token so the layout pass cannot
/// trim, re-indent or wrap it. TeX treats such a character as an ordinary
/// character, so it is content, not layout.
fn protect_unicode_whitespace(
    input: &str,
    prefix: char,
    replacements: &mut Vec<Replacement>,
) -> String {
    let skipped = SkipRegions::new(input);
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    for (offset, character) in input.char_indices() {
        if character.is_ascii() || !character.is_whitespace() || skipped.covers(offset) {
            continue;
        }
        let token = make_token(prefix, 'w', replacements.len());
        output.push_str(&input[last..offset]);
        output.push_str(&token);
        replacements.push(Replacement {
            token,
            original: character.to_string(),
        });
        last = offset + character.len_utf8();
    }
    output.push_str(&input[last..]);
    output
}

fn guard_linebreak_comments(
    input: &str,
    prefix: char,
    replacements: &mut Vec<Replacement>,
) -> String {
    let skipped = SkipRegions::new(input);
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && !skipped.covers(i) {
            let mut backslashes = 0;
            let mut j = i;
            while j > 0 && bytes[j - 1] == b'\\' {
                backslashes += 1;
                j -= 1;
            }
            if backslashes >= 2 && backslashes % 2 == 0 {
                let token = make_token(prefix, 'g', replacements.len());
                output.push_str(&input[last..i]);
                output.push_str(&token);
                replacements.push(Replacement {
                    token,
                    original: String::new(),
                });
                last = i;
            }
        }
        if bytes[i] == b'\n' {
            i += 1;
            continue;
        }
        i += 1;
    }
    output.push_str(&input[last..]);
    output
}

/// Put every command that must begin a line on its own line, so the layout
/// engine never splits a shared line itself: a newline it inserts without a
/// guard would become a space token.
///
/// The command set mirrors tex-fmt's own splitting patterns. When the text
/// before the split point does not already end in whitespace, it is terminated
/// with `%` so the newline is swallowed and the token stream is unchanged.
fn split_commands(input: &str) -> String {
    let skipped = SkipRegions::new(input);
    let mut output = String::with_capacity(input.len() + input.len() / 16);
    for (index, line) in input.split('\n').enumerate() {
        if index > 0 {
            output.push('\n');
        }
        if skipped.covers_line(index) {
            output.push_str(line);
            continue;
        }
        let comment = comment_index(line.as_bytes());
        let code = &line[..comment];
        let mut start = 0usize;
        for position in split_positions(code) {
            let preceding = &line[start..position];
            let trimmed = preceding.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            output.push_str(trimmed);
            if trimmed.len() == preceding.len() {
                output.push('%');
            }
            output.push('\n');
            start = position;
        }
        output.push_str(&line[start..]);
    }
    output
}

/// Freeze splitter text that is not a command because it follows an
/// even-length backslash run (for example `text\\item` is a line break and the
/// letters `item`). tex-fmt matches those bytes as a splitter anyway, so the
/// rest of the code line is replaced by a token and restored unchanged.
fn protect_escaped_splitters(
    input: &str,
    prefix: char,
    replacements: &mut Vec<Replacement>,
) -> String {
    let skipped = SkipRegions::new(input);
    let mut output = String::with_capacity(input.len());
    let mut last = 0usize;
    let mut line_start = 0usize;
    for (index, line) in input.split('\n').enumerate() {
        let start = line_start;
        line_start += line.len() + 1;
        if skipped.covers_line(index) {
            continue;
        }
        let comment = comment_index(line.as_bytes());
        let code = &line[..comment];
        let Some(run_start) = escaped_splitter_start(code) else {
            continue;
        };
        // Keep a real splitter later on the line visible to the layout pass so
        // environment and list indentation still sees it.
        let end = next_own_line_command(code, run_start + 1).unwrap_or(comment);
        let token = make_token(prefix, 's', replacements.len());
        output.push_str(&input[last..start + run_start]);
        output.push_str(&token);
        replacements.push(Replacement {
            token,
            original: input[start + run_start..start + end].to_owned(),
        });
        last = start + end;
    }
    output.push_str(&input[last..]);
    output
}

fn next_own_line_command(code: &str, from: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'\\' && command_starts(bytes, i) && requires_own_line(code, i) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Start of the backslash run of a splitter-looking command that is not a
/// command because the run has an even length.
fn escaped_splitter_start(code: &str) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && !command_starts(bytes, i) && requires_own_line(code, i) {
            let mut start = i;
            while start > 0 && bytes[start - 1] == b'\\' {
                start -= 1;
            }
            return Some(start);
        }
        i += 1;
    }
    None
}

fn split_positions(code: &str) -> Vec<usize> {
    let bytes = code.as_bytes();
    let mut positions = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && !escaped(bytes, i) {
            break;
        }
        if bytes[i] == b'\\' && command_starts(bytes, i) && requires_own_line(code, i) {
            positions.push(i);
            i += 1;
            continue;
        }
        i += 1;
    }
    positions
}

/// The commands in tex-fmt's splitting set: environments, `\item`, and the
/// sectioning commands.
fn requires_own_line(code: &str, position: usize) -> bool {
    if environment_command(code, position) {
        return true;
    }
    let rest = &code[position..];
    if let Some(tail) = rest.strip_prefix("\\item")
        && !tail.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    {
        return true;
    }
    for name in [
        "\\section",
        "\\subsection",
        "\\subsubsection",
        "\\chapter",
        "\\part",
    ] {
        if let Some(tail) = rest.strip_prefix(name) {
            let tail = tail.strip_prefix('*').unwrap_or(tail);
            return tail.starts_with('{');
        }
    }
    false
}

fn protect_escaped_braces(
    input: &str,
    prefix: char,
    replacements: &mut Vec<Replacement>,
) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if (bytes[i] == b'{' || bytes[i] == b'}') && escaped(bytes, i) {
            let token = make_token(prefix, 'e', replacements.len());
            output.push_str(&input[last..i - 1]);
            output.push_str(&token);
            replacements.push(Replacement {
                token,
                original: input[i - 1..=i].to_owned(),
            });
            last = i + 1;
        }
        i += 1;
    }
    output.push_str(&input[last..]);
    output
}

fn protect_delimiters(input: &str, prefix: char, replacements: &mut Vec<Replacement>) -> String {
    let tokens = [
        (make_token(prefix, 'd', 0), "("),
        (make_token(prefix, 'd', 1), ")"),
        (make_token(prefix, 'd', 2), "["),
        (make_token(prefix, 'd', 3), "]"),
    ];
    for (token, original) in &tokens {
        replacements.push(Replacement {
            token: token.clone(),
            original: (*original).to_owned(),
        });
    }
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && !escaped(bytes, i) {
            i = line_end(bytes, i);
            continue;
        }
        let replacement = match bytes[i] {
            b'(' => Some(&tokens[0].0),
            b')' => Some(&tokens[1].0),
            b'[' => Some(&tokens[2].0),
            b']' => Some(&tokens[3].0),
            _ => None,
        };
        if let Some(token) = replacement {
            output.push_str(&input[last..i]);
            output.push_str(token);
            last = i + 1;
        }
        i += 1;
    }
    output.push_str(&input[last..]);
    output
}

fn encode_blank_lines(input: &str, marker: &str) -> String {
    let mut output = String::with_capacity(input.len() + input.len() / 32);
    let mut lines: Vec<&str> = input.split('\n').collect();
    if input.ends_with('\n') {
        lines.pop();
    }
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        if line.trim().is_empty() {
            output.push_str(marker);
        } else {
            output.push_str(line);
        }
    }
    if !lines.is_empty() {
        output.push('\n');
    }
    output
}

fn check_depth(input: &str, lists: &[String]) -> Result<(), Diagnostic> {
    let mut depth = 0i32;
    let mut lowest = 0i32;
    let mut highest = 0i32;
    for (index, line) in input.split('\n').enumerate() {
        let comment = comment_index(line.as_bytes());
        let code = &line[..comment];
        let bytes = code.as_bytes();
        let mut opens = 0usize;
        let mut closes = 0usize;
        for (position, byte) in bytes.iter().enumerate() {
            match *byte {
                b'{' | b'(' | b'[' => opens += 1,
                b'}' | b')' | b']' => closes += 1,
                _ => {}
            }
            if bytes[position] == b'\\'
                && command_starts(bytes, position)
                && (code[position..].starts_with("\\begin")
                    || code[position..].starts_with("\\end"))
            {
                let begin = code[position..].starts_with("\\begin");
                let mut j = position + if begin { 6 } else { 4 };
                j = skip_spaces(code, j);
                if code.as_bytes().get(j) == Some(&b'{')
                    && let Some(close) = code[j + 1..].find('}')
                {
                    let name = &code[j + 1..j + 1 + close];
                    if begin {
                        opens += 1;
                    } else {
                        closes += 1;
                    }
                    if lists.iter().any(|candidate| candidate == name) {
                        if begin {
                            opens += 1;
                        } else {
                            closes += 1;
                        }
                    }
                }
            }
        }
        if opens > DEPTH_LIMIT || closes > DEPTH_LIMIT {
            return Err(Diagnostic::new(
                DiagnosticKind::Syntax,
                format!("nesting deeper than {DEPTH_LIMIT} levels is not supported"),
            )
            .with_location(index + 1, Some(1)));
        }
        depth += opens as i32 - closes as i32;
        lowest = lowest.min(depth);
        highest = highest.max(depth);
        if highest > DEPTH_LIMIT as i32 || -lowest > DEPTH_LIMIT as i32 {
            return Err(Diagnostic::new(
                DiagnosticKind::Syntax,
                format!("nesting deeper than {DEPTH_LIMIT} levels is not supported"),
            )
            .with_location(index + 1, Some(1)));
        }
    }
    Ok(())
}

fn restore_tokens(input: &str, prefix: char, replacements: &HashMap<String, String>) -> String {
    let mut output = String::with_capacity(input.len());
    let mut i = 0;
    while let Some(offset) = input[i..].find(prefix) {
        let start = i + offset;
        output.push_str(&input[i..start]);
        let Some(close) = input[start + prefix.len_utf8()..].find(prefix) else {
            // An unpaired marker cannot be a token; keep it and keep scanning so
            // one damaged marker does not leave the rest of the file tokenised.
            output.push(prefix);
            i = start + prefix.len_utf8();
            continue;
        };
        let end = start + prefix.len_utf8() + close + prefix.len_utf8();
        let token = &input[start..end];
        if let Some(original) = replacements.get(token) {
            output.push_str(original);
        } else {
            output.push_str(token);
        }
        i = end;
    }
    output.push_str(&input[i..]);
    output
}

fn escaped(bytes: &[u8], position: usize) -> bool {
    let mut count = 0;
    let mut i = position;
    while i > 0 && bytes[i - 1] == b'\\' {
        count += 1;
        i -= 1;
    }
    count % 2 == 1
}

fn command_starts(bytes: &[u8], position: usize) -> bool {
    let mut count = 0;
    let mut i = position + 1;
    while i > 0 && bytes[i - 1] == b'\\' {
        count += 1;
        i -= 1;
    }
    count % 2 == 1
}

fn comment_index(bytes: &[u8]) -> usize {
    for (position, byte) in bytes.iter().enumerate() {
        if *byte == b'%' && !escaped(bytes, position) {
            return position;
        }
    }
    bytes.len()
}

fn environment_command(code: &str, position: usize) -> bool {
    let rest = if code[position..].starts_with("\\begin") {
        position + 6
    } else if code[position..].starts_with("\\end") {
        position + 4
    } else {
        return false;
    };
    let mut j = rest;
    while code.as_bytes().get(j) == Some(&b' ') || code.as_bytes().get(j) == Some(&b'\t') {
        j += 1;
    }
    code.as_bytes().get(j) == Some(&b'{')
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| from + offset)
}
