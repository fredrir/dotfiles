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
        if plausible(&class) {
            found.insert(class);
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
    let mut previous = None;
    while let Some((_, character)) = chars.next() {
        if character == '/' && matches!(chars.peek(), Some((_, '*'))) {
            chars.next();
            skip_block_comment(&mut chars);
            previous = Some('/');
            continue;
        }
        if character == '/'
            && matches!(chars.peek(), Some((_, '/')))
            && line_comment_allowed(previous)
        {
            chars.next();
            skip_line_comment(&mut chars);
            previous = Some('\n');
            continue;
        }
        if matches!(character, '\'' | '"' | '`') && (character == '`' || quote_boundary(previous)) {
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
            previous = Some(quote);
            continue;
        }
        previous = Some(character);
    }
    texts
}

fn skip_block_comment(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) {
    while let Some((_, character)) = chars.next() {
        if character == '*' && matches!(chars.peek(), Some((_, '/'))) {
            chars.next();
            return;
        }
    }
}

fn skip_line_comment(chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>) {
    for (_, character) in chars.by_ref() {
        if character == '\n' {
            return;
        }
    }
}

fn quote_boundary(previous: Option<char>) -> bool {
    previous.is_none_or(|character| {
        character.is_whitespace()
            || matches!(
                character,
                '=' | '(' | '[' | '{' | ',' | ':' | '!' | '&' | '|' | '?' | '>' | ';' | '+'
            )
    })
}

fn line_comment_allowed(previous: Option<char>) -> bool {
    !matches!(previous, Some(':' | '/' | '\\' | '('))
}

fn apply_classes(source: &str) -> Vec<String> {
    let mut classes = Vec::new();
    let mut offset = 0;
    let mut previous = None;
    while offset < source.len() {
        let rest = &source[offset..];
        if rest.starts_with("/*") {
            offset = rest.find("*/").map_or(source.len(), |end| offset + end + 2);
            previous = Some('/');
            continue;
        }
        if rest.starts_with("//") && line_comment_allowed(previous) {
            offset = rest.find('\n').map_or(source.len(), |end| offset + end + 1);
            previous = Some('\n');
            continue;
        }
        let character = rest.chars().next().expect("offset is in bounds");
        if matches!(character, '\'' | '"' | '`') {
            offset = string_end(source, offset, character);
            previous = Some(character);
            continue;
        }
        if let Some(after) = rest.strip_prefix("@apply") {
            let end = apply_end(after);
            classes.extend(apply_prelude_classes(&after[..end]));
            offset += "@apply".len() + end;
            previous = Some('y');
            continue;
        }
        offset += character.len_utf8();
        previous = Some(character);
    }
    classes
}

fn apply_prelude_classes(text: &str) -> Vec<String> {
    let mut classes = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut chars = text.char_indices().peekable();
    let mut previous = None;
    while let Some((_, character)) = chars.next() {
        if escaped {
            current.push(character);
            escaped = false;
            previous = Some(character);
            continue;
        }
        if character == '\\' {
            current.push(character);
            escaped = true;
            previous = Some(character);
            continue;
        }
        if let Some(active) = quote {
            current.push(character);
            if character == active {
                quote = None;
            }
            previous = Some(character);
            continue;
        }
        match character {
            '/' if matches!(chars.peek(), Some((_, '*'))) => {
                chars.next();
                skip_block_comment(&mut chars);
                flush_apply_class(&mut classes, &mut current, depth);
                previous = Some('/');
            }
            '/' if matches!(chars.peek(), Some((_, '/'))) && line_comment_allowed(previous) => {
                chars.next();
                skip_line_comment(&mut chars);
                flush_apply_class(&mut classes, &mut current, depth);
                previous = Some('\n');
            }
            '\'' | '"' | '`' => {
                current.push(character);
                quote = Some(character);
                previous = Some(character);
            }
            '[' | '(' | '{' => {
                current.push(character);
                depth += 1;
                previous = Some(character);
            }
            ']' | ')' | '}' if depth > 0 => {
                current.push(character);
                depth = depth.saturating_sub(1);
                previous = Some(character);
            }
            character if character.is_whitespace() && depth == 0 => {
                flush_apply_class(&mut classes, &mut current, depth);
                previous = Some(character);
            }
            character => {
                current.push(character);
                previous = Some(character);
            }
        }
    }
    flush_apply_class(&mut classes, &mut current, depth);
    classes
}

