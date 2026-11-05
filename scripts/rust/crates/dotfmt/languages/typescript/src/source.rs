use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use oxc_diagnostics::OxcDiagnostic;

/// One-based line and UTF-16 column for a byte offset.
pub(crate) fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.encode_utf16().count() + 1);
    (line, column)
}

/// Converts an oxc parser diagnostic into the formatter's fatal diagnostic.
pub(crate) fn syntax_diagnostic(text: &str, error: &OxcDiagnostic) -> Diagnostic {
    let diagnostic = Diagnostic::new(DiagnosticKind::Syntax, error.to_string());
    let offset = error
        .labels
        .as_ref()
        .first()
        .map(|label| label.offset() as usize);
    match offset {
        Some(offset) => {
            let (line, column) = line_column(text, offset);
            diagnostic.with_location(line, Some(column))
        }
        None => diagnostic,
    }
}

const NESTING_LIMIT: usize = 8192;
/// Bound the lookahead used to classify an angle bracket. A generic type that
/// does not close within this many bytes is treated as ambiguous and ignored.
const MAX_ANGLE_LOOKAHEAD: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Code,
    JsxChildren,
    JsxTag { closing: bool, self_closing: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Angle {
    OpeningTag,
    ClosingTag,
    Fragment,
    Generic,
    Ignore,
}

/// A conservative guard against parser/formatter stack exhaustion.
///
/// oxc is recursive; a few adversarial inputs (30k nested JSX or 100k parens)
/// overflow even the 32 MiB worker stack. This scanner ignores strings,
/// comments, template literals, and regular expressions, counts balanced
/// `()[]{}` nesting, and distinguishes JSX elements from generic angle
/// brackets.
///
/// Angle classification is intentionally conservative. In code (outside a
/// JSX child), fragments, self-closing tags, and tags followed by another tag
/// are JSX. A simple `<Identifier>` is also JSX when a matching closing tag is
/// found in a bounded window; otherwise it is balanced as a generic. Constructs
/// with generic syntax markers (commas, `extends`/`keyof`/`infer`, nested
/// angles, unions, defaults, and so on) are generics. That choice keeps
/// repeated flat type assertions, generic calls, and declarations from
/// accumulating while still counting nested JSX whose children are themselves
/// tags. Ambiguous components with plain text children beyond the closing-tag
/// lookahead are under-counted; treating every such angle as JSX could reject
/// ordinary flat generic code instead.
pub(crate) fn nesting_exceeded(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut modes = vec![Mode::Code];
    // Brace depth to restore when returning from a JSX expression container.
    let mut braces = Vec::new();
    let mut bracket_depth = 0usize;
    let mut jsx_depth = 0usize;
    let mut generic_depth = 0usize;
    let mut expression_expected = true;
    let mut index = 0usize;

    while index < bytes.len() {
        let mode = *modes.last().unwrap_or(&Mode::Code);
        let byte = bytes[index];
        match mode {
            Mode::Code => {
                if byte == b'/' {
                    if bytes.get(index + 1) == Some(&b'/') {
                        index = skip_line_comment(bytes, index);
                        continue;
                    }
                    if bytes.get(index + 1) == Some(&b'*') {
                        index = skip_block_comment(bytes, index);
                        continue;
                    }
                    if expression_expected && let Some(end) = skip_regex(bytes, index) {
                        index = end;
                        expression_expected = false;
                        continue;
                    }
                    expression_expected = true;
                    index += 1;
                    continue;
                }
                if matches!(byte, b'\'' | b'"') {
                    index = skip_quoted(bytes, index, byte);
                    expression_expected = false;
                    continue;
                }
                if byte == b'`' {
                    index = skip_template(bytes, index);
                    expression_expected = false;
                    continue;
                }
                if is_ident_start(byte) {
                    let start = index;
                    index += 1;
                    while index < bytes.len() && is_ident_continue(bytes[index]) {
                        index += 1;
                    }
                    expression_expected = is_expression_keyword(&bytes[start..index]);
                    continue;
                }
                if byte.is_ascii_digit() {
                    index += 1;
                    while index < bytes.len()
                        && (bytes[index].is_ascii_alphanumeric()
                            || matches!(bytes[index], b'_' | b'.'))
                    {
                        index += 1;
                    }
                    expression_expected = false;
                    continue;
                }
                match byte {
                    b'(' | b'[' | b'{' => {
                        bracket_depth += 1;
                        if bracket_depth > NESTING_LIMIT {
                            return true;
                        }
                        expression_expected = true;
                        index += 1;
                    }
                    b')' | b']' => {
                        bracket_depth = bracket_depth.saturating_sub(1);
                        expression_expected = false;
                        index += 1;
                    }
                    b'}' => {
                        bracket_depth = bracket_depth.saturating_sub(1);
                        if braces.last().copied() == Some(bracket_depth) {
                            braces.pop();
                            if modes.len() > 1 {
                                modes.pop();
                            }
                        }
                        expression_expected = false;
                        index += 1;
                    }
                    b'<' => {
                        match classify_code_angle(bytes, index, generic_depth, expression_expected)
                        {
                            Angle::Generic => {
                                generic_depth += 1;
                                if generic_depth > NESTING_LIMIT {
                                    return true;
                                }
                                expression_expected = true;
                                index += 1;
                            }
                            Angle::OpeningTag => {
                                modes.push(Mode::JsxTag {
                                    closing: false,
                                    self_closing: false,
                                });
                                index += 1;
                            }
                            Angle::ClosingTag => {
                                modes.push(Mode::JsxTag {
                                    closing: true,
                                    self_closing: false,
                                });
                                index += 2;
                            }
                            Angle::Fragment => {
                                modes.push(Mode::JsxTag {
                                    closing: false,
                                    self_closing: false,
                                });
                                index += 1;
                            }
                            Angle::Ignore => {
                                expression_expected = true;
                                index += 1;
                            }
                        }
                    }
                    b'>' => {
                        let arrow = index > 0 && bytes[index - 1] == b'=';
                        if generic_depth > 0 && !arrow {
                            generic_depth -= 1;
                        }
                        expression_expected = !arrow;
                        index += 1;
                    }
                    b'=' if bytes.get(index + 1) == Some(&b'>') => {
                        expression_expected = true;
                        index += 2;
                    }
                    b'=' => {
                        expression_expected = true;
                        index += 1;
                    }
                    b',' | b';' | b':' | b'?' | b'!' | b'&' | b'|' | b'+' | b'-' | b'*' | b'%'
                    | b'^' | b'~' => {
                        expression_expected = true;
                        index += 1;
                    }
                    b'.' => {
                        expression_expected = false;
                        index += 1;
                    }
                    _ => index += 1,
                }
            }
            Mode::JsxChildren => match byte {
                b'<' => match bytes.get(index + 1).copied() {
                    Some(b'/') => {
                        modes.push(Mode::JsxTag {
                            closing: true,
                            self_closing: false,
                        });
                        index += 2;
                    }
                    Some(b'>') => {
                        modes.push(Mode::JsxTag {
                            closing: false,
                            self_closing: false,
                        });
                        index += 1;
                    }
                    Some(next) if is_ident_start(next) => {
                        modes.push(Mode::JsxTag {
                            closing: false,
                            self_closing: false,
                        });
                        index += 1;
                    }
                    _ => index += 1,
                },
                b'{' => {
                    bracket_depth += 1;
                    if bracket_depth > NESTING_LIMIT {
                        return true;
                    }
                    braces.push(bracket_depth - 1);
                    modes.push(Mode::Code);
                    expression_expected = true;
                    index += 1;
                }
                _ => index += 1,
            },
            Mode::JsxTag { .. } => match byte {
                b'\'' | b'"' => index = skip_quoted(bytes, index, byte),
                b'{' => {
                    bracket_depth += 1;
                    if bracket_depth > NESTING_LIMIT {
                        return true;
                    }
                    braces.push(bracket_depth - 1);
                    modes.push(Mode::Code);
                    expression_expected = true;
                    index += 1;
                }
                b'/' => {
                    let mut next = index + 1;
                    while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                        next += 1;
                    }
                    if bytes.get(next) == Some(&b'>')
                        && let Some(Mode::JsxTag { self_closing, .. }) = modes.last_mut()
                    {
                        *self_closing = true;
                    }
                    index += 1;
                }
                b'>' => {
                    let tag = modes.pop().unwrap_or(Mode::Code);
                    if let Mode::JsxTag {
                        closing,
                        self_closing,
                    } = tag
                    {
                        if closing {
                            if matches!(modes.last(), Some(Mode::JsxChildren)) {
                                modes.pop();
                                jsx_depth = jsx_depth.saturating_sub(1);
                            }
                        } else if !self_closing {
                            modes.push(Mode::JsxChildren);
                            jsx_depth += 1;
                            if jsx_depth > NESTING_LIMIT {
                                return true;
                            }
                        }
                    }
                    index += 1;
                }
                _ => index += 1,
            },
        }
    }
    false
}

