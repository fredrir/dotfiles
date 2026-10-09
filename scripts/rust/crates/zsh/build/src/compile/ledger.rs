//! What startup code defines and calls, in execution order, so deferred code
//! stays in place when code that runs before it at runtime depends on it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use zshrs_parse::parser::{ZshCommand, ZshList, ZshSimple};

use super::Compiler;
use super::walk::{self, Visit};
use crate::expand::{self, Env, Var};

/// Any name of a kind, for names only known at runtime.
const ANY: &str = "*";

/// Filled by `compinit`, read by completion code that runs after it.
pub(crate) const COMPS: &str = "_comps";

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
    /// the code only knows at runtime may be any name of its kind.
    pub fn conflict(&self, touch: Touch, (kind, name): &Key) -> Option<String> {
        let found = |set: &BTreeSet<Key>| {
            set.iter()
                .find(|own| own.0 == *kind && (own.1 == *name || own.1 == ANY))
                .map(|own| match own.1 == ANY {
                    true => format!("{kind} names set at runtime, {kind} {name} changed later"),
                    false => format!("{kind} {name} changed later"),
                })
        };
        found(&self.defined).or_else(|| {
            (touch != Touch::Call)
                .then(|| found(&self.called))
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
    bodies: BTreeMap<String, Names>,
    /// The deferred `compinit` that `compdef` calls queue behind.
    pub compinit: Option<usize>,
}

/// A ledger position to roll back to after code that never runs.
pub(crate) struct Mark {
    events: usize,
    units: usize,
    collecting: Option<Names>,
}

impl Ledger {
    pub fn mark(&self) -> Mark {
        Mark {
            events: self.events.len(),
            units: self.units.len(),
            collecting: self.collecting.clone(),
        }
    }

    pub fn reset(&mut self, mark: Mark) {
        self.truncate((mark.events, mark.units));
        self.collecting = mark.collecting;
    }

    pub fn len(&self) -> (usize, usize) {
        (self.events.len(), self.units.len())
    }

    pub fn truncate(&mut self, (events, units): (usize, usize)) {
        self.events.truncate(events);
        self.units.truncate(units);
        self.compinit = self.compinit.filter(|unit| *unit < units);
    }

    pub fn event(&mut self, touch: Touch, key: Key) {
        let touch = match (touch, key.0, self.compinit) {
            (Touch::Define, "completion", Some(unit)) => Touch::Queued(unit),
            _ => touch,
        };
        self.events.push((touch, key));
    }

    /// The touches of calling `name`, through the functions its body calls.
    fn calling(&self, name: &str) -> Vec<(Touch, Key)> {
        let mut seen = BTreeSet::from([name.to_string()]);
        let mut pending = vec![name.to_string()];
        let mut out = Vec::new();
        while let Some(name) = pending.pop() {
            let Some(names) = self.bodies.get(&name) else {
                continue;
            };
            for (touch, key) in names.touches() {
                if let (Touch::Call, ("function", callee)) = (touch, key)
                    && seen.insert(callee.clone())
                {
                    pending.push(callee.clone());
                }
                out.push((touch, key.clone()));
            }
        }
        out
    }
}

impl Compiler {
    pub(super) fn record_in_place(&mut self, names: &Names) {
        for (touch, key) in names.touches() {
            self.ledger.event(touch, key.clone());
        }
    }

    fn touch(&mut self, touch: Touch, key: Key) {
        let mut touches = vec![(touch, key)];
        if let (Touch::Call, ("function", name)) = (touches[0].0, &touches[0].1) {
            touches.extend(self.ledger.calling(name));
        }
        for (touch, key) in touches {
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
        self.ledger
            .bodies
            .insert(name.to_string(), body_names(body));
        self.touch(Touch::Define, ("function", name.to_string()));
    }

    /// A global assignment, unless the name is local to a function being compiled.
    pub(super) fn record_assign(&mut self, name: &str, local: bool) {
        if !local && !self.state.is_local(name) {
            self.touch(Touch::Define, ("var", var_name(name)));
        }
    }

    pub(super) fn record_command(&mut self, name: &str, args: &[String]) {
        if self.helper_matches(name) {
            for var in helper_vars(name) {
                self.record_assign(var, false);
            }
            return;
        }
        if args.iter().any(|arg| arg.contains(COMPS)) {
            self.touch(Touch::Call, ("var", COMPS.to_string()));
        }
        let values: Vec<String> = args
            .iter()
            .map(|arg| expand::scalar(arg, &self.state).unwrap_or_else(|| arg.clone()))
            .collect();
        for (touch, key) in command_touches(name, &values) {
            match key {
                ("var", name) => self.record_assign(&name, false),
                key => self.touch(touch, key),
            }
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
    match name {
        "alias" => operands
            .filter_map(|operand| operand.split_once('=').map(|(name, _)| name))
            .map(|alias| define("alias", alias))
            .collect(),
        "unalias" => operands.map(|alias| define("alias", alias)).collect(),
        "unfunction" => operands.map(|name| define("function", name)).collect(),
        "unset" if flags.contains('f') => operands.map(|name| define("function", name)).collect(),
        "unset" => operands.map(|name| define("var", name)).collect(),
        "setopt" | "unsetopt" => operands
            .map(|option| define("option", &option_name(option)))
            .collect(),
        "zstyle" => {
            let styled = match values.first().map(String::as_str) {
                Some("-e") => values.get(1..3),
                Some(value) if value.starts_with('-') => None,
                _ => values.get(..2),
            };
            styled
                .map(|pair| define("zstyle", &pair.join(" ")))
                .into_iter()
                .collect()
        }
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
        _ if static_name(name) == ANY => Vec::new(),
        _ => vec![(Touch::Call, ("function", name.to_string()))],
    }
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

/// A name whose value is unknown until it runs stands for every name of its kind.
fn static_name(name: &str) -> String {
    if name.is_empty() || name.contains(['$', '`', '"', '\'', '(']) {
        ANY.to_string()
    } else {
        name.to_string()
    }
}

/// A parameter by name, its subscript dropped and tied names joined.
fn var_name(name: &str) -> String {
    match name.split('[').next().unwrap_or(name) {
        "PATH" => "path".to_string(),
        "FPATH" => "fpath".to_string(),
        name => name.to_string(),
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

/// What running a function body touches, read from its text.
pub(crate) fn body_names(lists: &[ZshList]) -> Names {
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
        .filter(|words| declares_local(words))
        .flat_map(|words| words.iter().skip(1))
        .filter(|word| !word.starts_with(['-', '+']))
        .map(|word| word.split('=').next().unwrap_or(word).to_string())
        .collect();
    let local_options = words.iter().any(|words| match words.split_first() {
        Some((name, args)) if name == "emulate" => args
            .iter()
            .any(|arg| arg.starts_with('-') && arg.contains('L')),
        Some((name, args)) if name == "setopt" => args.iter().any(|arg| local_option(arg)),
        _ => false,
    });
    let mut names = Names::default();
    for (_, visit) in &visits {
        if let Visit::Command(ZshCommand::FuncDef(node)) = visit
            && node.auto_call_args.is_none()
        {
            for name in &node.names {
                names.defined.insert(("function", walk::text(name)));
            }
        }
    }
    for (simple, words) in simples.iter().zip(&words) {
        let values: Vec<String> = words
            .iter()
            .map(|word| expand::scalar(word, &Opaque).unwrap_or_else(|| word.clone()))
            .collect();
        let assigned = simple.words.is_empty().then_some(&simple.assigns);
        let mut touches: Vec<(Touch, Key)> = assigned
            .into_iter()
            .flatten()
            .map(|assign| (Touch::Define, ("var", var_name(&assign.name))))
            .collect();
        if serde_json::to_string(simple).is_ok_and(|json| json.contains(COMPS)) {
            touches.push((Touch::Call, ("var", COMPS.to_string())));
        }
        if let Some((name, args)) = values.split_first() {
            touches.extend(command_touches(name, args));
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
    }
    names
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
