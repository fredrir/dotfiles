
mod apply;
mod dir;
mod plan;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use workstation::{Answer, Completions, Style};

use plan::{Deep, Plan, show};

const PROGRAM: &str = "flatten";

const ROWS: usize = 12;

const WIDTH: usize = 100;

const ARROW: &str = " -> ";

#[derive(Parser)]
#[command(
    version,
    about = "Lift a directory's contents up out of the directories holding them",
    long_about = "Lift a directory's contents up out of the directories holding them.

By default only redundant nesting is undone: while the directory holds \
exactly one entry and that entry is a directory, that directory is emptied \
into the target and removed. Nothing can be overwritten that way, so it runs \
without asking and without saying anything.

With --deep every entry underneath the directory comes up to the top and \
every directory underneath is removed. That prints the plan, asks about each \
name two entries want, and asks once more before it starts.

Symlinks are moved, never followed, so a link to a directory comes up as \
itself and a loop is not a hang.",
    after_long_help = "Examples:
  flatten documents        Undo the folder-inside-a-folder-of-the-same-name
  flatten -n -d pack       Show what a deep flatten would do, and stop
  flatten -d -y pack       Flatten the whole subtree without being asked"
)]
struct Cli {
    #[arg(
        value_name = "DIRECTORY",
        value_hint = ValueHint::DirPath,
        required_unless_present = "shell"
    )]
    directories: Vec<PathBuf>,

    #[arg(short = 'd', long)]
    deep: bool,

    #[arg(short = 'n', long = "dry-run")]
    dry_run: bool,

    #[arg(short, long)]
    yes: bool,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short, long)]
    all: bool,

    #[command(flatten)]
    completions: Completions,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    if cli.directories.is_empty() {
        return workstation::fail(PROGRAM, "missing directory");
    }
    let style = Style::for_stdout();
    // `a` at one collision answers the ones after it, including the ones in
    // the directories still to come: it was asked of this run, not of this
    // directory.
    let mut answered_all = false;
    let mut status = ExitCode::SUCCESS;
    for target in &cli.directories {
        match flatten(target, &cli, &style, &mut answered_all) {
            Ok(true) => {}
            Ok(false) => status = ExitCode::FAILURE,
            Err(message) => status = workstation::fail(PROGRAM, message),
        }
    }
    status
}

fn flatten(
    target: &Path,
    cli: &Cli,
    style: &Style,
    answered_all: &mut bool,
) -> Result<bool, String> {
    require_directory(target)?;
    if cli.deep
        && let Some(what) = protected(target)
    {
        return Err(format!("refusing to flatten {what}"));
    }
    let made = if cli.deep {
        plan::deep(target)
    } else {
        plan::collapse(target)
    };
    match made.map_err(|error| format!("{}: {error}", target.display()))? {
        Plan::Nothing => {
            if cli.verbose || cli.dry_run {
                println!("{PROGRAM}: nothing to flatten in {}", shorten(target));
            }
            Ok(true)
        }
        Plan::Collapse(collapse) => {
            let rows: Vec<(String, String)> = collapse
                .entries
                .iter()
                .map(|entry| (collapse.source(entry), show(entry)))
                .collect();
            if cli.dry_run {
                heading(target, cli, style);
                section("move", &rows, cli, style);
                println!();
                println!(
                    "  {}",
                    count(rows.len(), "moved", &[], collapse.chain.len())
                );
                return Ok(true);
            }
            let done = apply::collapse(target, &collapse, &mut narrator(cli, style))
                .map_err(|error| format!("{}: {error}", target.display()))?;
            report(&done, cli, style, false)
        }
        Plan::Deep(deep) => run_deep(target, deep, cli, style, answered_all),
    }
}

