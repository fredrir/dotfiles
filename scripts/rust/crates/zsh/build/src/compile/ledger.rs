//! What startup code defines and calls, in execution order, so deferred code
//! stays in place when code that runs before it at runtime depends on it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use zshrs_parse::parser::{ForList, ZshAssignValue, ZshCommand, ZshFor, ZshList, ZshSimple};

use super::walk::{self, Visit};
use super::{Compiler, bindkey};
use crate::expand::{self, Env, Var};
use crate::{scan, script};

/// Any name of a kind, for names only known at runtime.
pub(super) const ANY: &str = "*";

/// Filled by `compinit`, read by completion code that runs after it.
pub(crate) const COMPS: &str = "_comps";

const ZSTYLE: &str = "zstyle";

/// Option names that make settings made in a function revert when it returns.
const LOCAL_OPTIONS: &[&str] = &["localoptions", "localpatterns", "localtraps"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Touch {
    Define,
    Call,
    /// A `compdef` queued behind the deferred `compinit` of this unit: a
    /// definition only when that unit stays in place.
    Queued(usize),
}

/// A name in one of the shell's tables, such as `alias ll` or `function gcm`.
pub(crate) type Key = (&'static str, String);

#[derive(Debug, Default, Clone)]
pub(crate) struct Names {
    pub defined: BTreeSet<Key>,
    pub called: BTreeSet<Key>,
}

impl Names {
    /// Why a later touch changes what this code defines or relies on; a name
    /// the code only knows at runtime may be any name of its kind, and a
    /// zstyle read depends on each define whose pattern matches its context.
    pub fn conflict(&self, touch: Touch, (kind, name): &Key) -> Option<String> {
        let found = |set: &BTreeSet<Key>, covers: &dyn Fn(&str) -> bool| {
            set.iter()
                .find(|own| own.0 == *kind && covers(&own.1))
                .map(|own| match own.1 == ANY {
                    true => format!("{kind} names set at runtime, {kind} {name} changed later"),
                    false => format!("{kind} {name} changed later"),
                })
        };
        let styles = *kind == ZSTYLE;
        let defined = |own: &str| match styles && touch == Touch::Call {
            true => own == ANY || style_applies(own, name),
            false => covers(kind, own, name),
        };
        let called = |own: &str| match styles {
            true => own == ANY || style_applies(name, own),
            false => covers(kind, own, name),
        };
        found(&self.defined, &defined).or_else(|| {
            (touch != Touch::Call)
                .then(|| found(&self.called, &called))
                .flatten()
        })
    }

    pub fn touches(&self) -> impl Iterator<Item = (Touch, &Key)> {
        let defined = self.defined.iter().map(|key| (Touch::Define, key));
        defined.chain(self.called.iter().map(|key| (Touch::Call, key)))
    }

    pub fn defines_var(&self, name: &str) -> bool {
        self.defined.contains(&("var", name.to_string()))
    }
}

/// What running code touches, and the variables it keeps local, which the
/// functions it calls see as its own.
#[derive(Debug, Clone)]
pub(crate) struct Body {
    pub names: Names,
    pub locals: BTreeSet<String>,
    /// Anonymous functions it runs, each with locals of its own.
    pub anonymous: Vec<Body>,
    /// Functions it defines, by name.
    pub functions: BTreeMap<String, Body>,
}

/// Deferred code: its wrapper text and what it touches.
#[derive(Debug)]
pub(crate) struct Unit {
    pub head: String,
    pub tail: String,
    pub place: PathBuf,
    pub names: Names,
    /// Events recorded before the unit's place.
    pub after: usize,
}

#[derive(Debug, Default)]
pub(crate) struct Ledger {
    /// What the deferred code being compiled touches.
    pub collecting: Option<Names>,
    /// What code that runs in place touches, in order.
    pub events: Vec<(Touch, Key)>,
    pub units: Vec<Unit>,
    /// What calling each function defined so far touches.
    bodies: BTreeMap<String, Body>,
    /// Each change to `bodies` with the body it replaced, to roll back.
    replaced: Vec<(String, Option<Body>)>,
    /// The deferred `compinit` that `compdef` calls queue behind.
    pub compinit: Option<usize>,
}

/// A ledger position to roll back to after code that never runs.
pub(crate) struct Mark {
    len: (usize, usize, usize),
    collecting: Option<Names>,
}

impl Ledger {
    pub fn mark(&self) -> Mark {
        Mark {
            len: self.len(),
            collecting: self.collecting.clone(),
        }
    }

    pub fn reset(&mut self, mark: Mark) {
        self.truncate(mark.len);
        self.collecting = mark.collecting;
    }

    pub fn len(&self) -> (usize, usize, usize) {
        (self.events.len(), self.units.len(), self.replaced.len())
    }

    pub fn truncate(&mut self, (events, units, replaced): (usize, usize, usize)) {
        self.events.truncate(events);
        self.units.truncate(units);
        self.compinit = self.compinit.filter(|unit| *unit < units);
        while self.replaced.len() > replaced {
            let Some((name, body)) = self.replaced.pop() else {
                break;
            };
            match body {
                Some(body) => self.bodies.insert(name, body),
                None => self.bodies.remove(&name),
            };
        }
    }

    /// Sets or forgets the body known for function `name`.
    fn set_body(&mut self, name: &str, body: Option<Body>) {
        let previous = match body {
            Some(body) => self.bodies.insert(name.to_string(), body),
            None => self.bodies.remove(name),
        };
        self.replaced.push((name.to_string(), previous));
    }

    pub fn event(&mut self, touch: Touch, key: Key) {
        let touch = match (touch, key.0, self.compinit) {
            (Touch::Define, "completion", Some(unit)) => Touch::Queued(unit),
            _ => touch,
        };
        self.events.push((touch, key));
    }

    /// What running `body` touches, through the functions it calls, which
    /// also see its locals, and the functions it leaves defined.
    fn running(&self, body: &Body) -> (Vec<(Touch, Key)>, BTreeMap<String, Body>) {
        let mut seen = BTreeSet::new();
        let mut defined: BTreeMap<&str, &Body> = BTreeMap::new();
        let mut pending = vec![(body, BTreeSet::new())];
        let mut out = Vec::new();
        while let Some((body, locals)) = pending.pop() {
            defined.extend(
                body.functions
                    .iter()
                    .map(|(name, body)| (name.as_str(), body)),
            );
            let inner: BTreeSet<String> = locals.union(&body.locals).cloned().collect();
            pending.extend(body.anonymous.iter().map(|body| (body, inner.clone())));
            for (touch, key) in body.names.touches() {
                match (touch, key) {
                    (_, ("var", var)) if locals.contains(var) => continue,
                    (Touch::Call, ("function", callee)) => {
                        let called = defined.get(callee.as_str()).copied();
                        if let Some(called) = called.or_else(|| self.bodies.get(callee))
                            && seen.insert((callee, inner.clone()))
                        {
                            pending.push((called, inner.clone()));
                        }
                    }
                    _ => {}
                }
                out.push((touch, key.clone()));
            }
        }
        let defined = defined
            .into_iter()
            .map(|(name, body)| (name.to_string(), body.clone()))
            .collect();
        (out, defined)
    }
}

impl Compiler {
    pub(super) fn record_in_place(&mut self, names: &Names) {
        for (touch, key) in names.touches() {
            self.ledger.event(touch, key.clone());
        }
    }

    /// A touch, and when it calls a function, what calling it touches; the
    /// functions it defines are known to later calls.
    pub(super) fn touch(&mut self, touch: Touch, key: Key) {
        let called = match (touch, &key) {
            (Touch::Call, ("function", name)) => self.ledger.bodies.get(name),
            _ => None,
        };
        let (touches, functions) = called
            .map(|body| self.ledger.running(body))
            .unwrap_or_default();
        // An `alias` or `setopt` run here is read in full by the compiler
        let assigned = key.0 == "var";
        let touches: Vec<(Touch, Key)> = std::iter::once((touch, key)).chain(touches).collect();
        self.alias_touches(&touches[usize::from(!assigned)..]);
        self.record(touches);
        for (name, body) in functions {
            self.ledger.set_body(&name, Some(body));
        }
    }

    /// Records touches, but for variables local to the function being
    /// compiled; a function defined or removed by name loses the body known
    /// for it until a definition is read.
    fn record(&mut self, touches: Vec<(Touch, Key)>) {
        for (touch, key) in touches {
            if key.0 == "var" && self.state.is_local(&key.1) {
                continue;
            }
            if (touch, key.0) == (Touch::Define, "function")
                && self.ledger.bodies.contains_key(&key.1)
            {
                self.ledger.set_body(&key.1, None);
            }
            match &mut self.ledger.collecting {
                Some(names) if touch == Touch::Call => {
                    names.called.insert(key);
                }
                Some(names) => {
                    names.defined.insert(key);
                }
                None => self.ledger.event(touch, key),
            }
        }
    }

    pub(super) fn record_function(&mut self, name: &str, body: &[ZshList]) {
        self.touch(Touch::Define, ("function", name.to_string()));
        self.ledger.set_body(name, Some(read_body(body, true)));
    }

    /// A global assignment, unless the name is local to a function being compiled.
    pub(super) fn record_assign(&mut self, name: &str, local: bool) {
        if !local && !self.state.is_local(name) {
            self.touch(Touch::Define, assigned(name));
        }
    }

    /// What code the compiler does not follow touches, read from its text;
    /// the functions it defines are known to later calls.
    pub(super) fn record_unfollowed(&mut self, lists: &[ZshList]) {
        let body = read_body(lists, self.state.in_function());
        let (touches, functions) = self.ledger.running(&body);
        self.alias_touches(&touches);
        self.record(touches);
        for (name, body) in functions {
            self.ledger.set_body(&name, Some(body));
        }
    }

    /// The parameters a `for` or `select` loop the compiler follows assigns.
    pub(super) fn record_loop(&mut self, node: &ZshFor) {
        for name in loop_assigned(node) {
            self.record_assign(&name, false);
        }
    }

    /// What a simple command's words and values touch as they expand.
    pub(super) fn record_expansions(&mut self, simple: &ZshSimple) {
        for (touch, key) in expansion_touches(simple) {
            self.touch(touch, key);
        }
    }

    pub(super) fn record_command(&mut self, name: &str, args: &[String]) {
        if self.helper_matches(name) {
            for var in helper_vars(name) {
                self.record_assign(var, false);
            }
            return;
        }
        let values: Vec<String> = args
            .iter()
            .map(|arg| expand::scalar(arg, &self.state).unwrap_or_else(|| arg.clone()))
            .collect();
        for (touch, key) in command_touches(name, &values) {
            self.touch(touch, key);
        }
    }
}

/// What a simple command with these argument values defines or calls.
pub(crate) fn command_touches(name: &str, values: &[String]) -> Vec<(Touch, Key)> {
    let flags: String = values
        .iter()
        .filter(|value| value.starts_with('-'))
        .flat_map(|value| value.chars().skip(1))
        .collect();
    let operands = values
        .iter()
        .map(String::as_str)
        .filter(|value| !value.starts_with(['-', '+']));
    let define = |kind: &'static str, name: &str| (Touch::Define, (kind, static_name(name)));
    let assign = |name: &str| (Touch::Define, assigned(name));
    match name {
        "alias" => operands
            .filter_map(|operand| operand.split_once('=').map(|(name, _)| name))
            .map(|alias| define("alias", alias))
            .collect(),
        "unalias" => operands.map(|alias| define("alias", alias)).collect(),
        "unfunction" => operands.map(|name| define("function", name)).collect(),
        "unset" if flags.contains('f') => operands.map(|name| define("function", name)).collect(),
        "unset" => operands.map(assign).collect(),
        "read" => read_targets(values)
            .iter()
            .map(|name| assign(name))
            .collect(),
        "print" => option_arguments(values, "CfuvxX", 'v')
            .map(assign)
            .collect(),
        "printf" => option_arguments(values, "v", 'v').map(assign).collect(),
        "getopts" => values
            .get(1)
            .map(String::as_str)
            .into_iter()
            .chain(["OPTARG", "OPTIND"])
            .map(assign)
            .collect(),
        "zparseopts" => zparseopts_targets(values)
            .iter()
            .map(|name| assign(name))
            .collect(),
        "let" => values
            .iter()
            .flat_map(|value| arithmetic_assigned(value))
            .map(|name| assign(&name))
            .collect(),
        "setopt" | "unsetopt" => operands
            .map(|option| define("option", &option_name(option)))
            .collect(),
        "zstyle" => zstyle_touches(values),
        "zle" => {
            let widget = match values.first().map(String::as_str) {
                Some("-N" | "-C" | "-D") => values.get(1),
                Some("-A") => values.get(2),
                _ => None,
            };
            widget
                .map(|widget| define("widget", widget))
                .into_iter()
                .collect()
        }
        "compdef" if !flags.contains(['k', 'K', 'p', 'P', 'e']) => values
            .iter()
            .filter(|value| !value.starts_with('-'))
            .skip(usize::from(!flags.contains('d')))
            .map(|command| define("completion", command.split('=').next().unwrap_or(command)))
            .collect(),
        "add-zsh-hook" => operands
            .take(1)
            .map(|hook| define("var", &format!("{hook}_functions")))
            .collect(),
        "bindkey" => bindkey::touches(values),
        "builtin" => values
            .split_first()
            .map(|(name, rest)| command_touches(name, rest))
            .unwrap_or_default(),
        _ if static_name(name) == ANY => Vec::new(),
        _ => vec![(Touch::Call, ("function", name.to_string()))],
    }
}

/// What `zstyle` with these argument values defines, reads or sets: styles
/// as `PATTERN STYLE` or `CONTEXT STYLE`, `*` when only known at runtime.
fn zstyle_touches(values: &[String]) -> Vec<(Touch, Key)> {
    let style = |touch, pair: &[String]| (touch, (ZSTYLE, static_name(&pair.join(" "))));
    let any = |touch| (touch, (ZSTYLE, ANY.to_string()));
    let set = |name: &String| (Touch::Define, assigned(name));
    let Some((option, rest)) = values.split_first() else {
        return Vec::new();
    };
    let pair = rest.get(..2);
    match option.as_str() {
        "-e" | "-" | "--" => pair
            .map(|pair| style(Touch::Define, pair))
            .into_iter()
            .collect(),
        "-s" | "-b" | "-a" => {
            let read = pair.map(|pair| style(Touch::Call, pair));
            read.into_iter().chain(rest.get(2).map(set)).collect()
        }
        "-t" | "-T" | "-m" => pair
            .map(|pair| style(Touch::Call, pair))
            .into_iter()
            .collect(),
        "-g" => {
            let read = rest
                .get(1..3)
                .map_or_else(|| any(Touch::Call), |pair| style(Touch::Call, pair));
            rest.first().map(set).into_iter().chain([read]).collect()
        }
        "-d" => match rest.split_first() {
            Some((pattern, styles)) if !styles.is_empty() => styles
                .iter()
                .map(|name| style(Touch::Define, &[pattern.clone(), name.clone()]))
                .collect(),
            _ => vec![any(Touch::Define)],
        },
        option if option.starts_with('-') => Vec::new(),
        _ => values
            .get(..2)
            .map(|pair| style(Touch::Define, pair))
            .into_iter()
            .collect(),
    }
}

/// Whether the zstyle define `PATTERN STYLE` applies to the read `CONTEXT
/// STYLE`: the same style, its pattern matching the context; a pattern
/// that cannot be read is taken to match.
fn style_applies(define: &str, read: &str) -> bool {
    let (Some((pattern, style)), Some((context, wanted))) =
        (define.rsplit_once(' '), read.rsplit_once(' '))
    else {
        return define == read;
    };
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    style == wanted
        && expand::pattern(pattern, &Opaque)
            .is_none_or(|pattern| pattern.matches_with(context, options))
}

/// Options of a builtin and its operands: options in `takes` take an
/// argument, attached or the next value; options in `numeric` take an
/// attached number, or the next value when it is one.
fn options<'a>(
    values: &'a [String],
    takes: &str,
    numeric: &str,
) -> (Vec<(char, Option<&'a str>)>, Vec<&'a str>) {
    let number = |value: &&str| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
    let mut found = Vec::new();
    let mut rest = values.iter().map(String::as_str).peekable();
    while let Some(cluster) = rest.next_if(|value| value.len() > 1 && value.starts_with('-')) {
        if cluster == "--" {
            break;
        }
        for (at, letter) in cluster.char_indices().skip(1) {
            let attached = &cluster[at + letter.len_utf8()..];
            if takes.contains(letter) {
                let argument = match attached {
                    "" => rest.next(),
                    attached => Some(attached),
                };
                found.push((letter, argument));
                break;
            }
            found.push((letter, None));
            if numeric.contains(letter) {
                if attached.starts_with(|c: char| c.is_ascii_digit()) {
                    break;
                }
                if attached.is_empty() {
                    rest.next_if(number);
                }
            }
        }
    }
    (found, rest.collect())
}

/// The arguments given to option `letter` of a builtin whose options in
/// `takes` take one.
fn option_arguments<'a>(
    values: &'a [String],
    takes: &str,
    letter: char,
) -> impl Iterator<Item = &'a str> {
    let (found, _) = options(values, takes, "");
    found
        .into_iter()
        .filter(move |(option, _)| *option == letter)
        .filter_map(|(_, argument)| argument)
}

