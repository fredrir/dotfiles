//! Questions about parsed statements that do not depend on build state.

use serde_json::Value;
use zshrs_parse::lexer::untokenize_preserve_quotes;
use zshrs_parse::parser::{
    ForList, ZshAssign, ZshCommand, ZshFuncDef, ZshList, ZshPipe, ZshProgram, ZshSimple,
};

pub fn words(simple: &ZshSimple) -> Vec<String> {
    simple
        .words
        .iter()
        .map(|word| untokenize_preserve_quotes(word))
        .collect()
}

pub fn text(word: &str) -> String {
    untokenize_preserve_quotes(word)
}

/// The command a list runs when it is one plain pipeline stage.
pub fn sole(list: &ZshList) -> Option<&ZshCommand> {
    if list.flags.async_ || list.sublist.next.is_some() || list.sublist.flags.not {
        return None;
    }
    let pipe = &list.sublist.pipe;
    pipe.next.is_none().then_some(&pipe.cmd)
}

pub fn sole_simple(list: &ZshList) -> Option<&ZshSimple> {
    match sole(list)? {
        ZshCommand::Simple(simple) => Some(simple),
        _ => None,
    }
}

fn pipes(list: &ZshList) -> Vec<&ZshPipe> {
    let mut out = Vec::new();
    let mut sublist = Some(&list.sublist);
    while let Some(current) = sublist {
        let mut pipe = Some(&current.pipe);
        while let Some(stage) = pipe {
            out.push(stage);
            pipe = stage.next.as_deref();
        }
        sublist = current.next.as_ref().map(|(_, next)| next.as_ref());
    }
    out
}

/// Every command in `lists`, with the loop depth it runs at; function bodies excluded.
pub fn visit<'a>(lists: &'a [ZshList], depth: usize, out: &mut Vec<(usize, Visit<'a>)>) {
    for list in lists {
        for pipe in pipes(list) {
            command(&pipe.cmd, depth, out);
        }
    }
}

