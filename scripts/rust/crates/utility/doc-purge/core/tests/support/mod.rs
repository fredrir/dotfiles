//! Shared helpers for the doc-purge 2.0 tests (SPEC §7).
//!
//! `matrix.rs`, `conformance.rs`, `registry.rs` and `properties.rs` each pull
//! this file in with `#[path = "support/mod.rs"] mod support;`, so the helpers
//! — and the matrix table they build — cannot drift between test binaries.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::sync::OnceLock;

use doc_purge_core::lang::{Dialect, LANGUAGES};
use doc_purge_core::purge::{self, Outcome};
use doc_purge_core::syntax::{At, DocStyle};
use doc_purge_core::{Change, Edited};

/// The nine canonical cells, in SPEC §7 table order.
pub const CELLS: [&str; 9] = [
    "line_alone",
    "line_inline",
    "line_in_string",
    "block_alone",
    "block_between_tokens",
    "directive_kept",
    "load_bearing",
    "unterminated",
    "idempotent",
];

const CODE_LINE: &str = "x = 1\n";
const CODE_HEAD: &str = "x = 1";
const CODE_TAIL: &str = "y = 2";
const CODE_PAIR: &str = "x = 1\ny = 2\n";
const CODE_TOGETHER: &str = "a=1\n";

pub fn rebuilt(source: &str, changes: &[Change]) -> String {
    let mut text = source.to_string();
    for change in changes.iter().rev() {
        text.replace_range(change.range.clone(), &change.text);
    }
    text
}

pub fn content(source: &str, edited: Edited) -> String {
    let content = String::from_utf8(edited.content).expect("purged content is utf-8");
    assert_eq!(
        rebuilt(source, &edited.changes),
        content,
        "replaying the change list does not rebuild the purged content"
    );
    content
}

pub fn outcome(source: &str, dialect: Dialect) -> Outcome {
    purge::purge(source.as_bytes(), dialect)
}

pub fn purged(source: &str, dialect: Dialect) -> String {
    match outcome(source, dialect) {
        Outcome::Changed(edited, _) => content(source, *edited),
        Outcome::Untouched(_) => source.to_string(),
        Outcome::Skipped(reason) => panic!("left alone: {reason}\nsource: {source:?}"),
    }
}

pub fn survives(source: &str, dialect: Dialect) {
    assert_eq!(
        purged(source, dialect),
        source,
        "{dialect:?} should have left the file alone"
    );
}

pub fn counts(source: &str, dialect: Dialect) -> (usize, usize) {
    match outcome(source, dialect) {
        Outcome::Changed(edited, _) => (edited.minus, edited.plus),
        _ => (0, 0),
    }
}

pub fn left_alone(source: &str, dialect: Dialect) -> Option<&'static str> {
    match outcome(source, dialect) {
        Outcome::Skipped(reason) => Some(reason),
        _ => None,
    }
}