/// The parameters `read` with these argument values sets.
fn read_targets(values: &[String]) -> Vec<String> {
    let (found, mut names) = options(values, "du", "kt");
    if let Some(first) = names.first_mut() {
        *first = first.split('?').next().unwrap_or_default();
    }
    names.retain(|name| !name.is_empty());
    match names.is_empty() {
        true if found.iter().any(|(letter, _)| *letter == 'A') => vec!["reply".to_string()],
        true => vec!["REPLY".to_string()],
        false => names.into_iter().map(str::to_string).collect(),
    }
}

/// The parameters `zparseopts` fills: its `-a` and `-A` names and the
/// `=name` of each spec.
fn zparseopts_targets(values: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = values.iter().map(String::as_str);
    let mut specs = Vec::new();
    while let Some(value) = rest.next() {
        match value {
            "-D" | "-E" | "-F" | "-K" | "-M" => {}
            "-" | "--" => break,
            option if option.starts_with("-a") || option.starts_with("-A") => {
                names.extend(match &option[2..] {
                    "" => rest.next(),
                    attached => Some(attached),
                });
            }
            spec => {
                specs.push(spec);
                break;
            }
        }
    }
    for spec in specs.into_iter().chain(rest) {
        if static_name(spec) == ANY {
            names.push(ANY);
            continue;
        }
        let mut escaped = false;
        let split = spec.char_indices().find(|(at, c)| {
            let found = *c == '=' && !escaped && *at > 0;
            escaped = *c == '\\' && !escaped;
            found
        });
        names.extend(split.map(|(at, _)| &spec[at + 1..]));
    }
    names.into_iter().map(str::to_string).collect()
}

