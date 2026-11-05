use std::fs;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::edit::{apply_fixes, select_fixes};
use dotfmt_core::language::Language;
use rayon::prelude::*;

use super::engine::Formatted;
use super::plan::{BufferJob, Job, Work};
use super::{
    BufferChange, BufferOutcome, Change, FileOutcome, FixMode, InspectBufferOutcome,
    InspectOutcome, Inspection, Outcome, Plan, Repairs, Session, attach_path, with_stack,
};

impl Session {
    pub fn execute(&self, plan: Plan) -> Result<Outcome, Diagnostic> {
        let Plan { work, parallel } = plan;
        let needs_stack = match &work {
            Work::Owns(_) => false,
            Work::Format { files, buffer, .. } | Work::Inspect { files, buffer, .. } => {
                !files.is_empty()
                    || buffer.as_ref().is_some_and(|buffer| {
                        matches!(
                            buffer.language,
                            Some(
                                Language::Bib
                                    | Language::Latex
                                    | Language::Lua
                                    | Language::Plist
                                    | Language::Toml
                                    | Language::Yaml
                                    | Language::TypeScript
                                    | Language::Javascript
                                    | Language::Css
                                    | Language::Html,
                            )
                        )
                    })
            }
        };
        let execute = move || match work {
            Work::Owns(paths) => Outcome::Owned(paths),
            Work::Format {
                files,
                buffer,
                unreadable,
                check,
                clean,
                fixes,
            } => {
                let buffer = buffer.map(|buffer| format_buffer(buffer, check, clean, fixes));
                let files = if parallel {
                    files
                        .par_iter()
                        .map(|job| format_file(job, check, clean, fixes))
                        .collect()
                } else {
                    files
                        .iter()
                        .map(|job| format_file(job, check, clean, fixes))
                        .collect()
                };
                Outcome::Formatted {
                    files,
                    buffer,
                    unreadable,
                    check,
                }
            }
            Work::Inspect {
                files,
                buffer,
                unreadable,
                fix,
                diff,
                json,
                type_aware,
                project_analysis,
            } => {
                let buffer =
                    buffer.map(|buffer| inspect_buffer(buffer, fix, type_aware, project_analysis));
                let files = if parallel {
                    files
                        .par_iter()
                        .map(|job| inspect_file(job, fix, diff, type_aware, project_analysis))
                        .collect()
                } else {
                    files
                        .iter()
                        .map(|job| inspect_file(job, fix, diff, type_aware, project_analysis))
                        .collect()
                };
                Outcome::Inspected {
                    files,
                    buffer,
                    unreadable,
                    fix,
                    diff,
                    json,
                    type_aware,
                }
            }
        };
        if parallel {
            Ok(self.pool()?.install(execute))
        } else if needs_stack {
            with_stack(execute)
        } else {
            Ok(execute())
        }
    }
}

fn format_file(job: &Job, check: bool, clean: bool, fixes: bool) -> FileOutcome {
    let result = (|| {
        let input = fs::read(&job.absolute)
            .map_err(|error| Diagnostic::new(DiagnosticKind::Io, error.to_string()))?;
        // `--check` and editor range/paste paths must report layout only.
        let formatted = if check || !fixes {
            job.engine
                .format_text(&job.path, &job.absolute, &input, job.markdown, clean)?
        } else {
            job.engine
                .format(&job.path, &job.absolute, &input, job.markdown, clean)?
        };
        let (changed, repairs) = match formatted {
            Formatted::Text { text, repairs } => {
                let changed = text.as_bytes() != input;
                if changed && !check {
                    dotfmt_core::file::replace(&job.absolute, text.as_bytes())
                        .map_err(|error| Diagnostic::new(DiagnosticKind::Io, error.to_string()))?;
                }
                (changed, repairs)
            }
            Formatted::Verbatim => (false, Repairs::default()),
        };
        Ok(Change { changed, repairs })
    })()
    .map_err(|error| attach_path(error, &job.path));
    FileOutcome {
        path: job.path.clone(),
        language: job.language,
        result,
    }
}

fn format_buffer(job: BufferJob, check: bool, clean: bool, fixes: bool) -> BufferOutcome {
    let input = job.input;
    let absolute = job.absolute;
    let result = match job.engine {
        Some(engine) => {
            let formatted = if check || !fixes {
                engine.format_text(&input.path, &absolute, &input.input, job.markdown, clean)
            } else {
                engine.format(&input.path, &absolute, &input.input, job.markdown, clean)
            };
            formatted.map(|formatted| match formatted {
                Formatted::Text { text, repairs } => {
                    let output = text.into_bytes();
                    BufferChange {
                        change: Change {
                            changed: output != input.input,
                            repairs,
                        },
                        output,
                    }
                }
                Formatted::Verbatim => BufferChange {
                    output: input.input.clone(),
                    change: Change {
                        changed: false,
                        repairs: Repairs::default(),
                    },
                },
            })
        }
        None => Ok(BufferChange {
            output: input.input,
            change: Change {
                changed: false,
                repairs: Repairs::default(),
            },
        }),
    }
    .map_err(|error| attach_path(error, &input.path));
    BufferOutcome {
        path: input.path,
        language: job.language,
        result,
    }
}

