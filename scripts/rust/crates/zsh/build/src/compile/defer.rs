//! Code run after the first prompt through the dotfiles' `defer`: oh-my-zsh
//! plugins, `cached_eval` output, sourced files and oh-my-zsh's compinit
//! section, each wrapped in a function and queued at its original place.
//!
//! Queued code runs after the rest of startup, so a unit stays in place when
//! that code changes or calls what it defines, or when it would mean something
//! else inside a function.

use std::ops::{Range, RangeInclusive};
use std::path::Path;

use glob::Pattern;
use zshrs_parse::parser::{ZshCommand, ZshList};

use super::edits::Edits;
use super::ledger::{COMPS, Names, Touch, Unit, local_option};
use super::walk::{self, Visit};
use super::{Compiler, Cx, Mode, rules};
use crate::config;
use crate::scan;
use crate::script::{self, Script};

/// True when a prompt follows; otherwise deferred code runs in place.
const PROMPT: &str = "[[ -o zle && ! -v ZSH_EXECUTION_STRING && ! -v ZSH_SCRIPT ]]";

/// Before a deferred `compinit`: the `fpath` it sees, and `compdef` queued in
/// order with the deferred code until `compinit` defines it.
const QUEUE: &str = "typeset -ga __zb_fpath=(\"${fpath[@]}\")\n\
compdef() { defer \"compdef ${(j: :)${(@q)@}}\" }\n";

const FPATH: &str = "local -a fpath=(\"${__zb_fpath[@]}\")\nunset __zb_fpath\n";

/// Without a `compdef` from `compinit`, queued calls fail as they did before it.
const UNQUEUE: &str = "[[ $functions[compdef] == *'defer \"compdef '* ]] && unfunction compdef\n";

/// What `[defer]` selects.
#[derive(Debug, Default)]
pub struct Deferral {
    compinit: bool,
    plugins: Vec<Pattern>,
    evals: Vec<Pattern>,
    files: Vec<Pattern>,
}

impl Deferral {
    pub fn new(config: &config::Defer, root: &Path) -> Result<Self, String> {
        let patterns = |values: &[String], root: Option<&Path>| {
            values
                .iter()
                .map(|value| {
                    let value = root.map_or_else(
                        || value.clone(),
                        |root| root.join(value).to_string_lossy().into_owned(),
                    );
                    Pattern::new(&value).map_err(|error| format!("defer: {value}: {error}"))
                })
                .collect::<Result<Vec<_>, String>>()
        };
        Ok(Self {
            compinit: config.compinit,
            plugins: patterns(&config.plugins, None)?,
            evals: patterns(&config.evals, None)?,
            files: patterns(&config.files, Some(root))?,
        })
    }

    /// An `_omz_source` path, `plugins/<name>/<name>.plugin.zsh`.
    pub fn plugin(&self, relative: &str) -> bool {
        relative
            .strip_prefix("plugins/")
            .and_then(|rest| rest.split('/').next())
            .is_some_and(|name| self.plugins.iter().any(|pattern| pattern.matches(name)))
    }

    pub fn eval(&self, name: &str) -> bool {
        self.evals.iter().any(|pattern| pattern.matches(name))
    }

    pub fn file(&self, path: &Path) -> bool {
        self.files.iter().any(|pattern| pattern.matches_path(path))
    }
}

/// oh-my-zsh's compinit section: top-level statements by index and the lines they own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub lists: Range<usize>,
    pub lines: RangeInclusive<usize>,
}

impl Compiler {
    /// Compiles a unit with nested deferral off, then queues it when `wanted`.
    pub(super) fn deferred(
        &mut self,
        wanted: bool,
        place: &Path,
        cx: &Cx,
        inline: impl FnOnce(&mut Self) -> Option<String>,
    ) -> Option<String> {
        if !(wanted && self.may_defer(cx)) {
            return inline(self);
        }
        self.defer_suspended += 1;
        self.ledger.collecting = Some(Names::default());
        let text = inline(self);
        let names = self.ledger.collecting.take().unwrap_or_default();
        self.defer_suspended -= 1;
        let Some(text) = text else {
            self.record_in_place(&names);
            return None;
        };
        let (reason, compinit) =
            script::parse(&text).map_or((Some("compiled code does not parse"), false), |program| {
                let reason =
                    hazard(&program.lists).or_else(|| scope_hazard(&names, &program.lists));
                (reason, calls_compinit(&program.lists))
            });
        if let Some(reason) = reason {
            self.kept_in_place(place, reason);
            self.record_in_place(&names);
            return Some(text);
        }
        let (head, tail) = self.wrapper(&text, compinit, place, names);
        Some(format!("{head}{text}{tail}"))
    }

