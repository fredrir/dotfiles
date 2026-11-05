use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use dotfmt::app::{
    Buffer, FixMode, Inspection, Operation, Options, Outcome, Request, Selection, Session,
};
use dotfmt::render::{self, Mode};
use dotfmt_core::config::Resolver;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::language::Language;
use workstation::Style;

use crate::args::Cli;

pub fn run(mut cli: Cli) -> Result<ExitCode, String> {
    cli.languages.sort();
    cli.languages.dedup();
    let mut languages: Vec<Language> = cli
        .languages
        .iter()
        .map(|selected| selected.language)
        .collect();
    languages.sort();
    languages.dedup();
    let dialect_hint = match cli.languages.as_slice() {
        [selected] => selected.dialect,
        _ => None,
    };
    if let Some(source) = &cli.migrate {
        return crate::migrate::run(&cli, source);
    }
    if cli.add || cli.sync {
        return crate::placement::run(&cli);
    }
    if let Some(path) = &cli.explain {
        return explain(&cli, path, &languages, dialect_hint);
    }
    let inspect = cli.fix || cli.fix_unsafe || cli.diagnose;
    if cli.diff && !cli.fix && !cli.fix_unsafe {
        return Err("--diff requires --fix or --fix-unsafe".into());
    }
    if cli.json && !inspect {
        return Err("--json requires --diagnose, --fix or --fix-unsafe".into());
    }
    if cli.editor {
        cli.quiet |= !cli.verbose;
        if cli.targets.len() > 1 {
            return Err("--editor accepts one filename for standard input".into());
        }
        if cli
            .targets
            .first()
            .is_some_and(|path| path != Path::new("-"))
        {
            cli.stdin = cli.targets.pop();
        }
    }
    let streamed = cli.stdin.is_some()
        || cli.editor
        || cli.targets.iter().any(|path| path == Path::new("-"))
        || (cli.targets.is_empty() && !cli.check && !inspect && !io::stdin().is_terminal());
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
                    (languages.len() == 1)
                        .then(|| languages[0].default_stdin().to_path_buf())
                })
                .ok_or("standard input needs --editor FILENAME, --stdin FILENAME or one --lang LANGUAGE")?;
            if cli.dialect.is_none() && (cli.stdin.is_none() || path.extension().is_none()) {
                cli.dialect = dialect_hint.map(str::to_owned);
            }
            cli.targets.retain(|path| path != Path::new("-"));
            Some(Buffer {
                path,
                input: read_input()?,
            })
        } else {
            None
        };
        if cli.targets.is_empty() && !streamed {
            if cli.check || inspect {
                cli.targets.push(PathBuf::from("."));
            } else {
                workstation::cli::command::<Cli>()
                    .print_help()
                    .map_err(|error| error.to_string())?;
                println!();
                return Ok(ExitCode::SUCCESS);
            }
        }
        if inspect {
            let fix = if cli.fix_unsafe {
                FixMode::Unsafe
            } else if cli.fix {
                FixMode::Safe
            } else {
                FixMode::None
            };
            Operation::Inspect {
                targets: cli.targets.clone(),
                buffer,
                fix,
                diff: cli.diff,
                json: cli.json,
                type_aware: None,
            }
        } else {
            Operation::Format {
                targets: cli.targets.clone(),
                buffer,
                check: cli.check,
            }
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
        languages: languages.clone(),
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
        Outcome::Inspected {
            files,
            buffer,
            unreadable,
            fix,
            diff,
            json,
            type_aware: _,
        } => {
            report_inspection(
                &files,
                buffer.as_ref(),
                unreadable,
                fix,
                diff,
                json,
                cli.quiet,
                cli.verbose,
            )?;
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

fn explain(
    cli: &Cli,
    path: &Path,
    languages: &[Language],
    dialect_hint: Option<&'static str>,
) -> Result<ExitCode, String> {
    let dialect = cli.dialect.clone().or_else(|| match dialect_hint {
        Some(hint) if path.extension().is_none() => Some(hint.to_owned()),
        _ => None,
    });
    let explanation = Session::new(Resolver::new(), Options::default())
        .explain(path, languages.to_vec(), dialect)
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
        Ok(Outcome::Inspected { buffer, .. }) => buffer
            .iter()
            .filter_map(|buffer| buffer.result.as_ref().err())
            .collect(),
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

#[allow(clippy::too_many_arguments)]
fn report_inspection(
    files: &[dotfmt::app::InspectOutcome],
    buffer: Option<&dotfmt::app::InspectBufferOutcome>,
    unreadable: usize,
    fix: FixMode,
    diff: bool,
    json: bool,
    quiet: bool,
    verbose: bool,
) -> Result<(), String> {
    if json {
        let value = inspection_json(files, buffer, fix);
        println!(
            "{}",
            serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    let mut errors = Vec::new();
    for file in files {
        match &file.result {
            Ok(inspection) => report_one(
                &file.path.display().to_string(),
                inspection,
                fix,
                diff,
                quiet,
                verbose,
                &mut errors,
            )?,
            Err(error) => errors.push(format!("{}: {error}", file.path.display())),
        }
    }
    if let Some(buffer) = buffer {
        match &buffer.result {
            Ok(inspection) => {
                report_one(
                    &buffer.path.display().to_string(),
                    inspection,
                    fix,
                    diff,
                    quiet,
                    verbose,
                    &mut errors,
                )?;
                if fix != FixMode::None && !diff {
                    io::stdout()
                        .write_all(inspection.after.as_bytes())
                        .map_err(|error| format!("stdout: {error}"))?;
                }
            }
            Err(error) => errors.push(format!("{}: {error}", buffer.path.display())),
        }
    }
    if unreadable > 0 {
        errors.push(format!("{unreadable} unreadable directories"));
    }
    errors.sort();
    errors.dedup();
    for error in &errors {
        eprintln!("dotfmt: {error}");
    }
    Ok(())
}

fn report_one(
    path: &str,
    inspection: &Inspection,
    fix: FixMode,
    diff: bool,
    quiet: bool,
    verbose: bool,
    _errors: &mut Vec<String>,
) -> Result<(), String> {
    let text = inspection
        .before
        .strip_prefix('\u{feff}')
        .unwrap_or(&inspection.before);
    let index = dotfmt_core::position::LineIndex::new(text);
    for finding in &inspection.findings {
        let position = index.position(text, finding.start);
        let severity = match finding.severity {
            dotfmt_core::edit::Severity::Error => "error",
            dotfmt_core::edit::Severity::Warning => "warning",
            dotfmt_core::edit::Severity::Information => "info",
            dotfmt_core::edit::Severity::Hint => "hint",
        };
        if !quiet {
            eprintln!(
                "dotfmt: {path}:{}:{}: {severity} [{}] {}",
                position.line + 1,
                position.character + 1,
                finding.code,
                finding.message
            );
        }
        if verbose {
            for related in &finding.related {
                let position = index.position(text, related.start);
                eprintln!(
                    "dotfmt: {path}:{}:{}: {}",
                    position.line + 1,
                    position.character + 1,
                    related.message
                );
            }
        }
    }
    if diff {
        if let Some(diff) = &inspection.diff {
            print!("{diff}");
        }
    } else if fix != FixMode::None && !quiet {
        let remaining = inspection.findings.len();
        if inspection.changed {
            eprintln!(
                "dotfmt: {path}: applied {} fix(es){}",
                inspection.applied.len(),
                if remaining == 0 {
                    String::new()
                } else {
                    format!("; {remaining} finding(s) need review")
                }
            );
        } else if remaining == 0 {
            eprintln!("dotfmt: {path}: clean");
        } else {
            eprintln!("dotfmt: {path}: {remaining} finding(s) need review");
        }
    }
    if fix == FixMode::None && inspection.findings.is_empty() && verbose && !quiet {
        eprintln!("dotfmt: {path}: clean");
    }
    if fix == FixMode::None && !inspection.fixes.is_empty() && !quiet {
        let safe = inspection
            .fixes
            .iter()
            .filter(|fix| fix.applicability == dotfmt_core::edit::Applicability::Safe)
            .count();
        let unsafe_fixes = inspection.fixes.len() - safe;
        eprintln!(
            "dotfmt: {path}: {safe} safe and {unsafe_fixes} unsafe fix(es) available; use --fix or --fix-unsafe"
        );
    }
    Ok(())
}

fn inspection_json(
    files: &[dotfmt::app::InspectOutcome],
    buffer: Option<&dotfmt::app::InspectBufferOutcome>,
    fix: FixMode,
) -> serde_json::Value {
    let mut entries = Vec::new();
    for file in files {
        entries.push(inspection_entry(
            &file.path.display().to_string(),
            file.language.name(),
            &file.result,
        ));
    }
    if let Some(buffer) = buffer {
        entries.push(inspection_entry(
            &buffer.path.display().to_string(),
            buffer
                .language
                .map_or("unmapped", |language| language.name()),
            &buffer.result,
        ));
    }
    serde_json::json!({
        "version": 1,
        "mode": match fix {
            FixMode::None => "diagnose",
            FixMode::Safe => "fix",
            FixMode::Unsafe => "fix-unsafe",
        },
        "files": entries,
    })
}

fn inspection_entry(
    path: &str,
    language: &str,
    result: &Result<Inspection, Diagnostic>,
) -> serde_json::Value {
    match result {
        Err(error) => serde_json::json!({
            "path": path,
            "language": language,
            "error": error.to_string(),
        }),
        Ok(inspection) => {
            let text = inspection
                .before
                .strip_prefix('\u{feff}')
                .unwrap_or(&inspection.before);
            let index = dotfmt_core::position::LineIndex::new(text);
            let range = |start: usize, end: usize| {
                let start = index.position(text, start);
                let end = index.position(text, end);
                serde_json::json!({
                    "start": {"line": start.line, "character": start.character},
                    "end": {"line": end.line, "character": end.character},
                })
            };
            let findings: Vec<serde_json::Value> = inspection
                .findings
                .iter()
                .map(|finding| {
                    serde_json::json!({
                        "code": finding.code,
                        "message": finding.message,
                        "severity": match finding.severity {
                            dotfmt_core::edit::Severity::Error => "error",
                            dotfmt_core::edit::Severity::Warning => "warning",
                            dotfmt_core::edit::Severity::Information => "info",
                            dotfmt_core::edit::Severity::Hint => "hint",
                        },
                        "range": range(finding.start, finding.end),
                        "tags": finding.tags,
                        "related": finding.related.iter().map(|related| serde_json::json!({
                            "range": range(related.start, related.end),
                            "message": related.message,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let fixes: Vec<serde_json::Value> = inspection
                .fixes
                .iter()
                .map(|fix| {
                    serde_json::json!({
                        "title": fix.title,
                        "kind": fix.kind,
                        "applicability": fix.applicability,
                        "diagnosticCode": fix.finding.as_ref().map(|finding| finding.code.clone()),
                        "edits": fix.edits.iter().map(|edit| serde_json::json!({
                            "range": range(edit.start, edit.end),
                            "newText": edit.new_text,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            serde_json::json!({
                "path": path,
                "language": language,
                "findings": findings,
                "fixes": fixes,
                "applied": inspection.applied,
                "changed": inspection.changed,
            })
        }
    }
}
