//! Aliases zsh expands as it parses the sources, for the build to expand in
//! the bundle: wordcode from `zcompile -U` expands none, and a bundle read as
//! text could expand a word again, so such words are quoted.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

/// Words that are reserved in command position; never expanded by the build.
const RESERVED: &[&str] = &[
    "!",
    "[[",
    "{",
    "}",
    "case",
    "coproc",
    "do",
    "done",
    "elif",
    "else",
    "esac",
    "fi",
    "for",
    "foreach",
    "function",
    "if",
    "nocorrect",
    "repeat",
    "select",
    "then",
    "time",
    "until",
    "while",
    "always",
    "declare",
    "export",
    "float",
    "integer",
    "local",
    "readonly",
    "typeset",
];

/// Reserved words after which zsh still reads a command name.
const COMMAND_NEXT: &[&str] = &[
    "!",
    "{",
    "coproc",
    "do",
    "elif",
    "else",
    "if",
    "nocorrect",
    "then",
    "time",
    "until",
    "while",
    "always",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Known {
        text: String,
        global: bool,
    },
    /// Set to an unknown value, or not known on every path; global or
    /// not, when every path agrees.
    Unknown {
        global: Option<bool>,
    },
}

impl Entry {
    fn global(&self) -> Option<bool> {
        match self {
            Entry::Known { global, .. } => Some(*global),
            Entry::Unknown { global } => *global,
        }
    }
}

/// Aliases on every path here; a name absent from both tables is no alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Regular and global aliases, which share one namespace.
    names: BTreeMap<String, Entry>,
    suffixes: BTreeMap<String, Entry>,
    /// Code the build does not follow may have set any alias.
    any: bool,
    /// The `aliases` option; `None` when unknown.
    pub option: Option<bool>,
}

