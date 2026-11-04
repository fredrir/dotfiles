use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use dotfmt::app::{Buffer, Operation, Options, Outcome, Request, Selection, Session};
use dotfmt::render::{self, Mode};
use dotfmt_core::config::Resolver;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use workstation::Style;

use crate::args::Cli;

pub fn run(mut cli: Cli) -> Result<ExitCode, String> {
    cli.languages.sort();
    cli.languages.dedup();
    if cli.add || cli.sync {
        return crate::placement::run(&cli);
    }
    if let Some(path) = &cli.explain {
        return explain(&cli, path);
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
        clean: cli.clean,
    });
    if let Some(input) = editor_input
        && outcome.as_ref().map_or(true, Outcome::failed)
    {
        io::stdout()
            .write_all(&input)
            .map_err(|error| format!("stdout: {error}"))?;
        let unexpected = unexpected(&outcome);
        for error in &unexpected {
            eprintln!("dotfmt: {error}");
        }
        return Ok(if unexpected.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    let outcome = outcome.map_err(joined)?;
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
                        let repairs = &formatted.change.repairs;
                        let verb = if check { "would fix" } else { "fixed" };
                        if cli.verbose {
                            for fix in render::fixes(repairs, verb) {
                                eprintln!("dotfmt: {}:{fix}", buffer.path.display());
                            }
                        } else if !cli.quiet && !repairs.is_empty() {
                            eprintln!(
                                "dotfmt: {}: {verb} {}",
                                buffer.path.display(),
                                render::repairs(repairs)
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

fn explain(cli: &Cli, path: &Path) -> Result<ExitCode, String> {
    let explanation = Session::new(Resolver::new(), Options::default())
        .explain(path, cli.languages.clone(), cli.dialect.clone())
        .map_err(joined)?;
    let mut rows = Vec::new();
    match explanation.selection {
        Selection::Language(language) => rows.push(("language", language.name().to_owned())),
        Selection::Disabled(language) => {
            rows.push(("language", language.name().to_owned()));
            rows.push(("skipped", "disabled".to_owned()));
        }
        Selection::Excluded => rows.push(("skipped", "excluded".to_owned())),
        Selection::Unmapped => rows.push(("skipped", "unmapped".to_owned())),
    }
    if let Some(dialect) = explanation.dialect {
        rows.push(("dialect", dialect.to_owned()));
    }
    for source in &explanation.sources {
        rows.push(("config", source.display().to_string()));
    }
    let output: String = rows
        .iter()
        .map(|(key, value)| format!("{key:<9}{value}\n"))
        .collect();
    io::stdout()
        .write_all(output.as_bytes())
        .map_err(|error| format!("stdout: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

/// Editor mode keeps syntax errors quiet; anything else is dotfmt's own failure.
fn unexpected(outcome: &Result<Outcome, Vec<Diagnostic>>) -> Vec<&Diagnostic> {
    let errors: Vec<&Diagnostic> = match outcome {
        Err(errors) => errors.iter().collect(),
        Ok(Outcome::Formatted { buffer, .. }) => buffer
            .iter()
            .filter_map(|buffer| buffer.result.as_ref().err())
            .collect(),
        Ok(Outcome::Owned(_)) => Vec::new(),
    };
    errors
        .into_iter()
        .filter(|error| error.kind != DiagnosticKind::Syntax)
        .collect()
}

fn joined(errors: Vec<Diagnostic>) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\ndotfmt: ")
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