fn classify_code_angle(
    bytes: &[u8],
    index: usize,
    generic_depth: usize,
    expression_expected: bool,
) -> Angle {
    let next = bytes.get(index + 1).copied();
    if generic_depth > 0 {
        return if next.is_some_and(is_ident_start) {
            Angle::Generic
        } else {
            Angle::Ignore
        };
    }
    match next {
        Some(b'/') => Angle::ClosingTag,
        Some(b'>') => Angle::Fragment,
        Some(byte) if is_ident_start(byte) => {
            let Some(end) = generic_end(bytes, index) else {
                return if byte.is_ascii_lowercase() && expression_expected {
                    Angle::OpeningTag
                } else {
                    Angle::Ignore
                };
            };
            if angle_is_generic(bytes, index, end) {
                Angle::Generic
            } else {
                Angle::OpeningTag
            }
        }
        _ => Angle::Ignore,
    }
}

/// Finds the `>` balancing a generic opening angle without being fooled by
/// comments, strings, template literals, arrow `=>`, or nested angles.
fn generic_end(text: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    let limit = text.len().min(start.saturating_add(MAX_ANGLE_LOOKAHEAD));
    let mut angles = 1usize;
    let mut parens = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    while index < limit {
        let byte = text[index];
        if byte == b'/' && text.get(index + 1) == Some(&b'/') {
            index = skip_line_comment(text, index);
            continue;
        }
        if byte == b'/' && text.get(index + 1) == Some(&b'*') {
            index = skip_block_comment(text, index);
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            index = skip_quoted(text, index, byte);
            continue;
        }
        if byte == b'`' {
            index = skip_template(text, index);
            continue;
        }
        match byte {
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'<' => angles += 1,
            b'>' => {
                let arrow = index > 0 && text[index - 1] == b'=';
                if !arrow {
                    angles = angles.saturating_sub(1);
                    if angles == 0 {
                        return Some(index);
                    }
                }
            }
            b';' if angles == 1 && parens == 0 && brackets == 0 && braces == 0 => {
                return None;
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn angle_is_generic(text: &[u8], start: usize, end: usize) -> bool {
    let content = &text[start + 1..end];
    let Some(first) = content.iter().position(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    let first_byte = content[first];
    if matches!(first_byte, b'{' | b'[' | b'\'' | b'"' | b'`' | b'<' | b'(') {
        return true;
    }

    let mut parens = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    let mut angles = 0usize;
    let mut comma = false;
    let mut arrow = false;
    let mut nested = false;
    let mut union = false;
    let mut keyword = false;
    let mut question = false;
    let mut colon = false;
    let mut words = 0usize;
    let mut equals_words = None;
    let mut index = 0usize;
    while index < content.len() {
        let byte = content[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if byte == b'/' && content.get(index + 1) == Some(&b'/') {
            index = skip_line_comment(content, index);
            continue;
        }
        if byte == b'/' && content.get(index + 1) == Some(&b'*') {
            index = skip_block_comment(content, index);
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            index = skip_quoted(content, index, byte);
            continue;
        }
        if byte == b'`' {
            index = skip_template(content, index);
            continue;
        }
        if is_ident_start(byte) {
            let word_start = index;
            index += 1;
            while index < content.len() && is_ident_continue(content[index]) {
                index += 1;
            }
            if parens == 0 && brackets == 0 && braces == 0 && angles == 0 {
                words += 1;
                if is_generic_keyword(&content[word_start..index]) {
                    keyword = true;
                }
            }
            continue;
        }
        let top_level = parens == 0 && brackets == 0 && braces == 0 && angles == 0;
        match byte {
            b'(' => parens += 1,
            b')' => parens = parens.saturating_sub(1),
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'{' => braces += 1,
            b'}' => braces = braces.saturating_sub(1),
            b'<' => {
                if top_level {
                    nested = true;
                }
                angles += 1;
            }
            b'>' => angles = angles.saturating_sub(1),
            b',' if top_level => comma = true,
            b'=' if top_level => {
                if content.get(index + 1) == Some(&b'>') {
                    arrow = true;
                    index += 1;
                } else if equals_words.is_none() {
                    equals_words = Some(words);
                }
            }
            b':' if top_level => colon = true,
            b'?' if top_level => question = true,
            b'|' | b'&' if top_level => union = true,
            _ => {}
        }
        index += 1;
    }

    let strong = comma
        || arrow
        || nested
        || union
        || keyword
        || (question && colon)
        || equals_words == Some(1);
    if strong {
        return true;
    }

    let mut next = end + 1;
    while next < text.len() && text[next].is_ascii_whitespace() {
        next += 1;
    }
    let next_byte = text.get(next).copied();
    if matches!(next_byte, Some(b'<' | b'/')) {
        return false;
    }
    if matches!(
        next_byte,
        Some(
            b'(' | b'='
                | b','
                | b';'
                | b')'
                | b']'
                | b'}'
                | b'|'
                | b'&'
                | b'?'
                | b':'
                | b'.'
                | b'['
                | b'!'
                | b'>'
        )
    ) {
        return true;
    }
    if words > 1 {
        return false;
    }
    if colon && !question {
        return false;
    }

    let name_start = start + 1;
    let mut name_end = name_start;
    while name_end < end
        && !text[name_end].is_ascii_whitespace()
        && !matches!(text[name_end], b'/' | b'>')
    {
        name_end += 1;
    }
    let has_close = has_matching_closing_tag(text, next, &text[name_start..name_end]);
    if matches!(next_byte, Some(b'{')) {
        return !has_close;
    }
    if has_close {
        return false;
    }
    // Without a closing tag, a simple `<Identifier>` followed by text is a
    // type assertion or generic usage, not an unclosed JSX element. Balancing
    // it at its `>` keeps repeated flat constructs from accumulating.
    true
}

/// Looks for a matching closing tag in a bounded window. This distinguishes
/// `<div>text</div>` from the flat type assertion `<string>text` without
/// scanning the rest of a large file for the latter.
fn has_matching_closing_tag(text: &[u8], start: usize, name: &[u8]) -> bool {
    const MAX_CLOSING_TAG_LOOKAHEAD: usize = 64 * 1024;
    if name.is_empty() {
        return false;
    }
    let end = text
        .len()
        .min(start.saturating_add(MAX_CLOSING_TAG_LOOKAHEAD));
    let mut index = start;
    while index + 1 < end {
        if text[index] == b';' {
            return false;
        }
        if text[index] == b'<' && text[index + 1] == b'/' {
            let name_start = index + 2;
            let mut name_end = name_start;
            while name_end < end
                && !text[name_end].is_ascii_whitespace()
                && !matches!(text[name_end], b'>' | b'/')
            {
                name_end += 1;
            }
            if &text[name_start..name_end] == name {
                let mut after = name_end;
                while after < end && text[after].is_ascii_whitespace() {
                    after += 1;
                }
                if text.get(after) == Some(&b'>') {
                    return true;
                }
            }
            index = name_end;
            continue;
        }
        index += 1;
    }
    false
}

fn skip_line_comment(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 2;
    while index < bytes.len() && bytes[index] != b'\n' {
        index += 1;
    }
    index
}

fn skip_block_comment(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 2;
    while index + 1 < bytes.len() {
        if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            return index + 2;
        }
        index += 1;
    }
    bytes.len()
}

fn skip_quoted(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == quote {
            return index + 1;
        }
        index += 1;
    }
    bytes.len()
}

fn skip_template(bytes: &[u8], start: usize) -> usize {
    let mut index = start + 1;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == b'`' {
            return index + 1;
        }
        index += 1;
    }
    bytes.len()
}

fn skip_regex(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    let mut class = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' | b'\r' => return None,
            b'\\' => index += 2,
            b'[' if !class => {
                class = true;
                index += 1;
            }
            b']' if class => {
                class = false;
                index += 1;
            }
            b'/' if !class => {
                index += 1;
                while bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
                    index += 1;
                }
                return Some(index);
            }
            _ => index += 1,
        }
    }
    None
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

fn is_expression_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"return"
            | b"throw"
            | b"case"
            | b"delete"
            | b"typeof"
            | b"void"
            | b"new"
            | b"in"
            | b"instanceof"
            | b"yield"
            | b"await"
            | b"else"
            | b"do"
            | b"of"
    )
}

