use std::path::Path;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use tex_fmt::args::TabChar;

use crate::config::{Config, IndentType, LineEndings};
use crate::dialect::Dialect;
use crate::protect;

mod wrap;

/// Format a TeX-family source. The input is UTF-8; a BOM is preserved.
pub fn format(input: &str, config: &Config, dialect: Dialect) -> Result<String, Diagnostic> {
    let (bom, input) = match input.strip_prefix('\u{feff}') {
        Some(input) => (true, input),
        None => (false, input),
    };
    let text = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = if text.trim().is_empty() {
        String::new()
    } else {
        format_once(&text, config, dialect)?
    };
    if config.verify {
        let again = format_once(&output, config, dialect)?;
        if again != output {
            return Err(internal("formatting does not settle after one pass"));
        }
    }
    if config.final_newline && !output.is_empty() {
        output.push('\n');
    }
    if config.line_endings == LineEndings::Windows {
        output = output.replace('\n', "\r\n");
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(output)
}

fn format_once(text: &str, config: &Config, dialect: Dialect) -> Result<String, Diagnostic> {
    let defaults = dialect.defaults();
    let mut verbatims = merge(defaults.verbatims, &config.verbatims);
    let mut lists = merge(defaults.lists, &config.lists);
    let mut no_indent_envs = merge(defaults.no_indent_envs, &config.no_indent_envs);
    let prepared = protect::prepare(text, dialect, &verbatims, &lists)?;

    if dialect == Dialect::Context {
        for (placeholder, start, _) in prepared.context() {
            let name = start.trim_start_matches("\\start");
            if lists.iter().any(|candidate| candidate == name) {
                lists.push(placeholder.clone());
            }
            if no_indent_envs.iter().any(|candidate| candidate == name) {
                no_indent_envs.push(placeholder.clone());
            }
        }
        lists.sort();
        lists.dedup();
        no_indent_envs.sort();
        no_indent_envs.dedup();
    } else {
        verbatims.sort();
        verbatims.dedup();
    }

    let args = tex_fmt::args::Args {
        check: false,
        print: false,
        fail_on_change: false,
        // Wrapping and indentation width are handled after tex-fmt, with a
        // one-character tab size so its i8 indent arithmetic cannot overflow.
        wrap: false,
        wraplen: config.width,
        wrapmin: config.width,
        tabsize: 1,
        tabchar: TabChar::Space,
        stdin: false,
        config: None,
        lists,
        verbatims,
        no_indent_envs,
        wrap_chars: vec![' '],
        verbosity: "error".parse().expect("static log level"),
        arguments: false,
        files: Vec::new(),
        recursive: false,
        format_tables: config.format_tables,
    };
    let mut current = prepared.text.clone();
    let mut formatted = String::new();
    let mut converged = false;
    for _ in 0..6 {
        let pass = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut logs = Vec::new();
            tex_fmt::format::format_file(&current, Path::new("<dotfmt>"), &args, &mut logs)
        }))
        .map_err(|_| internal("LaTeX formatter failed"))?;
        let pass = reindent(&pass, config);
        let pass = if config.wrap {
            wrap::wrap_text(&pass, config)
        } else {
            pass
        };
        let denormalized = denormalize(&pass, config);
        formatted = pass;
        if denormalized == current {
            converged = true;
            break;
        }
        current = denormalized;
    }
    if !converged {
        return Err(internal("formatting does not settle after six passes"));
    }
    let formatted = blank_lines(&formatted, &prepared.marker, config);
    let mut output = prepared.restore(&formatted);
    output.truncate(output.trim_end_matches('\n').len());
    Ok(output)
}

fn merge(defaults: &[&str], extra: &[String]) -> Vec<String> {
    defaults
        .iter()
        .map(|name| (*name).to_owned())
        .chain(extra.iter().cloned())
        .collect()
}

/// tex-fmt is given a tab size of one, so leading spaces are indentation
/// levels. Convert them to the configured width and character.
fn reindent(text: &str, config: &Config) -> String {
    let mut output = String::with_capacity(text.len());
    let mut ignore = false;
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let tail = line.trim_end();
        let off = tail.ends_with("% tex-fmt: off");
        let on = tail.ends_with("% tex-fmt: on");
        let skip = tail.ends_with("% tex-fmt: skip");
        if ignore || off || on || skip {
            output.push_str(line);
        } else {
            let trimmed = line.trim_start_matches([' ', '\t']);
            if trimmed.is_empty() {
                // Empty lines carry no indentation.
            } else {
                let level = line.len() - trimmed.len();
                match config.indent_type {
                    IndentType::Spaces => {
                        output.push_str(&" ".repeat(level * config.indent));
                    }
                    IndentType::Tabs => {
                        for _ in 0..level {
                            output.push('\t');
                        }
                    }
                }
                output.push_str(trimmed);
            }
        }
        if off {
            ignore = true;
        } else if on {
            ignore = false;
        }
        if lines.peek().is_some() {
            output.push('\n');
        }
    }
    output
}

/// tex-fmt works in one-space indentation levels. Convert the final indentation
/// back to levels before the next pass, so a second pass sees the same depth.
fn denormalize(text: &str, config: &Config) -> String {
    let mut output = String::with_capacity(text.len());
    let mut ignore = false;
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let tail = line.trim_end();
        let off = tail.ends_with("% tex-fmt: off");
        let on = tail.ends_with("% tex-fmt: on");
        let skip = tail.ends_with("% tex-fmt: skip");
        if ignore || off || on || skip {
            output.push_str(line);
        } else {
            let trimmed = line.trim_start_matches([' ', '\t']);
            if trimmed.is_empty() {
                // Blank marker lines are comments, so this is only a raw blank.
            } else {
                let leading = line.len() - trimmed.len();
                let level = match config.indent_type {
                    IndentType::Spaces => leading / config.indent,
                    IndentType::Tabs => leading,
                };
                for _ in 0..level {
                    output.push(' ');
                }
                output.push_str(trimmed);
            }
        }
        if off {
            ignore = true;
        } else if on {
            ignore = false;
        }
        if lines.peek().is_some() {
            output.push('\n');
        }
    }
    output
}

fn blank_lines(text: &str, marker: &str, config: &Config) -> String {
    let mut output: Vec<&str> = Vec::new();
    let mut pending = 0usize;
    let mut ignore = false;
    for line in text.split('\n') {
        let tail = line.trim_end();
        let off = tail.ends_with("% tex-fmt: off");
        let on = tail.ends_with("% tex-fmt: on");
        if ignore {
            if line.trim() == marker {
                output.push("");
            } else {
                output.push(line);
            }
        } else if line.trim() == marker || line.trim().is_empty() {
            pending += 1;
        } else {
            if !output.is_empty() {
                output.extend(std::iter::repeat_n("", pending.min(config.blank_lines)));
            }
            pending = 0;
            output.push(line);
        }
        if off {
            ignore = true;
        } else if on {
            ignore = false;
        }
    }
    output.join("\n")
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
