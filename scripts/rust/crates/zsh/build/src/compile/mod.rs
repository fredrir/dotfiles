//! Walks startup code in execution order, tracking build-time state, and
//! rewrites what that state decides: sources, helpers and folded commands.

mod aliases;
mod bindkey;
mod compinit;
pub mod defer;
mod edits;
mod ledger;
mod rules;
pub mod system;
pub mod walk;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Range, RangeInclusive};
use std::path::{Path, PathBuf};

use zshrs_parse::parser::{
    SublistOp, ZshAssign, ZshAssignValue, ZshCommand, ZshCond, ZshFor, ZshFuncDef, ZshIf, ZshList,
    ZshProgram, ZshSimple, ZshSublist,
};

use crate::alias;
use crate::expand::{self, Env, Mode as Expand, Var};
use crate::fold::{Constants, Folder};
use crate::quote;
use crate::scan::{self, ZeroForm, ZeroRef};
use crate::script::{self, Script, Segment};
use crate::state::{Function, State};
use crate::wordcode;
use defer::Deferral;
use edits::Edits;
use ledger::Ledger;

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
    /// A startup file zsh reads itself: `return` leaves only that file and
    /// `$0` names the shell.
    Startup,
}

impl Mode {
    fn file_scoped(self) -> bool {
        matches!(self, Mode::Plain | Mode::Startup)
    }
}

/// Where function definitions in compiled code are reported as coming from.
#[derive(Clone, Copy, Debug)]
pub enum Origin<'a> {
    /// At their own line in this file.
    File(&'a str),
    /// All at this `file:line`, such as the `eval` that ran them.
    Fixed(&'a str),
}

#[derive(Clone, Copy)]
pub struct Cx<'a> {
    pub mode: Mode,
    pub origin: Option<&'a Path>,
    pub analysis: bool,
    pub source: Option<Origin<'a>>,
}

