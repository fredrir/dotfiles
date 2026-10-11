//! Aliases as startup code sets them, in source order and deferred code's
//! included, and their expansion in the statements zsh parses with them.

use std::ops::Range;

use zshrs_parse::parser::{ZshCommand, ZshList};

use super::edits::Edits;
use super::ledger::{ANY, Key, Touch};
use super::walk::{self, Visit};
use super::{Compiler, Cx, Origin};
use crate::alias::{self, Kind, Left};
use crate::expand;
use crate::script::{self, Script};
use crate::wordcode::{self, Expansion};

/// What an alias expands to where a test shows zsh expanding one.
const MARKER: &str = "__zbv_alias";

/// Tables holding aliases; assigning one sets aliases the build cannot name.
const TABLES: &[&str] = &[
    "aliases",
    "galiases",
    "saliases",
    "dis_aliases",
    "dis_galiases",
    "dis_saliases",
];

impl Compiler {
    /// `alias`, `unalias`, `disable -a` and `enable -a`, as the alias table sees them.
    pub(super) fn alias_command(&mut self, command: &str, args: &[String]) {
        let mut flags = String::new();
        let mut operands = Vec::new();
        let mut ended = false;
        for arg in args {
            match arg.as_str() {
                "--" if !ended => ended = true,
                flag if !ended && flag.len() > 1 && flag.starts_with(['-', '+']) => {
                    flags.push_str(&flag[1..]);
                }
                _ => {
                    ended = true;
                    operands.push(arg);
                }
            }
        }
        let suffix = flags.contains('s');
        let values: Vec<Option<String>> = operands
            .iter()
            .map(|operand| expand::scalar(operand, &self.state))
            .collect();
        let table = &mut self.state.alias_table;
        let seen = &mut self.alias_seen;
        match command {
            "alias" if !flags.contains('L') => {
                let kind = match (suffix, flags.contains('g')) {
                    (true, _) => Kind::Suffix,
                    (false, true) => Kind::Global,
                    (false, false) => Kind::Regular,
                };
                for (operand, value) in operands.iter().zip(values) {
                    match value {
                        Some(value) => {
                            if let Some((name, text)) = value.split_once('=') {
                                table.define(kind, name, Some(text.to_string()), seen);
                            }
                        }
                        None => match literal_name(operand) {
                            Some(name) => table.define(kind, name, None, seen),
                            None => table.forget_all(seen),
                        },
                    }
                }
            }
            "unalias" if flags.contains('a') => table.remove_where(suffix, |_| true),
            "unalias" => {
                for value in values {
                    match (value, flags.contains('m')) {
                        (Some(pattern), true) => match glob::Pattern::new(&pattern) {
                            Ok(pattern) => table.remove_where(suffix, |name| pattern.matches(name)),
                            Err(_) => table.forget_all(seen),
                        },
                        (Some(name), false) => table.remove(suffix, &name),
                        (None, _) => table.forget_all(seen),
                    }
                }
            }
            "disable" | "enable" if flags.contains('a') => {
                for value in values {
                    match value {
                        Some(name) => table.forget(&name, seen),
                        None => table.forget_all(seen),
                    }
                }
            }
            _ => {}
        }
    }

    /// The `alias` and `unalias` commands in the body of functions `names`.
    pub(super) fn record_alias_body(&mut self, names: &[String], body: &[ZshList]) {
        let commands = alias_commands(body);
        for name in names {
            match commands.is_empty() {
                true => self.alias_bodies.remove(name),
                false => self.alias_bodies.insert(name.clone(), commands.clone()),
            };
        }
    }

    /// A call of a function that sets aliases, such as one naming them by
    /// its arguments; false when a name cannot be told.
    pub(super) fn alias_call(&mut self, name: &str, args: &[String]) -> bool {
        // Inlined by the build, which follows the aliases the file sets
        if name == "_omz_source" && self.helper_ok("_omz_source") && !self.state.omz_alias_styles {
            return true;
        }
        let Some(commands) = self.alias_bodies.get(name).cloned() else {
            return false;
        };
        let values: Vec<Option<String>> = args
            .iter()
            .map(|arg| expand::scalar(arg, &self.state))
            .collect();
        self.alias_effects(&commands, |raw, state| {
            positional(raw, &values).or_else(|| expand::scalar(raw, state))
        })
    }