pub enum Visit<'a> {
    Command(&'a ZshCommand),
    Subshell(&'a ZshProgram),
}

fn command<'a>(cmd: &'a ZshCommand, depth: usize, out: &mut Vec<(usize, Visit<'a>)>) {
    out.push((depth, Visit::Command(cmd)));
    match cmd {
        ZshCommand::Subsh(body) => out.push((depth, Visit::Subshell(body))),
        ZshCommand::Cursh(body) => visit(&body.lists, depth, out),
        ZshCommand::For(node) => visit(&node.body.lists, depth + 1, out),
        ZshCommand::While(node) | ZshCommand::Until(node) => {
            visit(&node.cond.lists, depth, out);
            visit(&node.body.lists, depth + 1, out);
        }
        ZshCommand::Repeat(node) => visit(&node.body.lists, depth + 1, out),
        ZshCommand::If(node) => {
            visit(&node.cond.lists, depth, out);
            visit(&node.then.lists, depth, out);
            for (cond, body) in &node.elif {
                visit(&cond.lists, depth, out);
                visit(&body.lists, depth, out);
            }
            if let Some(body) = &node.else_ {
                visit(&body.lists, depth, out);
            }
        }
        ZshCommand::Case(node) => node
            .arms
            .iter()
            .for_each(|arm| visit(&arm.body.lists, depth, out)),
        ZshCommand::Try(node) => {
            visit(&node.try_block.lists, depth, out);
            visit(&node.always.lists, depth, out);
        }
        ZshCommand::Redirected(inner, _) => command(inner, depth, out),
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Return {
    /// Loop depth within the file; `None` inside a function body.
    pub depth: Option<usize>,
    pub argument: Option<String>,
}

/// Every `return` in source order; `None` when one sits where it cannot be
/// rewritten, such as inside a subshell.
pub fn returns(lists: &[ZshList]) -> Option<Vec<Return>> {
    let mut out = Vec::new();
    returns_in(lists, Some(0), &mut out)?;
    Some(out)
}

pub fn file_returns(lists: &[ZshList]) -> Option<usize> {
    returns(lists).map(|found| found.iter().filter(|site| site.depth.is_some()).count())
}

fn returns_in(lists: &[ZshList], depth: Option<usize>, out: &mut Vec<Return>) -> Option<()> {
    for list in lists {
        for pipe in pipes(list) {
            returns_command(&pipe.cmd, depth, out)?;
        }
    }
    Some(())
}

fn returns_command(cmd: &ZshCommand, depth: Option<usize>, out: &mut Vec<Return>) -> Option<()> {
    let deeper = depth.map(|depth| depth + 1);
    match cmd {
        ZshCommand::Simple(simple) => {
            let words = words(simple);
            if words.first().map(String::as_str) == Some("return") {
                if words.len() > 2 || !simple.redirs.is_empty() {
                    return None;
                }
                out.push(Return {
                    depth,
                    argument: words.get(1).cloned(),
                });
            }
        }
        ZshCommand::Subsh(body) if depth.is_some() && contains_word(&body.lists, "return") => {
            return None;
        }
        ZshCommand::Subsh(body) => returns_in(&body.lists, None, out)?,
        ZshCommand::Cursh(body) => returns_in(&body.lists, depth, out)?,
        ZshCommand::FuncDef(node) => returns_in(&node.body.lists, None, out)?,
        ZshCommand::For(node) => returns_in(&node.body.lists, deeper, out)?,
        ZshCommand::While(node) | ZshCommand::Until(node) => {
            returns_in(&node.cond.lists, depth, out)?;
            returns_in(&node.body.lists, deeper, out)?;
        }
        ZshCommand::Repeat(node) => returns_in(&node.body.lists, deeper, out)?,
        ZshCommand::If(node) => {
            returns_in(&node.cond.lists, depth, out)?;
            returns_in(&node.then.lists, depth, out)?;
            for (cond, body) in &node.elif {
                returns_in(&cond.lists, depth, out)?;
                returns_in(&body.lists, depth, out)?;
            }
            if let Some(body) = &node.else_ {
                returns_in(&body.lists, depth, out)?;
            }
        }
        ZshCommand::Case(node) => {
            for arm in &node.arms {
                returns_in(&arm.body.lists, depth, out)?;
            }
        }
        ZshCommand::Try(node) => {
            returns_in(&node.try_block.lists, depth, out)?;
            returns_in(&node.always.lists, depth, out)?;
        }
        ZshCommand::Redirected(inner, _) => returns_command(inner, depth, out)?,
        ZshCommand::Time(Some(_)) if depth.is_some() => return None,
        _ => {}
    }
    Some(())
}

fn contains_word(lists: &[ZshList], name: &str) -> bool {
    serde_json::to_string(lists).is_ok_and(|json| json.contains(&format!("\"{name}\"")))
}

/// Named functions `lists` define when run, outside other function bodies.
pub fn function_names(lists: &[ZshList]) -> Vec<String> {
    let mut visits = Vec::new();
    visit(lists, 0, &mut visits);
    visits
        .iter()
        .filter_map(|(_, visit)| match visit {
            Visit::Command(ZshCommand::FuncDef(node)) if node.auto_call_args.is_none() => {
                Some(node.names.iter().map(|name| text(name)).collect::<Vec<_>>())
            }
            _ => None,
        })
        .flatten()
        .collect()
}

/// Whether `line` defines `name` as `function name`, where zsh expands no alias.
pub fn defines_with_keyword(line: &str, name: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    words
        .windows(2)
        .any(|pair| pair[0] == "function" && pair[1].split(['(', '{']).next() == Some(name))
}

/// Function names declared at the start of lines, for code the parser rejects.
pub fn declared_names(text: &str) -> Vec<String> {
    let is_name = |name: &str| {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.' | '+'))
    };
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let name = match line.strip_prefix("function ") {
                Some(rest) => rest.split(['(', ' ', '{']).next()?,
                None => line.split_once("()")?.0.trim_end(),
            };
            is_name(name).then(|| name.to_string())
        })
        .collect()
}

pub fn contains_function(lists: &[ZshList]) -> bool {
    let mut visits = Vec::new();
    visit(lists, 0, &mut visits);
    visits
        .iter()
        .any(|(_, visit)| matches!(visit, Visit::Command(ZshCommand::FuncDef(_))))
}

pub fn contains_loop_control(lists: &[ZshList]) -> bool {
    ["break", "continue", "return"]
        .iter()
        .any(|word| contains_word(lists, word))
}

/// Functions whose only run is one call: `(definition, call)` statement
/// indices where only definitions lie between a function's definition, a
/// statement starting with its call, and its removal by `unset -f` or
/// `unfunction`, and `source` names it nowhere else.
pub fn single_calls(lists: &[ZshList], source: &str) -> Vec<(usize, usize)> {
    let next_run =
        |from: usize| (from..lists.len()).find(|&index| defined(&lists[index]).is_none());
    let mut out = Vec::new();
    for (definition, list) in lists.iter().enumerate() {
        let Some(node) = defined(list).filter(|node| node.auto_call_args.is_none()) else {
            continue;
        };
        let [name] = node.names.as_slice() else {
            continue;
        };
        let name = text(name);
        let Some(call) = next_run(definition + 1) else {
            continue;
        };
        let Some(removal) = next_run(call + 1) else {
            continue;
        };
        let redefined = lists[definition + 1..removal]
            .iter()
            .filter_map(defined)
            .any(|node| node.names.iter().any(|other| text(other) == name));
        let Some(removed) = removal_mentions(&lists[removal], &name) else {
            continue;
        };
        let leaves = contains_loop_control(&lists[call..=call])
            || contains_word(&lists[call..=call], "exit");
        if !redefined
            && !leaves
            && first_command(&lists[call]).as_deref() == Some(name.as_str())
            && source.matches(name.as_str()).count() == 2 + removed
        {
            out.push((definition, call));
        }
    }
    out
}

fn defined(list: &ZshList) -> Option<&ZshFuncDef> {
    match sole(list)? {
        ZshCommand::FuncDef(node) => Some(node),
        _ => None,
    }
}

/// The command a statement runs first, outside a pipeline.
fn first_command(list: &ZshList) -> Option<String> {
    let pipe = &list.sublist.pipe;
    match &pipe.cmd {
        ZshCommand::Simple(simple)
            if !list.flags.async_ && pipe.next.is_none() && simple.assigns.is_empty() =>
        {
            simple.words.first().map(|word| text(word))
        }
        _ => None,
    }
}

/// How often a statement removing function `name` names it; `None` when it
/// does not remove it.
fn removal_mentions(list: &ZshList, name: &str) -> Option<usize> {
    let simple = sole_simple(list).filter(|simple| simple.assigns.is_empty())?;
    let words = words(simple);
    let (command, args) = words.split_first()?;
    let flags: String = args
        .iter()
        .filter(|arg| arg.starts_with('-'))
        .flat_map(|arg| arg.chars().skip(1))
        .collect();
    if !(command == "unfunction" || command == "unset" && flags.contains('f')) {
        return None;
    }
    let matches = |arg: &String| {
        let Some(value) = crate::expand::scalar(arg, &super::Unknown) else {
            return false;
        };
        let plain = |c: char| c.is_ascii_alphanumeric() || "_-.:+*?[]!".contains(c);
        match flags.contains('m') {
            true => {
                value.chars().all(plain)
                    && glob::Pattern::new(&value).is_ok_and(|pattern| pattern.matches(name))
            }
            false => value == name,
        }
    };
    args.iter()
        .filter(|arg| !arg.starts_with('-'))
        .any(matches)
        .then(|| words.iter().map(|word| word.matches(name).count()).sum())
}

/// Names a statement may assign, for constructs the compiler does not model.
pub fn assigned(lists: &[ZshList]) -> Vec<String> {
    let mut visits = Vec::new();
    visit(lists, 0, &mut visits);
    let mut names = Vec::new();
    for (_, visit) in visits {
        let Visit::Command(cmd) = visit else { continue };
        match cmd {
            ZshCommand::Simple(simple) => {
                names.extend(
                    simple
                        .assigns
                        .iter()
                        .map(|assign: &ZshAssign| assign.name.clone()),
                );
                let words = words(simple);
                if words.first().is_some_and(|word| {
                    DECLARATIONS.contains(&word.as_str()) || word == "read" || word == "unset"
                }) {
                    names.extend(
                        words[1..]
                            .iter()
                            .filter(|word| !word.starts_with(['-', '+']))
                            .map(|word| word.split('=').next().unwrap_or_default().to_string()),
                    );
                }
            }
            ZshCommand::For(node) => names.push(node.var.clone()),
            _ => {}
        }
    }
    names
}

pub const DECLARATIONS: &[&str] = &[
    "typeset", "local", "declare", "export", "readonly", "integer", "float",
];

/// `$+commands[name]` or `${+commands[name]}`, maybe negated with `!`: the
/// command name and whether the test is negated.
pub fn command_test(expr: &str) -> Option<(&str, bool)> {
    let expr = expr.trim();
    let (expr, negated) = match expr.strip_prefix('!') {
        Some(rest) => (rest.trim_start(), true),
        None => (expr, false),
    };
    let name = expr
        .strip_prefix("$+commands[")
        .and_then(|rest| rest.strip_suffix(']'))
        .or_else(|| {
            expr.strip_prefix("${+commands[")
                .and_then(|rest| rest.strip_suffix("]}"))
        })?;
    let plain = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'));
    plain.then_some((name, negated))
}

/// The command a simple command runs, past `builtin` and quotes.
fn command_words(simple: &ZshSimple) -> Vec<String> {
    let words: Vec<String> = words(simple)
        .iter()
        .map(|word| word.trim_matches(['\'', '"']).to_string())
        .collect();
    match words.split_first() {
        Some((first, rest)) if first == "builtin" => rest.to_vec(),
        _ => words,
    }
}

/// Whether a simple command may leave an emulation other than zsh in effect.
pub fn sets_emulation(simple: &ZshSimple, in_function: bool) -> bool {
    match command_words(simple).split_first() {
        Some((name, args)) if name == "emulate" => changes_emulation(args, in_function),
        Some((name, args)) if name == "eval" => args.iter().any(|arg| arg.contains("emulate")),
        _ => false,
    }
}

/// Whether `emulate ARGS` leaves an emulation other than zsh in effect; `-L`
/// restores it when the function running it returns.
fn changes_emulation(args: &[String], in_function: bool) -> bool {
    let local = args
        .iter()
        .any(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('L'));
    let command = args.iter().any(|arg| arg == "-c");
    let mut rest = args.iter();
    let mut mode = None;
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "-o" | "+o" => {
                rest.next();
            }
            flag if flag.starts_with(['-', '+']) => {}
            word => {
                mode = Some(word);
                break;
            }
        }
    }
    mode.is_some_and(|mode| mode != "zsh") && !command && !(local && in_function)
}

