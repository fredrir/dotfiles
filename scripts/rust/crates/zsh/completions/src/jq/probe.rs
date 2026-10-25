use std::path::Path;
use std::time::Duration;

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use super::filter::Program;
use crate::process;

const TIMEOUT: Duration = Duration::from_secs(2);
const HITS: usize = 256;
const MARK: &str = r#"["DEBUG:",{"zcomp":"#;
// Reports the keys of each object reaching it, at most 300, preferring those that match.
const SUMMARY: &str = r#"(select(type == "object") | {zcomp: ((keys_unsorted | if length > 300 then map(select(ascii_downcase | startswith(PARTIAL))) else . end | .[:300]) as $__zcomp_keys | [$__zcomp_keys[] as $__zcomp_key | .[$__zcomp_key] | [$__zcomp_key, type, (if type == "string" then .[:48] | tojson elif type == "array" or type == "object" then length else tojson end)]])} | debug | empty)"#;
// jq's exit status for a filter that does not compile.
const COMPILE_ERROR: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    pub types: Vec<String>,
    pub preview: String,
}

// How the command line runs jq: input flags and variables, then its inputs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub flags: Vec<String>,
    pub files: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    // `--args` or `--jsonargs` and the values after it.
    pub trailing: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Probed {
    pub keys: Vec<Key>,
    pub compiled: bool,
}

#[derive(Deserialize)]
struct Hit {
    zcomp: Vec<(String, String, Value)>,
}

pub fn keys(jq: &Path, cwd: &Path, run: &Run, program: &Program, partial: &str) -> Probed {
    let partial = serde_json::to_string(&partial.to_lowercase()).unwrap_or_default();
    // A leading space keeps a filter such as `-1` from reading as an option.
    let filter = format!(
        " {}{}{}",
        program.before,
        SUMMARY.replace("PARTIAL", &partial),
        program.after
    );
    let mut args = run.flags.clone();
    args.push(filter);
    args.extend(run.files.iter().cloned());
    args.extend(run.trailing.iter().cloned());
    let output = process::stderr_lines(
        jq,
        &args,
        cwd,
        run.stdin.clone(),
        TIMEOUT,
        |line| line.starts_with(MARK),
        HITS,
    );
    Probed {
        keys: merge(&output.lines),
        compiled: output.code != Some(COMPILE_ERROR),
    }
}

// Keys from every object the probe saw, in first-seen order, with all the types they held.
pub fn merge(lines: &[String]) -> Vec<Key> {
    let mut keys: IndexMap<String, Key> = IndexMap::new();
    for line in lines {
        let Ok((_, hit)) = serde_json::from_str::<(String, Hit)>(line) else {
            continue;
        };
        for (name, kind, value) in hit.zcomp {
            let key = keys.entry(name.clone()).or_insert_with(|| Key {
                name,
                types: Vec::new(),
                preview: preview(&kind, &value),
            });
            if !key.types.contains(&kind) {
                key.types.push(kind);
            }
        }
    }
    keys.into_values().collect()
}

fn preview(kind: &str, value: &Value) -> String {
    let count = value.as_u64().unwrap_or(0);
    let plural = |noun: &str| {
        let s = if count == 1 { "" } else { "s" };
        format!("{count} {noun}{s}")
    };
    match (kind, value) {
        (_, Value::String(text)) => text.clone(),
        ("array", _) => plural("item"),
        ("object", _) => plural("key"),
        _ => value.to_string(),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/jq/probe_tests.rs"]
mod tests;