impl Default for Table {
    fn default() -> Self {
        Self {
            names: BTreeMap::new(),
            suffixes: BTreeMap::new(),
            any: false,
            option: Some(true),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Regular,
    Global,
    Suffix,
}

impl Table {
    pub fn define(&mut self, kind: Kind, name: &str, text: Option<String>, seen: &mut Seen) {
        let global = kind == Kind::Global;
        let entry = text.map_or(
            Entry::Unknown {
                global: Some(global),
            },
            |text| Entry::Known { text, global },
        );
        seen.insert(kind, name);
        match kind {
            Kind::Suffix => self.suffixes.insert(name.to_string(), entry),
            _ => self.names.insert(name.to_string(), entry),
        };
    }

    pub fn remove(&mut self, suffix: bool, name: &str) {
        match suffix {
            true => self.suffixes.remove(name),
            false => self.names.remove(name),
        };
    }

    /// `unalias -a`, or `unalias -m` with names known only by pattern.
    pub fn remove_where(&mut self, suffix: bool, matches: impl Fn(&str) -> bool) {
        let table = if suffix {
            &mut self.suffixes
        } else {
            &mut self.names
        };
        table.retain(|name, _| !matches(name));
    }

    /// An alias that code may have removed: unknown when it was one.
    pub fn maybe_remove(&mut self, suffix: bool, name: &str) {
        let table = if suffix {
            &mut self.suffixes
        } else {
            &mut self.names
        };
        if let Some(entry) = table.get_mut(name) {
            *entry = Entry::Unknown {
                global: entry.global(),
            };
        }
    }

    /// An alias set by code the build does not follow.
    pub fn forget(&mut self, name: &str, seen: &mut Seen) {
        seen.insert(Kind::Global, name);
        seen.insert(Kind::Suffix, name);
        self.names
            .insert(name.to_string(), Entry::Unknown { global: None });
        self.suffixes
            .insert(name.to_string(), Entry::Unknown { global: None });
    }

    /// Any alias may have been set or removed.
    pub fn forget_all(&mut self, seen: &mut Seen) {
        self.any = true;
        seen.any = true;
    }

    /// What holds on both paths.
    pub fn merge(&mut self, other: &Table) {
        merge_entries(&mut self.names, &other.names);
        merge_entries(&mut self.suffixes, &other.suffixes);
        self.any |= other.any;
        if self.option != other.option {
            self.option = None;
        }
    }

    /// Known aliases as `alias` commands zsh reads back; unknown ones as a
    /// global alias to `marker`, so expanding one shows.
    pub fn definitions(&self, marker: &str) -> String {
        let mut out = String::new();
        for (name, entry) in &self.names {
            let line = match entry {
                Entry::Known { text, global } => {
                    let flag = if *global { "-g " } else { "" };
                    format!(
                        "alias {flag}-- {}\n",
                        crate::quote::word(&format!("{name}={text}"))
                    )
                }
                Entry::Unknown { global } => {
                    let flag = if *global == Some(false) { "" } else { "-g " };
                    format!(
                        "alias {flag}-- {}\n",
                        crate::quote::word(&format!("{name}={marker}"))
                    )
                }
            };
            out.push_str(&line);
        }
        for (name, entry) in &self.suffixes {
            let text = match entry {
                Entry::Known { text, .. } => text.clone(),
                Entry::Unknown { .. } => marker.to_string(),
            };
            out.push_str(&format!(
                "alias -s -- {}\n",
                crate::quote::word(&format!("{name}={text}"))
            ));
        }
        out
    }
}

/// A name that is an alias on one path only keeps the kind it has there,
/// as no path expands it elsewhere.
fn merge_entries(left: &mut BTreeMap<String, Entry>, right: &BTreeMap<String, Entry>) {
    let names: BTreeSet<String> = left.keys().chain(right.keys()).cloned().collect();
    for name in names {
        let (one, other) = (left.get(&name), right.get(&name));
        if one == other {
            continue;
        }
        let global = match (one.map(Entry::global), other.map(Entry::global)) {
            (None, Some(global)) | (Some(global), None) => global,
            (Some(one), Some(other)) if one == other => one,
            _ => None,
        };
        left.insert(name, Entry::Unknown { global });
    }
}

/// Alias names that may be set by the time zsh parses a statement of a
/// bundle read as text: those set when the bundle statement it is part of
/// starts, and any set since.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    regular: BTreeSet<String>,
    global: BTreeSet<String>,
    suffixes: BTreeSet<String>,
    any: bool,
}

impl Seen {
    /// The names that may be aliases with `table` in effect.
    pub fn from_table(table: &Table) -> Self {
        let mut seen = Seen {
            any: table.any,
            ..Seen::default()
        };
        for (name, entry) in &table.names {
            let kind = match entry.global() {
                Some(false) => Kind::Regular,
                _ => Kind::Global,
            };
            seen.insert(kind, name);
        }
        for name in table.suffixes.keys() {
            seen.insert(Kind::Suffix, name);
        }
        seen
    }

    fn insert(&mut self, kind: Kind, name: &str) {
        let set = match kind {
            Kind::Regular => &mut self.regular,
            Kind::Global => &mut self.global,
            Kind::Suffix => &mut self.suffixes,
        };
        set.insert(name.to_string());
    }

    /// Each name as an alias of its kind to `marker`, so expanding one shows.
    pub fn definitions(&self, marker: &str) -> String {
        let line = |flag: &str, name: &str| {
            format!(
                "alias {flag}-- {}\n",
                crate::quote::word(&format!("{name}={marker}"))
            )
        };
        let regular = self
            .regular
            .iter()
            .filter(|name| !self.global.contains(*name))
            .map(|name| line("", name));
        let global = self.global.iter().map(|name| line("-g ", name));
        let suffixes = self.suffixes.iter().map(|name| line("-s ", name));
        regular.chain(global).chain(suffixes).collect()
    }