/// Whether running `lists`, or a function they define, may change the emulation.
pub fn changes_emulation_in(lists: &[ZshList], in_function: bool) -> bool {
    let mut visits = Vec::new();
    visit(lists, 0, &mut visits);
    visits.iter().any(|(_, visit)| match visit {
        Visit::Command(ZshCommand::Simple(simple)) => sets_emulation(simple, in_function),
        Visit::Command(ZshCommand::FuncDef(node)) => changes_emulation_in(&node.body.lists, true),
        _ => false,
    })
}

/// Whether running `lists` may change an option, outside nested function bodies.
pub fn sets_options(lists: &[ZshList]) -> bool {
    let mut visits = Vec::new();
    visit(lists, 0, &mut visits);
    visits.iter().any(|(_, visit)| match visit {
        Visit::Command(ZshCommand::Simple(simple)) => {
            simple
                .assigns
                .iter()
                .any(|assign| assign.name.starts_with("options"))
                || command_words(simple).first().is_some_and(|name| {
                    matches!(name.as_str(), "setopt" | "unsetopt" | "emulate" | "set")
                })
        }
        _ => false,
    })
}

/// Whether `list`, the first statement of some code, reads the status the
/// command before it left: `$?`, `$status`, or a bare `return` or `exit`.
pub fn reads_status(list: &ZshList) -> bool {
    let bare = sole_simple(list).is_some_and(|simple| {
        simple.redirs.is_empty()
            && matches!(command_words(simple).as_slice(), [name] if name == "return" || name == "exit")
    });
    let text = top_level_text(std::slice::from_ref(list));
    bare || ["$?", "${?", "$status", "${status", "pipestatus"]
        .iter()
        .any(|read| text.contains(read))
}

