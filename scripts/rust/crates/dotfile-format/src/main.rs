//! Format a tree with the tool that owns each language.
//!
//! One orchestrator over ten providers. The tree is walked once, every file
//! is sorted into the row of the provider table that owns its extension, and
//! each row's programs are handed the list. `dotfmt` sits in that table next
//! to stylua and shfmt: a subprocess with a file list, not a special case —
//! except that its row is built from its own answer to `--owns` rather than
//! from an extension list here, because its selection rules are per directory
//! and only it reads them.
//!
//! Two of the providers are handed this repository's config outright. taplo
//! and biome both search upward from the directory they are run in and would
//! find nothing here, so both of them formatted this tree at their own
//! defaults and called it clean. A target that has a config of its own keeps
//! it.
//!
//! `--check` runs the same formatters in verify mode and the linters as well,
//! and writes nothing. It never runs clippy — that is a different question
//! than whether this tree is formatted.
//!
//! `--add` and `--sync` are the other half: this repository keeps one config
//! per language under `shared/tools/`, and these two put them into another
//! project. `--add` asks about each and can introduce a config a project does
//! not have; `--sync` refreshes the ones already there and never introduces
//! one.
//!
//! A tool nobody installed is named in the report and changes nothing else.
//! Four of the ten providers are missing on the machine this was written on,
//! and a run there is still a success.
//!
//! What a run says is a count, and one line for each provider that has
//! something to report with the files it fell over on under it. The table,
//! the commands and everything the tools said are behind `--verbose`.
//!
//! stdout carries data — the completion script, the command dump, the help —
//! and the prompts. Everything a person reads is on stderr.

mod configs;
mod lang;
mod owns;
mod render;
mod run;
mod walk;

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, ValueHint};
use workstation::{Completions, Style};

use lang::{Lang, Mode};

const PROGRAM: &str = "dotfile-format";

#[derive(Parser)]
#[command(
    version,
    about = "Format a tree with the tool that owns each language",
    long_about = "Format a tree with the tool that owns each language.

Every file under the target is sorted by extension into the row of the \
provider table that owns it — ruff for Python, biome for the web languages, \
stylua for Lua, cargo fmt for Rust, taplo for TOML, yamlfmt for YAML, \
sqlfluff for SQL, shfmt for shell, goimports and gofmt for Go. dotfmt is \
asked which files it owns rather than guessed at. The rows run in parallel; \
the programs within one row run in order.

taplo and biome are pointed at this repository\'s configuration on the \
command line, because neither would find it otherwise. A target that has a \
config of its own for one of them keeps it.

--check runs the same formatters in verify mode and the linters as well, \
writes nothing, and exits 1 if there is anything to report. It does not run \
clippy.

--add and --sync copy this repository's tool configuration into another \
project: --add asks about each file, --sync replaces only the ones already \
there.

A run reports a count, and one line for each provider that has something to \
say with the files it fell over on under it. --verbose adds the per-language \
table, the commands as they were run, and everything the tools said.

A tool that is not installed is named in the report and is never a failure. \
Files git ignores are left out, .git and the usual build directories are not \
walked, and symlinked directories are not followed.",
    after_long_help = "Examples:
  dotfile format .                 Format the tree under the working directory
  dotfile format --check .         Verify formatting and lint, changing nothing
  dotfile format src/main.rs       Format one file
  dotfile format -a ~/code/thing   Offer this repo's tool configs to a project
  dotfile format -s ~/code/thing   Refresh the configs that project already has"
)]
struct Cli {
    /// File or directory to format
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    target: Option<PathBuf>,

    /// Verify formatting and run the linters, writing nothing
    #[arg(long, conflicts_with_all = ["add", "sync"])]
    check: bool,

    /// Offer this repository's tool configs, asking about each
    #[arg(short = 'a', long, conflicts_with = "sync")]
    add: bool,

    /// Replace the tool configs the target already has, without asking
    #[arg(short = 's', long)]
    sync: bool,

    /// Show each command as it is run, and what it said
    #[arg(short, long, conflicts_with = "quiet")]
    verbose: bool,

    /// Say nothing; the exit code is the whole report
    #[arg(short, long)]
    quiet: bool,