    fn names(&self) -> impl Iterator<Item = &String> {
        self.regular
            .iter()
            .chain(&self.global)
            .chain(self.suffixes.iter())
    }
}

/// A word of a statement, where zsh could expand an alias in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub range: Range<usize>,
    /// Where zsh reads a command name.
    pub command: bool,
    /// Unquoted and without expansions: what an alias name can match.
    pub plain: bool,
    /// Names a function as `name()`, which zsh expands as a command name.
    pub function: bool,
}

/// The words of `text` that are no assignment, function name, reserved word
/// or part of a substitution zsh parses when it runs; `None` for syntax this
/// reader does not follow.
pub fn words(text: &str) -> Option<Vec<Word>> {
    let mut lexer = Lexer {
        bytes: text.as_bytes(),
        pos: 0,
        words: Vec::new(),
        command: true,
        mode: Mode::Normal,
        cases: 0,
        heredocs: Vec::new(),
        simple: true,
    };
    lexer.run()?;
    Some(lexer.words)
}

/// The words of an alias value that is one simple command of plain words.
fn value_words(text: &str) -> Option<Vec<Word>> {
    let mut lexer = Lexer {
        bytes: text.as_bytes(),
        pos: 0,
        words: Vec::new(),
        command: true,
        mode: Mode::Normal,
        cases: 0,
        heredocs: Vec::new(),
        simple: true,
    };
    lexer.run()?;
    lexer.simple.then_some(lexer.words)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    /// Inside `[[ ]]`.
    Cond,
    /// After `for`, `foreach` or `select`, before `in`, `(` or a separator.
    LoopNames,
    /// A loop's words, up to a separator or `)`.
    LoopWords {
        paren: bool,
    },
    /// After `case`: its word, then `in`.
    CaseWord,
    /// A `case` pattern, up to `)`.
    Pattern,
    /// After `function`: names, up to `{` or `()`.
    FunctionNames,
    /// After `repeat`: the count.
    Count,
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
    words: Vec<Word>,
    command: bool,
    mode: Mode,
    /// `case` statements open around the cursor.
    cases: usize,
    heredocs: Vec<(Vec<u8>, bool)>,
    /// Only one command of plain words and arguments so far.
    simple: bool,
}