fn run_deep(
    target: &Path,
    mut plan: Deep,
    cli: &Cli,
    style: &Style,
    answered_all: &mut bool,
) -> Result<bool, String> {
    if plan.unreadable > 0 {
        let in_them = if plan.unreadable == 1 { "it" } else { "them" };
        eprintln!(
            "{PROGRAM}: {} {} could not be read, so what is in {in_them} stays",
            plan.unreadable,
            directories(plan.unreadable)
        );
    }

    heading(target, cli, style);
    let claimed: Vec<(String, String)> = plan
        .moves()
        .map(|spot| (plan.source(spot), show(plan.name(spot))))
        .collect();
    let asked: Vec<(String, String)> = plan
        .collisions
        .iter()
        .map(|spot| (plan.source(*spot), show(plan.name(*spot))))
        .collect();
    section("move", &claimed, cli, style);
    section(&style.red("overwrite"), &asked, cli, style);

    if cli.dry_run {
        refuse(&plan.refuse_shadowed());
        println!();
        println!("  {}", tally(&plan, asked.len()));
        return Ok(true);
    }

    if !plan.collisions.is_empty() && !cli.yes {
        println!();
        for spot in plan.collisions.clone() {
            if *answered_all {
                plan.accept(spot);
                continue;
            }
            let question = format!(
                "  replace {} with {}? [Y/n/a] ",
                plan.holder(spot),
                plan.source(spot)
            );
            match workstation::confirm_each(&question) {
                Some(Answer::Yes) => plan.accept(spot),
                Some(Answer::All) => {
                    *answered_all = true;
                    plan.accept(spot);
                }
                Some(Answer::No) => {}
                // The answers ran out; leave the prompt's line and stop.
                None => {
                    println!();
                    return Ok(false);
                }
            }
        }
    } else if cli.yes {
        for spot in plan.collisions.clone() {
            plan.accept(spot);
        }
    }

    refuse(&plan.refuse_shadowed());
    println!();
    println!("  {}", tally(&plan, 0));

    if !cli.yes {
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
    println!();
    let done = apply::deep(target, &plan, &mut narrator(cli, style))
        .map_err(|error| format!("{}: {error}", target.display()))?;
    report(&done, cli, style, true)
}

fn heading(target: &Path, cli: &Cli, style: &Style) {
    println!();
    let mut line = format!(
        "  {}  {}",
        style.bold(PROGRAM),
        style.teal(&shorten(target))
    );
    if cli.deep {
        line += &format!("  {}", style.dim("deep"));
    }
    println!("{line}");
}

fn section(header: &str, rows: &[(String, String)], cli: &Cli, style: &Style) {
    if rows.is_empty() {
        return;
    }
    let limit = if cli.all { usize::MAX } else { ROWS };
    let shown = rows.len().min(limit);
    let destination = rows[..shown]
        .iter()
        .map(|(_, to)| to.chars().count())
        .max()
        .unwrap_or(0);
    // A path too long for the line is shown from its tail, which is the end
    // that says which file this is.
    let room = width()
        .saturating_sub(4 + ARROW.len() + destination)
        .max(16);
    let source = rows[..shown]
        .iter()
        .map(|(from, _)| from.chars().count())
        .max()
        .unwrap_or(0)
        .min(room);

    println!();
    println!("  {}", style.bold(header));
    for (from, to) in &rows[..shown] {
        let from = clip(from, source);
        let padding = " ".repeat(source.saturating_sub(from.chars().count()));
        println!("    {from}{padding}{}{to}", style.dim(ARROW));
    }
    if rows.len() > shown {
        let more = format!("… and {} more", rows.len() - shown);
        println!("    {}", style.dim(&more));
    }
}

fn tally(plan: &Deep, asking: usize) -> String {
    let moved = plan.moves().count();
    let below: usize = plan.dirs[1..]
        .iter()
        .map(|node| node.leaves.len())
        .sum::<usize>();
    let staying = below - moved - asking;
    let mut left = Vec::new();
    if asking > 0 {
        left.push(format!("{asking} to ask about"));
    }
    if staying > 0 {
        let where_it_is = if staying == 1 { "it is" } else { "they are" };
        left.push(format!("{staying} left where {where_it_is}"));
    }
    let removable = plan.removable();
    count(
        moved,
        "moved",
        &left,
        removable.iter().filter(|gone| **gone).count(),
    )
}

fn count(moved: usize, verb: &str, extra: &[String], removed: usize) -> String {
    let mut parts = vec![format!("{moved} {} {verb}", entries(moved))];
    parts.extend(extra.iter().cloned());
    parts.push(format!("{removed} {} removed", directories(removed)));
    parts.join(", ")
}

fn refuse(refusals: &[plan::Refusal]) {
    for refusal in refusals {
        eprintln!(
            "{PROGRAM}: {} stays where it is: {}",
            refusal.source, refusal.reason
        );
    }
}

fn narrator<'a>(cli: &'a Cli, style: &'a Style) -> impl FnMut(&str, &str) + use<'a> {
    move |from: &str, to: &str| {
        if cli.verbose {
            println!("  {from}{}{to}", style.dim(ARROW));
        }
    }
}

fn report(done: &apply::Done, cli: &Cli, style: &Style, spoke: bool) -> Result<bool, String> {
    for failure in &done.failures {
        eprintln!("{PROGRAM}: {}: {}", failure.path, failure.error);
    }
    if cli.verbose {
        println!("  {}", count(done.moved, "moved", &[], done.removed));
    } else if spoke {
        println!("  {}", style.dim("done"));
    }
    Ok(done.failures.is_empty())
}

fn require_directory(directory: &Path) -> Result<(), String> {
    match fs::metadata(directory) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!("not a directory: {}", directory.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(format!(
            "no such file or directory: {}",
            directory.display()
        )),
        Err(error) => Err(format!("{}: {error}", directory.display())),
    }
}

fn protected(target: &Path) -> Option<&'static str> {
    let real = fs::canonicalize(target).ok()?;
    if real.parent().is_none() {
        return Some("the filesystem root");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| fs::canonicalize(home).ok());
    (home.as_deref() == Some(real.as_path())).then_some("your home directory")
}

fn width() -> usize {
    // A terminal too narrow to hold a row is not worth laying out for.
    workstation::terminal_width()
        .filter(|width| *width >= 40)
        .unwrap_or(WIDTH)
}

fn shorten(path: &Path) -> String {
    let shown = path.display().to_string();
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return shown;
    };
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => shown,
    }
}

fn clip(text: &str, room: usize) -> String {
    let length = text.chars().count();
    if length <= room {
        return text.to_string();
    }
    let tail: String = text.chars().skip(length - room.saturating_sub(1)).collect();
    format!("…{tail}")
}

fn entries(count: usize) -> &'static str {
    if count == 1 { "entry" } else { "entries" }
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
