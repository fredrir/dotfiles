#![forbid(unsafe_code)]

mod sections;

use std::ffi::OsString;
use std::io::{self, BufRead, ErrorKind, Write};
use std::mem;
use std::process::{Child, ChildStdin, Command, ExitCode, ExitStatus, Stdio};

use clap::Parser;
use workstation::{Completable, Completions};

use sections::{Layout, Sections};

const PROGRAM: &str = "gdelta";
const SPLIT_FEATURE: &str = "--features=split";

#[derive(Parser)]
#[command(
    version,
    about = "Render a diff with delta, side by side only for files changed on both sides"
)]
struct Cli {
    #[arg(
        value_name = "DELTA_ARGS",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    delta: Vec<OsString>,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run(PROGRAM, |cli: Cli| render(&cli.delta))
}

fn render(delta: &[OsString]) -> Result<ExitCode, String> {
    let status = stream(delta, io::stdin().lock()).map_err(|error| error.to_string())?;
    Ok(workstation::exit_code(status.code().unwrap_or(1)))
}

fn stream(delta: &[OsString], input: impl BufRead) -> io::Result<ExitStatus> {
    let mut runs = Runs::new(delta);
    for section in Sections::new(input) {
        let section = section?;
        if closed(runs.write(section.layout, &section.text))? {
            break;
        }
    }
    runs.finish()
}

// delta stops reading once the viewer closes its output.
fn closed(written: io::Result<()>) -> io::Result<bool> {
    match written {
        Ok(()) => Ok(false),
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(true),
        Err(error) => Err(error),
    }
}

// Neighbouring files with the same layout share one delta process.
struct Runs<'a> {
    delta: &'a [OsString],
    current: Option<Run>,
    unplaced: Vec<u8>,
    failed: Option<ExitStatus>,
}

impl<'a> Runs<'a> {
    fn new(delta: &'a [OsString]) -> Runs<'a> {
        Runs {
            delta,
            current: None,
            unplaced: Vec::new(),
            failed: None,
        }
    }

    fn write(&mut self, layout: Option<Layout>, text: &[u8]) -> io::Result<()> {
        let layout = match (layout, &self.current) {
            (Some(layout), _) => layout,
            (None, Some(run)) => run.layout,
            (None, None) => {
                self.unplaced.extend_from_slice(text);
                return Ok(());
            }
        };
        let run = match self.current.take() {
            Some(run) if run.layout == layout => self.current.insert(run),
            previous => {
                if let Some(run) = previous {
                    self.settle(run)?;
                }
                let run = self.current.insert(Run::spawn(self.delta, layout)?);
                run.stdin.write_all(&mem::take(&mut self.unplaced))?;
                run
            }
        };
        run.stdin.write_all(text)
    }

    fn finish(mut self) -> io::Result<ExitStatus> {
        if self.current.is_none() {
            closed(self.write(Some(Layout::Unified), &[]))?;
        }
        if let Some(run) = self.current.take() {
            self.settle(run)?;
        }
        Ok(self.failed.unwrap_or_default())
    }

    fn settle(&mut self, run: Run) -> io::Result<()> {
        let status = run.wait()?;
        if !status.success() && self.failed.is_none() {
            self.failed = Some(status);
        }
        Ok(())
    }
}

struct Run {
    layout: Layout,
    child: Child,
    stdin: ChildStdin,
}

impl Run {
    fn spawn(delta: &[OsString], layout: Layout) -> io::Result<Run> {
        let mut command = Command::new("delta");
        command.args(delta).stdin(Stdio::piped());
        if layout == Layout::Split {
            command.arg(SPLIT_FEATURE);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        Ok(Run {
            layout,
            child,
            stdin,
        })
    }

    fn wait(self) -> io::Result<ExitStatus> {
        let Run {
            mut child, stdin, ..
        } = self;
        drop(stdin);
        child.wait()
    }
}