impl Lexer<'_> {
    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.bytes[self.pos..].starts_with(prefix)
    }

    fn separator(&mut self) {
        self.simple = false;
        self.command = true;
        if matches!(
            self.mode,
            Mode::LoopWords { paren: false } | Mode::LoopNames
        ) {
            self.mode = Mode::Normal;
        }
    }

    fn run(&mut self) -> Option<()> {
        while let Some(byte) = self.peek(0) {
            match byte {
                b' ' | b'\t' => self.pos += 1,
                b'\\' if self.peek(1) == Some(b'\n') => self.pos += 2,
                b'\n' => {
                    self.pos += 1;
                    self.heredoc_bodies()?;
                    if self.mode != Mode::Cond {
                        self.separator();
                    }
                }
                b'#' => {
                    self.simple = false;
                    while self.peek(0).is_some_and(|byte| byte != b'\n') {
                        self.pos += 1;
                    }
                }
                _ if self.mode == Mode::Cond => self.cond_token()?,
                b';' => {
                    self.pos += 1;
                    if matches!(self.peek(0), Some(b';' | b'&' | b'|')) && self.cases > 0 {
                        self.pos += 1;
                        self.simple = false;
                        self.mode = Mode::Pattern;
                        self.command = false;
                    } else {
                        self.separator();
                    }
                }
                b'&' if self.peek(1) == Some(b'>') => self.redirection()?,
                b'&' => {
                    self.pos += 1;
                    if matches!(self.peek(0), Some(b'&' | b'!' | b'|')) {
                        self.pos += 1;
                    }
                    self.separator();
                }
                b'|' => {
                    self.pos += 1;
                    if matches!(self.peek(0), Some(b'|' | b'&')) {
                        self.pos += 1;
                    }
                    if self.mode != Mode::Pattern {
                        self.separator();
                    }
                }
                b'<' | b'>' if self.peek(1) == Some(b'(') => self.word()?,
                b'<' | b'>' => self.redirection()?,
                b'0'..=b'9' if self.fd_redirection() => self.redirection()?,
                b'(' => self.open_paren()?,
                b')' => {
                    self.pos += 1;
                    self.simple = false;
                    match self.mode {
                        Mode::Pattern => {
                            self.mode = Mode::Normal;
                            self.command = true;
                        }
                        Mode::LoopWords { paren: true } => {
                            self.mode = Mode::Normal;
                            self.command = true;
                        }
                        _ => self.command = false,
                    }
                }
                _ => self.word()?,
            }
        }
        (self.heredocs.is_empty() && self.mode != Mode::Cond).then_some(())
    }

    fn fd_redirection(&self) -> bool {
        let digits = self.bytes[self.pos..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        matches!(self.peek(digits), Some(b'<' | b'>')) && self.peek(digits + 1) != Some(b'(')
    }

    fn open_paren(&mut self) -> Option<()> {
        self.simple = false;
        if self.command && self.starts_with(b"()") {
            self.pos += 2;
            return Some(());
        }
        if (self.command || self.mode == Mode::LoopNames) && self.starts_with(b"((") {
            self.pos = balanced(self.bytes, self.pos)?;
            self.command = self.mode == Mode::LoopNames;
            self.mode = Mode::Normal;
            return Some(());
        }
        match self.mode {
            Mode::Pattern => self.pos += 1,
            Mode::LoopNames => {
                self.pos += 1;
                self.mode = Mode::LoopWords { paren: true };
                self.command = false;
            }
            _ => {
                self.pos += 1;
                self.command = true;
            }
        }
        Some(())
    }

    fn redirection(&mut self) -> Option<()> {
        self.simple = false;
        while self.peek(0).is_some_and(|byte| byte.is_ascii_digit()) {
            self.pos += 1;
        }
        let heredoc = self.starts_with(b"<<") && !self.starts_with(b"<<<");
        let strip = heredoc && self.peek(2) == Some(b'-');
        while matches!(self.peek(0), Some(b'<' | b'>' | b'&' | b'|' | b'!' | b'-')) {
            self.pos += 1;
        }
        // `<&-` and `>&-` close a descriptor and take no word
        if self.bytes[..self.pos].ends_with(b"&-") {
            return Some(());
        }
        while matches!(self.peek(0), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
        let start = self.pos;
        let (end, _) = word_end(self.bytes, start)?;
        if end == start {
            return None;
        }
        if heredoc {
            let delimiter: Vec<u8> = self.bytes[start..end]
                .iter()
                .copied()
                .filter(|byte| !matches!(byte, b'\'' | b'"' | b'\\'))
                .collect();
            self.heredocs.push((delimiter, strip));
        }
        self.pos = end;
        Some(())
    }

    fn heredoc_bodies(&mut self) -> Option<()> {
        for (delimiter, strip) in std::mem::take(&mut self.heredocs) {
            loop {
                let end = self.bytes[self.pos..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(self.bytes.len(), |offset| self.pos + offset);
                let mut line = &self.bytes[self.pos..end];
                if strip {
                    while line.first() == Some(&b'\t') {
                        line = &line[1..];
                    }
                }
                let last = end >= self.bytes.len();
                self.pos = (end + 1).min(self.bytes.len());
                if line == delimiter.as_slice() {
                    break;
                }
                if last {
                    return None;
                }
            }
        }
        Some(())
    }

    fn cond_token(&mut self) -> Option<()> {
        let start = self.pos;
        let (end, plain) = word_end(self.bytes, start)?;
        if end == start {
            self.pos += 1;
            return Some(());
        }
        self.pos = end;
        if &self.bytes[start..end] == b"]]" {
            self.mode = Mode::Normal;
            self.command = false;
            return Some(());
        }
        self.push(start..end, false, plain);
        Some(())
    }

    fn word(&mut self) -> Option<()> {
        let start = self.pos;
        let (end, plain) = word_end(self.bytes, start)?;
        if end == start {
            return None;
        }
        self.pos = end;
        let text = &self.bytes[start..end];
        let name = std::str::from_utf8(text).ok()?;
        match self.mode {
            Mode::LoopNames => {
                if name == "in" {
                    self.mode = Mode::LoopWords { paren: false };
                }
                return Some(());
            }
            Mode::FunctionNames => {
                if name == "{" || self.skip_parens() {
                    self.mode = Mode::Normal;
                    self.command = true;
                }
                return Some(());
            }
            Mode::Count => {
                self.mode = Mode::Normal;
                self.command = true;
                return Some(());
            }
            Mode::CaseWord => {
                if name == "in" {
                    self.mode = Mode::Pattern;
                    self.command = false;
                } else {
                    self.push(start..end, false, plain);
                }
                return Some(());
            }
            Mode::Pattern if name == "esac" => {
                self.cases = self.cases.saturating_sub(1);
                self.mode = Mode::Normal;
                self.command = false;
                return Some(());
            }
            Mode::Pattern | Mode::LoopWords { .. } => {
                self.push(start..end, false, plain);
                return Some(());
            }
            Mode::Normal | Mode::Cond => {}
        }
        // zsh closes a brace group at `}` in any position
        if plain && name == "}" {
            self.simple = false;
            self.command = false;
            return Some(());
        }
        if self.command && plain && RESERVED.contains(&name) {
            self.simple = false;
            self.reserved(name);
            return Some(());
        }
        if self.command && assignment(text) {
            self.simple = false;
            if self.peek(0) == Some(b'(') {
                self.pos = balanced(self.bytes, self.pos)?;
            }
            return Some(());
        }
        if self.command && plain && self.skip_parens() {
            self.simple = false;
            self.words.push(Word {
                range: start..end,
                command: true,
                plain,
                function: true,
            });
            return Some(());
        }
        self.push(start..end, self.command, plain);
        self.command = false;
        Some(())
    }

    fn push(&mut self, range: Range<usize>, command: bool, plain: bool) {
        self.words.push(Word {
            range,
            command,
            plain,
            function: false,
        });
    }

    fn reserved(&mut self, name: &str) {
        self.command = COMMAND_NEXT.contains(&name);
        match name {
            "for" | "foreach" | "select" => self.mode = Mode::LoopNames,
            "case" => {
                self.cases += 1;
                self.mode = Mode::CaseWord;
            }
            "function" => self.mode = Mode::FunctionNames,
            "repeat" => self.mode = Mode::Count,
            "[[" => self.mode = Mode::Cond,
            "esac" => self.cases = self.cases.saturating_sub(1),
            _ => {}
        }
    }

    /// Skips `()` after a function name, blanks allowed before it.
    fn skip_parens(&mut self) -> bool {
        let mut at = self.pos;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        if self.bytes[at..].starts_with(b"()") {
            self.pos = at + 2;
            return true;
        }
        false
    }
}

/// `NAME=`, `NAME+=` or `NAME[...]=` at the start of a word.
fn assignment(text: &[u8]) -> bool {
    let name = text
        .iter()
        .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
        .count();
    if name == 0 || text[0].is_ascii_digit() {
        return false;
    }
    let mut rest = &text[name..];
    if rest.first() == Some(&b'[') {
        let Some(close) = rest.iter().position(|byte| *byte == b']') else {
            return false;
        };
        rest = &rest[close + 1..];
    }
    rest.starts_with(b"=") || rest.starts_with(b"+=")
}

/// The end of the word at `start`, and whether it is plain text; `None` when
/// a quote or substitution does not close.
fn word_end(bytes: &[u8], start: usize) -> Option<(usize, bool)> {
    let mut pos = start;
    let mut plain = true;
    let mut special = false;
    while let Some(&byte) = bytes.get(pos) {
        match byte {
            b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b')' => break,
            b'<' | b'>' if bytes.get(pos + 1) == Some(&b'(') => {
                plain = false;
                pos = balanced(bytes, pos + 1)?;
            }
            b'<' | b'>' => break,
            b'(' if pos == start || bytes.get(pos + 1) == Some(&b')') => break,
            b'(' => {
                plain = false;
                pos = balanced(bytes, pos)?;
            }
            b'\\' => {
                plain = false;
                pos += 2;
            }
            b'\'' => {
                plain = false;
                pos += 1 + bytes[pos + 1..].iter().position(|byte| *byte == b'\'')? + 1;
            }
            b'"' => {
                plain = false;
                pos = double_quoted(bytes, pos)?;
            }
            b'`' => {
                plain = false;
                pos = backquoted(bytes, pos)?;
            }
            b'$' => {
                plain = false;
                pos = dollar(bytes, pos)?;
            }
            b'*' | b'?' | b'[' | b']' | b'{' | b'}' | b'=' | b'~' | b'#' | b'^' | b'!' => {
                special = true;
                pos += 1;
            }
            _ => pos += 1,
        }
    }
    let end = pos.min(bytes.len());
    let reserved = matches!(&bytes[start..end], b"{" | b"}" | b"!" | b"[[" | b"]]");
    Some((end, plain && (!special || reserved)))
}

/// The end of the parenthesized text opening at `open`.
fn balanced(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut pos = open;
    while let Some(&byte) = bytes.get(pos) {
        match byte {
            b'(' => {
                depth += 1;
                pos += 1;
            }
            b')' => {
                depth -= 1;
                pos += 1;
                if depth == 0 {
                    return Some(pos);
                }
            }
            b'\\' => pos += 2,
            b'\'' => pos += 1 + bytes[pos + 1..].iter().position(|byte| *byte == b'\'')? + 1,
            b'"' => pos = double_quoted(bytes, pos)?,
            b'`' => pos = backquoted(bytes, pos)?,
            b'$' if bytes.get(pos + 1) == Some(&b'\'') => pos = dollar(bytes, pos)?,
            _ => pos += 1,
        }
    }
    None
}

fn double_quoted(bytes: &[u8], open: usize) -> Option<usize> {
    let mut pos = open + 1;
    while let Some(&byte) = bytes.get(pos) {
        match byte {
            b'"' => return Some(pos + 1),
            b'\\' => pos += 2,
            b'`' => pos = backquoted(bytes, pos)?,
            b'$' => pos = dollar(bytes, pos)?,
            _ => pos += 1,
        }
    }
    None
}

fn backquoted(bytes: &[u8], open: usize) -> Option<usize> {
    let mut pos = open + 1;
    while let Some(&byte) = bytes.get(pos) {
        match byte {
            b'`' => return Some(pos + 1),
            b'\\' => pos += 2,
            _ => pos += 1,
        }
    }
    None
}

fn dollar(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at + 1) {
        Some(b'\'') => {
            let mut pos = at + 2;
            while let Some(&byte) = bytes.get(pos) {
                match byte {
                    b'\\' => pos += 2,
                    b'\'' => return Some(pos + 1),
                    _ => pos += 1,
                }
            }
            None
        }
        Some(b'(') => balanced(bytes, at + 1),
        Some(b'{') => {
            let mut depth = 0usize;
            let mut pos = at + 1;
            while let Some(&byte) = bytes.get(pos) {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(pos + 1);
                        }
                    }
                    b'\\' => pos += 1,
                    b'\'' => {
                        pos += bytes[pos + 1..].iter().position(|byte| *byte == b'\'')? + 1;
                    }
                    b'"' => pos = double_quoted(bytes, pos)? - 1,
                    b'`' => pos = backquoted(bytes, pos)? - 1,
                    b'$' if bytes.get(pos + 1) == Some(&b'(') => {
                        pos = balanced(bytes, pos + 1)? - 1
                    }
                    _ => {}
                }
                pos += 1;
            }
            None
        }
        _ => Some(at + 1),
    }
}