pub struct Compiler {
    pub state: State,
    pub folder: Folder,
    pub constants: Constants,
    /// Problems the dotfiles can fix.
    pub warnings: Vec<String>,
    /// Third-party code left to runtime.
    pub skipped: Vec<String>,
    /// `name()` definitions after an alias of that name.
    pub redefined: Vec<String>,
    pub inlined: usize,
    pub folded: usize,
    /// `PATH_HELPER_ROOT` for the `path_helper` emulation.
    pub path_helper_root: String,
    /// System files that native emulations read.
    pub dependencies: Vec<PathBuf>,
    /// Function name to the `file:line` it was defined at before bundling.
    pub origins: BTreeMap<String, String>,
    /// Definitions native expansions call, emitted ahead of the bundle.
    pub helpers: Vec<&'static str>,
    /// What runs after the first prompt.
    pub deferral: Deferral,
    /// The zsh that checks code the parser needs respelled.
    pub zsh: Option<PathBuf>,
    pub deferred: usize,
    ledger: Ledger,
    /// Above zero inside deferred code and loop bodies, where nothing more is deferred.
    defer_suspended: usize,
    /// Above zero inside loop bodies, where build state may not hold on each pass.
    loops: usize,
    /// `loops` where the file being compiled starts; its returns also leave the loops above.
    loop_floor: usize,
    /// Whether compiled code may leave an emulation other than zsh.
    emulation_changed: bool,
    stack: Vec<PathBuf>,
    line: usize,
    root: PathBuf,
    startup: bool,
    refusals: Vec<String>,
    /// The aliases zsh parses the statement being compiled with.
    alias_parse: alias::Table,
    /// Alias names a bundle read as text may have set by the statement being compiled.
    alias_seen: alias::Seen,
    /// Above zero inside a statement of the bundle that holds others.
    alias_depth: usize,
    /// The alias table at each `return` of each file or function being compiled.
    alias_exits: Vec<Vec<alias::Table>>,
    /// The next `lists` starts statements zsh parses one at a time.
    events: bool,
    /// The next script compiled is `eval` output, which zsh parses at once.
    eval_parse: bool,
    /// Above zero inside code that never runs.
    dead: usize,
    /// Alias problems reported, by name or place and reason.
    alias_notes: BTreeSet<(String, &'static str)>,
    /// `alias` and `unalias` commands in function bodies, by function.
    alias_bodies: BTreeMap<String, Vec<Vec<String>>>,
    /// The call being recorded set its aliases by name already.
    alias_call_handled: bool,
}

struct Snapshot {
    state: State,
    constants: usize,
    warnings: usize,
    skipped: usize,
    redefined: usize,
    inlined: usize,
    folded: usize,
    ledger: (usize, usize, usize),
    origins: BTreeMap<String, String>,
    helpers: usize,
    alias_exits: usize,
}

impl Compiler {
    pub fn new(state: State, folder: Folder, root: PathBuf) -> Self {
        Self {
            state,
            folder,
            constants: Constants::default(),
            warnings: Vec::new(),
            skipped: Vec::new(),
            redefined: Vec::new(),
            inlined: 0,
            folded: 0,
            path_helper_root: String::new(),
            dependencies: Vec::new(),
            origins: BTreeMap::new(),
            helpers: Vec::new(),
            deferral: Deferral::default(),
            zsh: None,
            deferred: 0,
            ledger: Ledger::default(),
            defer_suspended: 0,
            loops: 0,
            loop_floor: 0,
            emulation_changed: false,
            stack: Vec::new(),
            line: 0,
            root,
            startup: false,
            refusals: Vec::new(),
            alias_parse: alias::Table::default(),
            alias_seen: alias::Seen::default(),
            alias_depth: 0,
            alias_exits: Vec::new(),
            events: false,
            eval_parse: false,
            dead: 0,
            alias_notes: BTreeSet::new(),
            alias_bodies: BTreeMap::new(),
            alias_call_handled: false,
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
                source: None,
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
                source: path.to_str().map(Origin::File),
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
            redefined: self.redefined.len(),
            inlined: self.inlined,
            folded: self.folded,
            ledger: self.ledger.len(),
            origins: self.origins.clone(),
            helpers: self.helpers.len(),
            alias_exits: self.alias_exits.last().map_or(0, Vec::len),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.state = snapshot.state;
        self.constants.truncate(snapshot.constants);
        self.warnings.truncate(snapshot.warnings);
        self.skipped.truncate(snapshot.skipped);
        self.redefined.truncate(snapshot.redefined);
        self.inlined = snapshot.inlined;
        self.folded = snapshot.folded;
        self.ledger.truncate(snapshot.ledger);
        self.origins = snapshot.origins;
        self.helpers.truncate(snapshot.helpers);
        if let Some(exits) = self.alias_exits.last_mut() {
            exits.truncate(snapshot.alias_exits);
        }
    }

    /// The `file:line` of the statement being compiled.
    pub(crate) fn location(&self, cx: &Cx) -> Option<String> {
        match cx.source? {
            Origin::File(file) => Some(format!("{file}:{}", self.line)),
            Origin::Fixed(location) => Some(location.to_string()),
        }
    }

    fn define(&mut self, names: impl IntoIterator<Item = String>, location: &str) {
        for name in names {
            self.origins.insert(name, location.to_string());
        }
    }

    pub fn compile(&mut self, script: &Script, cx: &Cx) -> Result<String, String> {
        let mut edits = Edits::default();
        let floor = std::mem::replace(&mut self.loop_floor, self.loops);
        let whole = std::mem::take(&mut self.eval_parse);
        let outer = self.alias_parse.clone();
        if whole {
            self.alias_parse = self.state.alias_table.clone();
        }
        self.alias_exits.push(Vec::new());
        // In the bundle, these run inside a wrapper statement, which zsh parses at once
        let wrapped = match cx.mode {
            Mode::Root => false,
            Mode::Plain => walk::file_returns(&script.program.lists) != Some(0),
            Mode::Function | Mode::Startup => true,
        } || self.defer_suspended > 0;
        self.alias_depth += usize::from(wrapped);
        self.compile_lists(script, &mut edits, cx, !whole);
        self.alias_depth -= usize::from(wrapped);
        self.join_alias_exits();
        self.alias_parse = outer;
        self.loop_floor = floor;
        if cx.analysis {
            return Ok(String::new());
        }
        let returns = match cx.mode {
            Mode::Plain | Mode::Startup => {
                walk::file_returns(&script.program.lists).ok_or("return inside a subshell")?
                    - edits.returns_omitted
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
            (true, Mode::Startup, _) => Some("$ZSH_ARGZERO".to_string()),
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

    /// The script's statements; `events` when zsh parses them one at a time.
    fn compile_lists(&mut self, script: &Script, edits: &mut Edits, cx: &Cx, events: bool) {
        let lists = &script.program.lists;
        match (self.compinit_window(script, cx), cx.origin) {
            (Some(window), Some(origin)) => {
                let (start, end) = (*window.lines.start(), *window.lines.end());
                let before = &lists[..window.lists.start];
                self.events = events;
                self.lists(script, before, start - 1, true, edits, cx);
                self.open_window();
                self.events = events;
                self.lists(script, &lists[window.lists.clone()], end, true, edits, cx);
                self.wrap_window(script, &window, edits, origin);
                let rest = &lists[window.lists.end..];
                self.events = events;
                self.lists(script, rest, script.line_count(), true, edits, cx);
            }
            _ => {
                self.events = events;
                self.lists(script, lists, script.line_count(), true, edits, cx);
            }
        }
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
        let section = if cx.analysis {
            None
        } else {
            compinit::find(lists, &segments)
        };
        let single = self.single_calls(script, lists, &segments, section.as_ref(), cx);
        let definition = |index: usize| single.iter().any(|(definition, ..)| *definition == index);
        let mut ended = false;
        let mut index = 0;
        let events = std::mem::take(&mut self.events);
        self.alias_depth += usize::from(!events);
        // Run-once bodies are parsed with their definition
        let mut bodies: BTreeMap<usize, alias::Table> = BTreeMap::new();
        for (position, segment) in segments.into_iter().enumerate() {
            if let Some(section) = section
                .as_ref()
                .filter(|section| section.segments.contains(&position))
            {
                if position == section.segments.start {
                    for list in &lists[section.lists.clone()] {
                        self.statement(script, list, None, edits, cx);
                    }
                    edits.replace_lines(section.lines.clone(), compinit::text(script, section));
                }
                index = section.lists.end;
                continue;
            }
            if events {
                self.alias_parse = self.state.alias_table.clone();
            }
            if events && self.alias_depth == 0 {
                self.alias_seen = alias::Seen::from_table(&self.state.alias_table);
            }
            let dead = ended.then(|| (self.state.clone(), self.ledger.mark()));
            let start = segment
                .as_ref()
                .map_or(index, |segment| segment.lists.start);
            for (definition, _, lines) in single.iter().filter(|(_, call, _)| *call == start) {
                let parse = bodies
                    .remove(definition)
                    .unwrap_or_else(|| self.alias_parse.clone());
                self.single_call(script, &lists[*definition], lines, parse, edits, cx);
            }
            match segment {
                Some(segment) => {
                    let range = segment.lists.clone();
                    index = range.end;
                    if omitted(script, &segment) {
                        edits.returns_omitted += dropped_returns(&lists[range], cx);
                        edits.replace_lines(segment.lines.clone(), String::new());
                        continue;
                    }
                    let before = edits.len();
                    let mark = edits.mark();
                    let bytes = script.byte_range(&segment.lines);
                    let whole = bytes.clone();
                    let line = *segment.lines.start();
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
                            None if edits.len() == before && !definition(range.start) => self
                                .text_layer(script, bytes, line, &lists[range.clone()], edits, cx),
                            None => {}
                        }
                    } else {
                        for list in &lists[range.clone()] {
                            ended |= self.statement(script, list, None, edits, cx).1;
                        }
                        self.text_layer(script, bytes, line, &lists[range.clone()], edits, cx);
                    }
                    if definition(range.start) {
                        bodies.insert(range.start, self.alias_parse.clone());
                    } else if dead.is_none() {
                        self.alias_segment(script, whole, mark, edits, cx);
                    }
                }
                None => {
                    ended |= self.statement(script, &lists[index], None, edits, cx).1;
                    index += 1;
                }
            }
            if let Some((state, mark)) = dead {
                self.state = state;
                self.ledger.reset(mark);
            }
        }
        self.alias_depth -= usize::from(!events);
        ended
    }

    /// Functions in `lists` whose only run is one call there: definition and
    /// call indices, with the lines the definition owns.
    fn single_calls(
        &self,
        script: &Script,
        lists: &[ZshList],
        segments: &[Option<Segment>],
        section: Option<&compinit::Section>,
        cx: &Cx,
    ) -> Vec<(usize, usize, RangeInclusive<usize>)> {
        if cx.analysis {
            return Vec::new();
        }
        let mut starts = BTreeMap::new();
        let mut index = 0;
        for segment in segments {
            let end = segment
                .as_ref()
                .map_or(index + 1, |segment| segment.lists.end);
            starts.insert(index, segment.as_ref());
            index = end;
        }
        let open = |index: &usize| {
            starts.get(index).is_some_and(|segment| {
                segment.is_none_or(|segment| !omitted(script, segment))
                    && section.is_none_or(|section| !section.lists.contains(index))
            })
        };
        walk::single_calls(lists, &script.text)
            .into_iter()
            .filter(|(definition, call)| open(definition) && open(call))
            .filter_map(|(definition, call)| {
                let segment = starts.get(&definition).copied().flatten()?;
                (segment.lists.len() == 1).then(|| (definition, call, segment.lines.clone()))
            })
            .collect()
    }

    /// The body of a function whose only run is the call about to run,
    /// compiled as code running here; like any call, it leaves state as it was.
    fn single_call(
        &mut self,
        script: &Script,
        list: &ZshList,
        lines: &RangeInclusive<usize>,
        parse: alias::Table,
        edits: &mut Edits,
        cx: &Cx,
    ) {
        let Some(ZshCommand::FuncDef(node)) = walk::sole(list) else {
            return;
        };
        let range = script.byte_range(lines);
        let before = edits.len();
        let mark = edits.mark();
        edits.functions.push(range.clone());
        let saved = self.state.clone();
        let outer = std::mem::replace(&mut self.alias_parse, parse);
        let body = Cx {
            mode: Mode::Function,
            origin: None,
            analysis: false,
            source: cx.source,
        };
        self.state.push_frame();
        self.alias_exits.push(Vec::new());
        self.lists(script, &node.body.lists, *lines.end(), true, edits, &body);
        self.alias_exits.pop();
        self.state = saved;
        if edits.len() == before {
            let lists = std::slice::from_ref(list);
            self.text_layer(script, range.clone(), *lines.start(), lists, edits, cx);
        }
        self.alias_segment(script, range, mark, edits, cx);
        self.alias_parse = outer;
    }

    fn statement(
        &mut self,
        script: &Script,
        list: &ZshList,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) -> (Option<String>, bool) {
        self.line = script::start_line(list);
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
            ZshCommand::If(node) => self.branch(script, node, lines, edits, cx),
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
            .truth(&sublist.pipe.cmd, Decide::State)
            .map(|truth| truth != sublist.flags.not);
        let fixed = lines.filter(|_| {
            self.eliminates(cx) && self.truth(&sublist.pipe.cmd, Decide::Commands).is_some()
        });
        let runs = match (operator, truth) {
            (SublistOp::And, Some(truth)) => Some(truth),
            (SublistOp::Or, Some(truth)) => Some(!truth),
            _ => None,
        };
        if truth.is_none() {
            self.forget(std::slice::from_ref(&list_of(sublist.pipe.cmd.clone())));
        }
        match runs {
            Some(true) => {
                let before = edits.len();
                let (replacement, ends) = self.command(script, &rest.pipe.cmd, lines, edits, cx);
                if let (None, Some(lines)) = (&replacement, fixed) {
                    self.strip_test(script, list, lines, before, edits, cx);
                }
                (replacement, ends)
            }
            Some(false) => match fixed {
                Some(lines) => {
                    let dropped = list_of(rest.pipe.cmd.clone());
                    let returns = dropped_returns(std::slice::from_ref(&dropped), cx);
                    let range = script.byte_range(lines);
                    drop_code(script, range, None, returns, edits);
                    let status = if truth == Some(true) { ":\n" } else { "! :\n" };
                    (Some(status.to_string()), false)
                }
                None => (None, false),
            },
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

    /// An `if` chain; only the arm that runs when the build decides which.
    fn branch(
        &mut self,
        script: &Script,
        node: &ZshIf,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) -> (Option<String>, bool) {
        let last = lines.map_or(0, |lines| *lines.end());
        let owned = lines.is_some();
        // Conditions that run set what they assign before any arm.
        let conds = std::iter::once(&*node.cond).chain(node.elif.iter().map(|(cond, _)| cond));
        for cond in conds {
            self.forget(&cond.lists);
        }
        let saved = self.state.clone();
        let mut arms: Vec<(Option<&ZshProgram>, &ZshProgram)> =
            vec![(Some(&node.cond), &node.then)];
        arms.extend(node.elif.iter().map(|(cond, body)| (Some(cond), body)));
        if let Some(body) = &node.else_ {
            arms.push((None, body));
        }
        let kept = lines
            .filter(|_| self.eliminates(cx))
            .and_then(|lines| self.kept_arm(script, &arms, *lines.end(), cx));
        let mut exits = Vec::new();
        let mut decided = false;
        for (cond, body) in &arms {
            self.state = saved.clone();
            let taken = match cond {
                None => Some(true),
                Some(cond) => self.program_truth(cond, Decide::State),
            };
            if decided || taken == Some(false) {
                if kept.is_none() {
                    self.dead_arm(script, body, last, owned, edits, cx);
                }
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
        let ends = match exits.split_first() {
            None => {
                self.state = saved.clone();
                true
            }
            Some((first, rest)) => {
                let mut joined = first.clone();
                rest.iter().for_each(|state| joined.merge(state));
                self.state = joined;
                false
            }
        };
        let (Some(kept), Some(lines)) = (kept, lines) else {
            return (None, ends);
        };
        let text = kept.body.as_ref().map_or_else(String::new, |body| {
            edits.render_within(script, body.clone())
        });
        if script::parse(&text).is_err() {
            let after = std::mem::replace(&mut self.state, saved.clone());
            for (index, (_, body)) in arms.iter().enumerate() {
                if kept.index != Some(index) {
                    self.state = saved.clone();
                    self.dead_arm(script, body, last, owned, edits, cx);
                }
            }
            self.state = after;
            return (None, ends);
        }
        let range = script.byte_range(lines);
        drop_code(script, range, kept.body.as_ref(), kept.returns, edits);
        (Some(text), ends)
    }

    /// An arm that never runs, compiled for what it rewrites only.
    fn dead_arm(
        &mut self,
        script: &Script,
        body: &ZshProgram,
        last: usize,
        owned: bool,
        edits: &mut Edits,
        cx: &Cx,
    ) {
        let mark = self.ledger.mark();
        let exits = self.alias_exits.last().map_or(0, Vec::len);
        self.dead += 1;
        self.lists(script, &body.lists, last, owned, edits, cx);
        self.dead -= 1;
        if let Some(exits_now) = self.alias_exits.last_mut() {
            exits_now.truncate(exits);
        }
        self.ledger.reset(mark);
    }

    /// Whether decisions made here may drop code: they hold each time it runs.
    fn eliminates(&self, cx: &Cx) -> bool {
        !cx.analysis && self.loops == 0
    }

    /// The arm of an `if` chain ending at line `last` that runs, decided from
    /// the search path and constants, when its body owns whole lines and
    /// reads no status its test left.
    fn kept_arm(
        &self,
        script: &Script,
        arms: &[(Option<&ZshProgram>, &ZshProgram)],
        last: usize,
        cx: &Cx,
    ) -> Option<Kept> {
        let mut index = None;
        for (position, (cond, _)) in arms.iter().enumerate() {
            if cond.map_or(Some(true), |cond| {
                self.program_truth(cond, Decide::Commands)
            })? {
                index = Some(position);
                break;
            }
        }
        let body = match index {
            None => None,
            Some(position) => {
                let lists = &arms[position].1.lists;
                let segments = script.segments(lists, last);
                let owned: Vec<&Segment> = segments.iter().flatten().collect();
                let (first, end) = (owned.first()?, owned.last()?);
                if owned.len() != segments.len() {
                    return None;
                }
                let range = script.byte_range(&(*first.lines.start()..=*end.lines.end()));
                if walk::reads_status(&lists[0]) {
                    return None;
                }
                Some(range)
            }
        };
        let returns = arms
            .iter()
            .enumerate()
            .flat_map(|(position, (cond, body))| {
                let body = (index != Some(position)).then_some(*body);
                cond.iter().copied().chain(body)
            })
            .map(|program| dropped_returns(&program.lists, cx))
            .sum();
        Some(Kept {
            index,
            body,
            returns,
        })
    }

    /// Drops a decided test from `list`, `test && command` or `test || command`,
    /// keeping the command and folding its substitutions.
    fn strip_test(
        &mut self,
        script: &Script,
        list: &ZshList,
        lines: &RangeInclusive<usize>,
        before: usize,
        edits: &mut Edits,
        cx: &Cx,
    ) {
        let Some((_, rest)) = &list.sublist.next else {
            return;
        };
        let range = script.byte_range(lines);
        let text = &script.text[range.clone()];
        let command = ZshList {
            sublist: (**rest).clone(),
            flags: list.flags,
        };
        let Some(split) = test_end(text, list, &command) else {
            return;
        };
        if walk::reads_status(&command) {
            return;
        }
        let start = range.start + split;
        if edits.len() == before {
            let line = *lines.start() + text[..split].matches('\n').count();
            let commands = std::slice::from_ref(&command);
            self.text_layer(script, start..range.end, line, commands, edits, cx);
        }
        edits.replace_bytes(range.start..start, String::new());
    }

    fn for_loop(
        &mut self,
        script: &Script,
        node: &ZshFor,
        lines: Option<&RangeInclusive<usize>>,
        edits: &mut Edits,
        cx: &Cx,
    ) -> (Option<String>, bool) {
        self.record_loop(node);
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
        self.defer_suspended += 1;
        self.loops += 1;
        self.lists(script, &node.body.lists, last, lines.is_some(), edits, cx);
        self.loops -= 1;
        self.defer_suspended -= 1;
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
                    (false, None) => script::reprint(simple).map(|text| self.alias_piece(text, cx)),
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
            self.check_aliases(script, node, cx);
            self.emulation_changed |= walk::changes_emulation_in(&node.body.lists, true);
            let function = Function {
                file: cx.origin.map(Path::to_path_buf).unwrap_or_default(),
                fingerprint: walk::fingerprint(node),
            };
            let names: Vec<String> = node.names.iter().map(|name| walk::text(name)).collect();
            self.record_alias_body(&names, &node.body.lists);
            for name in names {
                self.record_function(&name, &node.body.lists);
                self.state.functions.insert(name, function.clone());
            }
            if !cx.analysis
                && let Some(location) = self.location(cx)
            {
                self.define(node.names.iter().map(|name| walk::text(name)), &location);
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
            source: cx.source,
        };
        self.state.push_frame();
        self.alias_exits.push(Vec::new());
        let option = self.state.alias_table.option;
        let last = lines.map_or(0, |lines| *lines.end());
        self.lists(
            script,
            &node.body.lists,
            last,
            lines.is_some(),
            edits,
            &body,
        );
        self.join_alias_exits();
        if walk::sets_local_options(&node.body.lists) {
            self.state.alias_table.option = option;
        }
        self.state.pop_frame();
    }

    fn simple(&mut self, simple: &ZshSimple, rewrite: bool, cx: &Cx) -> (Option<String>, bool) {
        self.record_expansions(simple);
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
        self.alias_call_handled = self.alias_call(&name, args);
        self.record_command(&name, args);
        self.alias_call_handled = false;
        let in_function = cx.mode == Mode::Function || self.state.in_function();
        self.emulation_changed |= walk::sets_emulation(simple, in_function);
        self.alias_option(&name, args);
        match name.as_str() {
            "return" => {
                let table = self.state.alias_table.clone();
                if let Some(exits) = self.alias_exits.last_mut() {
                    exits.push(table);
                }
                (None, true)
            }
            "exit" => (None, true),
            "source" | "." if simple.assigns.is_empty() => rules::source(self, args, rewrite, cx),
            "cached_eval" => rules::cached_eval(self, args, rewrite, cx),
            "eval" => rules::eval(self, simple, args, rewrite, cx),
            "emulate" if self.startup => {
                self.refusals.push("emulate is not modeled".to_string());
                (None, false)
            }
            "add_path" | "add_plugin_path" | "add_plugins" | "add_fpath" => {
                rules::helper(self, &name, args, rewrite)
            }
            "_omz_source" => rules::omz_source(self, args, rewrite, cx),
            "alias" | "unalias" | "disable" | "enable" => {
                self.alias_command(&name, args);
                if matches!(name.as_str(), "alias" | "unalias") && self.ledger.collecting.is_none()
                {
                    self.track_aliases(&name, args);
                }
                (None, false)
            }
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

    /// Aliases set in place; deferred code sets them after the bundle is parsed.
    fn track_aliases(&mut self, command: &str, args: &[String]) {
        let values: Vec<String> = args
            .iter()
            .filter_map(|arg| expand::scalar(arg, &self.state))
            .collect();
        let flags: String = values
            .iter()
            .filter(|value| value.starts_with('-'))
            .flat_map(|value| value.chars().skip(1))
            .collect();
        let operands = values.iter().filter(|value| !value.starts_with(['-', '+']));
        match command {
            _ if flags.contains('s') => {}
            "alias" => self
                .state
                .aliases
                .extend(operands.filter_map(|value| Some(value.split_once('=')?.0.to_string()))),
            _ if flags.contains(['a', 'm']) => self.state.aliases.clear(),
            _ => operands.for_each(|name| {
                self.state.aliases.remove(name);
            }),
        }
    }

    /// Parsed from source, zsh expands an alias in `name()`; `zcompile -U` wordcode does not.
    fn check_aliases(&mut self, script: &Script, node: &ZshFuncDef, cx: &Cx) {
        let line = script.text.lines().nth(self.line.saturating_sub(1));
        for name in node.names.iter().map(|name| walk::text(name)) {
            if self.state.aliases.contains(&name)
                && !line.is_some_and(|line| walk::defines_with_keyword(line, &name))
            {
                let place = self
                    .location(cx)
                    .or_else(|| cx.origin.map(|origin| origin.display().to_string()))
                    .unwrap_or_else(|| "generated code".to_string());
                self.redefined
                    .push(format!("{place}: {name}: alias redefined as function"));
            }
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
                    self.record_assign(variable, local);
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
        self.record_assign(&assign.name, local);
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

    /// Code that runs here without the compiler following it.
    fn forget(&mut self, lists: &[ZshList]) {
        self.record_unfollowed(lists);
        self.alias_unfollowed(lists);
        for name in walk::assigned(lists) {
            self.state.forget(&name);
        }
        self.emulation_changed |= walk::changes_emulation_in(lists, self.state.in_function());
    }

    /// Whether a condition succeeds, when the build decides it from what `decide` allows.
    fn program_truth(&self, program: &ZshProgram, decide: Decide) -> Option<bool> {
        let [list] = program.lists.as_slice() else {
            return None;
        };
        if list.flags.async_ {
            return None;
        }
        let mut node = &list.sublist;
        let mut status = self.pipe_truth(node, decide)?;
        while let Some((operator, next)) = &node.next {
            node = next;
            if (*operator == SublistOp::And) == status {
                status = self.pipe_truth(node, decide)?;
            }
        }
        Some(status)
    }

    fn pipe_truth(&self, node: &ZshSublist, decide: Decide) -> Option<bool> {
        if node.pipe.next.is_some() || node.flags.coproc {
            return None;
        }
        self.truth(&node.pipe.cmd, decide)
            .map(|truth| truth != node.flags.not)
    }

    fn truth(&self, cmd: &ZshCommand, decide: Decide) -> Option<bool> {
        match cmd {
            ZshCommand::Cond(cond) => self.cond(cond, decide),
            ZshCommand::Arith(expr) => {
                let expr = walk::text(expr);
                let (name, negated) = walk::command_test(&expr)?;
                Some(self.state.command(name)?.is_some() != negated)
            }
            ZshCommand::Simple(simple) if simple.assigns.is_empty() && simple.redirs.is_empty() => {
                let words = walk::words(simple);
                match words.as_slice() {
                    [word] if word == "true" || word == ":" => Some(true),
                    [word] if word == "false" => Some(false),
                    [word, name] if word == "has_cmd" && self.helper_matches("has_cmd") => {
                        let name = expand::scalar(name, self.env(decide))?;
                        Some(self.state.command(&name)?.is_some())
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn cond(&self, cond: &ZshCond, decide: Decide) -> Option<bool> {
        let env = self.env(decide);
        match cond {
            ZshCond::Not(inner) => self.cond(inner, decide).map(|truth| !truth),
            ZshCond::And(left, right) => match self.cond(left, decide)? {
                false => Some(false),
                true => self.cond(right, decide),
            },
            ZshCond::Or(left, right) => match self.cond(left, decide)? {
                true => Some(true),
                false => self.cond(right, decide),
            },
            ZshCond::Unary(operator, operand) => {
                let value = expand::scalar(&walk::text(operand), env)?;
                match walk::text(operator).as_str() {
                    "-n" => Some(!value.is_empty()),
                    "-z" => Some(value.is_empty()),
                    _ if decide == Decide::Commands || !value.starts_with('/') => None,
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
                let left = expand::scalar(&walk::text(left), env)?;
                let pattern = expand::pattern(&walk::text(right), env)?;
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

    /// The parameters a decision may read.
    fn env(&self, decide: Decide) -> &dyn Env {
        match decide {
            Decide::State => &self.state,
            Decide::Commands => &Unknown,
        }
    }

    pub(crate) fn helper_matches(&self, name: &str) -> bool {
        self.state
            .functions
            .get(name)
            .is_some_and(|function| rules::expected(name) == Some(function.fingerprint))
    }

    /// Folds substitutions and keeps `$0` and `return` meaning what they did
    /// in the file this code came from, within `range`, whole statements
    /// starting at line `line`.
    fn text_layer(
        &mut self,
        script: &Script,
        range: Range<usize>,
        line: usize,
        lists: &[ZshList],
        edits: &mut Edits,
        cx: &Cx,
    ) {
        if cx.analysis {
            return;
        }
        let definition =
            lists.len() == 1 && matches!(walk::sole(&lists[0]), Some(ZshCommand::FuncDef(_)));
        if definition {
            edits.functions.push(range.clone());
        }
        let layer = Layer {
            range,
            line,
            lists,
            definition,
        };
        if !self.text_edits(script, &layer, edits, cx, true) {
            self.text_edits(script, &layer, edits, cx, false);
        }
    }

    /// The text layer's edits, with `eval` of folded output in function
    /// bodies inlined when `inline`; false when inlined code did not check.
    fn text_edits(
        &mut self,
        script: &Script,
        layer: &Layer,
        edits: &mut Edits,
        cx: &Cx,
        inline: bool,
    ) -> bool {
        let Layer {
            range,
            line,
            lists,
            definition,
        } = layer;
        let text = &script.text[range.clone()];
        let scan = scan::scan(text);
        if scan.unterminated {
            return true;
        }
        let functions = *definition || walk::contains_function(lists);
        let place = rules::Place {
            current: !functions && self.loops == 0,
            top_level: !functions
                && cx.mode != Mode::Function
                && !self.state.in_function()
                && self.ledger.collecting.is_none(),
        };
        let body = match lists.first().and_then(walk::sole) {
            Some(ZshCommand::FuncDef(node)) if *definition && node.auto_call_args.is_none() => {
                Some(&node.body.lists)
            }
            _ => None,
        };
        let mut local: Vec<(Range<usize>, String, Kind)> = Vec::new();
        let mut values = BTreeMap::new();
        let mut inlined: Vec<(String, ZshProgram)> = Vec::new();
        for substitution in &scan.substitutions {
            let inner = &text[substitution.inner.clone()];
            let original = &text[substitution.range.clone()];
            let evaluated = body
                .filter(|_| inline)
                .and_then(|body| self.inline_eval(text, substitution, body));
            if let Some((statement, value, program)) = evaluated {
                let code = format!("{{\n{value}\n}}");
                values.insert(statement.start, value);
                inlined.push((format!("\"{original}\""), program));
                local.push((statement, code, Kind::Inline));
            } else if let Some((replacement, value)) = self.fold(inner, substitution) {
                values.insert(substitution.range.start, value);
                local.push((substitution.range.clone(), replacement, Kind::Fold));
            } else if let Some((replacement, helper)) =
                rules::substitute(self, inner, original, place)
            {
                local.push((
                    substitution.range.clone(),
                    replacement,
                    Kind::Native(helper),
                ));
            }
        }
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
        if cx.mode.file_scoped() && !definition {
            let found = walk::returns(lists).unwrap_or_default();
            let file_level = found.iter().filter(|site| site.depth.is_some()).count();
            let loops = self.loops - self.loop_floor;
            if file_level > 0
                && let Some(rewrites) = return_edits(text, &scan, &found, loops)
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
            return true;
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
        let replaced;
        let expected = if inlined.is_empty() {
            *lists
        } else {
            let evaluated = |simple: &ZshSimple| {
                let words = walk::words(simple);
                let [eval, arg] = words.as_slice() else {
                    return None;
                };
                let plain = simple.assigns.is_empty() && simple.redirs.is_empty();
                inlined
                    .iter()
                    .find(|(quoted, _)| plain && eval == "eval" && quoted == arg)
                    .map(|(_, program)| ZshCommand::Cursh(Box::new(program.clone())))
            };
            let Some(lists) = walk::replace_in_functions(lists, &evaluated) else {
                return false;
            };
            replaced = lists;
            &replaced
        };
        let same = script::parse(&edited).is_ok_and(|program| {
            program.lists.len() == expected.len()
                && program
                    .lists
                    .iter()
                    .zip(expected)
                    .all(|(left, right)| script::shape(left) == script::shape(right))
        });
        if !same {
            return inlined.is_empty();
        }
        let base = range.start;
        for zero in &scan.zero_refs {
            let folded = accepted.iter().any(|(range, _, kind)| {
                matches!(kind, Kind::Fold | Kind::Inline)
                    && range.start <= zero.range.start
                    && zero.range.end <= range.end
            });
            if folded {
                edits.zero_done.push(base + zero.range.start);
            }
        }
        for (range, replacement, kind) in accepted {
            if matches!(kind, Kind::Fold | Kind::Inline)
                && let Some(value) = values.get(&range.start)
            {
                self.line = line + text[..range.start].matches('\n').count();
                self.define_evaluated(value, cx);
            }
            match kind {
                Kind::Fold | Kind::Inline => self.folded += 1,
                Kind::Zero => edits.zero_done.push(base + range.start),
                Kind::Native(Some(helper)) if !self.helpers.contains(&helper) => {
                    self.helpers.push(helper)
                }
                Kind::Native(_) | Kind::Return => {}
            }
            edits.replace_bytes(base + range.start..base + range.end, replacement);
        }
        edits.returns_rewritten += returns;
        true
    }

    /// `eval "$(command)"` in the function body `body`, for a listed command:
    /// the statement and the code to run in its place, parsed with the bundle
    /// rather than on each call, when that code means the same either way.
    fn inline_eval(
        &mut self,
        text: &str,
        substitution: &scan::Substitution,
        body: &[ZshList],
    ) -> Option<(Range<usize>, String, ZshProgram)> {
        if !substitution.quoted || substitution.in_param {
            return None;
        }
        let statement = eval_statement(text, substitution)?;
        let output = self.fold_output(&text[substitution.inner.clone()])?;
        let Script {
            text: value,
            program,
            ..
        } = self
            .parse_generated(output)
            .ok()
            .filter(|script| !script.program.lists.is_empty())?;
        let top = walk::top_level_text(&program.lists);
        let frames = EVAL_FRAMES.iter().any(|name| top.contains(name));
        let aliased = walk::function_names(&program.lists)
            .iter()
            .any(|name| self.state.aliases.contains(name))
            // zsh expands aliases in it when the function runs
            || alias::words(&value).is_none_or(|words| {
                alias::mentioned(&value, &words, &self.state.alias_table, &self.alias_seen)
            });
        if frames || aliased || walk::sets_options(body) || self.parse_options_changed() {
            return None;
        }
        self.emulation_changed |= walk::changes_emulation_in(&program.lists, true);
        Some((statement, value, program))
    }

    /// Generated code, parsed; respelled where zsh reads `((( a )) && b)` as
    /// a subshell and the parser does not, once zsh reads both alike.
    pub(crate) fn parse_generated(&mut self, text: String) -> Result<Script, String> {
        let error = match Script::parse(text.clone()) {
            Ok(script) => return Ok(script),
            Err(error) => error,
        };
        let respelled = script::respace(&text).and_then(|text| Script::parse(text).ok());
        match (respelled, &self.zsh) {
            (Some(script), Some(zsh))
                if wordcode::same_code(zsh, &text, &script.text, &mut self.folder.cache) =>
            {
                Ok(script)
            }
            _ => Err(error),
        }
    }

    /// Whether startup code before here may have changed how zsh parses code.
    fn parse_options_changed(&self) -> bool {
        self.emulation_changed
            || self.ledger.events.iter().any(|(_, (kind, name))| {
                *kind == "option" && (name == "*" || PARSE_OPTIONS.contains(&name.as_str()))
            })
    }

    /// Functions a folded value defines when evaluated, reported where it is evaluated.
    fn define_evaluated(&mut self, value: &str, cx: &Cx) {
        let Some(location) = self.location(cx) else {
            return;
        };
        let names = match script::parse(value) {
            Ok(program) => walk::function_names(&program.lists),
            Err(_) => walk::declared_names(value),
        };
        self.define(names, &location);
    }

    /// `${__zbN}` and the value, for a listed command's output.
    fn fold(&mut self, inner: &str, substitution: &scan::Substitution) -> Option<(String, String)> {
        let value = self.fold_output(inner)?;
        if !substitution.quoted && !substitution.in_param && value.contains([' ', '\t', '\n', '\0'])
        {
            return None;
        }
        Some((
            format!("${{{}}}", self.constants.name(value.clone())),
            value,
        ))
    }

    /// A listed command's output, as its substitution `inner` expands.
    fn fold_output(&mut self, inner: &str) -> Option<String> {
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
        if !self.folder.applies(&argv, &self.state) {
            return None;
        }
        match self.folder.run(&argv, &self.state) {
            Ok(output) => Some(output.trim_end_matches('\n').to_string()),
            Err(error) => {
                self.note(format!("fold {error}"));
                None
            }
        }
    }
}

/// What `eval` changes about the code it runs: the call frames it adds and
/// line numbers relative to its string.
const EVAL_FRAMES: &[&str] = &[
    "funcstack",
    "functrace",
    "funcfiletrace",
    "funcsourcetrace",
    "zsh_eval_context",
    "ZSH_EVAL_CONTEXT",
    "LINENO",
    "%N",
    "%i",
    "%I",
];

/// Options that change how zsh parses code, as `option_name` spells them.
const PARSE_OPTIONS: &[&str] = &[
    "aliases",
    "cshjunkieloops",
    "cshjunkiequotes",
    "ignorebraces",
    "ignoreclosebraces",
    "kshglob",
    "multibyte",
    "multifuncdef",
    "posixaliases",
    "posixidentifiers",
    "rcquotes",
    "shglob",
    "shortloops",
    "shortrepeat",
];

/// What a build-time decision may read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Decide {
    /// Tracked state and files: enough to follow what code sets.
    State,
    /// The search path and constants: enough to drop code, as tracked
    /// variables miss what functions, `read` and runtime code set.
    Commands,
}

/// No parameter is known.
struct Unknown;

impl Env for Unknown {
    fn var(&self, _: &str) -> Var {
        Var::Unknown
    }
}

/// Statements of one text layer pass.
struct Layer<'a> {
    range: Range<usize>,
    line: usize,
    lists: &'a [ZshList],
    definition: bool,
}

/// The arm of a decided `if` chain that runs, if any, and the file-level
/// returns of the code dropped with the rest.
struct Kept {
    index: Option<usize>,
    body: Option<Range<usize>>,
    returns: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fold,
    /// `eval` of folded output, as the code itself.
    Inline,
    Zero,
    Return,
    Native(Option<&'static str>),
}

/// The `eval "$(...)"` statement around a quoted substitution.
pub(crate) fn eval_statement(
    text: &str,
    substitution: &scan::Substitution,
) -> Option<Range<usize>> {
    let before = text[..substitution.range.start].strip_suffix('"')?;
    let head = before.trim_end_matches([' ', '\t']);
    let start = head.strip_suffix("eval")?.len();
    let bounded = text[..start]
        .chars()
        .next_back()
        .is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '{' | '(' | '&' | '|'));
    let end = substitution.range.end;
    (bounded && head.len() < before.len() && text[end..].starts_with('"')).then_some(start..end + 1)
}

/// Where `command` starts in `text`, the source of `list`: `test && command`
/// or `test || command`, found where both halves parse back alone.
fn test_end(text: &str, list: &ZshList, command: &ZshList) -> Option<usize> {
    let mut test = list.clone();
    test.sublist.next = None;
    let (test, command) = (script::structure(&test), script::structure(command));
    let reparses = |source: &str, expected: &serde_json::Value| {
        script::parse(source)
            .is_ok_and(|program| matches!(program.lists.as_slice(), [only] if script::structure(only) == *expected))
    };
    let split = text
        .match_indices("&&")
        .chain(text.match_indices("||"))
        .map(|(at, _)| at + 2)
        .find(|&at| reparses(&text[..at - 2], &test) && reparses(&text[at..], &command))?;
    let mut rest = &text[split..];
    loop {
        let trimmed = rest
            .trim_start_matches([' ', '\t', '\n'])
            .trim_start_matches("\\\n");
        if trimmed.len() == rest.len() {
            break;
        }
        rest = trimmed;
    }
    Some(text.len() - rest.len())
}

/// Accounts for code in `range` a decided test drops, all but `kept`: its
/// `returns` file-level returns and its `$0` references are gone.
fn drop_code(
    script: &Script,
    range: Range<usize>,
    kept: Option<&Range<usize>>,
    returns: usize,
    edits: &mut Edits,
) {
    edits.returns_omitted += returns;
    for zero in scan::scan(&script.text[range.clone()]).zero_refs {
        let start = range.start + zero.range.start;
        if !kept.is_some_and(|kept| kept.contains(&start)) {
            edits.zero_done.push(start);
        }
    }
}

/// File-level returns of dropped code, which only file-scoped code rewrites.
fn dropped_returns(lists: &[ZshList], cx: &Cx) -> usize {
    match cx.mode.file_scoped() {
        true => walk::file_returns(lists).unwrap_or_default(),
        false => 0,
    }
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

/// `break` for each file-level return; `loops` are its file's loops around `text`.
fn return_edits(
    text: &str,
    scan: &scan::Scan,
    returns: &[walk::Return],
    loops: usize,
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
        let Some(depth) = found.depth.map(|depth| depth + loops) else {
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

pub(crate) fn unique_paths(values: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

pub(crate) fn unique(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}
