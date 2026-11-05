use std::path::Path;
use std::sync::Arc;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use oxc_allocator::Allocator;
use oxc_formatter::format_with_session;
use oxc_formatter_core::{FormatSession, InputKind, SessionServices, TailwindSorter};
use oxc_span::SourceType;

use crate::config::{Config, LineEndings};
use crate::dialect::Dialect;
use crate::source::syntax_diagnostic;

/// Formats TypeScript/TSX/JavaScript/JSX.
///
/// A BOM and the configured line ending are preserved. Syntax errors are
/// reported as [`DiagnosticKind::Syntax`]; the caller keeps the original file.
pub fn format(
    path: &Path,
    input: &str,
    config: &Config,
    dialect: Dialect,
) -> Result<String, Diagnostic> {
    let (bom, input) = match input.strip_prefix('\u{feff}') {
        Some(input) => (true, input),
        None => (false, input),
    };
    let text = normalize_line_endings(input);
    if text.trim().is_empty() {
        let mut output = String::new();
        if bom {
            output.insert(0, '\u{feff}');
        }
        return Ok(output);
    }
    let allocator = Allocator::default();
    let source_type = source_type(config, path, dialect);
    let options = config.js_options_for_path(path);
    let services = services(config);
    let session = FormatSession::with_services(&allocator, InputKind::PhysicalFile, services);
    let formatted = format_with_session(&session, &text, source_type, options)
        .map_err(|error| syntax_diagnostic(&text, &error))?;
    let mut output = formatted
        .print()
        .map_err(|error| internal(format!("formatting failed: {error}")))?
        .into_code();
    if config.embedded_css {
        output = format_css_in_js(&output, config, source_type);
    }
    output.truncate(output.trim_end_matches('\n').len());
    if config.final_newline && !output.is_empty() {
        output.push('\n');
    }
    if config.verify
        && let Some(removal) = oxc_formatter::detect_code_removal(&text, &output, source_type)
    {
        return Err(internal(format!("formatting would remove code: {removal}")));
    }
    if config.line_endings == LineEndings::Windows {
        output = output.replace('\n', "\r\n");
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(output)
}

fn normalize_line_endings(input: &str) -> String {
    if !input.contains('\r') {
        return input.to_owned();
    }
    let mut output = String::with_capacity(input.len());
    let mut bytes = input.chars().peekable();
    while let Some(character) = bytes.next() {
        if character == '\r' {
            if bytes.peek() == Some(&'\n') {
                bytes.next();
            }
            output.push('\n');
        } else {
            output.push(character);
        }
    }
    output
}

/// Format plain CSS-in-JS tagged templates (`css`/`styled.*`/`createGlobalStyle`).
///
/// Interpolated templates are left untouched: formatting them would require
/// understanding the embedded expression's runtime type. The scan is lexical and
/// deliberately conservative; if anything looks ambiguous the text is preserved.
fn format_css_in_js(input: &str, config: &Config, source_type: SourceType) -> String {
    use oxc_ast::ast::{Expression, TaggedTemplateExpression};
    use oxc_ast_visit::Visit;

    struct Templates<'a> {
        source: &'a str,
        config: &'a Config,
        edits: Vec<(usize, usize, String)>,
    }

    impl<'a> Visit<'a> for Templates<'a> {
        fn visit_tagged_template_expression(&mut self, expression: &TaggedTemplateExpression<'a>) {
            if expression.quasi.expressions.is_empty()
                && let Some(tag) = tag_root(&expression.tag)
                && matches!(tag, "css" | "styled" | "createGlobalStyle" | "keyframes")
            {
                let span = expression.quasi.span;
                let start = span.start as usize + 1;
                let end = span.end as usize;
                if start < end
                    && end <= self.source.len()
                    && let Some(content) = self.source.get(start..end.saturating_sub(1))
                    && let Some(replacement) = format_css_template(content, self.config)
                {
                    self.edits.push((start, end - 1, replacement));
                }
            }
            oxc_ast_visit::walk::walk_tagged_template_expression(self, expression);
        }
    }

    fn tag_root<'a>(expression: &'a Expression<'a>) -> Option<&'a str> {
        match expression {
            Expression::Identifier(identifier) => Some(identifier.name.as_str()),
            Expression::StaticMemberExpression(member) => tag_root(&member.object),
            Expression::ComputedMemberExpression(member) => tag_root(&member.object),
            Expression::CallExpression(call) => tag_root(&call.callee),
            Expression::ParenthesizedExpression(parenthesized) => {
                tag_root(&parenthesized.expression)
            }
            Expression::TSAsExpression(assertion) => tag_root(&assertion.expression),
            Expression::TSSatisfiesExpression(assertion) => tag_root(&assertion.expression),
            Expression::TSTypeAssertion(assertion) => tag_root(&assertion.expression),
            Expression::TSNonNullExpression(expression) => tag_root(&expression.expression),
            _ => None,
        }
    }

    let allocator = Allocator::default();
    let parsed = oxc_parser::Parser::new(&allocator, input, source_type).parse();
    if !parsed.diagnostics.is_empty() {
        return input.to_owned();
    }
    let mut templates = Templates {
        source: input,
        config,
        edits: Vec::new(),
    };
    templates.visit_program(&parsed.program);
    if templates.edits.is_empty() {
        return input.to_owned();
    }
    let mut output = input.to_owned();
    for (start, end, replacement) in templates.edits.into_iter().rev() {
        output.replace_range(start..end, &replacement);
    }
    output
}

fn format_css_template(content: &str, config: &Config) -> Option<String> {
    let first = content.find(|character: char| !character.is_whitespace())?;
    let last = content
        .rfind(|character: char| !character.is_whitespace())
        .map(|index| index + content[index..].chars().next().map_or(1, char::len_utf8))?;
    let leading = &content[..first];
    let trailing = &content[last..];
    let body = &content[first..last];
    let indent = leading
        .rsplit('\n')
        .next()
        .filter(|_| leading.contains('\n'))
        .unwrap_or("");
    let common = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let dedented = body
        .lines()
        .map(|line| line.get(common..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let css = dotfmt_css::Config {
        width: config.width,
        indent: config.indent,
        final_newline: false,
        tailwind: dotfmt_tailwind::Config {
            mode: dotfmt_tailwind::Mode::Off,
            ..Default::default()
        },
        ..Default::default()
    };
    let formatted = dotfmt_css::format(
        std::path::Path::new("inline.css"),
        &dedented,
        &css,
        dotfmt_css::Dialect::Css,
    )
    .ok()?;
    let formatted = formatted.trim_end_matches('\n');
    let mut lines = formatted.lines();
    let first_line = lines.next().unwrap_or("");
    let mut indented = String::from(first_line);
    for line in lines {
        indented.push('\n');
        indented.push_str(indent);
        indented.push_str(line);
    }
    Some(format!("{leading}{indented}{trailing}"))
}

fn services(config: &Config) -> SessionServices {
    let sorter = config.tailwind_sorter.clone();
    let tailwind_sorter: Option<TailwindSorter> = if config.tailwind_enabled() {
        sorter.map(|sorter| {
            Arc::new(move |lists: Vec<String>| {
                lists
                    .into_iter()
                    .map(|list| sorter.sort_list(&list))
                    .collect()
            }) as TailwindSorter
        })
    } else {
        None
    };
    SessionServices {
        tailwind_sorter,
        ..Default::default()
    }
}

fn source_type(config: &Config, path: &Path, dialect: Dialect) -> SourceType {
    let dialect = dialect.resolve(config.dialect, path);
    dialect.source_type_for_path(path)
}

fn internal(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
