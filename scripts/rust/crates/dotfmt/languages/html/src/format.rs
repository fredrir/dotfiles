use std::borrow::Cow;
use std::path::{Path, PathBuf};

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use crate::config::{Config, LineEndings};
use crate::dialect::Dialect;

const MAX_TAILWIND_LAYOUT_PASSES: usize = 3;

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
    if nesting_exceeded(&text) {
        return Err(Diagnostic::new(
            DiagnosticKind::Syntax,
            "input nesting exceeds the supported limit",
        ));
    }
    let dialect = dialect.resolve(config.dialect, path);
    let format_markup = |source: &str| {
        let (protected, preserved) = protect(source, dialect, config);
        let output = markup_fmt::format_text(
            &protected,
            dialect.language(),
            &config.markup_options(),
            |code, hints| Ok(embedded(code, hints.ext, config)),
        )?;
        Ok(restore(output, &preserved))
    };
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
        let sorted =
            crate::tailwind::sort_class_attributes(&output, sorter, &config.tailwind.attributes);
        if sorted != output {
            output = sorted;
            // Sorting can shorten a class attribute enough for the markup layout
            // to reflow. Re-run the layout on the sorted output until the pair
            // settles so a single pass returns the final layout.
            for _ in 0..MAX_TAILWIND_LAYOUT_PASSES {
                let laid_out = format_markup(&output).map_err(|_| invalid_markup())?;
                let mut laid_out = laid_out;
                laid_out.truncate(laid_out.trim_end_matches('\n').len());
                let sorted = crate::tailwind::sort_class_attributes(
                    &laid_out,
                    sorter,
                    &config.tailwind.attributes,
                );
                if sorted == output {
                    break;
                }
                output = sorted;
            }
        }
    }
    if markup_fmt::format_text(
        &output,
        dialect.language(),
        &config.markup_options(),
        |code, _| Ok(Cow::Borrowed(code)),
    )
    .is_err()
    {
        return Err(invalid_markup());
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

fn invalid_markup() -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::Internal,
        "formatting would produce invalid markup",
    )
}

const MAX_NESTING: usize = 512;

const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];
const RAW: &[&str] = &["script", "style", "textarea", "title"];

/// Rejects markup nested deeper than the formatter's recursion budget. The
/// parser previously overflowed the stack on a few thousand nested elements,
/// which aborts the whole process (and the language server) instead of
/// producing a diagnostic.
fn nesting_exceeded(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'<' => {
                let Some((kind, name, end, self_closing)) = tag_at(bytes, index) else {
                    index += 1;
                    continue;
                };
                index = end;
                match kind {
                    Tag::Closing => depth = depth.saturating_sub(1),
                    Tag::Opening => {
                        if self_closing || VOID.contains(&name.as_str()) {
                            continue;
                        }
                        depth += 1;
                        if depth > MAX_NESTING {
                            return true;
                        }
                        if RAW.contains(&name.as_str()) {
                            index = raw_end(bytes, index, &name);
                            depth = depth.saturating_sub(1);
                        }
                    }
                }
            }
            b'{' => {
                index = expression_end(bytes, index);
            }
            _ => index += 1,
        }
    }
    false
}

enum Tag {
    Opening,
    Closing,
}