fn is_generic_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"extends"
            | b"keyof"
            | b"typeof"
            | b"infer"
            | b"readonly"
            | b"const"
            | b"as"
            | b"satisfies"
            | b"unique"
            | b"asserts"
            | b"is"
            | b"in"
            | b"out"
    )
}

#[cfg(test)]
mod tests {
    use super::nesting_exceeded;

    const FLAT: usize = 8193;

    #[test]
    fn flat_self_closing_tags_do_not_accumulate() {
        let input = format!("export const X = <div>{}</div>;\n", "<br />".repeat(FLAT));
        assert!(!nesting_exceeded(&input));
    }

    #[test]
    fn flat_generic_arrows_do_not_accumulate() {
        let input = (0..FLAT)
            .map(|index| format!("export const f{index} = <T,>(x: T): T => x;\n"))
            .collect::<String>();
        assert!(!nesting_exceeded(&input));
    }

    #[test]
    fn flat_type_assertions_and_lowercase_generic_calls_do_not_accumulate() {
        let assertions = (0..FLAT)
            .map(|index| format!("const x{index} = <string>value{index};\n"))
            .collect::<String>();
        assert!(!nesting_exceeded(&assertions));

        let calls = (0..FLAT)
            .map(|index| format!("const x{index} = JSON.parse<string>(\"{{}}\");\n"))
            .collect::<String>();
        assert!(!nesting_exceeded(&calls));
    }

