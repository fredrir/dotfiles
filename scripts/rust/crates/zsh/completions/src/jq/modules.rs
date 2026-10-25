use std::fs;
use std::path::{Path, PathBuf};

use super::filter;
use super::lexer;
use crate::context::Context;

const DEPTH: usize = 3;
const LIMIT: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub name: String,
    pub data: bool,
}

// Where jq looks for modules: each `-L` in order, then `~/.jq` and the lib dir beside jq.
pub fn search_path(ctx: &Context, jq: Option<&Path>, library: &[String]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = library.iter().map(|dir| ctx.expand(dir)).collect();
    dirs.push(ctx.home.join(".jq"));
    if let Some(origin) = jq.and_then(|jq| {
        fs::canonicalize(jq)
            .ok()?
            .parent()?
            .parent()
            .map(Path::to_path_buf)
    }) {
        dirs.push(origin.join("lib/jq"));
    }
    dirs.retain(|dir| dir.is_dir());
    dirs
}

pub fn list(dirs: &[PathBuf]) -> Vec<Module> {
    let mut modules = Vec::new();
    for dir in dirs {
        walk(dir, dir, 0, &mut modules);
    }
    modules
}

fn walk(root: &Path, dir: &Path, depth: usize, modules: &mut Vec<Module>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if modules.len() >= LIMIT {
            return;
        }
        let hidden = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            if depth + 1 < DEPTH {
                walk(root, &path, depth + 1, modules);
            }
            continue;
        }
        let data = match path.extension().and_then(|ext| ext.to_str()) {
            Some("jq") => false,
            Some("json") => true,
            _ => continue,
        };
        let Ok(relative) = path
            .with_extension("")
            .strip_prefix(root)
            .map(Path::to_path_buf)
        else {
            continue;
        };
        let name = relative.to_string_lossy().to_string();
        // `a/b/b.jq` is also `a/b`.
        let short = relative
            .parent()
            .filter(|parent| parent.file_name() == relative.file_name())
            .map(|parent| parent.to_string_lossy().to_string());
        for name in std::iter::once(name).chain(short) {
            let module = Module { name, data };
            if !modules.contains(&module) {
                modules.push(module);
            }
        }
    }
}

pub fn resolve(dirs: &[PathBuf], name: &str, data: bool) -> Option<PathBuf> {
    let extension = if data { "json" } else { "jq" };
    let last = name.rsplit('/').next().unwrap_or(name);
    dirs.iter()
        .flat_map(|dir| {
            [
                dir.join(format!("{name}.{extension}")),
                dir.join(name).join(format!("{last}.{extension}")),
            ]
        })
        .find(|path| path.is_file())
}

// Functions a module file defines, with their arity.
pub fn functions(path: &Path) -> Vec<(String, usize)> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    filter::definitions(&text, &lexer::lex(&text)).0
}

#[cfg(test)]
#[path = "../../tests/unit/jq/modules_tests.rs"]
mod tests;
