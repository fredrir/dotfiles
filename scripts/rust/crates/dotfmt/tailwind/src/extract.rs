
use std::collections::BTreeSet;

use crate::sorter::split_classes;

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
        let end = apply_end(after);
        classes.extend(split_classes(&after[..end]));
        remaining = &after[end..];
    }
    classes
}

fn apply_end(text: &str) -> usize {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (offset, character) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(current) = quote {
            if character == current {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' if depth > 0 => depth = depth.saturating_sub(1),
            ';' | '}' | '\n' if depth == 0 => return offset,
            _ => {}
        }
    }
    text.len()
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
    if class.is_empty()
        || class.len() > 512
        || class.contains("://")
        || class.chars().any(char::is_control)
        || !class
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
    {
        return false;
    }

    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for character in class.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(current) = quote {
            if character == current {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' | '`' => quote = Some(character),
            '[' | '(' => stack.push(character),
            '{' if !stack.is_empty() => stack.push(character),
            ']' | ')' | '}' => {
                let Some(open) = stack.pop() else {
                    return false;
                };
                if !matches!((open, character), ('[', ']') | ('(', ')') | ('{', '}')) {
                    return false;
                }
            }
            '=' | ';' | '<' | '>' if stack.is_empty() => return false,
            '{' if stack.is_empty() => return false,
            _ => {}
        }
    }
    !escaped && quote.is_none() && stack.is_empty()
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
        let source = r#".btn { @apply p-4 flex items-center [&>svg]:size-4 data-[state=open]:bg-accent [content:'a;b'] content-['{hello world}']; }
.card:hover { color: red; }"#;
        let found = candidates(source);
        for class in [
            "p-4",
            "flex",
            "items-center",
            "[&>svg]:size-4",
            "data-[state=open]:bg-accent",
            "[content:'a;b']",
            "content-['{hello world}']",
            "btn",
            "card:hover",
        ] {
            assert!(found.contains(&class.to_owned()), "{class}: {found:?}");
        }
    }

    #[test]
    fn finds_arbitrary_variants_with_bracketed_punctuation() {
        let source = r#"
            <div className="[&>svg]:size-4 [&>*]:p-4 data-[state=open]:bg-accent aria-[sort=ascending] group-data-[collapsible=icon]:hidden content-['{hello world}']" />
            <span className={cn("hover:[mask-type:luminance]")} />
        "#;
        let found = candidates(source);
        for class in [
            "[&>svg]:size-4",
            "[&>*]:p-4",
            "data-[state=open]:bg-accent",
            "aria-[sort=ascending]",
            "group-data-[collapsible=icon]:hidden",
            "content-['{hello world}']",
            "hover:[mask-type:luminance]",
        ] {
            assert!(found.contains(&class.to_owned()), "{class}: {found:?}");
        }
    }

    #[test]
    fn ignores_urls_and_code_fragments() {
        let source = r#"const url = "https://example.com"; const f = "x = 1"; const g = "p-4";"#;
        let found = candidates(source);
        assert!(!found.iter().any(|class| class.contains("://")));
        assert!(!found.iter().any(|class| class.contains('=')));
        assert!(found.contains(&"p-4".to_owned()));
    }

    #[test]
    fn rejects_unbalanced_delimiters_quotes_and_controls() {
        for class in [
            "data-[state=open",
            "data-state=open]",
            "[&>svg]]:size-4",
            "[&>svg:size-4",
            "content-['{hello world}]",
            "foo=bar",
            ";flex",
            "p-\u{7}4",
            "\u{0}",
        ] {
            assert!(!plausible(class), "{class:?}");
        }
        for class in [
            "[&>svg]:size-4",
            "data-[state=open]:bg-accent",
            "aria-[sort=ascending]",
            "content-['{hello world}']",
            "bg-[url('/a b.svg')]",
            r"c\[d\ e\]",
        ] {
            assert!(plausible(class), "{class:?}");
        }
    }
}
