use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::cache::{self, Source};
use crate::context::{Context, fingerprint};
use crate::process;

const DESCRIPTIONS: &str = include_str!("../../assets/jq-builtins.txt");

pub const KEYWORDS: [(&str, &str); 16] = [
    ("if", "conditional"),
    ("then", "branch taken when the condition holds"),
    ("elif", "another condition"),
    ("else", "branch taken otherwise"),
    ("end", "end of if"),
    ("try", "catch errors"),
    ("catch", "handle the error from try"),
    ("reduce", "fold a stream into one value"),
    ("foreach", "fold a stream, emitting each state"),
    ("def", "define a function"),
    ("label", "target for break"),
    ("as", "bind to variables"),
    ("and", "logical and"),
    ("or", "logical or"),
    ("import", "import a module"),
    ("include", "include a module"),
];

pub const FORMATS: [(&str, &str); 12] = [
    ("@text", "tostring"),
    ("@json", "serialize as JSON"),
    ("@html", "escape for HTML"),
    ("@uri", "percent-encode"),
    ("@urid", "percent-decode"),
    ("@csv", "array to a CSV row"),
    ("@tsv", "array to a TSV row"),
    ("@sh", "quote for a POSIX shell"),
    ("@base64", "encode as base64"),
    ("@base64d", "decode base64"),
    ("@base32", "encode as base32"),
    ("@base32d", "decode base32"),
];

pub const VARIABLES: [(&str, &str); 3] = [
    ("$ENV", "environment variables"),
    ("$ARGS", "named and positional arguments"),
    ("$__loc__", "file and line here"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub arities: Vec<usize>,
    pub description: String,
}

pub struct BuiltinsSource {
    pub binary: PathBuf,
}

impl Source for BuiltinsSource {
    // `name/arity` as listed by `builtins`.
    type Value = Vec<String>;

    fn key(&self) -> String {
        "jq-builtins".into()
    }

    fn stamp(&self, _ctx: &Context) -> String {
        fingerprint(&self.binary)
    }

    fn ttl(&self) -> Option<Duration> {
        None
    }

    fn job(&self) -> Vec<String> {
        vec!["jq-builtins".into()]
    }

    fn build(&self, ctx: &Context) -> Option<Vec<String>> {
        let text = process::output(
            &self.binary,
            &["-nr", "builtins[]"],
            &ctx.cwd,
            Duration::from_secs(5),
        )?;
        let names: Vec<String> = text
            .lines()
            .filter(|line| line.contains('/'))
            .map(String::from)
            .collect();
        (!names.is_empty()).then_some(names)
    }
}

pub fn load(ctx: &Context, binary: PathBuf) -> Vec<Function> {
    let names = cache::load(ctx, &BuiltinsSource { binary }).unwrap_or_default();
    let parsed = names.iter().filter_map(|entry| {
        let (name, arity) = entry.rsplit_once('/')?;
        Some((name, arity.parse().ok()?))
    });
    functions(parsed, |name| {
        description(name).unwrap_or_default().to_string()
    })
}

// Each name once, with every arity it is defined for.
pub fn functions<'a>(
    defined: impl Iterator<Item = (&'a str, usize)>,
    describe: impl Fn(&str) -> String,
) -> Vec<Function> {
    let mut grouped: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (name, arity) in defined.filter(|(name, _)| !name.starts_with('_')) {
        let arities = grouped.entry(name).or_default();
        if !arities.contains(&arity) {
            arities.push(arity);
            arities.sort_unstable();
        }
    }
    grouped
        .into_iter()
        .map(|(name, arities)| Function {
            name: name.to_string(),
            arities,
            description: describe(name),
        })
        .collect()
}

pub fn description(name: &str) -> Option<&'static str> {
    DESCRIPTIONS.lines().find_map(|line| {
        let (known, description) = line.split_once('\t')?;
        (known == name).then_some(description)
    })
}
