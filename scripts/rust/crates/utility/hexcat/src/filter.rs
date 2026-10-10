use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};

use workstation::fail;

use crate::PROGRAM;
use crate::annotate::Annotator;

pub const STDIN: &str = "-";
pub const CAPACITY: usize = 64 * 1024;

enum Failure {
    Read(io::Error),
    Write(io::Error),
}

pub fn run(
    files: &[PathBuf],
    mut annotator: Option<Annotator>,
    page: bool,
) -> Result<ExitCode, String> {
    let mut pager = if page { less() } else { None };
    let status = match pager.as_mut().and_then(|less| less.stdin.take()) {
        Some(input) => print_all(files, &mut annotator, input),
        None => print_all(files, &mut annotator, io::stdout().lock()),
    };
    if let Some(mut less) = pager {
        less.wait().map_err(|error| format!("less: {error}"))?;
    }
    status
}

pub fn through(input: impl Read, annotator: Annotator) -> Result<(), String> {
    let mut output = BufWriter::with_capacity(CAPACITY, io::stdout().lock());
    let input = BufReader::with_capacity(CAPACITY, input);
    let printed = print(input, &mut Some(annotator), &mut output)
        .and_then(|()| output.flush().map_err(Failure::Write));
    match printed {
        Err(Failure::Read(error)) => Err(reason(&error)),
        Err(Failure::Write(error)) if error.kind() != io::ErrorKind::BrokenPipe => {
            Err(reason(&error))
        }
        _ => Ok(()),
    }
}

// The flags bat passes to less when it pages by itself.
fn less() -> Option<Child> {
    Command::new("less")
        .args(["-R", "-F", "-K"])
        .env("LESSCHARSET", "UTF-8")
        .stdin(Stdio::piped())
        .spawn()
        .ok()
}

fn print_all(
    files: &[PathBuf],
    annotator: &mut Option<Annotator>,
    output: impl Write,
) -> Result<ExitCode, String> {
    let mut output = BufWriter::with_capacity(CAPACITY, output);
    let stdin = [PathBuf::from(STDIN)];
    let files = if files.is_empty() { &stdin[..] } else { files };
    let mut status = ExitCode::SUCCESS;
    for file in files {
        let printed = open(file)
            .map_err(Failure::Read)
            .and_then(|input| print(input, annotator, &mut output));
        match printed {
            Ok(()) => {}
            Err(Failure::Read(error)) => {
                status = fail(PROGRAM, format!("{}: {}", file.display(), reason(&error)));
            }
            Err(Failure::Write(error)) if error.kind() == io::ErrorKind::BrokenPipe => {
                return Ok(status);
            }
            Err(Failure::Write(error)) => return Err(reason(&error)),
        }
    }
    match output.flush() {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(reason(&error)),
        _ => Ok(status),
    }
}

fn open(file: &Path) -> io::Result<BufReader<Box<dyn Read>>> {
    let source: Box<dyn Read> = if file.as_os_str() == STDIN {
        Box::new(io::stdin())
    } else {
        Box::new(File::open(file)?)
    };
    Ok(BufReader::with_capacity(CAPACITY, source))
}

// Flushes whenever the input runs dry, so interactive streams show up line by line.
fn print(
    mut input: BufReader<impl Read>,
    annotator: &mut Option<Annotator>,
    output: &mut impl Write,
) -> Result<(), Failure> {
    let mut line = Vec::new();
    while input.read_until(b'\n', &mut line).map_err(Failure::Read)? > 0 {
        match annotator {
            Some(annotator) => annotator.annotate(&line, output),
            None => output.write_all(&line),
        }
        .map_err(Failure::Write)?;
        if input.buffer().is_empty() {
            output.flush().map_err(Failure::Write)?;
        }
        line.clear();
    }
    Ok(())
}

pub fn reason(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "no such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        io::ErrorKind::IsADirectory => "is a directory".to_string(),
        _ => error.to_string(),
    }
}
