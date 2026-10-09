use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use dotfmt::app::{Buffer, Operation, Options, Outcome, Request, Session};
use dotfmt::render::{self, Mode};
use dotfmt_core::config::Resolver;
use workstation::Style;

use crate::args::Cli;

pub fn run(mut cli: Cli) -> Result<ExitCode, String> {
    cli.languages.sort();
    cli.languages.dedup();
    if cli.add || cli.sync {
        return crate::placement::run(&cli);
    }
    if cli.editor {
        cli.quiet |= !cli.verbose;
        if cli.targets.len() > 1 {
            return Err("--editor accepts one filename for standard input".into());
        }
        if cli.targets.first().is_some_and(|path| path != Path::new("-")) {
            cli.stdin = cli.targets.pop();
        }
    }
    let streamed = cli.stdin.is_some()
        || cli.editor
        || cli.targets.iter().any(|path| path == Path::new("-"))
        || (cli.targets.is_empty() && !cli.check && !io::stdin().is_terminal());
    let operation = if cli.owns {
        Operation::Owns {
            paths: read_input()?
                .split(|byte| *byte == 0)
                .filter(|name| !name.is_empty())
                .map(path_from_bytes)
                .collect::<Result<_, _>>()?,
        }
    } else {
        let buffer = if streamed {
            if cli
                .targets
                .iter()
                .filter(|path| *path == Path::new("-"))
                .count()
                > 1
            {
                return Err("standard input may only be read once".into());
            }
            let path = cli
                .stdin
                .clone()
                .or_else(|| {
                    (cli.languages.len() == 1)
                        .then(|| cli.languages[0].default_stdin().to_path_buf())
                })
                .ok_or("standard input needs --editor FILENAME, --stdin FILENAME or one --lang LANGUAGE")?;
            cli.targets.retain(|path| path != Path::new("-"));
            Some(Buffer {
                path,
                input: read_input()?,
                editor: cli.editor,
            })
        } else {
            None
        };
        if cli.targets.is_empty() && !streamed {
            if cli.check {
                cli.targets.push(PathBuf::from("."));
            } else {
                workstation::cli::command::<Cli>()
                    .print_help()
                    .map_err(|error| error.to_string())?;
                println!();
                return Ok(ExitCode::SUCCESS);
            }
        }
        Operation::Format {
            targets: cli.targets.clone(),
            buffer,
            check: cli.check,
        }
    };
    let options = Options {
        workers: std::env::var("RAYON_NUM_THREADS")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|workers| *workers > 0),
    };
    let editor_input = match &operation {
        Operation::Format {
            buffer: Some(buffer),
            ..
        } if cli.editor && !cli.verbose && !cli.check => Some(buffer.input.clone()),
        _ => None,
    };
    let mut session = Session::new(Resolver::new(), options);
    let outcome = session.run(Request {
        operation,
        languages: cli.languages.clone(),
        dialect: cli.dialect.clone(),
    });
    if let Some(input) = editor_input
        && outcome.as_ref().map_or(true, Outcome::failed)
    {
        io::stdout()
            .write_all(&input)
            .map_err(|error| format!("stdout: {error}"))?;
        return Ok(ExitCode::SUCCESS);
    }
    let outcome = outcome.map_err(|errors| {
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\ndotfmt: ")
    })?;
    let failed = outcome.failed();
    match outcome {
        Outcome::Owned(paths) => {
            let mut output = Vec::new();
            for path in paths {
                append_path(&mut output, &path)?;
                output.push(0);
            }
            io::stdout()
                .write_all(&output)
                .map_err(|error| format!("stdout: {error}"))?;
        }
        Outcome::Formatted {
            files,
            buffer,
            unreadable,
            check,
        } => {
            let mut diagnostics = Vec::new();
            if let Some(buffer) = buffer {
                match buffer.result {
                    Ok(formatted) => {
                        if !cli.quiet && !formatted.change.repairs.is_empty() {
                            eprintln!(
                                "dotfmt: {}: {} {}",
                                buffer.path.display(),
                                if check { "would fix" } else { "fixed" },
                                render::repairs(&formatted.change.repairs)
                            );
                        }
                        if !check {
                            io::stdout()
                                .write_all(&formatted.output)
                                .map_err(|error| format!("stdout: {error}"))?;
                        }
                    }
                    Err(error) => diagnostics.push(error.to_string()),
                }
            }
            diagnostics.extend(
                files
                    .iter()
                    .filter_map(|file| file.result.as_ref().err().map(ToString::to_string)),
            );
            diagnostics.sort();
            diagnostics.dedup();
            for diagnostic in diagnostics {
                eprintln!("dotfmt: {diagnostic}");
            }
            if unreadable > 0 {
                eprintln!("dotfmt: {unreadable} unreadable directories");
            }
            if !cli.targets.is_empty() && !cli.quiet && (!streamed || cli.verbose) {
                let rows = render::rows(&files, check, cli.verbose);
                let style = Style::for_stderr();
                let mode = if check { Mode::Check } else { Mode::Write };
                if cli.verbose {
                    for line in render::heading(
                        "dotfmt",
                        &cli.targets[0],
                        if check { "check" } else { "" },
                        &style,
                    ) {
                        eprintln!("{line}");
                    }
                    for line in render::report(&rows, mode, &style) {
                        eprintln!("{line}");
                    }
                    eprintln!("\n  {}", render::tally(&rows, mode));
                } else {
                    for line in render::summary(&rows, mode, &style) {
                        eprintln!("{line}");
                    }
                }
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn read_input() -> Result<Vec<u8>, String> {
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("stdin: {error}"))?;
    Ok(input)
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

fn append_path(output: &mut Vec<u8>, path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        output.extend_from_slice(path.as_os_str().as_bytes());
        Ok(())
    }
    #[cfg(not(unix))]
    {
        output.extend_from_slice(path.to_str().ok_or("filename is not UTF-8")?.as_bytes());
        Ok(())
    }
}