/// Parses one tag starting at `start` and returns its name, the offset after
/// `>`, and whether it closed itself. Comments, declarations and processing
/// instructions are skipped without counting.
fn tag_at(bytes: &[u8], start: usize) -> Option<(Tag, String, usize, bool)> {
    let next = *bytes.get(start + 1)?;
    if next == b'!' {
        // Comments, CDATA and declarations are skipped as a whole: a `>` inside
        // a comment must not end it early and expose commented-out tags to the
        // nesting counter.
        if bytes[start..].starts_with(b"<!--") {
            let end = find_seq(bytes, start + 4, b"-->")?;
            return Some((Tag::Opening, String::new(), end + 3, true));
        }
        if bytes[start..].starts_with(b"<![CDATA[") {
            let end = find_seq(bytes, start + 9, b"]]>")?;
            return Some((Tag::Opening, String::new(), end + 3, true));
        }
        let end = find(bytes, start + 2, b'>')?;
        return Some((Tag::Opening, String::new(), end + 1, true));
    }
    if next == b'?' {
        return Some((Tag::Opening, String::new(), find(bytes, start + 2, b'>')? + 1, true));
    }
    let closing = next == b'/';
    let name_start = start + if closing { 2 } else { 1 };
    let mut name_end = name_start;
    while name_end < bytes.len()
        && (bytes[name_end].is_ascii_alphanumeric()
            || matches!(bytes[name_end], b'-' | b':' | b'_'))
    {
        name_end += 1;
    }
    if name_end == name_start {
        return None;
    }
    let name = String::from_utf8_lossy(&bytes[name_start..name_end]).to_ascii_lowercase();
    let mut quote = None;
    let mut cursor = name_end;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(active) = quote {
            if byte == active {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'>' => {
                let self_closing = cursor > 0 && bytes[cursor - 1] == b'/';
                return Some((
                    if closing { Tag::Closing } else { Tag::Opening },
                    name,
                    cursor + 1,
                    self_closing,
                ));
            }
            _ => {}
        }
        cursor += 1;
    }
    Some((if closing { Tag::Closing } else { Tag::Opening }, name, bytes.len(), true))
}

/// Skips the contents of a raw-text element to just after its closing tag.
fn raw_end(bytes: &[u8], start: usize, name: &str) -> usize {
    let needle = format!("</{name}");
    let lowered = String::from_utf8_lossy(&bytes[start..]).to_ascii_lowercase();
    let Some(offset) = lowered.find(&needle) else {
        return bytes.len();
    };
    let close = start + offset;
    find(bytes, close, b'>').map_or(bytes.len(), |end| end + 1)
}

/// Skips a `{ ... }` template expression, respecting nested braces and strings.
fn expression_end(bytes: &[u8], start: usize) -> usize {
    let mut depth = 1usize;
    let mut quote = None;
    let mut escaped = false;
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if escaped {
            escaped = false;
        } else if let Some(active) = quote {
            if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
        } else {
            match byte {
                b'\'' | b'"' | b'`' => quote = Some(byte),
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return cursor + 1;
                    }
                }
                _ => {}
            }
        }
        cursor += 1;
    }
    bytes.len()
}

fn find(bytes: &[u8], start: usize, needle: u8) -> Option<usize> {
    bytes[start..].iter().position(|byte| *byte == needle).map(|offset| start + offset)
}

fn find_seq(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || start > bytes.len() {
        return None;
    }
    bytes[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn embedded<'a>(code: &'a str, extension: &str, config: &Config) -> Cow<'a, str> {
    embedded_format(code, extension, config).map_or(Cow::Borrowed(code), Cow::Owned)
}

fn embedded_format(code: &str, extension: &str, config: &Config) -> Option<String> {
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
    match extension {
        "css" | "pcss" | "scss" | "sass" | "less" => format_css(code, &path, config),
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => {
            format_typescript(code, &path, config)
        }
        "html" | "htm" | "vue" | "svelte" | "astro" => {
            let dialect = Dialect::for_path(&path).unwrap_or(Dialect::Html);
            format(&path, code, config, dialect).ok()
        }
        "json" => None,
        extension if extension.starts_with("markup-fmt-") => None,
        _ => fallback(code, config),
    }
}

fn format_css(code: &str, path: &Path, config: &Config) -> Option<String> {
    let mut css = config.css.clone();
    css.tailwind = config.tailwind.clone();
    css.tailwind_sorter = config.tailwind_sorter.clone();
    let dialect = dotfmt_css::dialect::Dialect::for_path(path).unwrap_or_default();
    dotfmt_css::format(path, code, &css, dialect).ok()
}