    /// `alias` and `unalias` in code the compiler does not follow.
    pub(super) fn alias_unfollowed(&mut self, lists: &[ZshList]) {
        let commands = alias_commands(lists);
        if !commands.is_empty()
            && !self.alias_effects(&commands, |raw, state| expand::scalar(raw, state))
        {
            self.state.alias_table.forget_all(&mut self.alias_seen);
        }
    }

    /// What `commands` may set, each named by `resolve`, as unknown: code
    /// may run them or not; false when a name cannot be told.
    fn alias_effects(
        &mut self,
        commands: &[Vec<String>],
        resolve: impl Fn(&str, &crate::state::State) -> Option<String>,
    ) -> bool {
        let mut names = Vec::new();
        for words in commands {
            let Some((command, operands)) = words.split_first() else {
                continue;
            };
            let mut flags = String::new();
            let mut ended = false;
            for operand in operands {
                if !ended && operand.len() > 1 && operand.starts_with(['-', '+']) {
                    ended |= operand == "--";
                    flags.push_str(&operand[1..]);
                    continue;
                }
                ended = true;
                let raw = match command.as_str() {
                    _ if flags.contains('L') => continue,
                    "alias" => match unquoted_split(operand) {
                        Some(raw) => raw,
                        None => continue,
                    },
                    _ if flags.contains(['a', 'm']) => return false,
                    _ => operand.as_str(),
                };
                let kind = match (command.as_str(), flags.contains('s'), flags.contains('g')) {
                    ("unalias", suffix, _) => Err(suffix),
                    (_, true, _) => Ok(Kind::Suffix),
                    (_, false, true) => Ok(Kind::Global),
                    _ => Ok(Kind::Regular),
                };
                match resolve(raw, &self.state) {
                    Some(alias) => names.push((alias, kind)),
                    None => return false,
                }
            }
        }
        for (alias, kind) in names {
            let table = &mut self.state.alias_table;
            match kind {
                Ok(kind) => table.define(kind, &alias, None, &mut self.alias_seen),
                Err(suffix) => table.maybe_remove(suffix, &alias),
            }
        }
        true
    }