/// What a matrix cell expects the purger to do with its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// The file is purged and the result is exactly this text.
    Purged(&'static str),
    /// The file is left byte-for-byte alone.
    Survives,
    /// The file is skipped. Each engine names its own bail
    /// (`unterminated block comment` generically, `unterminated long comment`
    /// for Lua), so only the shared prefix is asserted.
    Skipped(&'static str),
}

#[derive(Clone, Debug)]
pub struct Case {
    pub cell: &'static str,
    pub dialect: Dialect,
    pub source: String,
    pub expect: Expect,
}

/// Every canonical cell a single dialect can express.
///
/// A cell whose descriptor field is empty (`block: &[]`, `quotes: &[]`,
/// `directives: &[]`, …) is *skipped with a note on stderr*, never silently
/// passed: an empty descriptor is a hole in the dialect, and the note names it.
pub fn cells(dialect: Dialect) -> Vec<Case> {
    // A dialect whose engine has not been written yet answers with a skip, not
    // an empty `Out`. Asserting the skip is what keeps the gap visible: the row
    // fails the day the engine lands, which is the day the real rows must be
    // written. Never add a dialect here to make a red row green.
    //
    // The placeholder covers every cell, `idempotent` included: with no scanner
    // there is no purging pass to run twice, so the matrix checks that row's
    // skip as its whole claim (`matrix::idempotent`).
    let syntax = dialect.syntax();
    let line = syntax.line.first();
    let block = syntax.block.first();
    let mut rows: Vec<Case> = Vec::with_capacity(CELLS.len());

    match line {
        Some(line) => rows.push(Case {
            cell: "line_alone",
            dialect,
            source: format!("{} note\n{CODE_LINE}", line.marker),
            expect: Expect::Purged(CODE_LINE),
        }),
        None => skipped(
            dialect,
            "line_alone",
            "the descriptor has no line comment marker",
        ),
    }

    match line {
        Some(line) => {
            // Only `Anywhere` and `WordStart` markers are comments in the
            // middle of a line; a `LineStart`/`StartOfFile` marker is code
            // there, and the cell asserts exactly that by demanding the file
            // survive. The trap is a marker that is also a string delimiter
            // (VimL `"`, which must stay at `LineStart`): mid-line it opens a
            // string, and an unclosed one makes the scanner skip the file. Close
            // the string again, so the row keeps asserting "no comment starts
            // here" instead of the scanner's `unterminated string` bail.
            let inline = inline_marker(line.at);
            let closer = if inline {
                ""
            } else {
                quote_closer(dialect, line.marker).unwrap_or("")
            };
            if !closer.is_empty() {
                eprintln!(
                    "!!! matrix: the line_inline row for {} ({dialect:?}) closes the string that its {} marker opens mid-line: the marker is code there, and an unterminated string would skip the file",
                    syntax.name, line.marker
                );
            }
            rows.push(Case {
                cell: "line_inline",
                dialect,
                source: format!("{CODE_HEAD} {} note{closer}\n{CODE_TAIL}\n", line.marker),
                expect: if inline {
                    Expect::Purged(CODE_PAIR)
                } else {
                    Expect::Survives
                },
            });
        }
        None => skipped(
            dialect,
            "line_inline",
            "the descriptor has no line comment marker",
        ),
    }

    match string_with_marker(dialect) {
        Some((open, close, marker)) => rows.push(Case {
            cell: "line_in_string",
            dialect,
            source: format!("x = {open}{marker} note{close}\n"),
            expect: Expect::Survives,
        }),
        None => skipped(
            dialect,
            "line_in_string",
            "no quote delimiter and marker pair can share a line",
        ),
    }

    match block {
        Some(block) => rows.push(Case {
            cell: "block_alone",
            dialect,
            source: if inline_marker(block.at) {
                format!("{} note {}\n{CODE_LINE}", block.open, block.close)
            } else {
                format!("{}\nnote\n{}\n{CODE_LINE}", block.open, block.close)
            },
            expect: Expect::Purged(CODE_LINE),
        }),
        None => skipped(
            dialect,
            "block_alone",
            "the descriptor has no block comment",
        ),
    }

    match block {
        Some(block) if !inline_marker(block.at) => skipped(
            dialect,
            "block_between_tokens",
            "the block comment only opens at a line start",
        ),
        Some(block) => rows.push(Case {
            cell: "block_between_tokens",
            dialect,
            source: format!("a={}n{}1\n", block.open, block.close),
            expect: Expect::Purged(CODE_TOGETHER),
        }),
        None => skipped(
            dialect,
            "block_between_tokens",
            "the descriptor has no block comment",
        ),
    }

    match (line, block, syntax.directives.first()) {
        (_, _, None) => skipped(
            dialect,
            "directive_kept",
            "the descriptor lists no directives",
        ),
        (Some(line), _, Some(directive)) => rows.push(Case {
            cell: "directive_kept",
            dialect,
            source: format!("{} {directive} rest\n{CODE_LINE}", line.marker),
            expect: Expect::Survives,
        }),
        (None, Some(block), Some(directive)) if inline_marker(block.at) => rows.push(Case {
            cell: "directive_kept",
            dialect,
            source: format!(
                "{} {directive} rest {}\n{CODE_LINE}",
                block.open, block.close
            ),
            expect: Expect::Survives,
        }),
        (None, Some(_), Some(_)) => skipped(
            dialect,
            "directive_kept",
            "the only comment form opens at a line start",
        ),
        (None, None, Some(_)) => skipped(
            dialect,
            "directive_kept",
            "the descriptor has no comment marker",
        ),
    }

    match (line, block) {
        (Some(line), _) => rows.push(Case {
            cell: "load_bearing",
            dialect,
            source: format!("{} SAFETY: the pointer is valid\n{CODE_LINE}", line.marker),
            expect: Expect::Survives,
        }),
        (None, Some(block)) if inline_marker(block.at) => rows.push(Case {
            cell: "load_bearing",
            dialect,
            source: format!(
                "{} SAFETY: the pointer is valid {}\n{CODE_LINE}",
                block.open, block.close
            ),
            expect: Expect::Survives,
        }),
        _ => skipped(
            dialect,
            "load_bearing",
            "no comment form that can carry the marker",
        ),
    }

    match block {
        Some(block) => {
            // A dialect whose line-start comments own the indented block beneath
            // them reads a `/*` at a line start the same way: as a comment that
            // ends at EOF, not as an unterminated one. Open the block after a
            // token, exactly as `css.rs`'s
            // `css_an_unterminated_comment_skips_the_file` does, so the block
            // really is unterminated.
            let owns_indent_block = line.is_some_and(|line| line.owns_indent_block);
            let source = if owns_indent_block {
                eprintln!(
                    "!!! matrix: the unterminated row for {} ({dialect:?}) opens {} after a token: at a line start the block comment owns the indented block beneath it and ends at EOF",
                    syntax.name, block.open
                );
                format!("a {{ {} note\n", block.open)
            } else if inline_marker(block.at) {
                format!("{} note\n", block.open)
            } else {
                format!("{}\nnote\n", block.open)
            };
            let expect = if block.to_eof_ok {
                // Stylus: an unterminated block comment runs to EOF, so the
                // whole file is the comment.
                Expect::Purged("")
            } else {
                Expect::Skipped("unterminated")
            };
            rows.push(Case {
                cell: "unterminated",
                dialect,
                source,
                expect,
            });
        }
        None => skipped(
            dialect,
            "unterminated",
            "the descriptor has no block comment",
        ),
    }

    // SPEC §7: "any Purged case run twice". The first purging row is the
    // canonical one, so the idempotence cell reuses exactly that source.
    let repeat = rows
        .iter()
        .find(|row| matches!(row.expect, Expect::Purged(_)))
        .map(|row| (row.source.clone(), row.expect));
    match repeat {
        Some((source, expect)) => rows.push(Case {
            cell: "idempotent",
            dialect,
            source,
            expect,
        }),
        None => skipped(dialect, "idempotent", "no purging row to run twice"),
    }

    rows
}

/// Every cell for every dialect `LANGUAGES` names, built once.
pub fn cases() -> &'static [Case] {
    static CASES: OnceLock<Vec<Case>> = OnceLock::new();
    CASES.get_or_init(|| {
        let mut all: Vec<Case> = Vec::new();
        let mut seen: Vec<Dialect> = Vec::new();
        for language in LANGUAGES {
            if seen.contains(&language.dialect) {
                continue;
            }
            seen.push(language.dialect);
            all.extend(cells(language.dialect));
        }
        all
    })
}