/// One alias expansion of a word: its replacement as zsh reads it and as
/// the bundle spells it, quoted against a second expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub expanded: String,
    pub protected: String,
}

impl Edit {
    pub fn shifted(&self, base: usize) -> Edit {
        Edit {
            range: self.range.start + base..self.range.end + base,
            ..self.clone()
        }
    }

    pub fn shifted_back(&self, base: usize) -> Edit {
        Edit {
            range: self.range.start - base..self.range.end - base,
            ..self.clone()
        }
    }
}

/// Why a statement's aliases are left as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    pub name: String,
    pub reason: &'static str,
}

#[derive(Clone, Copy)]
struct Position {
    command: bool,
    eligible: bool,
}

/// Whether a plain word of `text` names an alias here or one set before.
pub fn mentioned(text: &str, words: &[Word], table: &Table, seen: &Seen) -> bool {
    table.any
        || seen.any
        || words.iter().filter(|word| word.plain).any(|word| {
            let name = &text[word.range.clone()];
            let suffix = name.rfind('.').map(|dot| &name[dot + 1..]);
            table.names.contains_key(name)
                || seen.regular.contains(name)
                || seen.global.contains(name)
                || suffix.is_some_and(|suffix| {
                    table.suffixes.contains_key(suffix) || seen.suffixes.contains(suffix)
                })
        })
}