/// The parameters a `for` or `select` loop assigns; the parser leaves the
/// words of `for a b (words)` in its names.
pub(crate) fn loop_assigned(node: &ZshFor) -> Vec<String> {
    let var = walk::text(&node.var);
    let names = var.split('(').next().unwrap_or_default().split_whitespace();
    let mut names: Vec<String> = names.map(str::to_string).collect();
    if let ForList::CStyle { init, cond, step } = &node.list {
        for expr in [init, cond, step] {
            names.extend(arithmetic_assigned(&walk::text(expr)));
        }
    }
    names
}

/// The parameters an arithmetic expression assigns, subscripts kept; one
/// named by an expansion is any parameter.
pub(crate) fn arithmetic_assigned(expr: &str) -> Vec<String> {
    const ASSIGN: &[&str] = &[
        "<<=", ">>=", "&&=", "||=", "^^=", "**=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
        "++", "--",
    ];
    let bytes = expr.as_bytes();
    let name_byte = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_';
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let previous = index.checked_sub(1).map(|at| bytes[at]);
        let start = index;
        let expansion = bytes[index] == b'$';
        if expansion {
            index += 1;
            if bytes.get(index) == Some(&b'{') {
                index += expr[index..].find('}').map_or(1, |close| close + 1);
            } else {
                while bytes.get(index).is_some_and(name_byte) {
                    index += 1;
                }
            }
        } else if (bytes[index].is_ascii_alphabetic() || bytes[index] == b'_')
            && !previous.is_some_and(|byte| name_byte(&byte) || matches!(byte, b'#' | b'.'))
        {
            while bytes.get(index).is_some_and(name_byte) {
                index += 1;
            }
            if bytes.get(index) == Some(&b'[') {
                index += expr[index..].find(']').map_or(0, |close| close + 1);
            }
        } else {
            index += 1;
            continue;
        }
        let after = expr[index..].trim_start();
        let before = expr[..start].trim_end();
        let assigns = ASSIGN.iter().any(|operator| after.starts_with(operator))
            || (after.starts_with('=') && !after.starts_with("=="))
            || before.ends_with("++")
            || before.ends_with("--");
        match (assigns, expansion) {
            (true, true) => out.push(ANY.to_string()),
            (true, false) => out.push(expr[start..index].to_string()),
            _ => {}
        }
    }
    out
}

