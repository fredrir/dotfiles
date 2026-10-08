//! Walks startup code in execution order, tracking build-time state, and
//! rewrites what that state decides: sources, helpers and folded commands.

mod edits;
mod rules;
pub mod walk;

use std::collections::BTreeSet;
use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};

use zshrs_parse::parser::{
    SublistOp, ZshAssign, ZshAssignValue, ZshCommand, ZshCond, ZshFor, ZshFuncDef, ZshIf, ZshList,
    ZshProgram, ZshSimple,
};

use crate::expand::{self, Env, Mode as Expand, Var};
use crate::fold::{Constants, Folder};
use crate::quote;
use crate::scan::{self, ZeroForm, ZeroRef};
use crate::script::{self, Script, Segment};
use crate::state::{Function, State};
use edits::Edits;

const OMIT: &str = "# zsh-build: omit";

/// How inlined code keeps the meaning of `return` and local declarations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The bundle itself; `return` leaves the bundle as it left `.zshrc`.
    Root,
    /// A sourced file; `return` leaves only that file.
    Plain,
    /// Code a helper function sourced; it runs in a function scope.
    Function,
}

#[derive(Clone, Copy)]
pub struct Cx<'a> {
    pub mode: Mode,
    pub origin: Option<&'a Path>,
    pub analysis: bool,
}

pub struct Compiler {
    pub state: State,
    pub folder: Folder,
    pub constants: Constants,
    /// Problems the dotfiles can fix.
    pub warnings: Vec<String>,
    /// Third-party code left to runtime.
    pub skipped: Vec<String>,
    pub inlined: usize,
    pub folded: usize,
    stack: Vec<PathBuf>,
    root: PathBuf,
}

struct Snapshot {
    state: State,
    constants: usize,
    warnings: usize,
    skipped: usize,
    inlined: usize,
    folded: usize,
}

impl Compiler {
    pub fn new(state: State, folder: Folder, root: PathBuf) -> Self {
        Self {
            state,
            folder,
            constants: Constants::default(),
            warnings: Vec::new(),
            skipped: Vec::new(),
            inlined: 0,
            folded: 0,
            stack: Vec::new(),
            root,
        }
    }

    /// Runs a startup file for its effect on state only.
    pub fn analyze(&mut self, path: &Path) -> Result<(), String> {
        let script = read(path)?;
        self.compile(
            &script,
            &Cx {
                mode: Mode::Root,
                origin: Some(path),
                analysis: true,
            },
        )
        .map(|_| ())
    }

    pub fn root(&mut self, path: &Path) -> Result<String, String> {
        let script = read(path)?;
        self.stack.push(path.to_path_buf());
        let result = self.compile(
            &script,
            &Cx {
                mode: Mode::Root,
                origin: None,
                analysis: false,
            },
        );
        self.stack.pop();
        result.map_err(|reason| format!("{}: {reason}", path.display()))
    }

    pub fn note(&mut self, note: String) {
        if !self.warnings.contains(&note) {
            self.warnings.push(note);
        }
    }