fn flush_apply_class(classes: &mut Vec<String>, current: &mut String, depth: usize) {
    if depth == 0 && !current.is_empty() {
        classes.push(std::mem::take(current));
    }
}

fn string_end(source: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    let body_start = start + quote.len_utf8();
    for (relative, character) in source[body_start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == quote {
            return body_start + relative + character.len_utf8();
        }
    }
    source.len()
}

fn apply_end(text: &str) -> usize {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut previous = None;
    let mut chars = text.char_indices().peekable();
    while let Some((offset, character)) = chars.next() {
        if escaped {
            escaped = false;
            previous = Some(character);
            continue;
        }
        if character == '\\' {
            escaped = true;
            previous = Some(character);
            continue;
        }
        if let Some(current) = quote {
            if character == current {
                quote = None;
            }
            previous = Some(character);
            continue;
        }
        match character {
            '/' if matches!(chars.peek(), Some((_, '*'))) => {
                chars.next();
                skip_block_comment(&mut chars);
                previous = Some('/');
            }
            '/' if matches!(chars.peek(), Some((_, '/'))) && line_comment_allowed(previous) => {
                chars.next();
                skip_line_comment(&mut chars);
                previous = Some('\n');
            }
            '\'' | '"' | '`' => {
                quote = Some(character);
                previous = Some(character);
            }
            '[' | '(' | '{' => {
                depth += 1;
                previous = Some(character);
            }
            ']' | ')' | '}' if depth > 0 => {
                depth = depth.saturating_sub(1);
                previous = Some(character);
            }
            ';' | '}' if depth == 0 => return offset,
            _ => previous = Some(character),
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
    fn finds_wrapped_multiline_apply_lists() {
        let found = candidates(
            ".a { @apply p-4\n flex m-2; }\n.b { @apply bg-[url('/a b.svg')] content-['a;b'] font-bold }",
        );
        for class in ["p-4", "flex", "m-2", "bg-[url('/a b.svg')]", "font-bold"] {
            assert!(found.contains(&class.to_owned()), "{class}: {found:?}");
        }
    }

    #[test]
    fn apply_ignores_comments_strings_and_nested_delimiters() {
        let found = apply_classes(
            r#"
.a { @apply p-4 /* ; } */ flex /* @apply p-9 */ m-2; }
.b { content: "@apply p-8"; }
.c { @apply bg-[url('a;b')] content-['x;y'] font-bold }
.d { @apply bg-[url(//cdn.example/x.svg)] p-4; }
"#,
        );
        for class in [
            "p-4",
            "flex",
            "m-2",
            "bg-[url('a;b')]",
            "font-bold",
            "bg-[url(//cdn.example/x.svg)]",
        ] {
            assert!(found.contains(&class.to_owned()), "{class}: {found:?}");
        }
        for class in ["p-9", "p-8"] {
            assert!(!found.contains(&class.to_owned()), "{class}: {found:?}");
        }
    }

    #[test]
    fn contractions_in_comments_and_jsx_text_do_not_swallow_class_lists() {
        for source in [
            "// don't\nclassName=\"p-4 flex m-2\"",
            "/* don't */ const x = <div className=\"p-4 flex m-2\"/>;",
            "const el = <p>Don't <div className=\"p-4 flex m-2\"/></p>;",
        ] {
            let found = candidates(source);
            for class in ["p-4", "flex", "m-2"] {
                assert!(
                    found.contains(&class.to_owned()),
                    "{class}: {source}: {found:?}"
                );
            }
        }
    }

    #[test]
    fn keeps_strings_next_to_operators_and_tagged_templates() {
        let source = r#"
const a = "p-4" + 'flex';
const b = ok && "m-2";
const c = maybe ? "block" : "hidden";
const t = tw`bg-red-500 items-center`;
"#;
        let found = candidates(source);
        for class in [
            "p-4",
            "flex",
            "m-2",
            "block",
            "hidden",
            "bg-red-500",
            "items-center",
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
