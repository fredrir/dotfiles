#![forbid(unsafe_code)]

mod config;
mod walk;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, SystemTime};

use clap::{Parser, ValueEnum, ValueHint};
use rayon::prelude::*;
use serde_json::json;
use workstation::{Completable, Completions, Style, path, text};

use doc_purge_core::Change;
use doc_purge_core::lang;
use doc_purge_core::purge::{self, LIMIT, Options as PurgeOptions, Outcome, Saved, TOO_LARGE};
use ui_batch::{Decision, Options as BatchOptions, Row, Run};

use config::Resolver;
use walk::{Found, Wanted};

const PROGRAM: &str = "doc-purge";

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Only {
    Comments,
    Docs,
    Glyphs,
}

#[derive(Parser)]
#[command(
    version,
    about = "Purge comments, doc strings and typographic glyphs from source files",
    long_about = "Purge comments, doc strings and typographic glyphs from source files.

Exit codes:
  0  nothing to purge, purged successfully, or --check found everything clean
  1  --check found at least one file that would change
  2  failure: an unreadable target, a bad flag, or a configuration error",
    after_long_help = "Examples:
  doc-purge .                Purge everything below here, after asking
  doc-purge src --dry        Show what would go, and change nothing
  doc-purge . -t py -t rs    Look at python and rust files only
  doc-purge . --check        Exit 1 when anything would change
  doc-purge . --diff         Show a unified diff with three lines of context
  doc-purge . -t css         Look at css, module css and postcss files
  doc-purge . -t scss        Look at scss files only
  doc-purge . -y             Purge without being asked
  doc-purge . --only comments  Purge comments, leaving doc strings and glyphs"
)]
struct Cli {
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    targets: Vec<PathBuf>,

    #[arg(short = 't', long = "type", value_name = "TYPE", value_delimiter = ',')]
    types: Vec<String>,

    #[arg(long)]
    check: bool,

    #[arg(long, conflicts_with_all = ["json"])]
    diff: bool,

    #[arg(long, value_name = "FILENAME", value_hint = ValueHint::FilePath, conflicts_with_all = ["targets", "json", "stats", "explain", "owns", "watch", "uncovered", "list_types"])]
    stdin: Option<PathBuf>,

    #[arg(short, long)]
    quiet: bool,

    #[arg(long, conflicts_with_all = ["diff", "stats", "explain"])]
    json: bool,

    #[arg(long)]
    stats: bool,

    #[arg(long, value_name = "KINDS", value_delimiter = ',')]
    only: Vec<Only>,

    #[arg(long)]
    no_glyphs: bool,

    #[arg(long, conflicts_with_all = ["json", "diff", "stats"])]
    explain: bool,

    #[arg(long, exclusive = true)]
    list_types: bool,

    #[arg(long, conflicts_with = "keep_docstrings")]
    aggressive: bool,

    #[arg(long)]
    keep_docstrings: bool,

    #[arg(long, conflicts_with_all = ["owns", "watch", "uncovered", "list_types", "diff", "json", "stats", "explain"])]
    editor: bool,

    #[arg(long, conflicts_with_all = ["targets", "stdin", "check", "watch", "uncovered", "list_types", "diff", "json", "stats", "explain", "editor"])]
    owns: bool,

    #[arg(long, conflicts_with_all = ["stdin", "editor", "owns", "uncovered", "list_types"])]
    watch: bool,