    /// Top-level code outside other deferred code, with `defer` defined as expected.
    fn may_defer(&mut self, cx: &Cx) -> bool {
        if cx.analysis
            || cx.mode == Mode::Function
            || self.state.in_function()
            || self.defer_suspended > 0
        {
            return false;
        }
        let Some(function) = self.state.functions.get("defer") else {
            self.note("defer: not defined; [defer] code runs in place".to_string());
            return false;
        };
        if rules::expected("defer") == Some(function.fingerprint) {
            return true;
        }
        let note = format!(
            "defer: unrecognized definition {:#018x}; [defer] code runs in place",
            function.fingerprint
        );
        self.note(note);
        false
    }

    /// The function header and the closing text that queues it.
    fn wrapper(
        &mut self,
        body: &str,
        compinit: bool,
        place: &Path,
        mut names: Names,
    ) -> (String, String) {
        let name = format!("__zb_defer_{}", self.ledger.units.len() + 1);
        let newline = if body.is_empty() || body.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        let (fpath, unqueue, queue) = if compinit {
            self.ledger.compinit = Some(self.ledger.units.len());
            names.defined.insert(("var", COMPS.to_string()));
            (FPATH, UNQUEUE, QUEUE)
        } else {
            ("", "", "")
        };
        let head = format!("{name}() {{\n{fpath}");
        let tail = format!(
            "{newline}{unqueue}}}\n{queue}if {PROMPT}; then\n  defer '{name}; unfunction {name}'\nelse\n  {name}; unfunction {name}\nfi\n"
        );
        self.ledger.units.push(Unit {
            head: head.clone(),
            tail: tail.clone(),
            place: place.to_path_buf(),
            names,
            after: self.ledger.events.len(),
        });
        (head, tail)
    }

    fn kept_in_place(&mut self, place: &Path, reason: &str) {
        let note = format!("{}: {reason}; not deferred", place.display());
        if place.starts_with(&self.root) {
            self.note(note);
        } else if !self.skipped.contains(&note) {
            self.skipped.push(note);
        }
    }

    /// Unwraps each unit that code running before it depends on, until none
    /// does, and counts what stays deferred.
    pub fn settle_deferral(&mut self, mut text: String) -> String {
        let units = std::mem::take(&mut self.ledger.units);
        let mut kept = vec![false; units.len()];
        let mut changed = true;
        while changed {
            changed = false;
            for (index, unit) in units.iter().enumerate() {
                if kept[index] {
                    continue;
                }
                let later_units = units
                    .iter()
                    .zip(&kept)
                    .filter(|(other, kept)| **kept && other.after > unit.after)
                    .flat_map(|(other, _)| other.names.touches());
                let later_events = self.ledger.events[unit.after..].iter().filter_map(
                    |(touch, key)| match touch {
                        Touch::Queued(compinit) if !kept[*compinit] => None,
                        Touch::Queued(_) => Some((Touch::Define, key)),
                        touch => Some((*touch, key)),
                    },
                );
                let conflict = later_events
                    .chain(later_units)
                    .find_map(|(touch, key)| unit.names.conflict(touch, key));
                if let Some(reason) = conflict {
                    self.kept_in_place(&unit.place, &reason);
                    kept[index] = true;
                    changed = true;
                }
            }
        }
        for (unit, kept) in units.iter().zip(&kept) {
            if *kept {
                text = text.replacen(&unit.head, "", 1).replacen(&unit.tail, "", 1);
            }
        }
        self.deferred = kept.iter().filter(|kept| !**kept).count();
        text
    }

    /// The statements between oh-my-zsh's compdump path and `_omz_source`,
    /// when `[defer] compinit` asks for them and they own whole lines.
    pub(super) fn compinit_window(&mut self, script: &Script, cx: &Cx) -> Option<Window> {
        let origin = cx.origin?;
        if !self.deferral.compinit
            || origin.file_name().is_none_or(|name| name != "oh-my-zsh.sh")
            || !self.may_defer(cx)
        {
            return None;
        }
        let lists = &script.program.lists;
        let Some(range) = window_lists(lists).filter(|range| calls_compinit(&lists[range.clone()]))
        else {
            self.kept_in_place(origin, "compinit section not found");
            return None;
        };
        if walk::file_returns(&lists[range.clone()]) != Some(0) {
            self.kept_in_place(origin, "return in the compinit section");
            return None;
        }
        let Some(lines) = owned_lines(script, &range) else {
            self.kept_in_place(origin, "compinit section shares lines");
            return None;
        };
        Some(Window {
            lists: range,
            lines,
        })
    }

    /// Starts compiling a window's statements as deferred code.
    pub(super) fn open_window(&mut self) {
        self.defer_suspended += 1;
        self.ledger.collecting = Some(Names::default());
    }

