//! Build-time evaluation of shell words whose value cannot change at runtime.
//! Anything it does not model yields `None` and stays a runtime expansion.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    Unknown,
    Unset,
    Scalar(String),
    Array(Vec<String>),
}

pub trait Env {
    fn var(&self, name: &str) -> Var;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Command arguments and `for` lists: braces, arrays and globs expand.
    Args,
    /// Assignment values and `[[ ]]` operands: one string.
    Scalar,
}

pub fn words(word: &str, env: &dyn Env, mode: Mode) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let alternatives = if mode == Mode::Args {
        braces(word)?
    } else {
        vec![word.to_string()]
    };
    for alternative in alternatives {
        out.extend(one(&alternative, env, mode)?);
    }
    Some(out)
}

pub fn scalar(word: &str, env: &dyn Env) -> Option<String> {
    let mut values = words(word, env, Mode::Scalar)?;
    match values.len() {
        0 => Some(String::new()),
        1 => values.pop(),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct Char {
    value: char,
    quoted: bool,
}

/// A `[[ == ]]` pattern; quoted parts match literally.
pub fn pattern(word: &str, env: &dyn Env) -> Option<glob::Pattern> {
    let (chars, _) = characters(word, env)?;
    if chars
        .iter()
        .any(|c| !c.quoted && matches!(c.value, '(' | ')' | '|' | '#' | '^' | '~' | '<' | '>'))
    {
        return None;
    }
    let mut pattern = String::new();
    for c in &chars {
        if c.quoted || !matches!(c.value, '*' | '?' | '[' | ']') {
            pattern.push_str(&glob::Pattern::escape(&c.value.to_string()));
        } else {
            pattern.push(c.value);
        }
    }
    glob::Pattern::new(&pattern).ok()
}

fn characters(word: &str, env: &dyn Env) -> Option<(Vec<Char>, bool)> {
    let mut parser = Parser {
        chars: word.chars().collect(),
        pos: 0,
        env,
        out: Vec::new(),
        any_quote: false,
    };
    parser.tilde()?;
    parser.word(false)?;
    Some((parser.out, parser.any_quote))
}

fn one(word: &str, env: &dyn Env, mode: Mode) -> Option<Vec<String>> {
    if let Some(elements) = whole_array(word, env, mode) {
        return Some(elements);
    }
    let (out, any_quote) = characters(word, env)?;
    if mode == Mode::Args {
        if let Some(matches) = glob(&out)? {
            return Some(matches);
        }
        if out.is_empty() && !any_quote {
            return Some(Vec::new());
        }
    }
    Some(vec![out.iter().map(|c| c.value).collect()])
}

/// `$name`, `${name}`, `"${name[@]}"`, `"${(@)name}"`: one word per element.
fn whole_array(word: &str, env: &dyn Env, mode: Mode) -> Option<Vec<String>> {
    let (name, quoted) = if let Some(inner) = word
        .strip_prefix("\"${")
        .and_then(|rest| rest.strip_suffix("[@]}\""))
    {
        (inner, true)
    } else if let Some(inner) = word
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
    {
        (inner, false)
    } else {
        (word.strip_prefix('$')?, false)
    };
    if !is_name(name) {
        return None;
    }
    match env.var(name) {
        Var::Array(elements) if mode == Mode::Args => Some(if quoted {
            elements
        } else {
            elements
                .into_iter()
                .filter(|element| !element.is_empty())
                .collect()
        }),
        _ => None,
    }
}

struct Parser<'a> {
    chars: Vec<char>,
    pos: usize,
    env: &'a dyn Env,
    out: Vec<Char>,
    any_quote: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn push(&mut self, value: char, quoted: bool) {
        self.out.push(Char { value, quoted });
    }

    fn push_str(&mut self, value: &str, quoted: bool) {
        for character in value.chars() {
            self.push(character, quoted);
        }
    }

    fn tilde(&mut self) -> Option<()> {
        if self.peek() != Some('~') {
            return Some(());
        }
        let rest = self.chars.get(1).copied();
        if rest.is_some_and(|c| c != '/') {
            return None;
        }
        let Var::Scalar(home) = self.env.var("HOME") else {
            return None;
        };
        self.pos = 1;
        self.push_str(&home, true);
        Some(())
    }

    fn word(&mut self, in_double: bool) -> Option<()> {
        while let Some(character) = self.peek() {
            match character {
                '"' if in_double => return Some(()),
                '"' => {
                    self.pos += 1;
                    self.any_quote = true;
                    self.word(true)?;
                    if self.peek() != Some('"') {
                        return None;
                    }
                    self.pos += 1;
                }
                '\'' if !in_double => {
                    self.pos += 1;
                    self.any_quote = true;
                    loop {
                        let next = self.peek()?;
                        self.pos += 1;
                        if next == '\'' {
                            break;
                        }
                        self.push(next, true);
                    }
                }
                '\\' => {
                    let next = *self.chars.get(self.pos + 1)?;
                    self.pos += 2;
                    if in_double && !matches!(next, '$' | '`' | '"' | '\\' | '\n') {
                        self.push('\\', true);
                    }
                    if next != '\n' {
                        self.push(next, true);
                    }
                }
                '$' => self.dollar(in_double)?,
                '`' => return None,
                '=' if self.pos == 0 && !in_double => return None,
                _ => {
                    self.pos += 1;
                    self.push(character, in_double);
                }
            }
        }
        if in_double { None } else { Some(()) }
    }

    fn dollar(&mut self, in_double: bool) -> Option<()> {
        match self.chars.get(self.pos + 1).copied() {
            Some('\'') if !in_double => {
                self.pos += 2;
                self.any_quote = true;
                loop {
                    let next = self.peek()?;
                    self.pos += 1;
                    match next {
                        '\'' => break,
                        '\\' => {
                            let escaped = self.peek()?;
                            self.pos += 1;
                            let value = match escaped {
                                'n' => '\n',
                                't' => '\t',
                                'e' | 'E' => '\x1b',
                                'a' => '\x07',
                                '\\' | '\'' | '"' => escaped,
                                _ => return None,
                            };
                            self.push(value, true);
                        }
                        _ => self.push(next, true),
                    }
                }
                Some(())
            }
            Some('{') => {
                let start = self.pos + 2;
                let end = matching_brace(&self.chars, start)?;
                let inner: String = self.chars[start..end].iter().collect();
                self.pos = end + 1;
                let value = self.braced(&inner)?;
                self.push_str(&value, in_double);
                Some(())
            }
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {
                let start = self.pos + 1;
                let mut end = start;
                while self
                    .chars
                    .get(end)
                    .is_some_and(|c| *c == '_' || c.is_ascii_alphanumeric())
                {
                    end += 1;
                }
                let name: String = self.chars[start..end].iter().collect();
                self.pos = end;
                if self.peek() == Some('[') {
                    return None;
                }
                let mut value = scalar_value(self.env.var(&name), in_double)?;
                while self.peek() == Some(':') {
                    let modifier = self.chars.get(self.pos + 1).copied()?;
                    if !"Aahtrelu".contains(modifier) {
                        break;
                    }
                    value = modify(&value, modifier)?;
                    self.pos += 2;
                }
                self.push_str(&value, in_double);
                Some(())
            }
            _ => None,
        }
    }

    fn braced(&self, inner: &str) -> Option<String> {
        let name_end = inner
            .find(|c: char| !(c == '_' || c.is_ascii_alphanumeric()))
            .unwrap_or(inner.len());
        let name = &inner[..name_end];
        if !is_name(name) {
            return None;
        }
        let rest = &inner[name_end..];
        let var = self.env.var(name);
        if rest.is_empty() {
            return scalar_value(var, true);
        }
        for (operator, colon) in [(":-", true), ("-", false), (":+", true), ("+", false)] {
            let Some(word) = rest.strip_prefix(operator) else {
                continue;
            };
            let set = match &var {
                Var::Unknown => return None,
                Var::Unset => false,
                Var::Scalar(value) => !(colon && value.is_empty()),
                Var::Array(values) => !(colon && values.is_empty()),
            };
            let use_word = if operator.ends_with('-') { !set } else { set };
            return if use_word {
                scalar(word, self.env)
            } else if operator.ends_with('-') {
                scalar_value(var, true)
            } else {
                Some(String::new())
            };
        }
        if rest.starts_with(':') {
            let mut value = scalar_value(var, true)?;
            let mut modifiers = rest.chars();
            while let Some(colon) = modifiers.next() {
                if colon != ':' {
                    return None;
                }
                value = modify(&value, modifiers.next()?)?;
            }
            return Some(value);
        }
        None
    }
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn scalar_value(var: Var, quoted: bool) -> Option<String> {
    match var {
        Var::Unknown => None,
        Var::Unset => Some(String::new()),
        Var::Scalar(value) => Some(value),
        Var::Array(values) if quoted => Some(values.join(" ")),
        Var::Array(_) => None,
    }
}

fn matching_brace(chars: &[char], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = start;
    while let Some(c) = chars.get(index) {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(index),
            '}' => depth -= 1,
            '\\' => index += 1,
            _ => {}
        }
        index += 1;
    }
    None
}

pub fn modify(value: &str, modifier: char) -> Option<String> {
    Some(match modifier {
        't' => value.rsplit('/').next().unwrap_or(value).to_string(),
        'h' => match value.rfind('/') {
            Some(0) => "/".to_string(),
            Some(index) => value[..index].to_string(),
            None => ".".to_string(),
        },
        'r' => {
            let tail_start = value.rfind('/').map_or(0, |index| index + 1);
            match value[tail_start..].rfind('.') {
                Some(dot) if dot > 0 => value[..tail_start + dot].to_string(),
                _ => value.to_string(),
            }
        }
        'e' => {
            let tail = value.rsplit('/').next().unwrap_or(value);
            match tail.rfind('.') {
                Some(dot) if dot > 0 => tail[dot + 1..].to_string(),
                _ => String::new(),
            }
        }
        'a' => absolute(value)?,
        'A' => {
            let absolute = absolute(value)?;
            std::fs::canonicalize(&absolute)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or(absolute)
        }
        'l' => value.to_lowercase(),
        'u' | 'U' => value.to_uppercase(),
        _ => return None,
    })
}

fn absolute(value: &str) -> Option<String> {
    if !value.starts_with('/') {
        return None;
    }
    let mut out = PathBuf::from("/");
    for component in Path::new(value).components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            _ => {}
        }
    }
    Some(out.to_string_lossy().into_owned())
}

