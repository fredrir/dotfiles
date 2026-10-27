use oxc_ast::ast::{Declaration, Program, Statement};
use oxc_span::{GetSpan, Span};

/// Formatting conventions inferred from the file, so edits read as hand-written.
pub struct Layout {
    pub unit: String,
    pub newline: &'static str,
    pub semicolons: bool,
}

impl Layout {
    pub fn detect(source: &str, program: &Program) -> Self {
        Self {
            unit: indent_unit(source),
            newline: if source.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            },
            semicolons: semicolons(source, program),
        }
    }

    pub fn terminator(&self) -> &'static str {
        if self.semicolons { ";" } else { "" }
    }
}

fn indent_unit(source: &str) -> String {
    for line in source.lines().take(1000) {
        let content = line.trim_start_matches([' ', '\t']);
        let width = line.len() - content.len();
        if width == 0 || content.is_empty() || content.starts_with('*') {
            continue;
        }
        return line[..width].to_string();
    }
    "  ".into()
}

fn semicolons(source: &str, program: &Program) -> bool {
    let mut terminated = 0;
    let mut bare = 0;
    for statement in &program.body {
        let needs_terminator = match statement {
            Statement::ExportDeclaration(export) => matches!(
                export.declaration,
                Declaration::VariableDeclaration(_) | Declaration::TSTypeAliasDeclaration(_)
            ),
            Statement::VariableDeclaration(_)
            | Statement::ExportNamedDeclaration(_)
            | Statement::ExportFromDeclaration(_)
            | Statement::ExpressionStatement(_)
            | Statement::ImportDeclaration(_)
            | Statement::ExportAllDeclaration(_)
            | Statement::TSTypeAliasDeclaration(_) => true,
            _ => false,
        };
        if needs_terminator {
            if source[..statement.span().end as usize].ends_with(';') {
                terminated += 1;
            } else {
                bare += 1;
            }
        }
    }
    terminated >= bare
}

pub fn line_start(source: &str, offset: usize) -> usize {
    source[..offset].rfind('\n').map_or(0, |index| index + 1)
}

pub fn indentation(source: &str, offset: usize) -> &str {
    let line = &source[line_start(source, offset)..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Moves every line after the first from indentation `from` to `to`.
pub fn reindent(text: &str, from: &str, to: &str) -> String {
    let mut lines = text.split('\n');
    let mut output = lines.next().unwrap_or_default().to_string();
    for line in lines {
        output.push('\n');
        if line.trim().is_empty() {
            output.push_str(line.trim_start_matches([' ', '\t']));
            continue;
        }
        output.push_str(to);
        output.push_str(
            line.strip_prefix(from)
                .unwrap_or_else(|| line.trim_start_matches([' ', '\t'])),
        );
    }
    output
}

pub fn has_comment(program: &Program, span: Span) -> bool {
    program
        .comments
        .iter()
        .any(|comment| span.start <= comment.span.start && comment.span.end <= span.end)
}

/// Extends a declaration's span over a trailing `;` it does not already cover.
pub fn with_terminator(source: &str, span: Span) -> std::ops::Range<usize> {
    let end = span.end as usize;
    if source[..end].ends_with(';') {
        return span.start as usize..end;
    }
    let rest = &source[end..];
    let trimmed = rest.trim_start_matches([' ', '\t']);
    if trimmed.starts_with(';') {
        span.start as usize..end + (rest.len() - trimmed.len()) + 1
    } else {
        span.start as usize..end
    }
}

/// Swaps the last `from` keyword in a declaration head for `to`.
pub fn swap_keyword(head: &str, from: &str, to: &str) -> Option<String> {
    let index = head.rfind(from)?;
    Some(format!(
        "{}{to}{}",
        &head[..index],
        &head[index + from.len()..]
    ))
}
