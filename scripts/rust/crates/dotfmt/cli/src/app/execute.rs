use std::fs;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::language::Language;
use rayon::prelude::*;

use super::plan::{BufferJob, Job, Work};
use super::{
    BufferChange, BufferOutcome, Change, FileOutcome, Outcome, Plan, Repairs, Session, attach_path,
    with_stack,
};

impl Session {
    pub fn execute(&self, plan: Plan) -> Result<Outcome, Diagnostic> {
        let Plan { work, parallel } = plan;
        let Work::Format {
            files,
            buffer,
            unreadable,
            check,
        } = work
        else {
            let Work::Owns(paths) = work else {
                unreachable!()
            };
            return Ok(Outcome::Owned(paths));
        };
        let needs_stack = !files.is_empty()
            || buffer
                .as_ref()
                .is_some_and(|buffer| buffer.language == Some(Language::Lua));
        let execute = || {
            let buffer = buffer.map(format_buffer);
            let files = if parallel {
                files
                    .par_iter()
                    .map(|job| format_file(job, check))
                    .collect()
            } else {
                files.iter().map(|job| format_file(job, check)).collect()
            };
            Outcome::Formatted {
                files,
                buffer,
                unreadable,
                check,
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

fn format_file(job: &Job, check: bool) -> FileOutcome {
    let result = (|| {
        let input = fs::read(&job.absolute)
            .map_err(|error| Diagnostic::new(DiagnosticKind::Io, error.to_string()))?;
        let formatted = job.engine.format(&job.path, &input, false, job.markdown)?;
        let changed = formatted.text.as_bytes() != input;
        if changed && !check {
            dotfmt_core::file::replace(&job.absolute, formatted.text.as_bytes())
                .map_err(|error| Diagnostic::new(DiagnosticKind::Io, error.to_string()))?;
        }
        Ok(Change {
            changed,
            repairs: formatted.repairs,
        })
    })()
    .map_err(|error| attach_path(error, &job.path));
    FileOutcome {
        path: job.path.clone(),
        language: job.language,
        result,
    }
}

fn format_buffer(job: BufferJob) -> BufferOutcome {
    let input = job.input;
    let result = match job.engine {
        Some(engine) => engine
            .format(&input.path, &input.input, input.editor, job.markdown)
            .map(|formatted| {
                let output = formatted.text.into_bytes();
                BufferChange {
                    change: Change {
                        changed: output != input.input,
                        repairs: formatted.repairs,
                    },
                    output,
                }
            }),
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