    #[arg(long, conflicts_with_all = ["stdin", "editor", "owns", "watch", "list_types", "diff", "json", "stats", "explain", "check"])]
    uncovered: bool,

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Code {
    Clean,
    Unclean,
    Failure,
}

impl Code {
    fn exit(self) -> ExitCode {
        match self {
            Code::Clean => ExitCode::SUCCESS,
            Code::Unclean => ExitCode::from(1),
            Code::Failure => ExitCode::from(2),
        }
    }
}

fn purge_options(cli: &Cli) -> PurgeOptions {
    let mut options = PurgeOptions::default();
    if cli.only.is_empty() {
        options.glyphs = !cli.no_glyphs;
        options.docs = !cli.keep_docstrings;
    } else {
        options.comments = cli.only.contains(&Only::Comments);
        options.docs = cli.only.contains(&Only::Docs);
        options.glyphs = cli.only.contains(&Only::Glyphs);
    }
    options.aggressive = cli.aggressive;
    options.explain = cli.explain;
    options
}

#[derive(Default)]
struct Done {
    path: PathBuf,
    language: String,
    minus: usize,
    plus: usize,
    comments: usize,
    glyphs: usize,
    docs: usize,
    saved: Saved,
    skip: Option<&'static str>,
    content: Option<Vec<u8>>,
    original: Option<Vec<u8>>,
    changes: Vec<Change>,
    failed: bool,
}

impl Done {
    fn changed(&self) -> bool {
        self.content.is_some()
    }