fn skipped(dialect: Dialect, cell: &'static str, why: &str) {
    eprintln!(
        "!!! matrix: no {cell} row for {} ({dialect:?}): {why}",
        dialect.syntax().name
    );
}

/// Whether a marker may open in the middle of a line.
pub fn inline_marker(at: At) -> bool {
    matches!(at, At::Anywhere | At::WordStart)
}

/// The delimiter that closes a string opened by `marker`, when the marker is
/// also a quote opener (VimL `"`).
fn quote_closer(dialect: Dialect, marker: &str) -> Option<&'static str> {
    dialect
        .syntax()
        .quotes
        .iter()
        .find(|quote| quote.open == marker)
        .map(|quote| {
            if quote.close.is_empty() {
                quote.open
            } else {
                quote.close
            }
        })
}

/// A `(quote open, quote close, marker)` triple that can share one line: the
/// marker must not contain either delimiter, or the delimiter would close (or
/// open) the string before the marker is reached.
fn string_with_marker(dialect: Dialect) -> Option<(&'static str, &'static str, &'static str)> {
    let syntax = dialect.syntax();
    let markers: Vec<&'static str> = syntax
        .line
        .iter()
        .map(|line| line.marker)
        .chain(syntax.block.iter().map(|block| block.open))
        .collect();
    // A plain `"…"` is the least surprising string in every family; raw and
    // templated forms are covered by the dialect's own descriptor tests.
    let plain = syntax.quotes.iter().find(|quote| quote.open == "\"");
    for quote in plain.into_iter().chain(syntax.quotes) {
        let close = if quote.close.is_empty() {
            quote.open
        } else {
            quote.close
        };
        if let Some(marker) = markers
            .iter()
            .copied()
            .find(|marker| !marker.contains(quote.open) && !marker.contains(close))
        {
            return Some((quote.open, close, marker));
        }
    }
    None
}

