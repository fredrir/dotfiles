#![forbid(unsafe_code)]

mod annotate;
mod cat;
mod colors;
mod filter;
mod foreground;
mod highlight;

use std::io::{self, IsTerminal};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};

use clap::{Parser, ValueHint};
use workstation::{ColorMode, Completable, Completions};

use annotate::Annotator;
use cat::CatFlags;

const PROGRAM: &str = "hexcat";

#[derive(Parser)]
#[command(
    version,
    about = "cat with bat highlighting and color swatches",
    args_override_self = true
)]
struct Cli {
    #[arg(
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Files to print; stdin when empty or -"
    )]
    files: Vec<PathBuf>,

    #[command(flatten)]
    cat: CatFlags,

    #[arg(
        long = "color",
        value_name = "WHEN",
        default_value = "auto",
        help = "Control colored output"
    )]
    color: ColorMode,

    #[arg(long = "filter", help = "Only add swatches to the input")]
    filter: bool,

    #[arg(long = "pager", help = "Only add swatches, then page through less")]
    pager: bool,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run::<Cli>(PROGRAM, |cli| {
        let terminal = io::stdout().is_terminal();
        let colored = cli.color.enabled(terminal);
        if cli.filter || cli.pager {
            let annotator = colored.then(Annotator::default);
            return filter::run(&cli.files, annotator, cli.pager && terminal);
        }
        let letters = cli.cat.letters();
        if !colored {
            return Err(launch_error("cat", cat::cat(&letters, &cli.files).exec()));
        }
        match cli.cat.highlight() {
            Some(highlight) => highlight::run(highlight, &cli.files),
            None => swatched_cat(&letters, &cli.files),
        }
    })
}

fn swatched_cat(letters: &str, files: &[PathBuf]) -> Result<ExitCode, String> {
    let mut cat = cat::cat(letters, files)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| launch_error("cat", error))?;
    let output = cat.stdout.take().expect("cat stdout is piped");
    filter::through(output, Annotator::default())?;
    let status = cat.wait().map_err(|error| launch_error("cat", error))?;
    Ok(status
        .code()
        .map_or(ExitCode::FAILURE, workstation::exit_code))
}

fn launch_error(program: &str, error: io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => format!("{program} not found"),
        _ => format!("{program}: {}", filter::reason(&error)),
    }
}