fn format_typescript(code: &str, path: &Path, config: &Config) -> Option<String> {
    let options = match path.extension().and_then(|extension| extension.to_str()) {
        Some("js" | "jsx" | "mjs" | "cjs") => &config.javascript,
        _ => &config.typescript,
    };
    let mut typescript = options.clone();
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

const PRESERVED_OPEN: char = '\u{e000}';
const PRESERVED_CLOSE: char = '\u{e001}';

/// Replaces `<script>` and `<style>` blocks that the embedded formatter declines with
/// opaque tokens. markup_fmt re-indents raw snippets even when the external formatter
/// leaves them untouched, which changes authored indentation and grows it on every
/// pass, so those blocks are restored byte-for-byte instead.
fn protect<'a>(
    source: &'a str,
    dialect: Dialect,
    config: &Config,
) -> (Cow<'a, str>, Vec<(String, &'a str)>) {
    if dialect == Dialect::Xml {
        return (Cow::Borrowed(source), Vec::new());
    }
    let mut replaced = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    let marker = format!("{PRESERVED_OPEN}dotfmt");
    let mut search = 0usize;
    while let Some(offset) = source[search..].find(&marker) {
        let after = search + offset + marker.len();
        let digits: String = source[after..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if !digits.is_empty()
            && source[after + digits.len()..].starts_with(':')
            && let Ok(value) = digits.parse::<usize>()
        {
            used.insert(value);
        }
        search = after;
    }
    let salt = (0usize..).find(|value| !used.contains(value)).unwrap_or(0);
    let prefix = format!("{PRESERVED_OPEN}dotfmt{salt}:");
    let mut offset = 0;
    while let Some(element) = next_embedded(&source[offset..]) {
        let start = offset + element.start;
        let end = offset + element.end;
        if preserves(&element, dialect, config) {
            let token = format!("{prefix}{}{PRESERVED_CLOSE}", replaced.len());
            replaced.push((start, end, token));
        }
        offset = end;
    }
    if replaced.is_empty() {
        return (Cow::Borrowed(source), Vec::new());
    }
    let mut protected = String::with_capacity(source.len());
    let mut cursor = 0;
    for (start, end, token) in &replaced {
        protected.push_str(&source[cursor..*start]);
        protected.push_str(token);
        cursor = *end;
    }
    protected.push_str(&source[cursor..]);
    let preserved = replaced
        .into_iter()
        .map(|(start, end, token)| (token, &source[start..end]))
        .collect();
    (Cow::Owned(protected), preserved)
}

fn restore(mut output: String, preserved: &[(String, &str)]) -> String {
    for (token, source) in preserved {
        if let Some(index) = output.find(token.as_str()) {
            output.replace_range(index..index + token.len(), source);
        }
    }
    output
}

fn preserves(element: &Embedded<'_>, dialect: Dialect, config: &Config) -> bool {
    match element.kind {
        // JSON scripts are already byte-preserved by markup_fmt and keeping them
        // as text tokens would join adjacent blocks onto one line.
        Kind::Script if is_json_script(element.tag) => false,
        Kind::Script => script_extension(element.tag, dialect)
            .is_none_or(|extension| embedded_format(element.content, extension, config).is_none()),
        Kind::Style => {
            let extension = attribute(element.tag, "lang").unwrap_or("css");
            embedded_format(element.content, extension, config).is_none()
        }
    }
}

fn is_json_script(tag: &str) -> bool {
    attribute(tag, "type").is_some_and(|kind| {
        let kind = kind.to_ascii_lowercase();
        matches!(kind.as_str(), "importmap" | "speculationrules")
            || kind.ends_with("/json")
            || kind.ends_with("+json")
    })
}

/// Mirrors markup_fmt's script dispatch: only JavaScript types (and no type) reach the
/// external formatter, so JSON and template scripts are always preserved.
fn script_extension(tag: &str, dialect: Dialect) -> Option<&str> {
    if let Some(kind) = attribute(tag, "type")
        && !matches!(
            kind.to_ascii_lowercase().as_str(),
            "module"
                | "application/javascript"
                | "text/javascript"
                | "application/ecmascript"
                | "text/ecmascript"
                | "application/x-javascript"
                | "application/x-ecmascript"
                | "text/x-javascript"
                | "text/x-ecmascript"
                | "text/jsx"
                | "text/babel"
        )
    {
        return None;
    }
    Some(
        attribute(tag, "lang").unwrap_or(if dialect == Dialect::Astro {
            "ts"
        } else {
            "js"
        }),
    )
}

#[derive(Clone, Copy)]
enum Kind {
    Script,
    Style,
}

struct Embedded<'a> {
    start: usize,
    end: usize,
    kind: Kind,
    tag: &'a str,
    content: &'a str,
}