/// What a recognized helper sets; the code it loads is compiled and recorded on its
/// own, and `defer` called by deferred work queues right after it.
fn helper_vars(name: &str) -> &'static [&'static str] {
    match name {
        "add_path" => &["path"],
        "add_fpath" => &["fpath"],
        "add_plugin_path" => &["zsh_plugin_path"],
        "add_plugins" => &["plugins", "zsh_plugin_sources"],
        _ => &[],
    }
}

/// Whether a name of this code stands for `name` of `kind`.
fn covers(kind: &str, own: &str, name: &str) -> bool {
    own == name || own == ANY || (kind == bindkey::KIND && bindkey::covers(own, name))
}

/// What a simple command's words and values touch as they expand: key
/// bindings `$(bindkey ...)` reads, the table `compinit` fills when they
/// name it, and what `${name:=word}` forms and `$(( ))` assign.
pub(crate) fn expansion_touches(simple: &ZshSimple) -> Vec<(Touch, Key)> {
    let mut touches: Vec<(Touch, Key)> = substitution_reads(simple)
        .into_iter()
        .map(|key| (Touch::Call, key))
        .collect();
    if serde_json::to_string(simple).is_ok_and(|json| json.contains(COMPS)) {
        touches.push((Touch::Call, ("var", COMPS.to_string())));
    }
    for text in texts(simple) {
        let scan = scan::scan(&text);
        let arithmetic = scan
            .arithmetic
            .iter()
            .flat_map(|range| arithmetic_assigned(&text[range.clone()]));
        let names: Vec<String> = scan.assigned.iter().cloned().chain(arithmetic).collect();
        touches.extend(names.iter().map(|name| (Touch::Define, assigned(name))));
    }
    touches
}

