use std::path::PathBuf;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Parser, ValueHint};
use dotfmt_core::language::Language;
use workstation::{Completable, Completions};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LanguageArg {
    pub language: Language,
    pub dialect: Option<&'static str>,
}

impl LanguageArg {
    fn parse(value: &str) -> Result<Self, String> {
        Ok(Self {
            language: Language::parse(value)?,
            dialect: dialect_hint(value),
        })
    }
}

fn dialect_hint(value: &str) -> Option<&'static str> {
    match value {
        "tsx" => Some("tsx"),
        "jsx" => Some("jsx"),
        "scss" => Some("scss"),
        "sass" => Some("sass"),
        "less" => Some("less"),
        _ => None,
    }
}

#[derive(Parser)]
#[command(version, about = "The one formatter to rule them all")]
pub struct Cli {
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    pub targets: Vec<PathBuf>,

    #[arg(short = 'l', long = "lang", value_delimiter = ',', value_parser = language(), value_name = "LANGUAGE")]
    pub languages: Vec<LanguageArg>,

    #[arg(long, value_name = "SOURCE", value_parser = ["prettier"], conflicts_with_all = ["add", "sync", "editor", "explain", "owns", "fix", "fix_unsafe", "diff", "diagnose", "json", "check", "clean"])]
    pub migrate: Option<String>,

    #[arg(long, conflicts_with_all = ["add", "sync", "fix", "fix_unsafe", "diagnose"])]
    pub check: bool,

    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync", "diagnose"])]
    pub fix: bool,

    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync", "diagnose"])]
    pub fix_unsafe: bool,

    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync"])]
    pub diff: bool,

    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync"])]
    pub diagnose: bool,

    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync"])]
    pub json: bool,

    #[arg(short = 'c', long, conflicts_with_all = ["add", "sync", "owns"])]
    pub clean: bool,

    #[arg(short = 'a', long, conflicts_with_all = ["sync", "stdin", "editor", "languages", "dialect"])]
    pub add: bool,

    #[arg(short = 's', long, conflicts_with_all = ["stdin", "editor", "languages", "dialect"])]
    pub sync: bool,

    #[arg(long, value_name = "DIALECT")]
    pub dialect: Option<String>,

    #[arg(short, long)]
    pub editor: bool,

    #[arg(long, value_name = "FILENAME", value_hint = ValueHint::FilePath, conflicts_with_all = ["targets", "check", "clean", "add", "sync", "editor", "stdin", "owns"])]
    pub explain: Option<PathBuf>,

    #[arg(long, value_name = "FILENAME", value_hint = ValueHint::FilePath, conflicts_with = "targets")]
    pub stdin: Option<PathBuf>,

    #[arg(long, conflicts_with_all = ["targets", "check", "stdin", "editor", "add", "sync", "dialect", "verbose"])]
    pub owns: bool,

    #[arg(short, long, conflicts_with = "quiet")]
    pub verbose: bool,

    #[arg(short, long)]
    pub quiet: bool,

    #[command(flatten)]
    pub completions: Completions,
}

fn language() -> impl TypedValueParser<Value = LanguageArg> {
    PossibleValuesParser::new(Language::ALL.into_iter().flat_map(|language| {
        language
            .aliases()
            .iter()
            .copied()
            .filter(move |alias| *alias != language.name())
            .chain(std::iter::once(language.name()))
    }))
    .try_map(|value| LanguageArg::parse(&value))
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}