/// Whether any alias name appears in `text` at all, for text the word
/// reader does not follow.
pub fn mentioned_anywhere(text: &str, table: &Table, seen: &Seen) -> bool {
    table.any
        || seen.any
        || table
            .names
            .keys()
            .chain(table.suffixes.keys())
            .chain(seen.names())
            .any(|name| text.contains(name.as_str()))
}

impl Table {
    /// `definitions`, and the option when it is off.
    pub fn parse_definitions(&self, marker: &str) -> String {
        let mut out = self.definitions(marker);
        if self.option == Some(false) {
            out.push_str("setopt no_aliases\n");
        }
        out
    }
}

/// The expansions zsh makes in `text`, statements it parses at once with
/// the aliases in `table`, and the words to quote against `seen`.
pub fn plan(text: &str, words: &[Word], table: &Table, seen: &Seen) -> Result<Vec<Edit>, Left> {
    let mut edits = Vec::new();
    let mut eligible = false;
    for word in words {
        let position = Position {
            command: word.command,
            eligible: word.command || eligible,
        };
        eligible = false;
        if !word.plain {
            continue;
        }
        let name = &text[word.range.clone()];
        if word.function {
            if table.any || table.names.contains_key(name) {
                return Err(Left {
                    name: name.to_string(),
                    reason: "names a function",
                });
            }
            continue;
        }
        match expand(name, position, table, seen, &mut Vec::new())? {
            Some((expanded, protected, trailing)) => {
                eligible = trailing;
                edits.push(Edit {
                    range: word.range.clone(),
                    expanded,
                    protected,
                });
            }
            None if guarded(name, position, seen) => edits.push(Edit {
                range: word.range.clone(),
                expanded: name.to_string(),
                protected: format!("\\{name}"),
            }),
            None => {}
        }
    }
    Ok(edits)
}

