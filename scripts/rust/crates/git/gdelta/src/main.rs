#![forbid(unsafe_code)]

mod sections;

use std::ffi::OsString;
use std::io::{self, BufRead, ErrorKind, Write};
use std::mem;
use std::process::{Child, ChildStdin, Command, ExitCode, ExitStatus, Stdio};

use clap::Parser;
use workstation::{Completable, Completions};

use sections::{Layout, Section, Sections, Status};

const PROGRAM: &str = "gdelta";

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
        if closed(runs.write(&section))? {
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

// Neighbouring files with the same layout and status share one delta process.
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

    fn write(&mut self, section: &Section) -> io::Result<()> {
        let run = match self.current.take() {
            Some(run) if run.fits(section) => self.current.insert(run),
            None if section.layout.is_none() && section.status.is_none() => {
                self.unplaced.extend_from_slice(&section.text);
                return Ok(());
            }
            previous => {
                self.current = previous;
                let layout = section.layout.unwrap_or(Layout::Unified);
                self.start(layout, section.status)?
            }
        };
        run.stdin.write_all(&section.text)
    }

    fn start(&mut self, layout: Layout, status: Option<Status>) -> io::Result<&mut Run> {
        if let Some(run) = self.current.take() {
            self.settle(run)?;
        }
        let run = self.current.insert(Run::spawn(self.delta, layout, status)?);
        run.stdin.write_all(&mem::take(&mut self.unplaced))?;
        Ok(run)
    }

    fn finish(mut self) -> io::Result<ExitStatus> {
        if self.current.is_none() {
            closed(self.start(Layout::Unified, None).map(|_| ()))?;
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
    status: Option<Status>,
    child: Child,
    stdin: ChildStdin,
}

impl Run {
    fn spawn(delta: &[OsString], layout: Layout, status: Option<Status>) -> io::Result<Run> {
        let mut child = Command::new("delta")
            .args(delta)
            .arg(features(layout, status))
            .stdin(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        Ok(Run {
            layout,
            status,
            child,
            stdin,
        })
    }

    fn fits(&self, section: &Section) -> bool {
        section.layout.is_none_or(|layout| layout == self.layout)
            && section
                .status
                .is_none_or(|status| Some(status) == self.status)
    }

    fn wait(self) -> io::Result<ExitStatus> {
        let Run {
            mut child, stdin, ..
        } = self;
        drop(stdin);
        child.wait()
    }
}

// The gitconfig styles each layout and status in a `[delta "<feature>"]` section.
fn features(layout: Layout, status: Option<Status>) -> String {
    let layout = match layout {
        Layout::Unified => "unified",
        Layout::Split => "split",
    };
    let status = match status {
        Some(Status::Added) => " added",
        Some(Status::Modified) => " modified",
        Some(Status::Deleted) => " deleted",
        None => "",
    };
    format!("--features={layout}{status}")
}
