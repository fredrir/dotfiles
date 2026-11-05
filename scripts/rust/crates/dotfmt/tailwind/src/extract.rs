//! Cheap class candidates for building a project order table.
//!
//! This deliberately over-collects: the bridge treats unknown strings as
//! unknown classes, so false positives only widen the cached order table. It is
//! not used to decide *where* classes are sorted; oxc/markup/css own that.

use std::collections::BTreeSet;

use crate::sorter::split_classes;

/// Collect candidate class tokens from TypeScript, CSS or markup source.
pub fn candidates(source: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    for text in quoted_text(source) {
        for class in split_classes(&text) {
            if plausible(class) {
                found.insert(class.to_owned());
            }
        }
    }
    for class in apply_classes(source) {
        if plausible(class) {
            found.insert(class.to_owned());
        }
    }
    for class in selector_classes(source) {
        if plausible(class) {
            found.insert(class.to_owned());
        }
    }
    found.into_iter().collect()
}

/// Contents of single, double and backtick strings, with escapes respected.
fn quoted_text(source: &str) -> Vec<String> {
    let mut texts = Vec::new();
    let mut chars = source.char_indices().peekable();
    while let Some((_, character)) = chars.next() {
        if !matches!(character, '\'' | '"' | '`') {
            continue;
        }
        let quote = character;
        let mut text = String::new();
        let mut escaped = false;
        for (_, character) in chars.by_ref() {
            if escaped {
                text.push(character);
                escaped = false;
                continue;
            }
            if character == '\\' {
                escaped = true;
                continue;
            }
            if character == quote {
                break;
            }
            text.push(character);
        }
        texts.push(text);
    }
    texts
}

fn apply_classes(source: &str) -> Vec<&str> {
    let mut classes = Vec::new();
    let mut remaining = source;
    while let Some(index) = remaining.find("@apply") {
        let after = &remaining[index + "@apply".len()..];
        let end = after.find([';', '}', '\n']).unwrap_or(after.len());
        classes.extend(split_classes(&after[..end]));
        remaining = &after[end..];
    }
    classes
}

fn selector_classes(source: &str) -> Vec<&str> {
    let mut classes = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'.' || index > 0 && is_ident(bytes[index - 1]) {
            index += 1;
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && is_class_byte(bytes[end]) {
            end += 1;
        }
        if end > start && !bytes[start].is_ascii_digit() {
            classes.push(&source[start..end]);
        }
        index = end.max(index + 1);
    }
    classes
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'$')
}

fn is_class_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'_' | b'-'
                | b':'
                | b'/'
                | b'['
                | b']'
                | b'('
                | b')'
                | b'!'
                | b'%'
                | b'#'
                | b','
                | b'.'
        )
}

fn plausible(class: &str) -> bool {
    !class.is_empty()
        && class.len() <= 512
        && !class.contains("://")
        && !class.contains(['<', '>', '=', ';', '{', '}'])
        && class
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tsx_and_arbitrary_values() {
        let source = r#"<div className={cn("p-4 flex", ok && "bg-[url('/a b.svg')]")} />"#;
        let found = candidates(source);
        assert!(found.contains(&"p-4".to_owned()));
        assert!(found.contains(&"flex".to_owned()));
        assert!(found.contains(&"bg-[url('/a b.svg')]".to_owned()));
    }

    #[test]
    fn finds_apply_and_selectors_in_css() {
        let source = ".btn { @apply p-4 flex items-center; }\n.card:hover { color: red; }";
        let found = candidates(source);
        for class in ["p-4", "flex", "items-center", "btn", "card:hover"] {
            assert!(found.contains(&class.to_owned()), "{class}: {found:?}");
        }
    }

    #[test]
    fn ignores_urls_and_code_fragments() {
        let source = r#"const url = "https://example.com"; const f = "x = 1";"#;
        let found = candidates(source);
        assert!(!found.iter().any(|class| class.contains("://")));
        assert!(!found.iter().any(|class| class.contains('=')));
    }
}