/// The expansion of `name` here, as zsh reads it, as quoted, and whether
/// the next word may expand too.
fn expand(
    name: &str,
    position: Position,
    table: &Table,
    seen: &Seen,
    in_use: &mut Vec<String>,
) -> Result<Option<(String, String, bool)>, Left> {
    let left = |reason| {
        Err(Left {
            name: name.to_string(),
            reason,
        })
    };
    if in_use.iter().any(|used| used == name) {
        return Ok(None);
    }
    let suffix = name
        .rfind('.')
        .filter(|dot| *dot > 0 && *dot + 1 < name.len())
        .map(|dot| &name[dot + 1..]);
    let entry = table.names.get(name);
    let suffix_entry = suffix
        .filter(|_| position.command)
        .and_then(|suffix| table.suffixes.get(suffix));
    let candidate = table.any
        || entry.is_some_and(|entry| entry.global() != Some(false))
        || (position.eligible && entry.is_some())
        || (entry.is_none() && suffix_entry.is_some());
    if !candidate {
        return Ok(None);
    }
    match table.option {
        Some(false) => return Ok(None),
        None => return left("aliases option not known on every path"),
        Some(true) => {}
    }
    if table.any {
        return left("set by code the build does not follow");
    }
    let (text, suffixed) = match (entry, suffix_entry) {
        (Some(Entry::Unknown { .. }), _) | (None, Some(Entry::Unknown { .. })) => {
            return left("not known on every path");
        }
        (Some(Entry::Known { text, .. }), _) => (text.clone(), false),
        (None, Some(Entry::Known { text, .. })) => (text.clone(), true),
        (None, None) => return Ok(None),
    };
    let Some(words) = value_words(&text) else {
        return left("value is not one command of plain words");
    };
    let key = if suffixed {
        suffix.unwrap_or(name).to_string()
    } else {
        name.to_string()
    };
    in_use.push(key);
    let mut expanded = String::new();
    let mut protected = String::new();
    let mut at = 0;
    let mut eligible = false;
    let mut trailing = text.ends_with([' ', '\t']);
    for word in &words {
        let inner = Position {
            command: position.command && word.command,
            eligible: (position.eligible && word.command) || eligible,
        };
        eligible = false;
        let between = &text[at..word.range.start];
        expanded.push_str(between);
        protected.push_str(between);
        let piece = &text[word.range.clone()];
        let found = if word.plain {
            expand(piece, inner, table, seen, in_use)?
        } else {
            None
        };
        match found {
            Some((inner_expanded, inner_protected, inner_trailing)) => {
                expanded.push_str(&inner_expanded);
                protected.push_str(&inner_protected);
                eligible = inner_trailing;
                if word.range.end == text.len() {
                    trailing |= inner_trailing;
                }
            }
            None => {
                expanded.push_str(piece);
                if word.plain && guarded(piece, inner, seen) {
                    protected.push('\\');
                }
                protected.push_str(piece);
            }
        }
        at = word.range.end;
    }
    in_use.pop();
    expanded.push_str(&text[at..]);
    protected.push_str(&text[at..]);
    if suffixed {
        let original = if guarded(
            name,
            Position {
                command: false,
                eligible: false,
            },
            seen,
        ) {
            format!("\\{name}")
        } else {
            name.to_string()
        };
        expanded = format!("{expanded} {name}");
        protected = format!("{protected} {original}");
        trailing = false;
    }
    Ok(Some((expanded, protected, trailing)))
}

/// Whether a bundle read as text could expand `name` here.
fn guarded(name: &str, position: Position, seen: &Seen) -> bool {
    let suffix = name
        .rfind('.')
        .filter(|dot| *dot > 0)
        .map(|dot| &name[dot + 1..]);
    seen.global.contains(name)
        || (position.eligible && seen.regular.contains(name))
        || (position.command && suffix.is_some_and(|suffix| seen.suffixes.contains(suffix)))
}

/// `text` with each edit's replacement, `expanded` or `protected`.
pub fn apply(text: &str, edits: &[Edit], protected: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for edit in edits {
        out.push_str(&text[at..edit.range.start]);
        out.push_str(if protected {
            &edit.protected
        } else {
            &edit.expanded
        });
        at = edit.range.end;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
#[path = "../tests/unit/alias_tests.rs"]
mod tests;