    #[test]
    fn nested_jsx_respects_the_limit() {
        let allowed = nested_jsx(8192);
        assert!(!nesting_exceeded(&allowed));
        let rejected = nested_jsx(8193);
        assert!(nesting_exceeded(&rejected));
    }

    #[test]
    fn nested_parentheses_respect_the_limit() {
        let allowed = format!("const x = {}1{};\n", "(".repeat(8192), ")".repeat(8192));
        assert!(!nesting_exceeded(&allowed));
        let rejected = format!("const x = {}1{};\n", "(".repeat(8193), ")".repeat(8193));
        assert!(nesting_exceeded(&rejected));
    }

    #[test]
    fn tags_inside_comments_are_ignored() {
        let tags = "<div>".repeat(FLAT);
        assert!(!nesting_exceeded(&format!("// {tags}\nconst x = 1;\n")));
        assert!(!nesting_exceeded(&format!("/* {tags} */\nconst x = 1;\n")));
    }

    #[test]
    fn tags_inside_strings_and_templates_are_ignored() {
        let tags = "<div>".repeat(FLAT);
        assert!(!nesting_exceeded(&format!("const value = \"{tags}\";\n")));
        assert!(!nesting_exceeded(&format!("const value = '{tags}';\n")));
        assert!(!nesting_exceeded(&format!("const value = `{tags}`;\n")));
    }

    #[test]
    fn tags_inside_regexes_are_ignored() {
        let tags = "<div>".repeat(FLAT);
        let input = format!("const pattern = /{tags}/;\n");
        assert!(!nesting_exceeded(&input));
    }

    #[test]
    fn nested_generic_angles_are_rejected() {
        let mut input = String::from("type X = ");
        input.push_str(&"A<".repeat(8193));
        input.push('B');
        input.push_str(&">".repeat(8193));
        input.push_str(";\n");
        assert!(nesting_exceeded(&input));
    }

    fn nested_jsx(depth: usize) -> String {
        let mut input = String::from("export const X = ");
        input.push_str(&"<div>".repeat(depth));
        input.push('x');
        input.push_str(&"</div>".repeat(depth));
        input.push_str(";\n");
        input
    }
}
