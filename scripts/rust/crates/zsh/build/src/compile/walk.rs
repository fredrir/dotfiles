//! Questions about parsed statements that do not depend on build state.

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