fn inspect_file(
    job: &Job,
    fix: FixMode,
    diff: bool,
    type_aware: Option<bool>,
    project_analysis: bool,
) -> InspectOutcome {
    let result = (|| {
        let input = fs::read(&job.absolute)
            .map_err(|error| Diagnostic::new(DiagnosticKind::Io, error.to_string()))?;
        let mut inspection = inspect(
            &job.engine,
            &job.path,
            &job.absolute,
            &input,
            job.markdown,
            fix,
            diff,
            type_aware,
            project_analysis,
        )?;
        if fix != FixMode::None && !diff && inspection.changed {
            dotfmt_core::file::replace(&job.absolute, inspection.after.as_bytes())
                .map_err(|error| Diagnostic::new(DiagnosticKind::Io, error.to_string()))?;
            inspection.changed = true;
        }
        Ok(inspection)
    })()
    .map_err(|error| attach_path(error, &job.path));
    InspectOutcome {
        path: job.path.clone(),
        language: job.language,
        result,
    }
}

fn inspect_buffer(
    job: BufferJob,
    fix: FixMode,
    type_aware: Option<bool>,
    project_analysis: bool,
) -> InspectBufferOutcome {
    let input = job.input;
    let result = match job.engine {
        Some(engine) => inspect(
            &engine,
            &input.path,
            &job.absolute,
            &input.input,
            job.markdown,
            fix,
            false,
            type_aware,
            project_analysis,
        )
        .map_err(|error| attach_path(error, &input.path)),
        None => Ok(Inspection {
            findings: Vec::new(),
            fixes: Vec::new(),
            applied: Vec::new(),
            before: String::from_utf8_lossy(&input.input).into_owned(),
            after: String::from_utf8_lossy(&input.input).into_owned(),
            changed: false,
            diff: None,
        }),
    };
    InspectBufferOutcome {
        path: input.path,
        language: job.language,
        result,
    }
}

#[allow(clippy::too_many_arguments)]
fn inspect(
    engine: &super::engine::Engine,
    path: &std::path::Path,
    absolute: &std::path::Path,
    input: &[u8],
    markdown: dotfmt_markdown::dialect::Dialect,
    fix: FixMode,
    diff: bool,
    type_aware: Option<bool>,
    project_analysis: bool,
) -> Result<Inspection, Diagnostic> {
    let original = std::str::from_utf8(input)
        .map_err(|_| Diagnostic::new(DiagnosticKind::Syntax, "not UTF-8"))?
        .to_owned();
    let (bom, text) = match original.strip_prefix('\u{feff}') {
        Some(text) => (true, text),
        None => (false, original.as_str()),
    };
    let selection = engine.fix_selection(fix);
    let unsafe_fixes = selection
        .as_ref()
        .map_or(fix == FixMode::Unsafe, |selection| {
            fix == FixMode::Unsafe && selection.r#unsafe
        });
    let organize = selection
        .as_ref()
        .is_none_or(|selection| selection.organize_imports);
    let analysis = engine.analyze(
        absolute,
        text.as_bytes(),
        fix != FixMode::None,
        unsafe_fixes,
        type_aware,
        organize,
        project_analysis,
    )?;
    let mut applied = Vec::new();
    let mut after = original.clone();
    if fix != FixMode::None {
        let mut current = text.to_owned();
        for _ in 0..3 {
            let pass = engine.analyze(
                absolute,
                current.as_bytes(),
                true,
                unsafe_fixes,
                type_aware,
                organize,
                project_analysis,
            )?;
            let selected = match &selection {
                Some(selection) => select_fixes(fix, selection, &pass.fixes),
                None => Vec::new(),
            };
            if selected.is_empty() {
                break;
            }
            let edited = apply_fixes(&current, &selected).map_err(|error| {
                Diagnostic::new(DiagnosticKind::Internal, format!("fix edits: {error}"))
            })?;
            let with_bom = if bom {
                format!("\u{feff}{edited}")
            } else {
                edited
            };
            let formatted =
                match engine.format_text(path, absolute, with_bom.as_bytes(), markdown, false)? {
                    Formatted::Text { text, .. } => text,
                    Formatted::Verbatim => with_bom,
                };
            let changed = formatted.strip_prefix('\u{feff}').unwrap_or(&formatted) != current;
            applied.extend(selected.iter().map(|fix| fix.title.clone()));
            current = formatted
                .strip_prefix('\u{feff}')
                .unwrap_or(&formatted)
                .to_owned();
            if !changed {
                break;
            }
        }
        let with_bom = if bom {
            format!("\u{feff}{current}")
        } else {
            current
        };
        after = match engine.format_text(path, absolute, with_bom.as_bytes(), markdown, false)? {
            Formatted::Text { text, .. } => text,
            Formatted::Verbatim => with_bom,
        };
    }
    let changed = after != original;
    let diff = diff.then(|| unified_diff(&path.display().to_string(), &original, &after));
    Ok(Inspection {
        findings: analysis.findings,
        fixes: analysis.fixes,
        applied,
        before: original,
        after,
        changed,
        diff,
    })
}

fn unified_diff(path: &str, before: &str, after: &str) -> String {
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(path, path)
        .to_string()
}
