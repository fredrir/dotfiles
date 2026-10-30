//! The `files.exclude` setting, matched like the VS Code Explorer.

use std::collections::HashSet;

use globset::{GlobBuilder, GlobMatcher};
use serde_json::{Map, Value};

pub struct Exclude {
    rules: Vec<Rule>,
}

struct Rule {
    matcher: GlobMatcher,
    /// Sibling clause such as `$(basename).ts`.
    when: Option<String>,
}

impl Exclude {
    pub fn new(patterns: &Map<String, Value>) -> Self {
        let rules = patterns
            .iter()
            .filter(|(_, value)| **value != Value::Bool(false))
            .filter_map(|(pattern, value)| {
                let glob = GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .build()
                    .ok()?;
                Some(Rule {
                    matcher: glob.compile_matcher(),
                    when: value.get("when").and_then(Value::as_str).map(str::to_owned),
                })
            })
            .collect();
        Self { rules }
    }

    /// `path` is relative to the workspace folder and `siblings` holds every name in its directory.
    pub fn matches(&self, path: &str, siblings: &HashSet<&str>) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        let stem = match name.rfind('.') {
            Some(dot) if dot > 0 => &name[..dot],
            _ => name,
        };
        self.rules.iter().any(|rule| {
            rule.matcher.is_match(path)
                && rule.when.as_ref().is_none_or(|when| {
                    siblings.contains(when.replacen("$(basename)", stem, 1).as_str())
                })
        })
    }
}
