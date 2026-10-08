#![forbid(unsafe_code)]

use dotfmt_core::syntax::{QuoteMode, scan};

#[test]
fn source_spans_preserve_unicode_and_escaped_trailing_whitespace() {
    let source = "配置\\ } café\\ ";
    let tokens: Vec<_> = scan(source, QuoteMode::Anywhere).collect();
    let reconstructed: String = tokens
        .iter()
        .map(|token| token.span.slice(source))
        .collect();
    assert_eq!(reconstructed, source);
    assert!(tokens.last().unwrap().escaped);
    assert!(!tokens.last().unwrap().is_structural());
}

#[test]
fn quoted_and_escaped_braces_cannot_close_a_block() {
    let source = r#"value = "a\" } b" '\'' \} }"#;
    let closes: Vec<_> = scan(source, QuoteMode::Anywhere)
        .filter(|token| token.character == '}' && token.is_structural())
        .map(|token| token.span.start)
        .collect();
    assert_eq!(closes, [source.len() - 1]);
}

#[test]
fn pattern_quotes_only_open_at_the_start_of_a_pattern() {
    let quoted = r#""file } name" }"#;
    let literal = r#"file"name }"#;
    for source in [quoted, literal] {
        let closes: Vec<_> = scan(source, QuoteMode::Start)
            .filter(|token| token.character == '}' && token.is_structural())
            .map(|token| token.span.start)
            .collect();
        assert_eq!(closes, [source.len() - 1]);
    }
}
