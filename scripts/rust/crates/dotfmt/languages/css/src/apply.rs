use dotfmt_tailwind::Sorter;

use crate::config::{IGNORE_COMMENT_DIRECTIVE, IGNORE_FILE_COMMENT_DIRECTIVE};
use crate::dialect::Dialect;

/// Sorts class lists in `@apply` at-rules after the CSS printer has laid them out.
///
/// The scanner only rewrites an `@apply` prelude when the whole prelude is a
/// plain class list (plus an optional trailing `!important` and trailing
/// comments). Interpolation, variables, interior comments, and other constructs
/// whose class expansion is unknown are left exactly as the printer wrote them.
pub(crate) fn sort_apply(css: &str, sorter: &Sorter, dialect: Dialect) -> String {
    if file_is_ignored(css, dialect) {
        return css.to_owned();
    }

    let mut output = String::with_capacity(css.len());
    let mut copied = 0;
    let mut cursor = 0;
    let mut depth = 0usize;
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut statement_start = true;
    let mut skip_statement = None;

    while cursor < css.len() {
        let rest = &css[cursor..];
        let character = rest.chars().next().expect("cursor is in bounds");

        if rest.starts_with("/*") {
            let Some(end) = block_comment_end(css, cursor) else {
                break;
            };
            if skip_statement.is_none() && is_directive(&css[cursor..end], IGNORE_COMMENT_DIRECTIVE)
            {
                if dialect == Dialect::Sass {
                    output.push_str(&css[copied..]);
                    return output;
                }
                skip_statement = Some(depth);
            }
            cursor = end;
            continue;
        }

        if rest.starts_with("//") && dialect != Dialect::Css && parentheses == 0 && brackets == 0 {
            let end = css[cursor..]
                .find('\n')
                .map_or(css.len(), |offset| cursor + offset);
            if skip_statement.is_none() && is_directive(&css[cursor..end], IGNORE_COMMENT_DIRECTIVE)
            {
                if dialect == Dialect::Sass {
                    output.push_str(&css[copied..]);
                    return output;
                }
                skip_statement = Some(depth);
            }
            cursor = end;
            continue;
        }

        if character == '"' || character == '\'' {
            cursor = string_end(css, cursor);
            statement_start = false;
            continue;
        }

        if is_interpolation(rest, dialect) {
            cursor = interpolation_end(css, cursor);
            statement_start = false;
            continue;
        }

        if character == '\\' {
            cursor += character.len_utf8();
            if let Some(escaped) = css[cursor..].chars().next() {
                cursor += escaped.len_utf8();
            }
            statement_start = false;
            continue;
        }

        if rest.starts_with("@apply")
            && at_keyword_boundary(rest)
            && statement_start
            && skip_statement.is_none()
        {
            let prelude_start = cursor + "@apply".len();
            let (prelude_end, prelude) = apply_prelude(css, prelude_start, dialect);
            if let Some(formatted) = format_prelude(prelude, sorter, dialect) {
                output.push_str(&css[copied..cursor]);
                output.push_str("@apply");
                if !formatted.is_empty() {
                    output.push(' ');
                    output.push_str(&formatted);
                }
                copied = prelude_end;
            }
            cursor = prelude_end;
            statement_start = false;
            continue;
        }

        match character {
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            '{' => {
                depth += 1;
                statement_start = true;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if skip_statement.is_some_and(|base| depth <= base) {
                    skip_statement = None;
                }
                statement_start = true;
            }
            ';' => {
                if skip_statement.is_some_and(|base| depth <= base) {
                    skip_statement = None;
                }
                statement_start = true;
            }
            '\n' if dialect == Dialect::Sass => {
                if skip_statement.is_some_and(|base| depth <= base) {
                    skip_statement = None;
                }
                statement_start = true;
            }
            _ if character.is_whitespace() => {}
            _ => statement_start = false,
        }
        cursor += character.len_utf8();
    }

    output.push_str(&css[copied..]);
    output
}

fn file_is_ignored(css: &str, dialect: Dialect) -> bool {
    if css.starts_with("/*") {
        return block_comment_end(css, 0)
            .is_some_and(|end| is_directive(&css[..end], IGNORE_FILE_COMMENT_DIRECTIVE));
    }
    if dialect != Dialect::Css && css.starts_with("//") {
        let end = css.find('\n').unwrap_or(css.len());
        return is_directive(&css[..end], IGNORE_FILE_COMMENT_DIRECTIVE);
    }
    false
}