/// The words and assigned values of a simple command, as written.
fn texts(simple: &ZshSimple) -> Vec<String> {
    let assigned = simple
        .assigns
        .iter()
        .flat_map(|assign| match &assign.value {
            ZshAssignValue::Scalar(value) => vec![walk::text(value)],
            ZshAssignValue::Array(values) => values.iter().map(|value| walk::text(value)).collect(),
        });
    walk::words(simple).into_iter().chain(assigned).collect()
}

/// Key bindings that `$(bindkey ...)` substitutions in a simple command read.
fn substitution_reads(simple: &ZshSimple) -> Vec<Key> {
    let mut out = Vec::new();
    for text in texts(simple) {
        for substitution in scan::scan(&text).substitutions {
            let inner = &text[substitution.inner];
            let Some(program) = inner
                .contains("bindkey")
                .then(|| script::parse(inner).ok())
                .flatten()
            else {
                continue;
            };
            let mut visits = Vec::new();
            walk::visit(&program.lists, 0, &mut visits);
            for (_, visit) in visits {
                let Visit::Command(ZshCommand::Simple(command)) = visit else {
                    continue;
                };
                let values: Vec<String> = walk::words(command)
                    .iter()
                    .map(|word| expand::scalar(word, &Opaque).unwrap_or_else(|| word.clone()))
                    .collect();
                if let Some((name, args)) = values.split_first() {
                    out.extend(
                        command_touches(name, args)
                            .into_iter()
                            .filter_map(|(_, key)| (key.0 == bindkey::KIND).then_some(key)),
                    );
                }
                out.extend(substitution_reads(command));
            }
        }
    }
    out
}