/// Brace expansion of unquoted `{a,b}` and `{1..3}`, outermost first.
fn braces(word: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = word.chars().collect();
    let mut quote: Option<char> = None;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        match (quote, c) {
            (None, '\\') => index += 1,
            (None, '\'' | '"') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '$') if chars.get(index + 1) == Some(&'{') => {
                index = matching_brace(&chars, index + 2)?;
            }
            (None, '{') => {
                if let Some((end, alternatives)) = alternatives(&chars, index)? {
                    let prefix: String = chars[..index].iter().collect();
                    let suffix: String = chars[end + 1..].iter().collect();
                    let mut out = Vec::new();
                    for alternative in alternatives {
                        out.extend(braces(&format!("{prefix}{alternative}{suffix}"))?);
                    }
                    return Some(out);
                }
            }
            _ => {}
        }
        index += 1;
    }
    Some(vec![word.to_string()])
}

/// `None` when the word cannot be expanded, `Some(None)` for a literal brace.
fn alternatives(chars: &[char], open: usize) -> Option<Option<(usize, Vec<String>)>> {
    let mut depth = 0usize;
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut index = open + 1;
    while let Some(&c) = chars.get(index) {
        match c {
            '{' => {
                depth += 1;
                current.push(c);
            }
            '}' if depth == 0 => {
                parts.push(current);
                if parts.len() > 1 {
                    return Some(Some((index, parts)));
                }
                let range = parts[0].split_once("..").and_then(|(from, to)| {
                    Some((from.parse::<i64>().ok()?, to.parse::<i64>().ok()?))
                });
                let Some((from, to)) = range else {
                    return Some(None);
                };
                let values: Vec<String> = if from <= to {
                    (from..=to).map(|n| n.to_string()).collect()
                } else {
                    (to..=from).rev().map(|n| n.to_string()).collect()
                };
                return Some(Some((index, values)));
            }
            '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            '\'' | '"' | '\\' | '$' => return None,
            _ => current.push(c),
        }
        index += 1;
    }
    Some(None)
}