    fn status(&self) -> &'static str {
        if self.failed {
            "failed"
        } else if self.skip.is_some() {
            "skipped"
        } else if self.changed() {
            "changed"
        } else {
            "untouched"
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Totals {
    files: usize,
    changed: usize,
    comments: usize,
    docs: usize,
    glyphs: usize,
    minus: usize,
    plus: usize,
}

fn main() -> ExitCode {
    workstation::run::<Cli>(PROGRAM, |cli| Ok(dispatch(cli)))
}

fn dispatch(cli: Cli) -> ExitCode {
    match run(&cli) {
        Ok(code) => code.exit(),
        Err(message) => {
            eprintln!("{PROGRAM}: {message}");
            Code::Failure.exit()
        }
    }
}

fn run(cli: &Cli) -> Result<Code, String> {
    if cli.list_types {
        list_types();
        return Ok(Code::Clean);
    }
    let wanted = wanted(&cli.types)?;
    let options = purge_options(cli);
    if cli.owns {
        return owns(&wanted, cli.quiet);
    }
    if cli.editor {
        return editor(cli, options);
    }
    if let Some(name) = &cli.stdin {
        return stream(cli, name, options);
    }
    let targets = if cli.targets.is_empty() {
        if !scans_without_target(cli) {
            workstation::cli::command::<Cli>().print_help().ok();
            println!();
            return Ok(Code::Clean);
        }
        vec![PathBuf::from(".")]
    } else {
        cli.targets.clone()
    };
    if cli.uncovered {
        return uncovered(cli, &targets);
    }
    if cli.watch {
        return watch(cli, &targets, &wanted, options);
    }
    execute(cli, &targets, &wanted, options)
}

fn scans_without_target(cli: &Cli) -> bool {
    cli.check || cli.diff || cli.json || cli.stats || cli.explain || cli.watch || cli.uncovered
}

fn execute(
    cli: &Cli,
    targets: &[PathBuf],
    wanted: &Wanted,
    options: PurgeOptions,
) -> Result<Code, String> {
    let resolver = Resolver::new();
    let gathered = walk::gather(targets, wanted, &resolver)?;
    if !cli.quiet {
        for warning in &gathered.warnings {
            eprintln!("{PROGRAM}: warning: {warning}");
        }
    }
    if gathered.unreadable > 0 {
        eprintln!(
            "{PROGRAM}: {} {} could not be read",
            gathered.unreadable,
            text::plural(gathered.unreadable, "directory", "directories")
        );
    }
    let style = Style::for_stdout();
    let labels: Vec<String> = targets.iter().map(|target| path::shorten(target)).collect();
    let run = Run::new(
        PROGRAM,
        &labels,
        &style,
        BatchOptions {
            every_row: cli.verbose,
            dry: cli.dry,
            yes: cli.yes,
        },
    );
    let mut done: Vec<Done> = gathered
        .files
        .par_iter()
        .map(|found| inspect(found, options))
        .collect();
    done.sort_by(|left, right| left.path.cmp(&right.path));

    let mut changes: Vec<&Done> = Vec::new();
    let mut notes: Vec<(String, String)> = gathered
        .notes
        .iter()
        .map(|note| (path::shorten(&note.path), note.reason.to_string()))
        .collect();
    let mut saved = Saved::default();
    let mut totals = Totals {
        files: done.len(),
        ..Totals::default()
    };
    for entry in &done {
        saved.add(&entry.saved);
        if let Some(reason) = entry.skip {
            notes.push((path::shorten(&entry.path), reason.to_string()));
            continue;
        }
        if !entry.changed() {
            continue;
        }
        totals.changed += 1;
        totals.comments += entry.comments;
        totals.docs += entry.docs;
        totals.glyphs += entry.glyphs;
        totals.minus += entry.minus;
        totals.plus += entry.plus;
        changes.push(entry);
    }
    notes.sort();
    let failed = done.iter().any(|entry| entry.failed);

    if cli.json {
        json(&done, &gathered.notes, totals);
    }
    if cli.diff {
        for entry in &changes {
            let original = entry.original.as_deref().unwrap_or_default();
            print!("{}", unified_diff(&entry.path, original, &entry.changes));
        }
    }
    if cli.stats {
        stats(&changes);
    }

    let human = (!cli.quiet || cli.explain) && !cli.json && !cli.diff && !cli.stats;
    if human {
        if changes.is_empty() {
            let reported = if cli.explain && done.iter().any(|entry| entry.saved.any()) {
                run.heading();
                run.section("kept", &kept(&done, &style));
                true
            } else if !notes.is_empty() || cli.verbose {
                run.heading();
                run.section("left alone", &left_alone(&notes, &style));
                true
            } else {
                false
            };
            if reported {
                println!();
            }
            println!("{PROGRAM}: nothing to purge");
        } else {
            run.heading();
            run.section(
                if cli.check || cli.explain {
                    "would purge"
                } else {
                    "purge"
                },
                &purged(&changes, &style),
            );
            if cli.explain {
                run.section("kept", &kept(&done, &style));
            } else if saved.any() {
                run.note("kept", &sentence(&saved));
            }
            run.section("left alone", &left_alone(&notes, &style));
            run.summary(&summary(
                totals.comments,
                totals.docs,
                totals.glyphs,
                changes.len(),
                totals.minus,
                totals.plus,
                &style,
            ));
        }
    }

    if cli.check {
        return Ok(if failed {
            Code::Failure
        } else if changes.is_empty() {
            Code::Clean
        } else {
            Code::Unclean
        });
    }
    let report_only = cli.diff || cli.json || cli.stats || cli.explain || cli.dry;
    if report_only {
        return Ok(if failed { Code::Failure } else { Code::Clean });
    }

    match run.decide() {
        Decision::Stop => return Ok(Code::Clean),
        Decision::Interrupted => return Ok(Code::Failure),
        Decision::Proceed => {}
    }
    let failures = if cli.quiet {
        apply(&changes)
    } else {
        run.apply(&changes, |entry| commit_entry(entry))
    };
    Ok(if failures == 0 && !failed {
        Code::Clean
    } else {
        Code::Failure
    })
}

fn inspect(found: &Found, options: PurgeOptions) -> Done {
    let mut done = Done {
        path: found.path.clone(),
        language: format!("{:?}", found.dialect),
        ..Done::default()
    };
    if let Some(reason) = found.skip {
        done.skip = Some(reason);
        return done;
    }
    let Ok(metadata) = fs::metadata(&found.path) else {
        done.skip = Some("could not be read");
        done.failed = found.explicit;
        return done;
    };
    if metadata.len() > LIMIT {
        done.skip = Some(TOO_LARGE);
        return done;
    }
    let Ok(bytes) = fs::read(&found.path) else {
        done.skip = Some("could not be read");
        done.failed = found.explicit;
        return done;
    };
    if found.check_generated && walk::generated(&bytes) {
        done.skip = Some("generated");
        return done;
    }
    match purge::purge_with(&bytes, found.dialect, options) {
        Outcome::Skipped(reason) => done.skip = Some(reason),
        Outcome::Untouched(saved) => done.saved = saved,
        Outcome::Changed(edited, saved) => {
            let edited = *edited;
            done.minus = edited.minus;
            done.plus = edited.plus;
            done.comments = edited.comments;
            done.glyphs = edited.glyphs;
            done.docs = edited.docs;
            done.saved = saved;
            done.content = Some(edited.content);
            done.original = Some(bytes);
            done.changes = edited.changes;
        }
    }
    done
}

fn stream(cli: &Cli, name: &Path, options: PurgeOptions) -> Result<Code, String> {
    let wanted = wanted(&cli.types)?;
    let input = read_stdin()?;
    let (output, code) = stream_input(cli, name, &wanted, &input, options)?;
    if !(cli.check || cli.diff) {
        write_stdout(&output)?;
    }
    Ok(code)
}

fn editor(cli: &Cli, options: PurgeOptions) -> Result<Code, String> {
    let wanted = match wanted(&cli.types) {
        Ok(wanted) => wanted,
        Err(error) => {
            let input = read_stdin()?;
            if cli.verbose {
                eprintln!("{PROGRAM}: {error}");
            }
            write_stdout(&input)?;
            return Ok(Code::Clean);
        }
    };
    let input = read_stdin()?;
    let name = match (&cli.stdin, cli.targets.as_slice()) {
        (Some(name), _) => name.clone(),
        (None, [only]) => only.clone(),
        (None, []) => {
            if cli.verbose {
                eprintln!("{PROGRAM}: --editor needs one TARGET or --stdin FILENAME");
            }
            write_stdout(&input)?;
            return Ok(Code::Clean);
        }
        (None, _) => {
            if cli.verbose {
                eprintln!("{PROGRAM}: --editor accepts one filename");
            }
            write_stdout(&input)?;
            return Ok(Code::Clean);
        }
    };
    match stream_input(cli, &name, &wanted, &input, options) {
        Ok((output, code)) => {
            if !cli.check {
                write_stdout(&output)?;
            }
            Ok(code)
        }
        Err(error) => {
            if cli.verbose {
                eprintln!("{PROGRAM}: {error}");
            }
            write_stdout(&input)?;
            Ok(Code::Clean)
        }
    }
}

fn stream_input(
    cli: &Cli,
    name: &Path,
    wanted: &Wanted,
    input: &[u8],
    options: PurgeOptions,
) -> Result<(Vec<u8>, Code), String> {
    let resolver = Resolver::new();
    let config = resolver.for_path(name).map_err(|error| error.to_string())?;
    let absolute = resolver.absolute(name);
    let Some(language) = lang::for_path(name) else {
        return Err(format!(
            "not a file type doc-purge reads: {}",
            name.display()
        ));
    };
    if !wanted.allows(name)
        || !config.allows_path(&absolute)
        || !config.language_enabled(language.name, &absolute)
    {
        return Ok((input.to_vec(), Code::Clean));
    }
    if config.generated == config::Generated::Skip && walk::generated(input) {
        return Ok((input.to_vec(), Code::Clean));
    }
    match purge::purge_with(input, language.dialect, options) {
        Outcome::Changed(edited, _) => {
            if cli.diff {
                print!("{}", unified_diff(name, input, &edited.changes));
            }
            let code = if cli.check {
                Code::Unclean
            } else {
                Code::Clean
            };
            Ok((edited.content, code))
        }
        Outcome::Untouched(_) | Outcome::Skipped(_) => Ok((input.to_vec(), Code::Clean)),
    }
}

fn owns(wanted: &Wanted, quiet: bool) -> Result<Code, String> {
    let input = read_stdin()?;
    let resolver = Resolver::new();
    let mut claimed: Vec<PathBuf> = Vec::new();
    let mut warned: BTreeSet<String> = BTreeSet::new();
    for bytes in input
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let path = path_from_bytes(bytes)?;
        if fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
            continue;
        }
        let config = resolver
            .for_path(&path)
            .map_err(|error| error.to_string())?;
        if !quiet {
            for warning in &config.warnings {
                if warned.insert(warning.clone()) {
                    eprintln!("{PROGRAM}: warning: {warning}");
                }
            }
        }
        let absolute = resolver.absolute(&path);
        let Some(language) = lang::for_path(&path) else {
            continue;
        };
        if !wanted.allows(&path)
            || !config.allows_path(&absolute)
            || !config.language_enabled(language.name, &absolute)
        {
            continue;
        }
        if config.generated == config::Generated::Skip && walk::generated_file(&path) {
            continue;
        }
        claimed.push(path);
    }
    for path in claimed {
        println!("{}", path.display());
    }
    Ok(Code::Clean)
}

fn uncovered(cli: &Cli, targets: &[PathBuf]) -> Result<Code, String> {
    let resolver = Resolver::new();
    let found = walk::uncover(targets, &resolver)?;
    if !cli.quiet {
        for warning in &found.warnings {
            eprintln!("{PROGRAM}: warning: {warning}");
        }
        if found.unreadable > 0 {
            eprintln!(
                "{PROGRAM}: {} {} could not be read",
                found.unreadable,
                text::plural(found.unreadable, "directory", "directories")
            );
        }
        let style = Style::for_stdout();
        println!();
        println!("  {}", style.bold("uncovered"));
        println!();
    }
    for (key, count) in &found.keys {
        println!("{count:>6}  {key}");
    }
    Ok(Code::Clean)
}

fn watch(
    cli: &Cli,
    targets: &[PathBuf],
    wanted: &Wanted,
    options: PurgeOptions,
) -> Result<Code, String> {
    if !cli.quiet {
        eprintln!("{PROGRAM}: watching {} target(s)", targets.len());
    }
    loop {
        execute(cli, targets, wanted, options)?;
        let before = snapshot(targets, wanted)?;
        loop {
            thread::sleep(Duration::from_millis(300));
            let after = snapshot(targets, wanted)?;
            if after != before {
                break;
            }
        }
    }
}

fn snapshot(targets: &[PathBuf], wanted: &Wanted) -> Result<Vec<(PathBuf, SystemTime)>, String> {
    let resolver = Resolver::new();
    let gathered = walk::gather(targets, wanted, &resolver)?;
    let mut found: Vec<(PathBuf, SystemTime)> = gathered
        .files
        .into_iter()
        .filter_map(|entry| {
            let modified = fs::metadata(&entry.path).ok()?.modified().ok()?;
            Some((entry.path, modified))
        })
        .collect();
    found.sort();
    Ok(found)
}

fn list_types() {
    let mut by_name: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();
    for language in lang::LANGUAGES {
        by_name
            .entry(language.name)
            .or_default()
            .extend(language.extensions.iter().copied());
    }
    for (name, mut extensions) in by_name {
        extensions.sort_unstable();
        extensions.dedup();
        println!("{name:<14} {}", extensions.join(" "));
    }
}

fn json(done: &[Done], notes: &[walk::Note], totals: Totals) {
    for entry in done {
        let mut object = json!({
            "type": "file",
            "path": entry.path.display().to_string(),
            "language": entry.language,
            "status": entry.status(),
        });
        if let Some(reason) = entry.skip {
            object["reason"] = json!(reason);
        }
        if entry.changed() {
            object["comments"] = json!(entry.comments);
            object["docs"] = json!(entry.docs);
            object["glyphs"] = json!(entry.glyphs);
            object["minus"] = json!(entry.minus);
            object["plus"] = json!(entry.plus);
            let changes: Vec<_> = entry
                .changes
                .iter()
                .map(|change| {
                    json!({
                        "start": change.range.start,
                        "end": change.range.end,
                        "text": change.text,
                    })
                })
                .collect();
            object["changes"] = json!(changes);
        }
        println!("{object}");
    }
    for note in notes {
        println!(
            "{}",
            json!({
                "type": "file",
                "path": note.path.display().to_string(),
                "status": "skipped",
                "reason": note.reason,
            })
        );
    }
    println!(
        "{}",
        json!({
            "type": "summary",
            "files": totals.files + notes.len(),
            "changed": totals.changed,
            "comments": totals.comments,
            "docs": totals.docs,
            "glyphs": totals.glyphs,
            "minus": totals.minus,
            "plus": totals.plus,
        })
    );
}

fn stats(changes: &[&Done]) {
    let mut by_extension: BTreeMap<String, Totals> = BTreeMap::new();
    for entry in changes {
        let key = entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .unwrap_or_else(|| "(none)".to_string());
        let totals = by_extension.entry(key).or_default();
        totals.files += 1;
        totals.changed += 1;
        totals.comments += entry.comments;
        totals.docs += entry.docs;
        totals.glyphs += entry.glyphs;
        totals.minus += entry.minus;
        totals.plus += entry.plus;
    }
    let mut rows: Vec<(String, Totals)> = by_extension.into_iter().collect();
    rows.sort_by(|left, right| {
        right
            .1
            .changed
            .cmp(&left.1.changed)
            .then_with(|| left.0.cmp(&right.0))
    });
    println!();
    println!(
        "  {:<14} {:>6} {:>9} {:>6} {:>7} {:>7} {:>7}",
        "extension", "files", "comments", "docs", "glyphs", "-lines", "+lines"
    );
    for (key, totals) in rows {
        println!(
            "  {:<14} {:>6} {:>9} {:>6} {:>7} {:>7} {:>7}",
            key,
            totals.changed,
            totals.comments,
            totals.docs,
            totals.glyphs,
            totals.minus,
            totals.plus
        );
    }
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

fn kept(done: &[Done], style: &Style) -> Vec<Row> {
    done.iter()
        .filter(|entry| entry.saved.any())
        .map(|entry| {
            let mut detail = sentence(&entry.saved);
            let markers = markers(&entry.saved);
            if !markers.is_empty() {
                detail.push_str("  ");
                detail.push_str(&markers.join(", "));
            }
            Row::detailed(path::shorten(&entry.path), style.dim(&detail))
        })
        .collect()
}

fn markers(saved: &Saved) -> Vec<&'static str> {
    let mut markers: Vec<&'static str> = Vec::new();
    for (_, marker) in &saved.markers {
        if !markers.contains(marker) {
            markers.push(marker);
        }
    }
    markers
}

fn wanted(types: &[String]) -> Result<Wanted, String> {
    if types.is_empty() {
        return Ok(Wanted::default());
    }
    let mut extensions = Vec::new();
    for token in types {
        let Some(found) = lang::extensions_for_token(token) else {
            return Err(format!(
                "unknown type: {token}\n{PROGRAM}: the types it reads are {}",
                lang::known()
            ));
        };
        extensions.extend(found.iter().map(|extension| extension.to_string()));
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
    if saved.important > 0 {
        parts.push(text::counted(
            saved.important,
            "important comment",
            "important comments",
        ));
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

fn apply(changes: &[&Done]) -> usize {
    let mut failures = 0;
    for entry in changes {
        let content = entry.content.as_deref().unwrap_or_default();
        if let Err(error) = commit(&entry.path, content) {
            eprintln!("{PROGRAM}: {}: {error}", entry.path.display());
            failures += 1;
        }
    }
    failures
}

fn commit_entry(entry: &Done) -> Result<(), String> {
    commit(&entry.path, entry.content.as_deref().unwrap_or_default())
        .map_err(|error| format!("{}: {error}", entry.path.display()))
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

fn read_stdin() -> Result<Vec<u8>, String> {
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("stdin: {error}"))?;
    Ok(input)
}

fn write_stdout(bytes: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout();
    stdout
        .write_all(bytes)
        .map_err(|error| format!("stdout: {error}"))?;
    stdout.flush().map_err(|error| format!("stdout: {error}"))
}

fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| "filename is not UTF-8".into())
    }
}

const CONTEXT: usize = 3;

struct Line {
    start: usize,
    end: usize,
}

fn split_lines(bytes: &[u8]) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut at = 0usize;
    while at < bytes.len() {
        match bytes[at] {
            b'\n' => {
                at += 1;
                lines.push(Line { start, end: at });
                start = at;
            }
            b'\r' => {
                at += if bytes.get(at + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                lines.push(Line { start, end: at });
                start = at;
            }
            _ => at += 1,
        }
    }
    if start < bytes.len() {
        lines.push(Line {
            start,
            end: bytes.len(),
        });
    }
    lines
}

fn line_of(lines: &[Line], at: usize) -> usize {
    if lines.is_empty() {
        return 0;
    }
    lines
        .binary_search_by(|line| {
            if line.end <= at {
                std::cmp::Ordering::Less
            } else if line.start > at {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .unwrap_or_else(|index| index.saturating_sub(1))
        .min(lines.len() - 1)
}

fn line_text(bytes: &[u8], line: &Line) -> String {
    let mut end = line.end;
    if end > line.start && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    if end > line.start && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[line.start..end]).into_owned()
}

#[derive(Clone, Copy)]
struct Group {
    first_change: usize,
    last_change: usize,
    first_line: usize,
    last_line: usize,
}

fn unified_diff(path: &Path, original: &[u8], changes: &[Change]) -> String {
    let lines = split_lines(original);
    if changes.is_empty() || lines.is_empty() {
        return String::new();
    }
    let mut groups: Vec<Group> = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        let first = line_of(&lines, change.range.start);
        let last = line_of(
            &lines,
            change.range.end.saturating_sub(1).max(change.range.start),
        );
        match groups.last_mut() {
            Some(group) if first <= group.last_line + 1 => {
                group.last_change = index;
                group.last_line = group.last_line.max(last);
            }
            _ => groups.push(Group {
                first_change: index,
                last_change: index,
                first_line: first,
                last_line: last,
            }),
        }
    }
    let mut hunks: Vec<Vec<Group>> = Vec::new();
    for group in groups {
        match hunks.last_mut() {
            Some(hunk)
                if group
                    .first_line
                    .saturating_sub(hunk.last().map_or(0, |last| last.last_line) + 1)
                    <= CONTEXT * 2 =>
            {
                hunk.push(group);
            }
            _ => hunks.push(vec![group]),
        }
    }

    let name = path.display().to_string();
    let name = name.trim_start_matches('/');
    let mut output = String::new();
    output.push_str(&format!("--- a/{name}\n+++ b/{name}\n"));
    let mut net: isize = 0;
    for hunk in hunks {
        let first_line = hunk.first().map_or(0, |group| group.first_line);
        let last_line = hunk.last().map_or(0, |group| group.last_line);
        let old_first = first_line.saturating_sub(CONTEXT);
        let old_last = (last_line + CONTEXT).min(lines.len().saturating_sub(1));
        let old_count = old_last - old_first + 1;
        let mut rendered = String::new();
        let mut removed = 0usize;
        let mut added = 0usize;
        let mut line = old_first;
        for group in &hunk {
            while line < group.first_line {
                rendered.push(' ');
                rendered.push_str(&line_text(original, &lines[line]));
                rendered.push('\n');
                line += 1;
            }
            let old_start = lines[group.first_line].start;
            let old_end = lines[group.last_line].end;
            let group_changes = &changes[group.first_change..=group.last_change];
            let new_bytes = splice(original, group_changes, old_start, old_end);
            for old_line in &lines[group.first_line..=group.last_line] {
                rendered.push('-');
                rendered.push_str(&line_text(original, old_line));
                rendered.push('\n');
            }
            let new_lines = split_lines(&new_bytes);
            for new_line in &new_lines {
                rendered.push('+');
                rendered.push_str(&line_text(&new_bytes, new_line));
                rendered.push('\n');
            }
            removed += group.last_line - group.first_line + 1;
            added += new_lines.len();
            line = group.last_line + 1;
        }
        while line <= old_last {
            rendered.push(' ');
            rendered.push_str(&line_text(original, &lines[line]));
            rendered.push('\n');
            line += 1;
        }
        let new_first = (old_first as isize + net).max(0) as usize;
        let new_count = old_count - removed + added;
        output.push_str(&format!(
            "@@ -{},{old_count} +{},{new_count} @@\n",
            old_first + 1,
            new_first + 1
        ));
        output.push_str(&rendered);
        net += added as isize - removed as isize;
    }
    output
}

fn splice(original: &[u8], changes: &[Change], start: usize, end: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(end - start);
    let mut at = start;
    for change in changes {
        if change.range.start < start || change.range.end > end {
            continue;
        }
        output.extend_from_slice(&original[at..change.range.start]);
        output.extend_from_slice(change.text.as_bytes());
        at = change.range.end;
    }
    output.extend_from_slice(&original[at..end]);
    output
}