/// Assert every invariant from SPEC §7 for one dialect/source pair.
///
/// A source the purger skips has nothing to check; the skip is named on stderr
/// so that a skipped case never looks like a passing one.
pub fn assert_invariants(dialect: Dialect, source: &str) {
    let first = purge::purge(source.as_bytes(), dialect);
    let (purged_content, changes) = match first {
        Outcome::Changed(edited, _) => {
            let Edited {
                content, changes, ..
            } = *edited;
            let text = String::from_utf8(content).expect("purged content is utf-8");
            assert_eq!(
                rebuilt(source, &changes),
                text,
                "{}: replaying the change list does not rebuild the purged content\n--- source ---\n{source:?}",
                label(dialect)
            );
            (text, changes)
        }
        Outcome::Untouched(_) => (source.to_string(), Vec::new()),
        Outcome::Skipped(reason) => {
            eprintln!(
                "!!! invariants: {} skipped this source ({reason}); nothing to assert",
                label(dialect)
            );
            return;
        }
    };

    check_ranges(dialect, source, &changes);
    check_gaps(dialect, source, &purged_content, &changes);
    check_code_survives(dialect, source, &changes);
    check_terminators(dialect, source, &purged_content);

    // 1. purge(purge(x)) == purge(x)  4. re-scanning the result is clean
    match purge::purge(purged_content.as_bytes(), dialect) {
        Outcome::Untouched(_) => {}
        Outcome::Changed(again, _) => panic!(
            "{}: purging is not idempotent; a second pass changed the file again\n--- original ---\n{source:?}\n--- once ---\n{purged_content:?}\n--- twice ---\n{:?}",
            label(dialect),
            String::from_utf8_lossy(&again.content)
        ),
        Outcome::Skipped(reason) => panic!(
            "{}: the purged file does not read back ({reason})\n--- original ---\n{source:?}\n--- once ---\n{purged_content:?}",
            label(dialect)
        ),
    }
}

fn label(dialect: Dialect) -> String {
    format!("{} ({dialect:?})", dialect.syntax().name)
}

/// 2. Every `Change.range` is in bounds, ascending, non-overlapping and on
///    character boundaries.
fn check_ranges(dialect: Dialect, source: &str, changes: &[Change]) {
    let mut previous_end = 0usize;
    for (index, change) in changes.iter().enumerate() {
        let start = change.range.start;
        let end = change.range.end;
        assert!(
            start <= end,
            "{}: change {index} has a backwards range {start}..{end}\n--- source ---\n{source:?}",
            label(dialect)
        );
        assert!(
            end <= source.len(),
            "{}: change {index} ends at {end}, past the {} bytes of source\n--- source ---\n{source:?}",
            label(dialect),
            source.len()
        );
        assert!(
            source.is_char_boundary(start) && source.is_char_boundary(end),
            "{}: change {index} {start}..{end} is not on character boundaries\n--- source ---\n{source:?}",
            label(dialect)
        );
        assert!(
            start >= previous_end,
            "{}: change {index} {start}..{end} is out of order or overlaps the change ending at {previous_end}\n--- source ---\n{source:?}",
            label(dialect)
        );
        previous_end = end;
    }
}

/// 3. `rebuilt(source, changes) == content` is asserted by [`content`] and by
///    [`assert_invariants`]; this walks the same list gap by gap so that the
///    failure names the change that went wrong.
///
/// 5. Bytes outside every `Change.range` are unchanged.
fn check_gaps(dialect: Dialect, source: &str, content: &str, changes: &[Change]) {
    let before = source.as_bytes();
    let after = content.as_bytes();
    let mut at = 0usize;
    let mut out = 0usize;
    for (index, change) in changes.iter().enumerate() {
        let gap = change.range.start - at;
        assert!(
            out + gap + change.text.len() <= after.len(),
            "{}: change {index} claims {} bytes of replacement text, but the purged file is only {} bytes",
            label(dialect),
            change.text.len(),
            after.len()
        );
        assert_eq!(
            &after[out..out + gap],
            &before[at..at + gap],
            "{}: change {index} moved bytes outside its range\n--- source ---\n{source:?}",
            label(dialect)
        );
        assert_eq!(
            &after[out + gap..out + gap + change.text.len()],
            change.text.as_bytes(),
            "{}: change {index} did not write its replacement text verbatim\n--- source ---\n{source:?}",
            label(dialect)
        );
        at = change.range.end;
        out += gap + change.text.len();
    }
    assert_eq!(
        &after[out..],
        &before[at..],
        "{}: bytes after the last change are not unchanged\n--- source ---\n{source:?}",
        label(dialect)
    );
}