/// `Some(Some(paths))` when the word is a pattern, `Some(None)` when literal.
fn glob(chars: &[Char]) -> Option<Option<Vec<String>>> {
    let mut chars = chars.to_vec();
    let mut null_glob = false;
    if chars.len() >= 3 && !chars[chars.len() - 1].quoted && chars[chars.len() - 1].value == ')' {
        let open = chars.iter().rposition(|c| c.value == '(' && !c.quoted)?;
        let qualifiers: String = chars[open + 1..chars.len() - 1]
            .iter()
            .map(|c| c.value)
            .collect();
        if qualifiers != "N" {
            return None;
        }
        null_glob = true;
        chars.truncate(open);
    }
    let pattern_chars = chars
        .iter()
        .any(|c| !c.quoted && matches!(c.value, '*' | '?' | '['));
    if !pattern_chars && !null_glob {
        if chars
            .iter()
            .any(|c| !c.quoted && matches!(c.value, '^' | '#' | '~' | '(' | '<'))
        {
            return None;
        }
        return Some(None);
    }
    if chars
        .iter()
        .any(|c| !c.quoted && matches!(c.value, '^' | '#' | '(' | '|' | '<'))
    {
        return None;
    }
    let mut pattern = String::new();
    for c in &chars {
        if c.quoted || !matches!(c.value, '*' | '?' | '[' | ']') {
            pattern.push_str(&glob::Pattern::escape(&c.value.to_string()));
        } else {
            pattern.push(c.value);
        }
    }
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    let mut matches: Vec<String> = glob::glob_with(&pattern, options)
        .ok()?
        .filter_map(Result::ok)
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    matches.sort();
    if matches.is_empty() && !null_glob {
        return None;
    }
    Some(Some(matches))
}

#[cfg(test)]
#[path = "../tests/unit/expand_tests.rs"]
mod tests;