fn is_directive(comment: &str, directive: &str) -> bool {
    let content = comment
        .strip_prefix("/*")
        .and_then(|content| content.strip_suffix("*/"))
        .or_else(|| comment.strip_prefix("//"))
        .unwrap_or(comment);
    content
        .trim_start()
        .strip_prefix(directive)
        .is_some_and(|rest| {
            rest.is_empty() || rest.starts_with(|character: char| character.is_ascii_whitespace())
        })
}

pub(crate) fn block_comment_end(css: &str, start: usize) -> Option<usize> {
    css[start + 2..]
        .find("*/")
        .map(|offset| start + 2 + offset + 2)
}

pub(crate) fn string_end(css: &str, start: usize) -> usize {
    let quote = css[start..]
        .chars()
        .next()
        .expect("string starts in bounds");
    let mut escaped = false;
    for (offset, character) in css[start + quote.len_utf8()..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == quote {
            return start + quote.len_utf8() + offset + quote.len_utf8();
        }
    }
    css.len()
}

fn is_interpolation(rest: &str, dialect: Dialect) -> bool {
    match dialect {
        Dialect::Scss | Dialect::Sass => rest.starts_with("#{"),
        Dialect::Less => rest.starts_with("@{"),
        Dialect::Auto | Dialect::Css => false,
    }
}

fn interpolation_end(css: &str, start: usize) -> usize {
    let mut depth = 0usize;
    for (offset, character) in css[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return start + offset + 1;
                }
            }
            _ => {}
        }
    }
    css.len()
}

/// Finds the end of an at-rule prelude and returns it without its terminator.
fn apply_prelude(css: &str, start: usize, dialect: Dialect) -> (usize, &str) {
    let mut cursor = start;
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    while cursor < css.len() {
        let rest = &css[cursor..];
        let character = rest.chars().next().expect("cursor is in bounds");
        if rest.starts_with("/*") {
            let Some(end) = block_comment_end(css, cursor) else {
                return (css.len(), &css[start..]);
            };
            cursor = end;
            continue;
        }
        if rest.starts_with("//")
            && dialect != Dialect::Css
            && parentheses == 0
            && brackets == 0
            && braces == 0
        {
            cursor = css[cursor..]
                .find('\n')
                .map_or(css.len(), |offset| cursor + offset);
            continue;
        }
        if character == '"' || character == '\'' {
            cursor = string_end(css, cursor);
            continue;
        }
        if is_interpolation(rest, dialect) {
            cursor = interpolation_end(css, cursor);
            continue;
        }
        if character == '\\' {
            cursor += character.len_utf8();
            if let Some(escaped) = css[cursor..].chars().next() {
                cursor += escaped.len_utf8();
            }
            continue;
        }
        if parentheses == 0 && brackets == 0 && braces == 0 {
            match character {
                ';' | '}' => return (cursor, &css[start..cursor]),
                '\n' if dialect == Dialect::Sass => return (cursor, &css[start..cursor]),
                _ => {}
            }
        }
        match character {
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            '{' => braces += 1,
            '}' => braces = braces.saturating_sub(1),
            _ => {}
        }
        cursor += character.len_utf8();
    }
    (css.len(), &css[start..])
}

fn format_prelude(prelude: &str, sorter: &Sorter, dialect: Dialect) -> Option<String> {
    let prelude = prelude.trim();
    if prelude.is_empty() || is_uncertain(prelude, dialect) {
        return None;
    }
    let (classes, trailing) = split_trailing_comments(prelude, dialect)?;
    let (classes, important) = split_important(classes);
    if classes.is_empty() {
        return None;
    }
    let sorted = sorter.sort_list(classes);
    if sorted.is_empty() {
        return None;
    }
    let mut output = sorted;
    if let Some(important) = important {
        output.push(' ');
        output.push_str(important);
    }
    if let Some(trailing) = trailing {
        output.push(' ');
        output.push_str(trailing);
    }
    Some(output)
}