    /// `setopt`, `unsetopt` and `emulate`, for the `aliases` option.
    pub(super) fn alias_option(&mut self, command: &str, args: &[String]) {
        let values: Vec<Option<String>> = args
            .iter()
            .map(|arg| expand::scalar(arg, &self.state))
            .collect();
        let option = &mut self.state.alias_table.option;
        match command {
            "setopt" | "unsetopt" => {
                for value in &values {
                    let Some(value) = value else {
                        *option = None;
                        continue;
                    };
                    if value.starts_with(['-', '+']) {
                        continue;
                    }
                    let name: String = value
                        .chars()
                        .filter(|c| *c != '_')
                        .flat_map(char::to_lowercase)
                        .collect();
                    let set = match name.as_str() {
                        "aliases" => true,
                        "noaliases" => false,
                        _ => continue,
                    };
                    *option = Some(set == (command == "setopt"));
                }
            }
            "emulate" => {
                let words: Vec<&str> = values.iter().flatten().map(String::as_str).collect();
                if words.len() != values.len() || words.contains(&"-c") {
                    return;
                }
                if words.iter().any(|word| !word.starts_with(['-', '+'])) {
                    *option = Some(true);
                }
                let mut rest = words.iter();
                while let Some(word) = rest.next() {
                    if matches!(*word, "-o" | "+o") {
                        let name = rest.next().map(|name| name.replace('_', "").to_lowercase());
                        match name.as_deref() {
                            Some("aliases") => *option = Some(*word == "-o"),
                            Some("noaliases") => *option = Some(*word == "+o"),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Aliases and the `aliases` option code may change that the build reads
    /// only from its text: a call, or code it does not follow.
    pub(super) fn alias_touches(&mut self, touches: &[(Touch, Key)]) {
        for (touch, (kind, name)) in touches {
            if *touch == Touch::Call {
                continue;
            }
            let table = &mut self.state.alias_table;
            match *kind {
                "alias" if self.alias_call_handled => {}
                "alias" if name == ANY => table.forget_all(&mut self.alias_seen),
                "alias" => table.forget(name, &mut self.alias_seen),
                "option" if name == ANY || name == "aliases" => table.option = None,
                "var" if TABLES.contains(&name.as_str()) => table.forget_all(&mut self.alias_seen),
                "var" if name == "options" => table.option = None,
                _ => {}
            }
        }
    }

    /// Joins the alias tables a file or function left through `return`.
    pub(super) fn join_alias_exits(&mut self) {
        for table in self.alias_exits.pop().unwrap_or_default() {
            self.state.alias_table.merge(&table);
        }
    }

    /// Expands the aliases zsh would in the statements at `range`, outside
    /// what was replaced or handled since `mark`, and quotes words a bundle
    /// read as text could expand again; leaves them when zsh reads the
    /// result otherwise.
    pub(super) fn alias_segment(
        &mut self,
        script: &Script,
        range: Range<usize>,
        mark: (usize, usize, usize),
        edits: &mut Edits,
        cx: &Cx,
    ) {
        if cx.analysis || self.dead > 0 {
            return;
        }
        let handled = edits.handled_since(script, mark);
        edits.alias_done.push(range.clone());
        if handled
            .iter()
            .any(|done| done.start <= range.start && range.end <= done.end)
        {
            return;
        }
        let nested: Vec<alias::Edit> = edits
            .aliases
            .iter()
            .filter(|edit| range.start <= edit.range.start && edit.range.end <= range.end)
            .map(|edit| edit.shifted_back(range.start))
            .collect();
        let text = &script.text[range.clone()];
        let outside = |edit: &alias::Edit| {
            let edit = edit.range.start + range.start..edit.range.end + range.start;
            !handled
                .iter()
                .any(|done| done.start < edit.end && edit.start < done.end)
        };
        match self.alias_rewrite(text, &nested, outside) {
            Ok(found) => edits
                .aliases
                .extend(found.iter().map(|edit| edit.shifted(range.start))),
            Err(left) => {
                let line = script.text[..range.start].matches('\n').count() + 1;
                self.alias_left(cx, line, left);
            }
        }
    }

    /// `text`, a statement generated from source words, with its aliases expanded.
    pub(super) fn alias_piece(&mut self, text: String, cx: &Cx) -> String {
        if cx.analysis || self.dead > 0 {
            return text;
        }
        match self.alias_rewrite(&text, &[], |_| true) {
            Ok(found) => alias::apply(&text, &found, true),
            Err(left) => {
                let line = self.line;
                self.alias_left(cx, line, left);
                text
            }
        }
    }

    /// The expansions to add to `text`, statements zsh parses at once, past
    /// the `nested` ones already in it and those `keep` rejects.
    pub(super) fn alias_rewrite(
        &mut self,
        text: &str,
        nested: &[alias::Edit],
        keep: impl Fn(&alias::Edit) -> bool,
    ) -> Result<Vec<alias::Edit>, Left> {
        let words = alias::words(text);
        let mentioned = match &words {
            Some(words) => alias::mentioned(text, words, &self.alias_parse, &self.alias_seen),
            None => alias::mentioned_anywhere(text, &self.alias_parse, &self.alias_seen),
        };
        if !mentioned && nested.is_empty() {
            return Ok(Vec::new());
        }
        let Some(zsh) = self.zsh.clone() else {
            return Err(Left {
                name: String::new(),
                reason: "zsh not found",
            });
        };
        let found: Vec<alias::Edit> = match &words {
            Some(words) => alias::plan(text, words, &self.alias_parse, &self.alias_seen)?
                .into_iter()
                .filter(|edit| keep(edit))
                .collect(),
            None => Vec::new(),
        };
        let mut all: Vec<alias::Edit> = nested
            .iter()
            .cloned()
            .chain(found.iter().cloned())
            .collect();
        all.sort_by_key(|edit| edit.range.start);
        let expanded = alias::apply(text, &all, false);
        let protected = alias::apply(text, &all, true);
        let reparses = protected == text
            || script::parse(&protected).is_ok_and(|program| {
                script::parse(text).is_ok_and(|original| {
                    program.lists.len() == original.lists.len()
                        && program
                            .lists
                            .iter()
                            .zip(&original.lists)
                            .all(|(left, right)| script::shape(left) == script::shape(right))
                })
            });
        let expansion = Expansion {
            aliases: &self.alias_parse.parse_definitions(MARKER),
            original: text,
            expanded: &expanded,
            seen: &self.alias_seen.definitions(MARKER),
            protected: &protected,
        };
        if !reparses || !wordcode::same_expansion(&zsh, &expansion, &mut self.folder.cache) {
            let reason = match words {
                Some(_) => "zsh reads the expansion otherwise",
                None => "statement not read by the build",
            };
            return Err(Left {
                name: String::new(),
                reason,
            });
        }
        Ok(found)
    }

    /// Reports aliases left as they are, once per alias or place and reason.
    fn alias_left(&mut self, cx: &Cx, line: usize, left: Left) {
        let place = match cx.source {
            Some(Origin::File(file)) => format!("{file}:{line}"),
            Some(Origin::Fixed(location)) => location.to_string(),
            None => cx.origin.map_or_else(
                || "generated code".to_string(),
                |origin| origin.display().to_string(),
            ),
        };
        let file = place
            .rsplit_once(':')
            .map_or(place.as_str(), |(file, _)| file);
        let key = match left.name.is_empty() {
            true => file.to_string(),
            false => left.name.clone(),
        };
        if !self.alias_notes.insert((key, left.reason)) {
            return;
        }
        let subject = match left.name.is_empty() {
            true => "aliases".to_string(),
            false => format!("alias {}", left.name),
        };
        let note = format!("{place}: {subject}: {}; not expanded", left.reason);
        let own = match cx.origin {
            Some(origin) => origin.starts_with(&self.root),
            None => !file.starts_with('/') || std::path::Path::new(file).starts_with(&self.root),
        };
        if own {
            self.note(note);
        } else if !self.skipped.contains(&note) {
            self.skipped.push(note);
        }
    }
}

/// An alias operand's name before `=`, when it is written literally.
fn literal_name(operand: &str) -> Option<&str> {
    let (name, _) = operand.split_once('=')?;
    (!name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !b"'\"\\$`".contains(&byte)))
    .then_some(name)
}

/// The text before the first unquoted `=` of an alias operand.
fn unquoted_split(operand: &str) -> Option<&str> {
    let mut quote = None;
    for (index, byte) in operand.bytes().enumerate() {
        match (quote, byte) {
            (None, b'\'' | b'"') => quote = Some(byte),
            (Some(open), _) if byte == open => quote = None,
            (None, b'=') => return Some(&operand[..index]),
            _ => {}
        }
    }
    None
}

/// `$N`, `${N}` or either in double quotes, as the call's argument `N`.
fn positional(raw: &str, values: &[Option<String>]) -> Option<String> {
    let raw = raw
        .strip_prefix('"')
        .and_then(|raw| raw.strip_suffix('"'))
        .unwrap_or(raw);
    let digits = raw.strip_prefix('$')?;
    let digits = digits
        .strip_prefix('{')
        .and_then(|digits| digits.strip_suffix('}'))
        .unwrap_or(digits);
    let index: usize = digits.parse().ok()?;
    match values.get(index.checked_sub(1)?) {
        Some(value) => value.clone(),
        None => Some(String::new()),
    }
}

/// The words of each `alias` and `unalias` command `lists` run, outside nested function bodies.
fn alias_commands(lists: &[ZshList]) -> Vec<Vec<String>> {
    let mut visits = Vec::new();
    walk::visit(lists, 0, &mut visits);
    visits
        .iter()
        .filter_map(|(_, visit)| match visit {
            Visit::Command(ZshCommand::Simple(simple)) => Some(walk::words(simple)),
            _ => None,
        })
        .filter(|words| matches!(words.first().map(String::as_str), Some("alias" | "unalias")))
        .collect()
}
