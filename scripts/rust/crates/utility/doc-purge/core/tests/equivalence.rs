#![forbid(unsafe_code)]

//! Scan invariants that hold now that the curly dialects run on their own
//! engine (SPEC §3):
//!
//! * every `Engine::Curly` and `Engine::Generic` descriptor's scan reaches a
//!   fixed point: removing its comment spans and scanning again finds no
//!   comments, and no non-whitespace byte outside those spans is lost;
//! * every `Engine::Generic` descriptor also reports the same comment spans on
//!   a second run over the same comment/string corpus.
//!
//! The two engines are intentionally different now — a descriptor cannot
//! express the Rust lifetime rule, C++ raw-tag or C# verbatim prefixes — so
//! there is no byte-for-byte cross-engine comparison left to make.
//!
//! `engine` and `curly` are private modules, so `core/src/lib.rs` exposes them
//! through `#[doc(hidden)] pub mod bridge` (`generic_scan`, `curly_scan`, and
//! the `Out`/`Bail` types they return). Nothing in `core/` is changed by the
//! test owner: if the bridge is renamed, only the `use` below moves.

use doc_purge_core::bridge::{Bail, Comment, Out, curly_scan, generic_scan};
use doc_purge_core::lang::Dialect;
use doc_purge_core::syntax::Engine;

/// Sources that exercise every lexical decision the scanners make in practice:
/// markers inside strings, raw and prefixed strings, nesting, continuations,
/// CRLF/CR/U+2028 terminators and glyph runs.
const SOURCES: &[&str] = &[
    "let a = 1; // note\n",
    "// note\nlet a = 1;\n",
    "let a = \"// not a comment\";\n",
    "let a = \"/* not a comment */\";\n",
    "let a = 'x'; // note\n",
    "/* one */ let a = 1;\n",
    "a /* b */ c\n",
    "/* a /* b */ c */\n",
    "let a = 1; /* one\ntwo */ let b = 2;\n",
    "let a = 1; // one \\\n still a comment\n",
    "let a = 1;\r\n// note\r\nlet b = 2;\r\n",
    "let a = 1;\r// note\rlet b = 2;\r",
    "let a = \"caf\u{e9}\"; // na\u{ef}ve\n",
    "let a = \"\u{1f680}\"; // note\n",
    "let a = \"\u{201c}smart\u{201d}\"; // note\n",
    "let a = 1;\u{2028}// note\n",
    "let s = r\"raw // not a comment\";\n",
    "let s = r#\"raw \"quoted\" // x\"#;\n",
    "let s = b\"bytes // x\";\n",
    "let a = \"unterminated\n",
    "/* unterminated\n",
    "\u{feff}// note\nlet a = 1;\n",
    "@\"verbatim // x\";\n",
    "#\"raw // x\"#;\n",
    "`template // x ${y} z`;\n",
    "",
    "\n\n",
    "let a = 1;\n\n\nlet b = 2; // note\n",
];

fn comment_spans(out: &Out) -> Vec<(usize, usize)> {
    out.comments
        .iter()
        .map(|comment| (comment.span.start, comment.span.end))
        .collect()
}

/// Mark every byte a comment span covers, asserting the spans are in bounds,
/// ordered and non-overlapping first. A malformed span would otherwise hide
/// whichever code byte it swallowed.
fn comment_mask(source: &[u8], comments: &[Comment]) -> Vec<bool> {
    let mut gone = vec![false; source.len()];
    let mut previous_end = 0usize;
    for (index, comment) in comments.iter().enumerate() {
        assert!(
            comment.span.start <= comment.span.end,
            "comment {index} is backwards: {}..{}",
            comment.span.start,
            comment.span.end
        );
        assert!(
            comment.span.end <= source.len(),
            "comment {index} ends at {}, past the end of the {} byte source",
            comment.span.end,
            source.len()
        );
        assert!(
            comment.span.start >= previous_end,
            "comment {index} {}..{} overlaps or is out of order (previous ended at {previous_end})",
            comment.span.start,
            comment.span.end
        );
        for byte in &mut gone[comment.span.clone()] {
            assert!(!*byte, "comment {index} overlaps an earlier comment");
            *byte = true;
        }
        previous_end = comment.span.end;
    }
    gone
}

fn without_comments(source: &[u8], comments: &[Comment]) -> Vec<u8> {
    let gone = comment_mask(source, comments);
    source
        .iter()
        .zip(gone)
        .filter_map(|(byte, gone)| (!gone).then_some(*byte))
        .collect()
}

