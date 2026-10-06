#![forbid(unsafe_code)]

mod curly;
mod dash;
mod docstring;
mod edit;
mod glyphs;
mod hash;
mod keep;
mod lang;
mod purge;
mod scan;
mod walk;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use rayon::prelude::*;
use workstation::{Completable, Completions, Style, path, text};

use purge::{Outcome, Saved};
use ui_batch::{Decision, Options, Row, Run};
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

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
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
    workstation::run::<Cli>(PROGRAM, |cli| {
        if cli.targets.is_empty() {
            workstation::cli::command::<Cli>().print_help().ok();
            println!();
            return Ok(ExitCode::SUCCESS);
        }
        let purged = run(&cli)?;
        Ok(if purged {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        })
    })
}

fn run(cli: &Cli) -> Result<bool, String> {
    let wanted = wanted(&cli.types)?;
    let gathered = walk::gather(&cli.targets, &wanted)?;
    if gathered.unreadable > 0 {
        eprintln!(
            "{PROGRAM}: {} {} could not be read",
            gathered.unreadable,
            text::plural(gathered.unreadable, "directory", "directories")
        );
    }
    let style = Style::for_stdout();
    let labels: Vec<String> = cli.targets.iter().map(|target| path::shorten(target)).collect();
    let run = Run::new(
        PROGRAM,
        &labels,
        &style,
        Options {
            every_row: cli.verbose,
            dry: cli.dry,
            yes: cli.yes,
        },
    );
    let done: Vec<Done> = gathered.files.par_iter().map(inspect).collect();

    let mut changes: Vec<&Done> = Vec::new();
    let mut skips: Vec<(String, String)> = gathered
        .notes
        .iter()
        .map(|note| (path::shorten(&note.path), note.reason.to_string()))
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
            skips.push((path::shorten(&entry.path), reason.to_string()));
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
        changes.push(entry);
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    skips.sort();

    if changes.is_empty() {
        if !skips.is_empty() || cli.verbose {
            run.heading();
            run.section("left alone", &left_alone(&skips, &style));
            println!();
        }
        println!("{PROGRAM}: nothing to purge");
        return Ok(true);
    }

    run.heading();
    run.section("purge", &purged(&changes, &style));
    if saved.any() {
        run.note("kept", &sentence(&saved));
    }
    run.section("left alone", &left_alone(&skips, &style));
    run.summary(&summary(
        comments,
        docs,
        glyphs,
        changes.len(),
        minus,
        plus,
        &style,
    ));

    match run.decide() {
        Decision::Stop => return Ok(true),
        Decision::Interrupted => return Ok(false),
        Decision::Proceed => {}
    }

    let failures = run.apply(&changes, |entry| {
        let content = entry.content.as_deref().unwrap_or_default();
        commit(&entry.path, content).map_err(|error| format!("{}: {error}", entry.path.display()))
    });
    Ok(failures == 0)
}

fn left_alone(skips: &[(String, String)], style: &Style) -> Vec<Row> {
    skips
        .iter()
        .map(|(path, reason)| Row::detailed(path, style.dim(reason)))
        .collect()
}

fn purged(changes: &[&Done], style: &Style) -> Vec<Row> {
    let counts: Vec<(String, String)> = changes
        .iter()
        .map(|entry| {
            (
                format!("-{}", entry.minus),
                if entry.plus > 0 {
                    format!("+{}", entry.plus)
                } else {
                    String::new()
                },
            )
        })
        .collect();
    let minus_room = counts
        .iter()
        .map(|(minus, _)| minus.len())
        .max()
        .unwrap_or(0);
    changes
        .iter()
        .zip(counts)
        .map(|(entry, (minus, plus))| {
            let lead = " ".repeat(minus_room - minus.len());
            Row::detailed(
                path::shorten(&entry.path),
                format!("{lead}{}  {}", style.red(&minus), style.green(&plus)),
            )
        })
        .collect()
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
        parts.push(text::counted(saved.shebangs, "shebang", "shebangs"));
    }
    if saved.directives > 0 {
        parts.push(text::counted(saved.directives, "directive", "directives"));
    }
    if saved.licenses > 0 {
        parts.push(text::counted(
            saved.licenses,
            "licence header",
            "licence headers",
        ));
    }
    parts.join(", ")
}

fn summary(
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
        parts.push(text::counted(comments, "comment", "comments"));
    }
    if docs > 0 {
        parts.push(text::counted(docs, "doc string", "doc strings"));
    }
    if glyphs > 0 {
        parts.push(text::counted(glyphs, "glyph", "glyphs"));
    }
    if parts.is_empty() {
        parts.push("nothing".to_string());
    }
    format!(
        "{} in {}   {} {}",
        parts.join(", "),
        text::counted(files, "file", "files"),
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
#[path = "../tests/unit/main_tests.rs"]
mod tests;