/// The words of `lists` outside function bodies, as written.
pub fn top_level_text(lists: &[ZshList]) -> String {
    fn collect(value: &Value, out: &mut String) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "FuncDef" {
                        if let Some(names) = child.get("names") {
                            collect(names, out);
                        }
                    } else {
                        collect(child, out);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| collect(item, out)),
            Value::String(string) => {
                out.push_str(&text(string));
                out.push('\n');
            }
            _ => {}
        }
    }
    let mut out = String::new();
    collect(&serde_json::to_value(lists).unwrap_or_default(), &mut out);
    out
}

/// `lists` with each simple command in a function body that `replace` maps
/// replaced by the command it maps to.
pub fn replace_in_functions(
    lists: &[ZshList],
    replace: &dyn Fn(&ZshSimple) -> Option<ZshCommand>,
) -> Option<Vec<ZshList>> {
    fn walk(
        value: &mut Value,
        in_function: bool,
        replace: &dyn Fn(&ZshSimple) -> Option<ZshCommand>,
    ) {
        let replacement = match value.as_object() {
            Some(map) if in_function && map.len() == 1 => map
                .get("Simple")
                .and_then(|simple| serde_json::from_value::<ZshSimple>(simple.clone()).ok())
                .and_then(|simple| replace(&simple))
                .and_then(|command| serde_json::to_value(command).ok()),
            _ => None,
        };
        if let Some(replacement) = replacement {
            *value = replacement;
            return;
        }
        match value {
            Value::Object(map) => map
                .iter_mut()
                .for_each(|(key, child)| walk(child, in_function || key == "FuncDef", replace)),
            Value::Array(items) => items
                .iter_mut()
                .for_each(|item| walk(item, in_function, replace)),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(lists).ok()?;
    walk(&mut value, false, replace);
    serde_json::from_value(value).ok()
}

pub fn for_words(list: &ForList) -> Option<Vec<String>> {
    match list {
        ForList::Words(words) => Some(words.iter().map(|word| text(word)).collect()),
        _ => None,
    }
}

/// Stable identity of statements, insensitive to layout and comments.
pub fn fingerprint_lists(lists: &[ZshList]) -> u64 {
    let mut value = serde_json::to_value(lists).unwrap_or_default();
    strip_positions(&mut value);
    let mut canonical = String::new();
    write_canonical(&value, &mut canonical);
    crate::script::hash(&canonical)
}

/// Stable identity of a function body, insensitive to layout and comments.
pub fn fingerprint(node: &ZshFuncDef) -> u64 {
    let mut value = serde_json::to_value(&node.body).unwrap_or_default();
    strip_positions(&mut value);
    let mut canonical = String::new();
    write_canonical(&value, &mut canonical);
    crate::script::hash(&canonical)
}

/// JSON with sorted keys, whatever map order serde_json was built with.
fn write_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn strip_positions(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("lineno");
            map.remove("body_source");
            map.values_mut().for_each(strip_positions);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_positions),
        serde_json::Value::String(string) if string.starts_with(crate::script::ANONYMOUS) => {
            *string = crate::script::ANONYMOUS.to_string();
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "../../tests/unit/compile/walk_tests.rs"]
mod tests;