fn next_embedded(source: &str) -> Option<Embedded<'_>> {
    let mut offset = 0;
    while let Some(index) = source[offset..].find('<') {
        let start = offset + index;
        let rest = &source[start + 1..];
        for (name, kind) in [("script", Kind::Script), ("style", Kind::Style)] {
            if starts_with_name(rest, name)
                && let Some(element) = embedded_at(source, start, name, kind)
            {
                return Some(element);
            }
        }
        offset = start + 1;
    }
    None
}

fn starts_with_name(rest: &str, name: &str) -> bool {
    rest.get(..name.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(name))
        && rest[name.len()..]
            .starts_with(|ch: char| ch.is_ascii_whitespace() || ch == '>' || ch == '/')
}

fn embedded_at<'a>(
    source: &'a str,
    start: usize,
    name: &'static str,
    kind: Kind,
) -> Option<Embedded<'a>> {
    let open_end = tag_end(source, start)?;
    let tag = &source[start..=open_end];
    // A self-closing tag has no content to preserve.
    if tag[..tag.len() - 1].trim_end().ends_with('/') {
        return None;
    }
    let content_start = open_end + 1;
    let (close_start, close_end) = close_tag(source, content_start, name)?;
    Some(Embedded {
        start,
        end: close_end,
        kind,
        tag,
        content: &source[content_start..close_start],
    })
}

fn tag_end(source: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (index, ch) in source[start..].char_indices() {
        match (quote, ch) {
            (Some(expected), ch) if ch == expected => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '>') => return Some(start + index),
            _ => {}
        }
    }
    None
}

fn close_tag(source: &str, from: usize, name: &str) -> Option<(usize, usize)> {
    let mut offset = from;
    while let Some(index) = source[offset..].find("</") {
        let start = offset + index;
        if starts_with_name(&source[start + 2..], name) {
            let end = tag_end(source, start)?;
            return Some((start, end + 1));
        }
        offset = start + 2;
    }
    None
}

/// Reads the raw value of the first `name` attribute in a start tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = &tag[tag.find(char::is_whitespace)?..];
    loop {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with('>') {
            return None;
        }
        let end = rest.find(|ch: char| ch.is_whitespace() || ch == '=' || ch == '>')?;
        let (attribute_name, remainder) = rest.split_at(end);
        let remainder = remainder.trim_start();
        let (value, next) = match remainder.strip_prefix('=') {
            Some(remainder) => {
                let remainder = remainder.trim_start();
                match remainder
                    .chars()
                    .next()
                    .filter(|ch| matches!(ch, '"' | '\''))
                {
                    Some(quote) => {
                        let quoted = &remainder[1..];
                        let end = quoted.find(quote)?;
                        (&quoted[..end], &quoted[end + 1..])
                    }
                    None => {
                        let end = remainder
                            .find(|ch: char| ch.is_whitespace() || ch == '>')
                            .unwrap_or(remainder.len());
                        (&remainder[..end], &remainder[end..])
                    }
                }
            }
            None => ("", remainder),
        };
        if attribute_name.eq_ignore_ascii_case(name) {
            return Some(value);
        }
        if next.len() == rest.len() {
            return None;
        }
        rest = next;
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
