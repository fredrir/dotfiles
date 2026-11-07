use std::path::Path;
use std::sync::Arc;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use oxc_allocator::Allocator;
use oxc_formatter::{JsFormatOptions, format_with_session};
use oxc_formatter_core::{FormatSession, InputKind, SessionServices, TailwindSorter};
use oxc_span::SourceType;

use crate::config::{Config, LineEndings};
use crate::dialect::Dialect;
use crate::source::syntax_diagnostic;

const MAX_FORMAT_PASSES: usize = 3;

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
    if crate::source::nesting_exceeded(&text) {
        return Err(Diagnostic::new(
            DiagnosticKind::Syntax,
            "input nesting exceeds the supported limit",
        ));
    }
    if text.trim().is_empty() {
        let mut output = String::new();
        if bom {
            output.insert(0, '\u{feff}');
        }
        return Ok(output);
    }
    let source_type = source_type(config, path, dialect);
    let options = config.js_options_for_path(path);
    let services = services(config);
    let mut output = format_pass(&text, config, source_type, &options, &services)?;
    let mut settled = false;
    for pass in 2..=MAX_FORMAT_PASSES {
        let next = format_pass(&output, config, source_type, &options, &services)?;
        if next == output {
            output = next;
            settled = true;
            if pass > 2 {
                debug_log(format_args!(
                    "{}: formatting settled on pass {pass}",
                    path.display()
                ));
            }
            break;
        }
        output = next;
    }
    if !settled {
        warn_log(format_args!(
            "{}: formatting did not settle after {MAX_FORMAT_PASSES} passes; using the last pass",
            path.display()
        ));
    }
    let tailwind = if config.tailwind_enabled() {
        config.tailwind_sorter.as_ref()
    } else {
        None
    };
    if config.verify
        && let Some(removal) =
            crate::verify::detect_code_removal(&text, &output, source_type, tailwind)
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

fn format_pass(
    text: &str,
    config: &Config,
    source_type: SourceType,
    options: &JsFormatOptions,
    services: &SessionServices,
) -> Result<String, Diagnostic> {
    let allocator = Allocator::default();
    let session =
        FormatSession::with_services(&allocator, InputKind::PhysicalFile, services.clone());
    let formatted = format_with_session(&session, text, source_type, options.clone())
        .map_err(|error| syntax_diagnostic(text, &error))?;
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
    Ok(output)
}

fn debug_log(message: std::fmt::Arguments<'_>) {
    if std::env::var_os("DOTFMT_DEBUG").is_some() {
        eprintln!("dotfmt: debug: {message}");
    }
}