fn is_uncertain(prelude: &str, dialect: Dialect) -> bool {
    let mut cursor = 0;
    while cursor < prelude.len() {
        let rest = &prelude[cursor..];
        let character = rest.chars().next().expect("cursor is in bounds");
        if rest.starts_with("/*") || (rest.starts_with("//") && dialect != Dialect::Css) {
            cursor = if rest.starts_with("/*") {
                let Some(end) = block_comment_end(prelude, cursor) else {
                    return true;
                };
                end
            } else {
                prelude[cursor..]
                    .find('\n')
                    .map_or(prelude.len(), |offset| cursor + offset)
            };
            continue;
        }
        if character == '"' || character == '\'' {
            cursor = string_end(prelude, cursor);
            continue;
        }
        if character == '\\' {
            cursor += character.len_utf8();
            if let Some(escaped) = prelude[cursor..].chars().next() {
                cursor += escaped.len_utf8();
            }
            continue;
        }
        match dialect {
            Dialect::Scss | Dialect::Sass if rest.starts_with("#{") => return true,
            Dialect::Less if rest.starts_with("@{") => return true,
            _ => {}
        }
        if (dialect != Dialect::Css && character == '$')
            || (dialect == Dialect::Less && character == '@')
        {
            return true;
        }
        cursor += character.len_utf8();
    }
    false
}

/// Splits a prelude into class text and trailing comments.
///
/// Any comment before the end of the class text makes the prelude ambiguous.
fn split_trailing_comments(prelude: &str, dialect: Dialect) -> Option<(&str, Option<&str>)> {
    let mut cursor = 0;
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut first_comment = None;
    while cursor < prelude.len() {
        let rest = &prelude[cursor..];
        let character = rest.chars().next().expect("cursor is in bounds");
        if character == '\\' {
            cursor += character.len_utf8();
            if let Some(escaped) = prelude[cursor..].chars().next() {
                cursor += escaped.len_utf8();
            }
            continue;
        }
        if rest.starts_with("/*") || (rest.starts_with("//") && dialect != Dialect::Css) {
            if parentheses != 0 || brackets != 0 {
                return None;
            }
            let end = if rest.starts_with("/*") {
                block_comment_end(prelude, cursor)?
            } else {
                prelude[cursor..]
                    .find('\n')
                    .map_or(prelude.len(), |offset| cursor + offset)
            };
            first_comment.get_or_insert(cursor);
            cursor = end;
            continue;
        }
        if first_comment.is_some() && !character.is_whitespace() {
            return None;
        }
        if character == '"' || character == '\'' {
            cursor = string_end(prelude, cursor);
            continue;
        }
        match character {
            '(' => parentheses += 1,
            ')' => parentheses = parentheses.saturating_sub(1),
            '[' => brackets += 1,
            ']' => brackets = brackets.saturating_sub(1),
            _ => {}
        }
        cursor += character.len_utf8();
    }
    match first_comment {
        Some(index) => {
            let trailing = prelude[index..].trim();
            if dialect != Dialect::Sass && trailing.contains("//") {
                return None;
            }
            Some((prelude[..index].trim_end(), Some(trailing)))
        }
        None => Some((prelude, None)),
    }
}

fn split_important(text: &str) -> (&str, Option<&str>) {
    let text = text.trim_end();
    if text.is_empty() {
        return (text, None);
    }
    let lower = text.to_ascii_lowercase();
    if lower.ends_with("!important") {
        let index = text.len() - "!important".len();
        if index == 0 || text[..index].ends_with(|character: char| character.is_whitespace()) {
            return (text[..index].trim_end(), Some(&text[index..]));
        }
    }
    if lower.ends_with("important") {
        let index = text.len() - "important".len();
        let before = text[..index].trim_end();
        if let Some(bang) = before.strip_suffix('!') {
            let bang_index = bang.len();
            if bang_index == 0
                || bang[..bang_index].ends_with(|character: char| character.is_whitespace())
            {
                return (bang.trim_end(), Some(&text[bang_index..]));
            }
        }
    }
    (text, None)
}

fn at_keyword_boundary(rest: &str) -> bool {
    rest["@apply".len()..]
        .chars()
        .next()
        .is_none_or(|character| {
            !character.is_alphanumeric()
                && character != '_'
                && character != '-'
                && character != '\\'
        })
}