    /// Wraps a compiled window, unless its compiled text would mean something
    /// else inside a function.
    pub(super) fn wrap_window(
        &mut self,
        script: &Script,
        window: &Window,
        edits: &mut Edits,
        origin: &Path,
    ) {
        self.defer_suspended -= 1;
        let names = self.ledger.collecting.take().unwrap_or_default();
        let bytes = script.byte_range(&window.lines);
        let text = edits.render_within(script, bytes.clone());
        let reason = match script::parse(&text) {
            Ok(program) => hazard(&program.lists)
                .or_else(|| names_zero(&text))
                .or_else(|| scope_hazard(&names, &program.lists)),
            Err(_) => Some("compiled section does not parse"),
        };
        if let Some(reason) = reason {
            self.kept_in_place(origin, reason);
            self.record_in_place(&names);
            return;
        }
        let (head, tail) = self.wrapper(&text, true, origin, names);
        edits.replace_bytes(bytes.start..bytes.start, head);
        edits.replace_bytes(bytes.end..bytes.end, tail);
    }
}

/// What deferral itself would break: the first prompt's hooks, or the
/// `fpath` a deferred `compinit` sees.
fn scope_hazard(names: &Names, lists: &[ZshList]) -> Option<&'static str> {
    if names.defines_var("precmd_functions") {
        return Some("precmd hook");
    }
    (names.defines_var("fpath") && calls_compinit(lists)).then_some("fpath set with compinit")
}

/// From the statement after the last top-level `ZSH_COMPDUMP` assignment to `_omz_source`.
pub fn window_lists(lists: &[ZshList]) -> Option<Range<usize>> {
    let end = lists.iter().position(|list| {
        matches!(walk::sole(list), Some(ZshCommand::FuncDef(node))
            if node.names.iter().any(|name| walk::text(name) == "_omz_source"))
    })?;
    let start = lists[..end].iter().rposition(|list| {
        walk::assigned(std::slice::from_ref(list))
            .iter()
            .any(|name| name == "ZSH_COMPDUMP")
    })? + 1;
    (start < end).then_some(start..end)
}

/// The lines `range` owns when its statements start and end whole segments.
fn owned_lines(script: &Script, range: &Range<usize>) -> Option<RangeInclusive<usize>> {
    let mut index = 0;
    let mut first = None;
    let mut last = None;
    for segment in script.segments(&script.program.lists, script.line_count()) {
        let covered = segment
            .as_ref()
            .map_or(index..index + 1, |segment| segment.lists.clone());
        let inside = range.start <= covered.start && covered.end <= range.end;
        let straddles = covered.start < range.end && range.start < covered.end && !inside;
        if straddles || (inside && segment.is_none()) {
            return None;
        }
        if let Some(segment) = segment.filter(|_| inside) {
            first.get_or_insert(*segment.lines.start());
            last = Some(*segment.lines.end());
        }
        index = covered.end;
    }
    Some(first?..=last?)
}

/// Why top-level code would mean something else inside a function, if it would.
pub fn hazard(lists: &[ZshList]) -> Option<&'static str> {
    let mut visits = Vec::new();
    walk::visit(lists, 0, &mut visits);
    visits.into_iter().find_map(|(depth, visit)| {
        let Visit::Command(ZshCommand::Simple(simple)) = visit else {
            return None;
        };
        let words = walk::words(simple);
        let (name, args) = words.split_first()?;
        let flags: String = args
            .iter()
            .filter(|arg| arg.starts_with(['-', '+']))
            .flat_map(|arg| arg.chars().skip(1))
            .collect();
        let named = args.iter().any(|arg| !arg.starts_with(['-', '+']));
        match name.as_str() {
            "local" => Some("local outside a function"),
            "typeset" | "declare" | "readonly" | "integer" | "float"
                if named && !flags.contains(['g', 'f', 'p']) =>
            {
                Some("declaration without -g")
            }
            "trap" if named => Some("trap"),
            "emulate" if flags.contains('L') => Some("local options"),
            "setopt" | "unsetopt" if args.iter().any(|arg| local_option(arg)) => {
                Some("local options")
            }
            "return" => Some("return"),
            "break" | "continue" if depth == 0 => Some("loop control outside a loop"),
            _ => None,
        }
    })
}

fn names_zero(text: &str) -> Option<&'static str> {
    let scan = scan::scan(text);
    (!scan.zero_refs.is_empty() || !scan.unsupported_zero.is_empty()).then_some("$0 reference")
}

/// Whether running `lists` calls `compinit`, anonymous functions included.
pub fn calls_compinit(lists: &[ZshList]) -> bool {
    let mut visits = Vec::new();
    walk::visit(lists, 0, &mut visits);
    visits.iter().any(|(_, visit)| match visit {
        Visit::Command(ZshCommand::Simple(simple)) => walk::words(simple)
            .first()
            .is_some_and(|word| word == "compinit"),
        Visit::Command(ZshCommand::FuncDef(node)) if node.auto_call_args.is_some() => {
            calls_compinit(&node.body.lists)
        }
        _ => false,
    })
}

#[cfg(test)]
#[path = "../../tests/unit/compile/defer_tests.rs"]
mod tests;