    /// Code that stays a runtime `source`: a warning when it is the dotfiles' own.
    pub fn left_to_runtime(&mut self, origin: Option<&Path>, reason: &str) {
        let place = origin.map_or_else(
            || "generated code".to_string(),
            |path| path.display().to_string(),
        );
        let note = format!("{place}: {reason}; left to runtime");
        if origin.is_none_or(|path| path.starts_with(&self.root)) {
            self.note(note);
        } else if !self.skipped.contains(&note) {
            self.skipped.push(note);
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state.clone(),
            constants: self.constants.len(),
            warnings: self.warnings.len(),
            skipped: self.skipped.len(),
            inlined: self.inlined,
            folded: self.folded,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.state = snapshot.state;
        self.constants.truncate(snapshot.constants);
        self.warnings.truncate(snapshot.warnings);
        self.skipped.truncate(snapshot.skipped);
        self.inlined = snapshot.inlined;
        self.folded = snapshot.folded;
    }

    pub fn compile(&mut self, script: &Script, cx: &Cx) -> Result<String, String> {
        let mut edits = Edits::default();
        self.lists(
            script,
            &script.program.lists,
            script.line_count(),
            true,
            &mut edits,
            cx,
        );
        if cx.analysis {
            return Ok(String::new());
        }
        let returns = match cx.mode {
            Mode::Plain => {
                walk::file_returns(&script.program.lists).ok_or("return inside a subshell")?
            }
            _ => 0,
        };
        if edits.returns_rewritten != returns {
            return Err("top-level return not rewritable".to_string());
        }
        let scan = scan::scan(&script.text);
        if scan.unterminated && cx.mode != Mode::Root {
            return Err("unbalanced quoting".to_string());
        }
        let stray_prompt = scan.zero_refs.iter().any(|zero| match zero.form {
            ZeroForm::Prompt { file } => {
                !edits.zero_done.contains(&zero.range.start)
                    && (file || !edits.in_function(zero.range.start))
            }
            ZeroForm::Name { .. } => false,
        });
        if stray_prompt {
            return Err("prompt file reference not rewritable".to_string());
        }
        let names_zero = !scan.unsupported_zero.is_empty()
            || scan
                .zero_refs
                .iter()
                .any(|zero| matches!(zero.form, ZeroForm::Name { .. }));
        let zero = match (names_zero, cx.mode, cx.origin) {
            (false, _, _) => None,
            (true, Mode::Root, _) | (true, _, None) => {
                return Err("$0 reference with no source file".to_string());
            }
            (true, _, Some(origin)) => Some(quote::word(&origin.to_string_lossy())),
        };
        script::parse(&edits.skeleton(script))
            .map_err(|error| format!("rewritten code does not parse: {error}"))?;
        let mut text = edits.render(script);
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if returns > 0 {
            text = format!("repeat 1 do\n{text}done\n");
        }
        Ok(match (zero, cx.mode) {
            (Some(origin), Mode::Function) => format!("0={origin}\n{text}"),
            (Some(origin), _) => {
                let saved = format!("__zb0_{}", self.stack.len());
                format!("{saved}=$0 0={origin}\n{text}0=${saved}\n")
            }
            (None, _) => text,
        })
    }

    /// Returns whether every path through `lists` ends in `return` or `exit`.
    fn lists(
        &mut self,
        script: &Script,
        lists: &[ZshList],
        last: usize,
        owned: bool,
        edits: &mut Edits,
        cx: &Cx,
    ) -> bool {
        let segments = if owned {
            script.segments(lists, last)
        } else {
            vec![None; lists.len()]
        };
        let mut ended = false;
        let mut index = 0;
        for segment in segments {
            let dead = ended.then(|| self.state.clone());
            match segment {
                Some(segment) => {
                    let range = segment.lists.clone();
                    index = range.end;
                    if omitted(script, &segment) {
                        edits.replace_lines(segment.lines.clone(), String::new());
                        continue;
                    }
                    let before = edits.len();
                    if range.len() == 1 {
                        let (replacement, ends) = self.statement(
                            script,
                            &lists[range.start],
                            Some(&segment.lines),
                            edits,
                            cx,
                        );
                        ended |= ends;
                        match replacement {
                            Some(text) => {
                                edits.replace_lines(segment.lines.clone(), placeholder(text))
                            }
                            None if edits.len() == before => {
                                self.text_layer(script, &segment, &lists[range], edits, cx)
                            }
                            None => {}
                        }
                    } else {
                        for list in &lists[range.clone()] {
                            ended |= self.statement(script, list, None, edits, cx).1;
                        }
                        self.text_layer(script, &segment, &lists[range], edits, cx);
                    }
                }
                None => {
                    ended |= self.statement(script, &lists[index], None, edits, cx).1;
                    index += 1;
                }
            }
            if let Some(state) = dead {
                self.state = state;
            }
        }
        ended
    }

    fn statement(
        &mut self,
        script: &Script,
        list: &ZshList,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) -> (Option<String>, bool) {
        if list.flags.async_ {
            return (None, false);
        }
        if list.sublist.next.is_some() || list.sublist.flags.not {
            return self.chain(script, list, lines, edits, cx);
        }
        if list.sublist.pipe.next.is_some() {
            self.forget(std::slice::from_ref(list));
            return (None, false);
        }
        self.command(script, &list.sublist.pipe.cmd, lines, edits, cx)
    }

    fn command(
        &mut self,
        script: &Script,
        cmd: &ZshCommand,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) -> (Option<String>, bool) {
        let last = lines.map_or(0, |lines| *lines.end());
        match cmd {
            ZshCommand::Simple(simple) => self.simple(simple, lines.is_some(), cx),
            ZshCommand::If(node) => (None, self.branch(script, node, lines, edits, cx)),
            ZshCommand::For(node) => self.for_loop(script, node, lines, edits, cx),
            ZshCommand::Cursh(body) => (
                None,
                self.lists(script, &body.lists, last, lines.is_some(), edits, cx),
            ),
            ZshCommand::FuncDef(node) => {
                self.function(script, node, lines, edits, cx);
                (None, false)
            }
            other => {
                let list = list_of(other.clone());
                self.forget(std::slice::from_ref(&list));
                (None, false)
            }
        }
    }

    /// `test && command` and `test || command`, where the test may be decided now.
    fn chain(
        &mut self,
        script: &Script,
        list: &ZshList,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) -> (Option<String>, bool) {
        let sublist = &list.sublist;
        let simple_chain = sublist.pipe.next.is_none()
            && sublist.next.as_ref().is_some_and(|(_, rest)| {
                rest.next.is_none() && !rest.flags.not && rest.pipe.next.is_none()
            });
        let Some((operator, rest)) = sublist.next.as_ref().filter(|_| simple_chain) else {
            self.forget(std::slice::from_ref(list));
            return (None, false);
        };
        let truth = self
            .truth(&sublist.pipe.cmd)
            .map(|truth| truth != sublist.flags.not);
        let runs = match (operator, truth) {
            (SublistOp::And, Some(truth)) => Some(truth),
            (SublistOp::Or, Some(truth)) => Some(!truth),
            _ => None,
        };
        if truth.is_none() {
            self.forget(std::slice::from_ref(&list_of(sublist.pipe.cmd.clone())));
        }
        match runs {
            Some(true) => self.command(script, &rest.pipe.cmd, lines, edits, cx),
            Some(false) => (None, false),
            None => {
                let saved = self.state.clone();
                let ends = self.command(script, &rest.pipe.cmd, None, edits, cx).1;
                if ends {
                    self.state = saved;
                } else {
                    self.state.merge(&saved);
                }
                (None, false)
            }
        }
    }

    fn branch(
        &mut self,
        script: &Script,
        node: &ZshIf,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) -> bool {
        let last = lines.map_or(0, |lines| *lines.end());
        let owned = lines.is_some();
        let saved = self.state.clone();
        let mut arms: Vec<(Option<&ZshProgram>, &ZshProgram)> =
            vec![(Some(&node.cond), &node.then)];
        arms.extend(node.elif.iter().map(|(cond, body)| (Some(cond), body)));
        if let Some(body) = &node.else_ {
            arms.push((None, body));
        }
        let mut exits = Vec::new();
        let mut decided = false;
        for (cond, body) in arms {
            self.state = saved.clone();
            let taken = match cond {
                None => Some(true),
                Some(cond) => self.program_truth(cond),
            };
            if decided || taken == Some(false) {
                self.lists(script, &body.lists, last, owned, edits, cx);
                continue;
            }
            if !self.lists(script, &body.lists, last, owned, edits, cx) {
                exits.push(self.state.clone());
            }
            decided |= taken == Some(true);
        }
        if !decided {
            exits.push(saved.clone());
        }
        let Some((first, rest)) = exits.split_first() else {
            self.state = saved;
            return true;
        };
        let mut joined = first.clone();
        rest.iter().for_each(|state| joined.merge(state));
        self.state = joined;
        false
    }

    fn for_loop(
        &mut self,
        script: &Script,
        node: &ZshFor,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) -> (Option<String>, bool) {
        if lines.is_some() && !node.is_select {
            let values = walk::for_words(&node.list).and_then(|words| {
                words
                    .iter()
                    .map(|word| expand::words(word, &self.state, Expand::Args))
                    .collect::<Option<Vec<_>>>()
            });
            if let Some(text) = values.and_then(|values| self.unroll(node, &values.concat(), cx)) {
                return (Some(text), false);
            }
        }
        let saved = self.state.clone();
        self.state.forget(&node.var);
        let last = lines.map_or(0, |lines| *lines.end());
        self.lists(script, &node.body.lists, last, lines.is_some(), edits, cx);
        self.state.merge(&saved);
        self.state.forget(&node.var);
        (None, false)
    }

    /// A loop over known words whose body sources or helpers fold, as straight-line code.
    fn unroll(&mut self, node: &ZshFor, values: &[String], cx: &Cx) -> Option<String> {
        let body = &node.body.lists;
        if walk::contains_loop_control(body) {
            return None;
        }
        let simples: Vec<&ZshSimple> = body.iter().map(walk::sole_simple).collect::<Option<_>>()?;
        if !simples.iter().any(|simple| rules::folds(simple)) {
            return None;
        }
        let snapshot = self.snapshot();
        let mut text = String::new();
        for value in values {
            self.state.set(&node.var, Var::Scalar(value.clone()));
            text.push_str(&format!("{}={}\n", node.var, quote::word(value)));
            for simple in &simples {
                let (replacement, ends) = self.simple(simple, true, cx);
                let piece = match (ends, replacement) {
                    (false, Some(piece)) => Some(piece),
                    (false, None) => script::reprint(simple),
                    (true, _) => None,
                };
                let Some(piece) = piece else {
                    self.restore(snapshot);
                    return None;
                };
                text.push_str(&piece);
                if !piece.ends_with('\n') {
                    text.push('\n');
                }
            }
        }
        Some(text)
    }

    fn function(
        &mut self,
        script: &Script,
        node: &ZshFuncDef,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) {
        if node.auto_call_args.is_none() {
            let function = Function {
                file: cx.origin.map(Path::to_path_buf).unwrap_or_default(),
                fingerprint: walk::fingerprint(node),
            };
            for name in &node.names {
                self.state
                    .functions
                    .insert(walk::text(name), function.clone());
            }
            return;
        }
        if let Some(lines) = lines {
            edits.functions.push(script.byte_range(lines));
        }
        let body = Cx {
            mode: Mode::Function,
            origin: None,
            analysis: cx.analysis,
        };
        self.state.push_frame();
        let last = lines.map_or(0, |lines| *lines.end());
        self.lists(
            script,
            &node.body.lists,
            last,
            lines.is_some(),
            edits,
            &body,
        );
        self.state.pop_frame();
    }

    fn simple(&mut self, simple: &ZshSimple, rewrite: bool, cx: &Cx) -> (Option<String>, bool) {
        if simple.words.is_empty() {
            for assign in &simple.assigns {
                self.assign(assign, false);
            }
            return (None, false);
        }
        let words = walk::words(simple);
        let name = expand::scalar(&words[0], &self.state).unwrap_or_else(|| words[0].clone());
        let rewrite = rewrite && simple.assigns.is_empty() && simple.redirs.is_empty();
        let args = &words[1..];
        match name.as_str() {
            "return" | "exit" => (None, true),
            "source" | "." if simple.assigns.is_empty() => rules::source(self, args, rewrite, cx),
            "cached_eval" => rules::cached_eval(self, args, rewrite, cx),
            "add_path" | "add_plugin_path" | "add_plugins" | "add_fpath" => {
                rules::helper(self, &name, args, rewrite)
            }
            "_omz_source" => rules::omz_source(self, args, rewrite, cx),
            "unset" => {
                let functions = args
                    .iter()
                    .any(|arg| arg.starts_with('-') && arg.contains('f'));
                for arg in args.iter().filter(|arg| !arg.starts_with('-')) {
                    if functions {
                        self.state.functions.remove(arg);
                    } else {
                        self.state.set(arg, Var::Unset);
                    }
                }
                (None, false)
            }
            "unfunction" => {
                args.iter().for_each(|arg| {
                    self.state.functions.remove(arg);
                });
                (None, false)
            }
            "zstyle" => {
                let values: Vec<String> = args
                    .iter()
                    .filter_map(|arg| expand::scalar(arg, &self.state))
                    .collect();
                if values.len() >= 2 && values[0].starts_with(":omz:") && values[1] == "aliases" {
                    self.state.omz_alias_styles = true;
                }
                (None, false)
            }
            declaration if walk::DECLARATIONS.contains(&declaration) => {
                self.declare(declaration, simple, args);
                (None, false)
            }
            _ => (None, false),
        }
    }

    fn declare(&mut self, name: &str, simple: &ZshSimple, args: &[String]) {
        let flags: String = args
            .iter()
            .filter(|arg| arg.starts_with(['-', '+']))
            .flat_map(|arg| arg.chars().skip(1))
            .collect();
        let global = flags.contains('g') || name == "export";
        let local = name == "local" || (self.state.in_function() && !global);
        let array = flags.contains('a') || flags.contains('A');
        for assign in &simple.assigns {
            self.assign(assign, local);
        }
        for arg in args.iter().filter(|arg| !arg.starts_with(['-', '+'])) {
            match arg.split_once('=') {
                Some((variable, value)) => {
                    if local {
                        self.state.declare_local(variable);
                    }
                    let value = if value.starts_with('(') {
                        Var::Unknown
                    } else {
                        expand::scalar(value, &self.state).map_or(Var::Unknown, Var::Scalar)
                    };
                    self.state.set(variable, value);
                }
                None if local => {
                    self.state.declare_local(arg);
                    self.state.set(
                        arg,
                        if array {
                            Var::Array(Vec::new())
                        } else {
                            Var::Unset
                        },
                    );
                }
                None if array && self.state.var(arg) == Var::Unset => {
                    self.state.set(arg, Var::Array(Vec::new()));
                }
                None => {}
            }
        }
    }

    fn assign(&mut self, assign: &ZshAssign, local: bool) {
        if local {
            self.state.declare_local(&assign.name);
        }
        let value = match &assign.value {
            ZshAssignValue::Scalar(value) => {
                expand::scalar(&walk::text(value), &self.state).map(Var::Scalar)
            }
            ZshAssignValue::Array(values) => values
                .iter()
                .map(|value| expand::words(&walk::text(value), &self.state, Expand::Args))
                .collect::<Option<Vec<_>>>()
                .map(|values| Var::Array(values.concat())),
        };
        let next = match (value, assign.append) {
            (None, _) => Var::Unknown,
            (Some(value), false) => value,
            (Some(value), true) => append(self.state.var(&assign.name), value),
        };
        self.state.set(&assign.name, next);
    }

    fn forget(&mut self, lists: &[ZshList]) {
        for name in walk::assigned(lists) {
            self.state.forget(&name);
        }
    }

    fn program_truth(&self, program: &ZshProgram) -> Option<bool> {
        let [list] = program.lists.as_slice() else {
            return None;
        };
        if list.sublist.next.is_some() || list.sublist.pipe.next.is_some() {
            return None;
        }
        self.truth(&list.sublist.pipe.cmd)
            .map(|truth| truth != list.sublist.flags.not)
    }

    fn truth(&self, cmd: &ZshCommand) -> Option<bool> {
        match cmd {
            ZshCommand::Cond(cond) => self.cond(cond),
            ZshCommand::Simple(simple) if simple.assigns.is_empty() && simple.redirs.is_empty() => {
                let words = walk::words(simple);
                match words.as_slice() {
                    [word] if word == "true" || word == ":" => Some(true),
                    [word] if word == "false" => Some(false),
                    [word, name] if word == "has_cmd" && self.helper_matches("has_cmd") => {
                        let name = expand::scalar(name, &self.state)?;
                        Some(self.state.command(&name)?.is_some())
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn cond(&self, cond: &ZshCond) -> Option<bool> {
        match cond {
            ZshCond::Not(inner) => self.cond(inner).map(|truth| !truth),
            ZshCond::And(left, right) => match self.cond(left)? {
                false => Some(false),
                true => self.cond(right),
            },
            ZshCond::Or(left, right) => match self.cond(left)? {
                true => Some(true),
                false => self.cond(right),
            },
            ZshCond::Unary(operator, operand) => {
                let value = expand::scalar(&walk::text(operand), &self.state)?;
                match walk::text(operator).as_str() {
                    "-n" => Some(!value.is_empty()),
                    "-z" => Some(value.is_empty()),
                    _ if !value.starts_with('/') => None,
                    "-e" | "-a" => Some(Path::new(&value).exists()),
                    "-f" => Some(Path::new(&value).is_file()),
                    "-d" => Some(Path::new(&value).is_dir()),
                    "-r" => Some(access(&value, nix::unistd::AccessFlags::R_OK)),
                    "-w" => Some(access(&value, nix::unistd::AccessFlags::W_OK)),
                    "-x" => Some(access(&value, nix::unistd::AccessFlags::X_OK)),
                    _ => None,
                }
            }
            ZshCond::Binary(left, operator, right) => {
                let operator = walk::text(operator);
                if !matches!(operator.as_str(), "==" | "=" | "!=") {
                    return None;
                }
                let left = expand::scalar(&walk::text(left), &self.state)?;
                let pattern = expand::pattern(&walk::text(right), &self.state)?;
                let options = glob::MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: false,
                    require_literal_leading_dot: false,
                };
                Some(pattern.matches_with(&left, options) == (operator != "!="))
            }
            ZshCond::Regex(..) => None,
        }
    }

    pub(crate) fn helper_matches(&self, name: &str) -> bool {
        self.state
            .functions
            .get(name)
            .is_some_and(|function| rules::expected(name) == Some(function.fingerprint))
    }

    /// Folds substitutions and keeps `$0` and `return` meaning what they did
    /// in the file this code came from, within one owned segment.
    fn text_layer(
        &mut self,
        script: &Script,
        segment: &Segment,
        lists: &[ZshList],
        edits: &mut Edits,
        cx: &Cx,
    ) {
        if cx.analysis {
            return;
        }
        let range = script.byte_range(&segment.lines);
        let text = &script.text[range.clone()];
        let definition =
            lists.len() == 1 && matches!(walk::sole(&lists[0]), Some(ZshCommand::FuncDef(_)));
        if definition {
            edits.functions.push(range.clone());
        }
        let scan = scan::scan(text);
        if scan.unterminated {
            return;
        }
        let mut local: Vec<(Range<usize>, String, Kind)> = Vec::new();
        for substitution in &scan.substitutions {
            if let Some(replacement) = self.fold(&text[substitution.inner.clone()], substitution) {
                local.push((substitution.range.clone(), replacement, Kind::Fold));
            }
        }
        let functions = definition || walk::contains_function(lists);
        if let Some(origin) = cx.origin {
            for zero in &scan.zero_refs {
                let ZeroForm::Prompt { file } = zero.form else {
                    continue;
                };
                if let Some(replacement) = (file || !functions)
                    .then(|| prompt_replacement(origin, zero))
                    .flatten()
                {
                    local.push((zero.range.clone(), replacement, Kind::Zero));
                }
            }
        }
        let mut returns = 0;
        if cx.mode == Mode::Plain && !definition {
            let found = walk::returns(lists).unwrap_or_default();
            let file_level = found.iter().filter(|site| site.depth.is_some()).count();
            if file_level > 0
                && let Some(rewrites) = return_edits(text, &scan, &found)
            {
                returns = file_level;
                local.extend(
                    rewrites
                        .into_iter()
                        .map(|(range, text)| (range, text, Kind::Return)),
                );
            }
        }
        if local.is_empty() {
            return;
        }
        local.sort_by_key(|(range, _, _)| (range.start, std::cmp::Reverse(range.end)));
        let mut accepted = Vec::new();
        let mut end = 0;
        for edit in local {
            if edit.0.start >= end {
                end = edit.0.end;
                accepted.push(edit);
            } else if edit.2 == Kind::Return {
                returns = 0;
            }
        }
        let mut edited = String::with_capacity(text.len());
        let mut position = 0;
        for (range, replacement, _) in &accepted {
            edited.push_str(&text[position..range.start]);
            edited.push_str(replacement);
            position = range.end;
        }
        edited.push_str(&text[position..]);
        let same = script::parse(&edited).is_ok_and(|program| {
            program.lists.len() == lists.len()
                && program
                    .lists
                    .iter()
                    .zip(lists)
                    .all(|(left, right)| script::shape(left) == script::shape(right))
        });
        if !same {
            return;
        }
        let base = range.start;
        for zero in &scan.zero_refs {
            let folded = accepted.iter().any(|(range, _, kind)| {
                *kind == Kind::Fold
                    && range.start <= zero.range.start
                    && zero.range.end <= range.end
            });
            if folded {
                edits.zero_done.push(base + zero.range.start);
            }
        }
        for (range, replacement, kind) in accepted {
            match kind {
                Kind::Fold => self.folded += 1,
                Kind::Zero => edits.zero_done.push(base + range.start),
                Kind::Return => {}
            }
            edits.replace_bytes(base + range.start..base + range.end, replacement);
        }
        edits.returns_rewritten += returns;
    }

    /// `${__zbN}` for a listed command's output.
    fn fold(&mut self, inner: &str, substitution: &scan::Substitution) -> Option<String> {
        let program = script::parse(inner).ok()?;
        let [list] = program.lists.as_slice() else {
            return None;
        };
        let simple = walk::sole_simple(list)?;
        if !simple.assigns.is_empty() || !simple.redirs.iter().all(rules::discards_stderr) {
            return None;
        }
        let argv: Vec<String> = walk::words(simple)
            .iter()
            .map(|word| expand::words(word, &self.state, Expand::Args))
            .collect::<Option<Vec<_>>>()?
            .concat();
        if !self.folder.allows(&argv) {
            return None;
        }
        let output = match self.folder.run(&argv, &self.state) {
            Ok(output) => output,
            Err(error) => {
                self.note(format!("fold {error}"));
                return None;
            }
        };
        let value = output.trim_end_matches('\n').to_string();
        if !substitution.quoted && !substitution.in_param && value.contains([' ', '\t', '\n', '\0'])
        {
            return None;
        }
        Some(format!("${{{}}}", self.constants.name(value)))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fold,
    Zero,
    Return,
}

fn list_of(cmd: ZshCommand) -> ZshList {
    ZshList {
        sublist: zshrs_parse::parser::ZshSublist {
            pipe: zshrs_parse::parser::ZshPipe {
                cmd,
                next: None,
                lineno: 0,
                merge_stderr: false,
            },
            next: None,
            flags: Default::default(),
        },
        flags: Default::default(),
    }
}

fn placeholder(text: String) -> String {
    if text.trim().is_empty() {
        ":\n".to_string()
    } else {
        text
    }
}

fn omitted(script: &Script, segment: &Segment) -> bool {
    let start = *segment.lines.start();
    start > 1 && script.lines(&(start - 1..=start - 1)).trim() == OMIT
}

fn append(current: Var, value: Var) -> Var {
    match (current, value) {
        (Var::Unknown, _) | (_, Var::Unknown) | (_, Var::Unset) => Var::Unknown,
        (Var::Unset, value) => value,
        (Var::Scalar(current), Var::Scalar(value)) => Var::Scalar(current + &value),
        (Var::Array(mut current), Var::Array(value)) => {
            current.extend(value);
            Var::Array(current)
        }
        (Var::Array(mut current), Var::Scalar(value)) => {
            current.push(value);
            Var::Array(current)
        }
        (Var::Scalar(current), Var::Array(value)) => {
            let mut out = vec![current];
            out.extend(value);
            Var::Array(out)
        }
    }
}

fn access(path: &str, mode: nix::unistd::AccessFlags) -> bool {
    nix::unistd::access(path, mode).is_ok()
}

/// `%x` and `%N` name the original file once its code runs from the bundle.
fn prompt_replacement(origin: &Path, zero: &ZeroRef) -> Option<String> {
    let origin = origin
        .to_str()
        .filter(|origin| quote::is_plain_path(origin))?;
    Some(if zero.in_param {
        format!("${{:-{origin}}}")
    } else {
        origin.to_string()
    })
}

fn return_edits(
    text: &str,
    scan: &scan::Scan,
    returns: &[walk::Return],
) -> Option<Vec<(Range<usize>, String)>> {
    let keywords: Vec<&scan::Word> = scan
        .words
        .iter()
        .filter(|word| &text[word.range.clone()] == "return")
        .collect();
    if keywords.len() != returns.len() {
        return None;
    }
    let mut out = Vec::new();
    for (keyword, found) in keywords.iter().zip(returns) {
        let Some(depth) = found.depth else {
            continue;
        };
        let mut range = keyword.range.clone();
        if let Some(argument) = &found.argument {
            let next = scan
                .words
                .iter()
                .find(|word| word.range.start > keyword.range.end)?;
            let between = &text[keyword.range.end..next.range.start];
            if text[next.range.clone()] != **argument
                || !between.chars().all(|c| c == ' ' || c == '\t')
            {
                return None;
            }
            range.end = next.range.end;
        }
        let replacement = if depth == 0 {
            "break".to_string()
        } else {
            format!("break {}", depth + 1)
        };
        out.push((range, replacement));
    }
    Some(out)
}

pub(crate) fn read(path: &Path) -> Result<Script, String> {
    read_text(path)
        .and_then(Script::parse)
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("read: {error}"))
}

pub(crate) fn unique(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}
