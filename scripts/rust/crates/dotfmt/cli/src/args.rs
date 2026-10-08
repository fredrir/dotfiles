use std::path::PathBuf;

use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Parser, ValueHint};
use dotfmt_core::language::Language;
use workstation::{Completable, Completions};

#[derive(Parser)]
#[command(version, about = "The one formatter to rule them all")]
pub struct Cli {
    /// Files or directories to format, or `-` for standard input.
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    pub targets: Vec<PathBuf>,

    /// Select configured languages: json, lua, conf, md or markdown.
    #[arg(short = 'l', long = "lang", value_delimiter = ',', value_parser = language(), value_name = "LANGUAGE")]
    pub languages: Vec<Language>,

    /// Check formatting without writing.
    #[arg(long, conflicts_with_all = ["add", "sync"])]
    pub check: bool,

    /// Offer dotfmt.dotfile configuration to the target.
    #[arg(short = 'a', long, conflicts_with_all = ["sync", "stdin", "editor", "languages", "dialect"])]
    pub add: bool,

    /// Refresh an existing target dotfmt.dotfile configuration.
    #[arg(short = 's', long, conflicts_with_all = ["stdin", "editor", "languages", "dialect"])]
    pub sync: bool,

    /// Select a dialect for one language (auto, jsonc, luau, gfm, ...).
    #[arg(long, value_name = "DIALECT")]
    pub dialect: Option<String>,

    /// Format standard input for an editor; JSON allows repairs.
    #[arg(short, long)]
    pub editor: bool,

    /// Treat standard input as the named file, including its configuration.
    #[arg(long, value_name = "FILENAME", value_hint = ValueHint::FilePath, conflicts_with = "targets")]
    pub stdin: Option<PathBuf>,

    /// Read NUL-separated filenames and emit the configured, included files.
    #[arg(long, conflicts_with_all = ["targets", "check", "stdin", "editor", "add", "sync", "dialect", "verbose"])]
    pub owns: bool,

    /// Show detailed output.
    #[arg(short, long, conflicts_with = "quiet")]
    pub verbose: bool,

    /// Report only failures.
    #[arg(short, long)]
    pub quiet: bool,

    #[command(flatten)]
    pub completions: Completions,
}

fn language() -> impl TypedValueParser<Value = Language> {
    PossibleValuesParser::new(Language::ALL.into_iter().flat_map(|language| {
        language
            .aliases()
            .iter()
            .copied()
            .filter(move |alias| *alias != language.name())
            .chain(std::iter::once(language.name()))
    }))
    .try_map(|value| Language::parse(&value))
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}