/// 6. Removing comment spans leaves every non-whitespace byte of the code
///    intact.
///
///    The scanner's spans are private, so the harness checks what it can see.
///    A change may only delete comment, doc or glyph material: its removed
///    bytes must carry one of the dialect's markers or quote delimiters, be a
///    non-ASCII glyph run, be part of a rewrite (`change.text` is non-empty),
///    or belong to a dialect whose doc style rewrites string bodies. Anything
///    else is a deletion of code bytes and is reported with the change.
fn check_code_survives(dialect: Dialect, source: &str, changes: &[Change]) {
    let syntax = dialect.syntax();
    let markers: Vec<&str> = syntax
        .line
        .iter()
        .map(|line| line.marker)
        .chain(syntax.block.iter().map(|block| block.open))
        .collect();
    let quotes: Vec<&str> = syntax.quotes.iter().map(|quote| quote.open).collect();
    let doc_bodies = matches!(
        syntax.doc,
        DocStyle::Body { .. } | DocStyle::Before { .. } | DocStyle::AfterName | DocStyle::Ldoc
    );
    for (index, change) in changes.iter().enumerate() {
        let removed = &source[change.range.clone()];
        if removed.trim().is_empty() {
            continue;
        }
        if !change.text.is_empty() {
            // A rewrite is declared by the change itself (glyph fold, first
            // sentence kept, escaped quote).
            continue;
        }
        if markers.iter().any(|marker| removed.contains(marker)) {
            continue;
        }
        if quotes.iter().any(|quote| removed.contains(quote)) {
            continue;
        }
        if removed.trim().chars().all(|letter| !letter.is_ascii()) {
            // A run of typographic glyphs that the sweep removed.
            continue;
        }
        if doc_bodies {
            // The docstring body is prose with no marker of its own.
            continue;
        }
        panic!(
            "{}: change {index} deleted {:?}, which carries no comment marker, quote or glyph — code bytes must never be purged\n--- source ---\n{source:?}",
            label(dialect),
            removed
        );
    }
}

/// 7. BOM / CRLF / CR / U+2028 survive as terminators: purging may remove a
///    terminator together with its line, but it may never invent a different
///    one.
fn check_terminators(dialect: Dialect, source: &str, content: &str) {
    if source.starts_with('\u{feff}') {
        assert!(
            content.starts_with('\u{feff}'),
            "{}: the byte order mark was dropped\n--- source ---\n{source:?}",
            label(dialect)
        );
    }
    let before = terminators(source);
    let after = terminators(content);
    for kind in &after {
        assert!(
            before.contains(kind),
            "{}: purging introduced a {kind:?} line terminator that the source did not use\n--- source ---\n{source:?}\n--- purged ---\n{content:?}",
            label(dialect)
        );
    }
}

fn terminators(text: &str) -> BTreeSet<&'static str> {
    const LS: &[u8] = "\u{2028}".as_bytes();
    const PS: &[u8] = "\u{2029}".as_bytes();
    let bytes = text.as_bytes();
    let mut kinds = BTreeSet::new();
    let mut at = 0usize;
    while at < bytes.len() {
        match bytes[at] {
            b'\r' if bytes.get(at + 1) == Some(&b'\n') => {
                kinds.insert("\r\n");
                at += 2;
            }
            b'\r' => {
                kinds.insert("\r");
                at += 1;
            }
            b'\n' => {
                kinds.insert("\n");
                at += 1;
            }
            _ if bytes[at..].starts_with(LS) => {
                kinds.insert("\u{2028}");
                at += LS.len();
            }
            _ if bytes[at..].starts_with(PS) => {
                kinds.insert("\u{2029}");
                at += PS.len();
            }
            _ => at += 1,
        }
    }
    kinds
}