fn warn_log(message: std::fmt::Arguments<'_>) {
    eprintln!("dotfmt: warning: {message}");
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

fn format_css_in_js(input: &str, config: &Config, source_type: SourceType) -> String {
    use oxc_ast::ast::{Expression, TaggedTemplateExpression, TemplateLiteral};
    use oxc_ast_visit::Visit;
    use oxc_span::GetSpan;

    struct Templates<'a> {
        source: &'a str,
        config: &'a Config,
        edits: Vec<(usize, usize, String)>,
    }

    impl<'a> Visit<'a> for Templates<'a> {
        fn visit_tagged_template_expression(&mut self, expression: &TaggedTemplateExpression<'a>) {
            if let Some(tag) = tag_root(&expression.tag)
                && matches!(tag, "css" | "styled" | "createGlobalStyle" | "keyframes")
            {
                if let Some(replacement) =
                    format_tagged_template(self.source, &expression.quasi, self.config)
                {
                    let span = expression.quasi.span;
                    let start = span.start as usize;
                    let end = span.end as usize;
                    if start < end && end <= self.source.len() {
                        self.edits.push((start + 1, end - 1, replacement));
                    }
                }
                self.visit_expression(&expression.tag);
                if let Some(type_arguments) = &expression.type_arguments {
                    self.visit_ts_type_parameter_instantiation(type_arguments);
                }
                return;
            }
            oxc_ast_visit::walk::walk_tagged_template_expression(self, expression);
        }
    }

    fn format_tagged_template(
        source: &str,
        quasi: &TemplateLiteral<'_>,
        config: &Config,
    ) -> Option<String> {
        let start = (quasi.span.start as usize).checked_add(1)?;
        let end = (quasi.span.end as usize).checked_sub(1)?;
        if start > end || end > source.len() {
            return None;
        }
        let (masked, interpolations) = mask_interpolations(source, quasi, start, end)?;
        let formatted = format_css_template(&masked, config)?;
        restore_interpolations(&formatted, &interpolations)
    }

    fn mask_interpolations(
        source: &str,
        quasi: &TemplateLiteral<'_>,
        content_start: usize,
        content_end: usize,
    ) -> Option<(String, Vec<(String, String)>)> {
        if quasi.expressions.len() + 1 != quasi.quasis.len() {
            return None;
        }
        let content = source.get(content_start..content_end)?;
        let mut masked = String::with_capacity(content.len());
        let mut interpolations = Vec::with_capacity(quasi.expressions.len());
        let mut copied = content_start;
        for (index, expression) in quasi.expressions.iter().enumerate() {
            let interpolation_start = quasi.quasis[index].span.end as usize;
            let interpolation_end = quasi.quasis[index + 1].span.start as usize;
            if interpolation_start < copied
                || interpolation_end > content_end
                || interpolation_start > interpolation_end
            {
                return None;
            }
            let interpolation = source.get(interpolation_start..interpolation_end)?;
            let expression_span = expression.span();
            if (expression_span.start as usize) < interpolation_start
                || (expression_span.end as usize) > interpolation_end
            {
                return None;
            }
            if !interpolation.starts_with("${") || !interpolation.ends_with('}') {
                return None;
            }
            let placeholder = format!("DOTFMT_EXPR_{index}");
            masked.push_str(source.get(copied..interpolation_start)?);
            masked.push_str(&placeholder);
            interpolations.push((placeholder, interpolation.to_owned()));
            copied = interpolation_end;
        }
        masked.push_str(source.get(copied..content_end)?);
        for (placeholder, _) in &interpolations {
            if masked.matches(placeholder.as_str()).count() != 1 {
                return None;
            }
        }
        Some((masked, interpolations))
    }

    fn restore_interpolations(
        formatted: &str,
        interpolations: &[(String, String)],
    ) -> Option<String> {
        for (placeholder, _) in interpolations {
            if formatted.matches(placeholder.as_str()).count() != 1 {
                return None;
            }
        }
        let mut restored = String::with_capacity(formatted.len());
        let mut cursor = 0;
        while cursor < formatted.len() {
            let rest = &formatted[cursor..];
            if let Some((placeholder, original)) = interpolations
                .iter()
                .find(|(placeholder, _)| rest.starts_with(placeholder.as_str()))
            {
                restored.push_str(original);
                cursor += placeholder.len();
            } else {
                let character = rest.chars().next()?;
                restored.push(character);
                cursor += character.len_utf8();
            }
        }
        Some(restored)
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
    let mut edits = templates.edits;
    edits.sort_by_key(|edit| edit.0);
    let mut disjoint = Vec::with_capacity(edits.len());
    for edit in edits {
        if disjoint
            .last()
            .is_some_and(|(_, end, _): &(usize, usize, String)| edit.0 < *end)
        {
            continue;
        }
        disjoint.push(edit);
    }
    let mut output = input.to_owned();
    for (start, end, replacement) in disjoint.into_iter().rev() {
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
    let first_line = lines.next().unwrap_or("").trim_end();
    let mut indented = String::from(first_line);
    for line in lines {
        indented.push('\n');
        let line = line.trim_end();
        if !line.is_empty() {
            indented.push_str(indent);
            indented.push_str(line);
        }
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
