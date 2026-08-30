mod curly;
mod dash;
mod docstring;
mod edit;
mod glyphs;
mod hash;
mod keep;
mod lang;
mod purge;
mod report;
mod scan;
mod walk;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, ValueHint};
use rayon::prelude::*;
use workstation::{Completions, Style};

use purge::{Outcome, Saved};
use report::Row;
use walk::{Found, Wanted};

const PROGRAM: &str = "doc-purge";

#[derive(Parser)]
#[command(
    version,
    about = "Purge comments, doc strings and typographic glyphs from source files",
    long_about = "Purge comments, doc strings and typographic glyphs from source files.",
    after_long_help = "Examples:
  doc-purge .                Purge everything below here, after asking
  doc-purge src --dry        Show what would go, and change nothing
  doc-purge . -t py -t rs    Look at python and rust files only
  doc-purge . -y             Purge without being asked"
)]
struct Cli {
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    targets: Vec<PathBuf>,

    #[arg(short = 't', long = "type", value_name = "TYPE", value_delimiter = ',')]
    types: Vec<String>,

    #[arg(long)]
    dry: bool,

    #[arg(short, long)]
    yes: bool,

    #[arg(short, long)]
    verbose: bool,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Default)]
struct Done {
    path: PathBuf,
    minus: usize,
    plus: usize,
    comments: usize,
    glyphs: usize,
    docs: usize,
    saved: Saved,
    skip: Option<&'static str>,
    content: Option<Vec<u8>>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    if cli.targets.is_empty() {
        Cli::command().print_help().ok();
        println!();
        return ExitCode::SUCCESS;
    }
    match run(&cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

fn run(cli: &Cli) -> Result<bool, String> {
    let wanted = wanted(&cli.types)?;
    let gathered = walk::gather(&cli.targets, &wanted)?;
    if gathered.unreadable > 0 {
        let plural = if gathered.unreadable == 1 {
            "directory"
        } else {
            "directories"
        };
        eprintln!(
            "{PROGRAM}: {} {plural} could not be read",
            gathered.unreadable
        );
    }
    let style = Style::for_stdout();
    let done: Vec<Done> = gathered.files.par_iter().map(inspect).collect();

    let mut rows = Vec::new();
    let mut skips: Vec<(String, String)> = gathered
        .notes
        .iter()
        .map(|note| (report::shorten(&note.path), note.reason.to_string()))
        .collect();
    let mut saved = Saved::default();
    let mut comments = 0usize;
    let mut glyphs = 0usize;
    let mut docs = 0usize;
    let mut minus = 0usize;
    let mut plus = 0usize;
    for entry in &done {
        saved.add(entry.saved);
        if let Some(reason) = entry.skip {
            skips.push((report::shorten(&entry.path), reason.to_string()));
            continue;
        }
        if entry.content.is_none() {
            continue;
        }
        comments += entry.comments;
        glyphs += entry.glyphs;
        docs += entry.docs;
        minus += entry.minus;
        plus += entry.plus;
        rows.push(Row {
            path: report::shorten(&entry.path),
            minus: entry.minus,
            plus: entry.plus,
        });
    }
    rows.sort_by(|left, right| left.path.cmp(&right.path));
    skips.sort();

    if rows.is_empty() {
        if !skips.is_empty() || cli.verbose {
            report::heading(&cli.targets, &style);
            report::listed("left alone", &skips, cli.verbose, &style);
            println!();
        }
        println!("{PROGRAM}: nothing to purge");
        return Ok(true);
    }

    report::heading(&cli.targets, &style);
    report::purged(&rows, cli.verbose, &style);
    if saved.any() {
        println!();
        println!("  {}", style.bold("kept"));
        println!("    {}", style.dim(&sentence(&saved)));
    }
    report::listed("left alone", &skips, cli.verbose, &style);
    println!();
    println!(
        "  {}",
        counted(comments, docs, glyphs, rows.len(), minus, plus, &style)
    );

    if cli.dry {
        return Ok(true);
    }
    if !cli.yes {
        println!();
        match workstation::confirm("  Continue? [Y/n] ") {
            Some(true) => {}
            Some(false) => {
                println!("{PROGRAM}: cancelled");
                return Ok(true);
            }
            None => {
                println!();
                return Ok(false);
            }
        }
    }

    let mut failures = 0usize;
    for entry in &done {
        let Some(content) = &entry.content else {
            continue;
        };
        if let Err(error) = commit(&entry.path, content) {
            eprintln!("{PROGRAM}: {}: {error}", entry.path.display());
            failures += 1;
        }
    }
    println!();
    println!("  {}", style.dim("done"));
    Ok(failures == 0)
}

fn inspect(found: &Found) -> Done {
    let mut done = Done {
        path: found.path.clone(),
        ..Done::default()
    };
    let Ok(metadata) = fs::metadata(&found.path) else {
        done.skip = Some("could not be read");
        return done;
    };
    if metadata.len() > walk::LIMIT {
        done.skip = Some("larger than doc-purge reads");
        return done;
    }
    let Ok(bytes) = fs::read(&found.path) else {
        done.skip = Some("could not be read");
        return done;
    };
    if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
        done.skip = Some("not utf-8 text");
        return done;
    }
    match purge::purge(&bytes, found.dialect) {
        Outcome::Skipped(reason) => done.skip = Some(reason),
        Outcome::Untouched(saved) => done.saved = saved,
        Outcome::Changed(edited, saved) => {
            done.minus = edited.minus;
            done.plus = edited.plus;
            done.comments = edited.comments;
            done.glyphs = edited.glyphs;
            done.docs = edited.docs;
            done.saved = saved;
            done.content = Some(edited.content);
        }
    }
    done
}

fn wanted(types: &[String]) -> Result<Wanted, String> {
    if types.is_empty() {
        return Ok(Wanted::default());
    }
    let mut extensions = Vec::new();
    for token in types {
        let Some(language) = lang::for_token(token) else {
            return Err(format!(
                "unknown type: {token}\n{PROGRAM}: the types it reads are {}",
                lang::known()
            ));
        };
        extensions.extend(language.extensions.iter().map(|found| found.to_string()));
    }
    extensions.sort();
    extensions.dedup();
    Ok(Wanted {
        extensions: Some(extensions),
    })
}

fn sentence(saved: &Saved) -> String {
    let mut parts = Vec::new();
    if saved.shebangs > 0 {
        parts.push(report::plural(saved.shebangs, "shebang", "shebangs"));
    }
    if saved.directives > 0 {
        parts.push(report::plural(saved.directives, "directive", "directives"));
    }
    if saved.licenses > 0 {
        parts.push(report::plural(
            saved.licenses,
            "licence header",
            "licence headers",
        ));
    }
    parts.join(", ")
}

fn counted(
    comments: usize,
    docs: usize,
    glyphs: usize,
    files: usize,
    minus: usize,
    plus: usize,
    style: &Style,
) -> String {
    let mut parts = Vec::new();
    if comments > 0 {
        parts.push(report::plural(comments, "comment", "comments"));
    }
    if docs > 0 {
        parts.push(report::plural(docs, "doc string", "doc strings"));
    }
    if glyphs > 0 {
        parts.push(report::plural(glyphs, "glyph", "glyphs"));
    }
    if parts.is_empty() {
        parts.push("nothing".to_string());
    }
    format!(
        "{} in {}   {} {}",
        parts.join(", "),
        report::plural(files, "file", "files"),
        style.red(&format!("-{minus}")),
        style.green(&format!("+{plus}"))
    )
}

fn commit(path: &Path, content: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.doc-purge"));
    let permissions = fs::metadata(path)?.permissions();
    fs::write(&temporary, content)?;
    fs::set_permissions(&temporary, permissions)?;
    fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests;