/// A name whose value is unknown until it runs stands for every name of its kind.
pub(super) fn static_name(name: &str) -> String {
    if name.is_empty() || name.contains(['$', '`', '"', '\'', '(']) {
        ANY.to_string()
    } else {
        name.to_string()
    }
}

/// What assigning `name` changes: an entry of the function, alias or
/// completion table, else the parameter, its subscript dropped and tied
/// names joined; a name only known at runtime stands for any of its kind.
pub(crate) fn assigned(name: &str) -> Key {
    let name = walk::text(name);
    let (base, subscript) = match name.split_once('[') {
        Some((base, rest)) => (base, Some(rest.strip_suffix(']').unwrap_or(rest))),
        None => (name.as_str(), None),
    };
    let entry = |kind| {
        let entry = subscript.and_then(|subscript| expand::scalar(subscript, &Opaque));
        (
            kind,
            entry.map_or_else(|| ANY.to_string(), |entry| static_name(&entry)),
        )
    };
    match static_name(base).as_str() {
        "functions" => entry("function"),
        "aliases" | "galiases" | "saliases" => entry("alias"),
        COMPS => entry("completion"),
        "PATH" => ("var", "path".to_string()),
        "FPATH" => ("var", "fpath".to_string()),
        base => ("var", base.to_string()),
    }
}

