use std::borrow::Cow;
use std::path::{Path, PathBuf};

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use crate::config::{Config, LineEndings};
use crate::dialect::Dialect;

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
    if let Some(offset) = bare_carriage_return(input) {
        let (line, column) = line_column(input, offset);
        return Err(Diagnostic::new(
            DiagnosticKind::Syntax,
            "error parsing: a carriage return must be followed by a line feed",
        )
        .with_location(line, Some(column)));
    }
    let text = input.replace("\r\n", "\n");
    let dialect = dialect.resolve(config.dialect, path);
    let format_markup = |source: &str| {
        markup_fmt::format_text(
            source,
            dialect.language(),
            &config.markup_options(),
            |code, hints| Ok(embedded(code, hints.ext, config)),
        )
    };
    // Format the document as written first. The optional-end-tag pre-pass is
    // only a repair path for markup that omits optional closing tags; running
    // it unconditionally can corrupt well-formed HTML with container elements.
    let mut prepared = Cow::Borrowed(text.as_str());
    let mut output = match format_markup(&prepared) {
        Ok(output) => output,
        Err(first) if dialect == Dialect::Html => {
            let with_optional = crate::optional::insert_optional_end_tags(&prepared);
            if with_optional.as_ref() == prepared.as_ref() {
                return Err(syntax(&prepared, first));
            }
            prepared = Cow::Owned(with_optional.into_owned());
            format_markup(&prepared).map_err(|error| syntax(&prepared, error))?
        }
        Err(error) => return Err(syntax(&prepared, error)),
    };
    output.truncate(output.trim_end_matches('\n').len());
    if config.tailwind_enabled()
        && let Some(sorter) = &config.tailwind_sorter
    {
        output =
            crate::tailwind::sort_class_attributes(&output, sorter, &config.tailwind.attributes);
    }
    if markup_fmt::format_text(
        &output,
        dialect.language(),
        &config.markup_options(),
        |code, _| Ok(Cow::Borrowed(code)),
    )
    .is_err()
    {
        return Err(Diagnostic::new(
            DiagnosticKind::Internal,
            "formatting would produce invalid markup",
        ));
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

/// Formats one embedded `<style>` or `<script>` snippet.
///
/// `markup_fmt` derives the extension from the tag's `lang` (and `type`)
/// attribute, so `lang="scss"`, `lang="ts"` and `lang="tsx"` arrive here as
/// `scss`, `ts` and `tsx`. A snippet that the embedded formatter rejects is
/// returned unchanged; one bad snippet must not fail the whole document.
fn embedded<'a>(code: &'a str, extension: &str, config: &Config) -> Cow<'a, str> {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let extension = match extension.as_str() {
        "typescript" => "ts",
        "javascript" | "ecmascript" => "js",
        "postcss" => "css",
        other => other,
    };
    let path = PathBuf::from(format!("inline.{extension}"));
    let formatted = match extension {
        "css" | "pcss" | "scss" | "sass" | "less" => format_css(code, &path, config),
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => {
            format_typescript(code, &path, config)
        }
        "html" | "htm" | "vue" | "svelte" | "astro" => {
            let dialect = Dialect::for_path(&path).unwrap_or(Dialect::Html);
            format(&path, code, config, dialect).ok()
        }
        // `markup_fmt` supplies `js`/`css` when the tag has no `lang`. An
        // explicitly empty or unrecognized language hint still tells us this is
        // script or style content; choose the embedded formatter only when
        // exactly one of them accepts the snippet.
        "json" => None,
        extension if extension.starts_with("markup-fmt-") => None,
        _ => fallback(code, config),
    };
    formatted.map_or(Cow::Borrowed(code), Cow::Owned)
}

fn format_css(code: &str, path: &Path, config: &Config) -> Option<String> {
    let mut css = config.css.clone();
    css.tailwind = config.tailwind.clone();
    css.tailwind_sorter = config.tailwind_sorter.clone();
    let dialect = dotfmt_css::dialect::Dialect::for_path(path).unwrap_or_default();
    dotfmt_css::format(path, code, &css, dialect).ok()
}

fn format_typescript(code: &str, path: &Path, config: &Config) -> Option<String> {
    let mut typescript = config.typescript.clone();
    typescript.tailwind = config.tailwind.clone();
    typescript.tailwind_sorter = config.tailwind_sorter.clone();
    let dialect =
        dotfmt_typescript::Dialect::for_path(path).unwrap_or(dotfmt_typescript::Dialect::Ts);
    dotfmt_typescript::format(path, code, &typescript, dialect).ok()
}

fn fallback(code: &str, config: &Config) -> Option<String> {
    let css = format_css(code, Path::new("inline.css"), config);
    let typescript = format_typescript(code, Path::new("inline.js"), config);
    match (css, typescript) {
        (Some(css), None) => Some(css),
        (None, Some(typescript)) => Some(typescript),
        (Some(css), Some(typescript)) if css == typescript => Some(css),
        _ => None,
    }
}

fn syntax(text: &str, error: markup_fmt::FormatError) -> Diagnostic {
    let diagnostic = Diagnostic::new(DiagnosticKind::Syntax, error.to_string());
    match error {
        markup_fmt::FormatError::Syntax(error) => {
            let message = format!("syntax error '{}'", error.kind);
            let (line, column) = line_column(text, error.pos);
            Diagnostic::new(DiagnosticKind::Syntax, message).with_location(line, Some(column))
        }
        _ => diagnostic,
    }
}

fn bare_carriage_return(input: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    bytes
        .iter()
        .enumerate()
        .find(|(offset, byte)| **byte == b'\r' && bytes.get(offset + 1) != Some(&b'\n'))
        .map(|(offset, _)| offset)
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.chars().count() + 1);
    (line, column)
}