    #[command(flatten)]
    completions: Completions,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    // Nothing named and nothing asked for: the help is the answer, so it is
    // what the caller wanted rather than a misuse of the tool.
    if cli.target.is_none() && !cli.check && !cli.add && !cli.sync {
        Cli::command().print_help().ok();
        println!();
        return ExitCode::SUCCESS;
    }
    match go(&cli) {
        Ok(status) => status,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

fn go(cli: &Cli) -> Result<ExitCode, String> {
    let style = Style::for_stderr();
    let target = cli.target.clone().unwrap_or_else(|| PathBuf::from("."));
    let (root, single) = resolve(&target)?;

    // A file named outright is used as given: no walk, no skip list, and no
    // asking git — naming a file is already the answer to all three.
    let found = match single {
        Some(name) => walk::Found {
            files: vec![name],
            ..walk::Found::default()
        },
        None => {
            let mut found = walk::walk(&root);
            found.files = walk::drop_ignored(&root, std::mem::take(&mut found.files));
            found
        }
    };

    if cli.add || cli.sync {
        return manage(cli, &root, &found.files, &style);
    }
    if !cli.quiet {
        warn(&found, cli.verbose);
    }

    let mode = if cli.check { Mode::Check } else { Mode::Write };

    // Two of the providers search upward from the directory they are run in
    // and would find nothing, so they are handed this repository's config
    // outright. A target with one of its own keeps it.
    let source = configs::source()?;
    let injected = configs::injections(&source, &root);

    // dotfmt's row is dotfmt's answer, not a guess from an extension list —
    // when there is an answer to be had. When there is not, the three
    // extensions this crate has always used are the row, because an empty one
    // would quietly stop formatting every `.conf` in the tree while the run
    // reported success.
    let (owned, unasked) = match owns::ask(&root, &found.files) {
        owns::Owned::Claimed(owned) => (owned, None),
        // Not installed is not a failure, and the row reads like every other
        // provider's: the files it would have had, and the tool that was not
        // there to take them.
        owns::Owned::Missing => (run::by_extension(&found.files), None),
        // Installed and unable to answer is a failure, because a dotfmt too
        // old to know `--owns` still formats — so the row would otherwise run
        // to completion under the wrong selection rule and say nothing.
        owns::Owned::Failed(said) => (run::by_extension(&found.files), Some(said)),
    };
    let claimed: HashSet<&OsStr> = owned.iter().map(|path| path.as_os_str()).collect();
    let rest: Vec<PathBuf> = found
        .files
        .iter()
        .filter(|path| !claimed.contains(path.as_os_str()))
        .cloned()
        .collect();
    let mut work = run::with_dotfmt(run::sort(rest), owned);

    // No row is handed a file that is encrypted at rest, whatever its name is
    // and whichever provider would have taken it. Reformatting one is a diff
    // the size of the file on every run at best, and at worst it breaks the
    // MAC that guards it — and a secrets file is the last thing that should be
    // rewritten by a tool nobody asked to look at it.
    let sealed = walk::drop_encrypted(&root, &mut work);
    if !cli.quiet && !sealed.is_empty() {
        // Leaving one alone is the tool working as intended, so this is a
        // `--verbose` line like a lockfile — unless it is the whole reason
        // there is nothing to do, when a silent no-op would be a mystery.
        note(&sealed, cli.verbose || work.is_empty());
    }

    if work.is_empty() && unasked.is_none() {
        if !cli.quiet {
            eprintln!("{PROGRAM}: nothing to format in {}", render::shorten(&root));
        }
        return Ok(ExitCode::SUCCESS);
    }

    let plan = run::Plan {
        mode,
        verbose: cli.verbose,
        injected: &injected,
    };
    let mut done = run::run(&root, work, &plan);
    if let Some(said) = unasked {
        match done.iter_mut().find(|ran| ran.lang == Lang::Dotfmt) {
            Some(ran) => ran.unasked(&said),
            // Nothing in the tree looked like dotfmt's even by extension, so
            // there is no row to mark and the failure is the row.
            None => {
                done.push(run::Ran::broken(Lang::Dotfmt, said));
                run::order(&mut done);
            }
        }
    }
    if !cli.quiet {
        report(&done, mode, cli.verbose, &root, cli.check, &style);
    }

    // Drift is only a finding in --check; a write run that reformatted half
    // the tree did exactly what it was asked to.
    let reported = done
        .iter()
        .any(|ran| ran.failed || (mode == Mode::Check && ran.findings));
    Ok(if reported {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The encrypted files this run passed over.
fn note(sealed: &[PathBuf], say: bool) {
    if !say {
        return;
    }
    let names: Vec<String> = sealed
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    eprintln!(
        "{PROGRAM}: {} encrypted {} left alone: {}",
        names.len(),
        if names.len() == 1 { "file" } else { "files" },
        names.join(", ")
    );
}

/// The whole of what a run says, which by default is a count and nothing
/// else. `--verbose` is where the table, the commands and the tools' own
/// words live.
fn report(done: &[run::Ran], mode: Mode, verbose: bool, root: &Path, check: bool, style: &Style) {
    if !verbose {
        for line in render::summary(done, mode, style) {
            eprintln!("{line}");
        }
        return;
    }
    let what = if check { "check" } else { "" };
    for line in render::heading(PROGRAM, root, what, style) {
        eprintln!("{line}");
    }
    for line in render::report(done, mode, style) {
        eprintln!("{line}");
    }
    eprintln!();
    eprintln!("  {}", render::tally(done, mode));
}

/// `--add` and `--sync`: this repository's tool configuration, put into
/// another project.
fn manage(cli: &Cli, root: &Path, files: &[PathBuf], style: &Style) -> Result<ExitCode, String> {
    let source = configs::source()?;
    let placements = configs::placements(root, &configs::detect(root, files));

    if !cli.quiet {
        let what = if cli.add { "add" } else { "sync" };
        for line in render::heading(PROGRAM, root, what, style) {
            eprintln!("{line}");
        }
        if let Some(note) = render::provenance(&source, style) {
            eprintln!("{note}");
            eprintln!();
        }
    }

    let done = if cli.add {
        let mut ask = |question: &str| {
            let answer = workstation::confirm_each(question);
            if answer.is_none() {
                // The answers ran out mid-prompt; finish the line it started.
                println!();
            }
            answer
        };
        configs::add(&source, &placements, &mut ask)?
    } else {
        configs::sync(&source, &placements)?
    };

    if !cli.quiet {
        if done.is_empty() {
            eprintln!("  {}", style.dim("nothing copied"));
        } else {
            for line in render::placed(&done, style) {
                eprintln!("{line}");
            }
        }
    }
    // Declining every file is an answer, not a failure.
    Ok(ExitCode::SUCCESS)
}

/// The run root, and the one file to work on when the target named a file.
///
/// `metadata` follows symlinks, so a link to a directory is a target and a
/// link to a file is that file — which is the only way a run reaches a file
/// outside the tree it is standing in.
fn resolve(target: &Path) -> Result<(PathBuf, Option<PathBuf>), String> {
    let metadata = fs::metadata(target).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => format!("no such file or directory: {}", target.display()),
        _ => format!("{}: {error}", target.display()),
    })?;
    if metadata.is_dir() {
        return Ok((real(target)?, None));
    }
    if !metadata.is_file() {
        return Err(format!("not a file or directory: {}", target.display()));
    }
    let holder = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let name = target
        .file_name()
        .ok_or_else(|| format!("not a file: {}", target.display()))?;
    Ok((real(holder)?, Some(PathBuf::from(name))))
}

fn real(directory: &Path) -> Result<PathBuf, String> {
    fs::canonicalize(directory).map_err(|error| format!("{}: {error}", directory.display()))
}

/// What the walk could not see, and what it deliberately passed over.
///
/// The first three are the tree being an incomplete account of itself, so they
/// are always said out loud. A lockfile left alone is the tool working as
/// intended rather than a warning, so it is named under `--verbose` — which is
/// still the difference between a documented rule and a silent one.
fn warn(found: &walk::Found, verbose: bool) {
    if verbose && !found.lockfiles.is_empty() {
        let names: Vec<String> = found
            .lockfiles
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        eprintln!(
            "{PROGRAM}: {} generated {} left alone: {}",
            names.len(),
            if names.len() == 1 {
                "lockfile"
            } else {
                "lockfiles"
            },
            names.join(", ")
        );
    }
    if found.unreadable > 0 {
        eprintln!(
            "{PROGRAM}: {} {} could not be read",
            found.unreadable,
            directories(found.unreadable)
        );
    }
    if found.deep > 0 {
        eprintln!(
            "{PROGRAM}: {} {} deeper than {} levels were not walked",
            found.deep,
            directories(found.deep),
            walk::MAX_DEPTH
        );
    }
    if found.capped {
        eprintln!(
            "{PROGRAM}: more than {} files under the target; the rest were left out",
            walk::MAX_FILES
        );
    }
}

fn directories(count: usize) -> &'static str {
    if count == 1 {
        "directory"
    } else {
        "directories"
    }
}

#[cfg(test)]
mod tests;