fn non_whitespace(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect()
}

fn non_whitespace_outside(source: &[u8], comments: &[Comment]) -> Vec<u8> {
    let gone = comment_mask(source, comments);
    source
        .iter()
        .zip(gone)
        .filter(|(byte, gone)| !*gone && !byte.is_ascii_whitespace())
        .map(|(byte, _)| *byte)
        .collect()
}

#[test]
fn both_scan_engines_are_idempotent_and_keep_code_outside_comments() {
    let mut curly_checked = 0usize;
    let mut curly_dialects: Vec<&'static str> = Vec::new();
    let mut generic_checked = 0usize;
    let mut generic_dialects: Vec<&'static str> = Vec::new();

    for dialect in Dialect::ALL.iter().copied() {
        let syntax = dialect.syntax();
        match dialect.engine() {
            Engine::Curly => {
                curly_dialects.push(syntax.name);
                for (index, source) in SOURCES.iter().enumerate() {
                    // The corpus deliberately carries unterminated strings and
                    // comments: a scanner that refuses them has nothing to keep
                    // or re-scan for that source.
                    let Ok(first) = curly_scan(source.as_bytes(), syntax) else {
                        continue;
                    };
                    let content = without_comments(source.as_bytes(), &first.comments);
                    assert_eq!(
                        non_whitespace_outside(source.as_bytes(), &first.comments),
                        non_whitespace(&content),
                        "{} ({dialect:?}): removing comment spans lost non-whitespace on source {index}: {source:?}",
                        syntax.name
                    );
                    match curly_scan(&content, syntax) {
                        Ok(second) => assert!(
                            second.comments.is_empty(),
                            "{} ({dialect:?}): a second scan still finds comments on source {index}: {source:?}\n--- after removing comments ---\n{:?}",
                            syntax.name,
                            String::from_utf8_lossy(&content)
                        ),
                        Err(Bail(reason)) => panic!(
                            "{} ({dialect:?}): the source no longer reads after removing comments on source {index} ({reason}): {source:?}\n--- after removing comments ---\n{:?}",
                            syntax.name,
                            String::from_utf8_lossy(&content)
                        ),
                    }
                    curly_checked += 1;
                }
            }
            Engine::Generic => {
                generic_dialects.push(syntax.name);
                for (index, source) in SOURCES.iter().enumerate() {
                    let Ok(first) = generic_scan(source.as_bytes(), syntax) else {
                        continue;
                    };
                    let content = without_comments(source.as_bytes(), &first.comments);
                    assert_eq!(
                        non_whitespace_outside(source.as_bytes(), &first.comments),
                        non_whitespace(&content),
                        "{} ({dialect:?}): removing comment spans lost non-whitespace on source {index}: {source:?}",
                        syntax.name
                    );
                    match generic_scan(&content, syntax) {
                        Ok(second) => assert!(
                            second.comments.is_empty(),
                            "{} ({dialect:?}): a second scan still finds comments on source {index}: {source:?}\n--- after removing comments ---\n{:?}",
                            syntax.name,
                            String::from_utf8_lossy(&content)
                        ),
                        Err(Bail(reason)) => panic!(
                            "{} ({dialect:?}): the source no longer reads after removing comments on source {index} ({reason}): {source:?}\n--- after removing comments ---\n{:?}",
                            syntax.name,
                            String::from_utf8_lossy(&content)
                        ),
                    }
                    let Ok(again) = generic_scan(source.as_bytes(), syntax) else {
                        panic!(
                            "{} ({dialect:?}): source {index} scans once but not twice: {source:?}",
                            syntax.name
                        );
                    };
                    assert_eq!(
                        comment_spans(&first),
                        comment_spans(&again),
                        "{} ({dialect:?}): a second run reports different comment spans on source {index}: {source:?}",
                        syntax.name
                    );
                    generic_checked += 1;
                }
            }
            _ => {}
        }
    }

    assert!(
        curly_checked > 0,
        "no Curly-engine descriptor scanned a single corpus source; the idempotence proof covers nothing"
    );
    assert!(
        generic_checked > 0,
        "no Generic-engine descriptor scanned a single corpus source; the idempotence proof covers nothing"
    );
    eprintln!(
        "!!! equivalence: {} curly scans across {} dialects; {} generic scans across {} dialects",
        curly_checked,
        curly_dialects.len(),
        generic_checked,
        generic_dialects.len()
    );
}
