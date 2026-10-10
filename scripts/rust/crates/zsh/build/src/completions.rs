//! `cached_eval` output that is a standard autoloadable `#compdef` script,
//! written as a file on `fpath` instead of into the bundle: compinit binds it
//! from its `#compdef` line and loads it on the first Tab.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use zshrs_parse::parser::{SublistOp, ZshCommand, ZshList};

use crate::compile::walk;
use crate::quote;
use crate::script::{self, Script};

#[derive(Debug, Default)]
pub struct Completions {
    /// Where the files go; `None` keeps completion scripts inlined.
    dir: Option<PathBuf>,
    /// Whether the directory was put on `fpath` ahead of a `compinit`.
    placed: bool,
    files: BTreeMap<String, String>,
}

/// What writing the files changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub changed: bool,
    /// A file was added, removed or bound to other commands.
    pub bindings: bool,
}

impl Completions {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            ..Self::default()
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The statement that puts the directory first on `fpath`.
    pub fn fpath(&self) -> Option<String> {
        let dir = self.dir.as_ref()?.to_str()?;
        Some(format!("fpath=({} \"${{fpath[@]}}\")\n", quote::word(dir)))
    }

    /// The `fpath` statement for code ahead of a `compinit`; from then on
    /// scripts are taken as files.
    pub fn place(&mut self) -> String {
        let text = self.fpath().unwrap_or_default();
        self.placed |= !text.is_empty();
        text
    }

    /// Takes `script` as a file when it is a clean autoloadable `#compdef`
    /// script; returns the file and the line of each function it defines.
    pub fn take(&mut self, script: &Script) -> Option<(PathBuf, Vec<(String, usize)>)> {
        if !self.placed {
            return None;
        }
        let name = autoloadable(script)?;
        let file = self.dir.as_ref()?.join(&name);
        let functions = script
            .program
            .lists
            .iter()
            .flat_map(|list| {
                let line = script::start_line(list);
                defined(list)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |name| (name, line))
            })
            .collect();
        self.files.insert(name, script.text.clone());
        Some((file, functions))
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Writes the files and removes the ones no longer produced.
    pub fn write(&self, dry_run: bool) -> Result<Written, String> {
        let Some(dir) = &self.dir else {
            return Ok(Written::default());
        };
        let mut written = Written::default();
        let existing = match fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect(),
            Err(_) => Vec::new(),
        };
        for name in existing
            .iter()
            .filter(|name| !self.files.contains_key(*name))
        {
            written.changed = true;
            written.bindings = true;
            if !dry_run {
                let path = dir.join(name);
                fs::remove_file(&path)
                    .map_err(|error| format!("remove {}: {error}", path.display()))?;
            }
        }
        for (name, text) in &self.files {
            let path = dir.join(name);
            let current = fs::read_to_string(&path).ok();
            if current.as_deref() == Some(text.as_str()) {
                continue;
            }
            written.changed = true;
            written.bindings |= current
                .as_deref()
                .is_none_or(|current| current.lines().next() != text.lines().next());
            if !dry_run {
                crate::emit::write_atomic(&path, text)?;
            }
        }
        Ok(written)
    }
}

/// The function a clean autoloadable `#compdef` script is named for: after
/// a `#compdef` line of plain names, only function definitions, `autoload`,
/// `compdef` of that function to those names, and a last
/// `if [ "$funcstack[1]" = "_NAME" ]` that calls it when autoloaded.
pub fn autoloadable(script: &Script) -> Option<String> {
    let names: Vec<String> = script
        .text
        .lines()
        .next()?
        .strip_prefix("#compdef ")?
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let plain = |name: &String| !name.starts_with('-') && name.bytes().all(quote::is_plain);
    if names.is_empty() || !names.iter().all(plain) {
        return None;
    }
    let (last, rest) = script.program.lists.split_last()?;
    let function = dispatch(last, &names)?;
    let file_name = function.starts_with('_')
        && !function.contains('/')
        && function.bytes().all(quote::is_plain);
    if !file_name {
        return None;
    }
    let mut defines = false;
    for list in rest {
        if let Some(defined) = defined(list) {
            defines |= defined.contains(&function);
            continue;
        }
        let words = walk::sole_simple(list)
            .filter(|simple| simple.assigns.is_empty() && simple.redirs.is_empty())
            .map(walk::words)?;
        match words.first().map(String::as_str) {
            Some("autoload") => {}
            Some("compdef") if binds(&words, &function, &names) => {}
            _ => return None,
        }
    }
    defines.then_some(function)
}

/// Functions a definition statement defines: `name() {...}`, optionally
/// guarded as `(( $+functions[name] )) || name() {...}`.
fn defined(list: &ZshList) -> Option<Vec<String>> {
    if list.flags.async_ || list.sublist.flags.not || list.sublist.pipe.next.is_some() {
        return None;
    }
    let definition = match (&list.sublist.pipe.cmd, &list.sublist.next) {
        (ZshCommand::FuncDef(node), None) => node,
        (ZshCommand::Arith(_), Some((SublistOp::Or, next)))
            if next.next.is_none() && next.pipe.next.is_none() && !next.flags.not =>
        {
            match &next.pipe.cmd {
                ZshCommand::FuncDef(node) => node,
                _ => return None,
            }
        }
        _ => return None,
    };
    definition.auto_call_args.is_none().then(|| {
        definition
            .names
            .iter()
            .map(|name| walk::text(name))
            .collect()
    })
}

/// `compdef FUNCTION NAMES...`, binding exactly the `#compdef` names.
fn binds(words: &[String], function: &str, names: &[String]) -> bool {
    words.len() > 2 && words[1] == function && words[2..] == *names
}

/// The function in `if [ "$funcstack[1]" = "_NAME" ]; then _NAME ...; [else compdef ...;] fi`.
fn dispatch(list: &ZshList, names: &[String]) -> Option<String> {
    let ZshCommand::If(node) = walk::sole(list)? else {
        return None;
    };
    let words = |lists: &[ZshList]| match lists {
        [list] => walk::sole_simple(list)
            .filter(|simple| simple.assigns.is_empty() && simple.redirs.is_empty())
            .map(walk::words),
        _ => None,
    };
    let test = words(&node.cond.lists)?;
    let [open, stack, equals, quoted, close] = test.as_slice() else {
        return None;
    };
    let function = quoted.strip_prefix('"')?.strip_suffix('"')?.to_string();
    let test = open == "[" && stack == "\"$funcstack[1]\"" && equals == "=" && close == "]";
    let calls = words(&node.then.lists)?.first() == Some(&function);
    let otherwise = match &node.else_ {
        None => true,
        Some(body) => words(&body.lists)
            .is_some_and(|words| words[0] == "compdef" && binds(&words, &function, names)),
    };
    (test && calls && node.elif.is_empty() && otherwise).then_some(function)
}

#[cfg(test)]
#[path = "../tests/unit/completions_tests.rs"]
mod tests;