/// An option name as zsh compares them: case, underscores and a `no` prefix ignored.
pub(crate) fn option_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect();
    name.strip_prefix("no").map_or(name.clone(), str::to_string)
}

pub(crate) fn local_option(arg: &str) -> bool {
    LOCAL_OPTIONS.contains(&option_name(arg).as_str())
}

/// Knows nothing: function bodies are read before their arguments exist.
struct Opaque;

impl Env for Opaque {
    fn var(&self, _: &str) -> Var {
        Var::Unknown
    }
}

/// What running `lists` touches, read from its text; in a function, the
/// variables and options it declares local stay its own.
pub(crate) fn read_body(lists: &[ZshList], in_function: bool) -> Body {
    let mut visits = Vec::new();
    walk::visit(lists, 0, &mut visits);
    let simples: Vec<&ZshSimple> = visits
        .iter()
        .filter_map(|(_, visit)| match visit {
            Visit::Command(ZshCommand::Simple(simple)) => Some(simple),
            _ => None,
        })
        .collect();
    let words: Vec<Vec<String>> = simples.iter().map(|simple| walk::words(simple)).collect();
    let locals: BTreeSet<String> = words
        .iter()
        .filter(|words| in_function && declares_local(words))
        .flat_map(|words| words.iter().skip(1))
        .filter(|word| !word.starts_with(['-', '+']))
        .map(|word| word.split('=').next().unwrap_or(word).to_string())
        .collect();
    let local_options = in_function
        && words.iter().any(|words| match words.split_first() {
            Some((name, args)) if name == "emulate" => args
                .iter()
                .any(|arg| arg.starts_with('-') && arg.contains('L')),
            Some((name, args)) if name == "setopt" => args.iter().any(|arg| local_option(arg)),
            _ => false,
        });
    let mut names = Names::default();
    let mut anonymous = Vec::new();
    let mut functions = BTreeMap::new();
    let mut touches: Vec<(Touch, Key)> = Vec::new();
    for (_, visit) in &visits {
        match visit {
            Visit::Command(ZshCommand::FuncDef(node)) if node.auto_call_args.is_some() => {
                anonymous.push(read_body(&node.body.lists, true));
            }
            Visit::Command(ZshCommand::FuncDef(node)) => {
                let body = read_body(&node.body.lists, true);
                for name in &node.names {
                    names.defined.insert(("function", walk::text(name)));
                    functions.insert(walk::text(name), body.clone());
                }
            }
            Visit::Command(ZshCommand::For(node)) => {
                let assigns = loop_assigned(node);
                touches.extend(assigns.iter().map(|name| (Touch::Define, assigned(name))));
            }
            Visit::Command(ZshCommand::Arith(expr)) => {
                let assigns = arithmetic_assigned(&walk::text(expr));
                touches.extend(assigns.iter().map(|name| (Touch::Define, assigned(name))));
            }
            _ => {}
        }
    }
    for (simple, words) in simples.iter().zip(&words) {
        let values: Vec<String> = words
            .iter()
            .map(|word| expand::scalar(word, &Opaque).unwrap_or_else(|| word.clone()))
            .collect();
        let bare = simple.words.is_empty().then_some(&simple.assigns);
        touches.extend(
            bare.into_iter()
                .flatten()
                .map(|assign| (Touch::Define, assigned(&assign.name))),
        );
        if words
            .first()
            .is_some_and(|name| walk::DECLARATIONS.contains(&name.as_str()))
        {
            touches.extend(
                values[1..]
                    .iter()
                    .filter(|value| !value.starts_with(['-', '+']))
                    .filter_map(|value| value.split_once('='))
                    .map(|(name, _)| (Touch::Define, assigned(name))),
            );
        }
        touches.extend(expansion_touches(simple));
        if let Some((name, args)) = values.split_first() {
            touches.extend(command_touches(name, args));
        }
    }
    for (touch, key) in touches {
        match (touch, key) {
            (_, ("var", name)) if locals.contains(&name) => {}
            (_, ("option", _)) if local_options => {}
            (Touch::Call, key) => {
                names.called.insert(key);
            }
            (_, key) => {
                names.defined.insert(key);
            }
        }
    }
    Body {
        names,
        locals,
        anonymous,
        functions,
    }
}

fn declares_local(words: &[String]) -> bool {
    let flags: String = words
        .iter()
        .skip(1)
        .filter(|word| word.starts_with('-'))
        .flat_map(|word| word.chars().skip(1))
        .collect();
    words.first().is_some_and(|name| {
        name == "local"
            || (walk::DECLARATIONS.contains(&name.as_str())
                && name != "export"
                && !flags.contains('g'))
    })
}

#[cfg(test)]
#[path = "../../tests/unit/compile/ledger_tests.rs"]
mod tests;
