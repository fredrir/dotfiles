use std::path::PathBuf;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Parser, ValueHint};
use dotfmt_core::language::Language;
use workstation::{Completable, Completions};

/// A `--lang` alias, with the dialect its name implies for extensionless stdin.
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
    /// Files, directories or `-` for stdin; editor mode takes a filename.
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    pub targets: Vec<PathBuf>,

    /// Select configured languages: bib, conf, css (scss, sass, less), env, html (htm, vue, svelte, astro), ignore, ini, javascript (js, jsx), json, latex, lua, markdown, plist, toml, typescript (ts, tsx), or yaml. The tsx, jsx, scss, sass and less aliases select that dialect for stdin and untitled input.
    #[arg(short = 'l', long = "lang", value_delimiter = ',', value_parser = language(), value_name = "LANGUAGE")]
    pub languages: Vec<LanguageArg>,

    /// Print a dotfmt.dotfile migrated from another formatter; currently prettier.
    #[arg(long, value_name = "SOURCE", value_parser = ["prettier"], conflicts_with_all = ["add", "sync", "editor", "explain", "owns", "fix", "fix_unsafe", "diff", "diagnose", "json", "check", "clean"])]
    pub migrate: Option<String>,

    /// Check formatting without writing.
    #[arg(long, conflicts_with_all = ["add", "sync", "fix", "fix_unsafe", "diagnose"])]
    pub check: bool,

    /// Apply safe fixes (unused imports, type-only imports, import ordering) and format.
    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync", "diagnose"])]
    pub fix: bool,

    /// Apply safe and unsafe fixes, including code removal. Implies --fix.
    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync", "diagnose"])]
    pub fix_unsafe: bool,

    /// Print a unified diff instead of writing when used with --fix.
    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync"])]
    pub diff: bool,

    /// Report findings without writing or fixing.
    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync"])]
    pub diagnose: bool,

    /// Emit machine-readable JSON for --diagnose/--fix.
    #[arg(long, conflicts_with_all = ["check", "clean", "owns", "editor", "add", "sync"])]
    pub json: bool,

    /// Format Markdown and strip all Markdown formatting; requires --lang markdown.
    #[arg(short = 'c', long, conflicts_with_all = ["add", "sync", "owns"])]
    pub clean: bool,

    /// Offer dotfmt.dotfile configuration to the target.
    #[arg(short = 'a', long, conflicts_with_all = ["sync", "stdin", "editor", "languages", "dialect"])]
    pub add: bool,

    /// Refresh an existing target dotfmt.dotfile configuration.
    #[arg(short = 's', long, conflicts_with_all = ["stdin", "editor", "languages", "dialect"])]
    pub sync: bool,

    /// Select a dialect for one language (auto, tsx, scss, jsonc, luau, gfm, ...).
    #[arg(long, value_name = "DIALECT")]
    pub dialect: Option<String>,

    /// Quietly format stdin as TARGET; infer language and dialect.
    /// Syntax errors pass input through and succeed unless --verbose or --check; other errors pass it through and fail.
    #[arg(short, long)]
    pub editor: bool,

    /// Explain how FILENAME is formatted, or why it is skipped.
    #[arg(long, value_name = "FILENAME", value_hint = ValueHint::FilePath, conflicts_with_all = ["targets", "check", "clean", "add", "sync", "editor", "stdin", "owns"])]
    pub explain: Option<PathBuf>,

    /// Treat standard input as the named file, including its configuration.
    #[arg(long, value_name = "FILENAME", value_hint = ValueHint::FilePath, conflicts_with = "targets")]
    pub stdin: Option<PathBuf>,

    /// Read NUL-separated filenames and emit the configured, included files.
    #[arg(long, conflicts_with_all = ["targets", "check", "stdin", "editor", "add", "sync", "dialect", "verbose"])]
    pub owns: bool,

    /// Show detailed output, including each repair.
    #[arg(short, long, conflicts_with = "quiet")]
    pub verbose: bool,

    /// Report only failures.
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
