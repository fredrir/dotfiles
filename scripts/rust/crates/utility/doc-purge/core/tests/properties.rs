#![forbid(unsafe_code)]

//! Property tests over generated comment/string/code soups (SPEC §7).
//!
//! There is no dev-dependency to lean on: a small deterministic LCG builds the
//! soups, so a failure is reproducible from the seed in the panic message and
//! the whole run stays well under ten seconds.

#[path = "support/mod.rs"]
mod support;

use doc_purge_core::lang::Dialect;
use doc_purge_core::purge::{self, Outcome};
use doc_purge_core::syntax::Family;

const CASES_PER_FAMILY: usize = 2000;

/// Statements that are code in every family; `soup` filters out any that
/// carry a family's own comment marker (Lisp's `;`), so the code projection of
/// a soup is known exactly.
const CODE: &[&str] = &[
    "x = 1",
    "value = 2",
    "total = a + b",
    "count = items.len()",
    "flag = a && b",
    "name = \"plain text\"",
    "index = table[key]",
    "result = f(a, b)",
    "x = 1;",
    "y = 2;",
];

/// Bodies for comments and strings. They deliberately carry the other
/// families' markers and typographic glyphs, because that is where scanners
/// get confused.
const PROSE: &[&str] = &[
    "note",
    "a note about the code",
    "// nested marker",
    "/* nested block */",
    "# nested hash",
    "-- nested dash",
    "\"quoted\"",
    "\u{201c}smart quotes\u{201d}",
    "an \u{2014} em dash",
    "naive implementations",
    "\u{1f680} rocket",
    "TODO",
    "SAFETY: the pointer is valid",
];

struct Rng {
    state: u64,
}

impl Rng {
    fn seeded(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state >> 11
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

struct Soup {
    source: String,
    /// Every code statement in the order it was written. Nothing in it may
    /// vanish: it carries no comment marker, quote or glyph.
    code: String,
}

fn family_name(family: Family) -> &'static str {
    match family {
        Family::Curly => "curly",
        Family::Hash => "hash",
        Family::Dash => "dash",
        Family::Css => "css",
        Family::Lisp => "lisp",
        Family::Markup => "markup",
        Family::Template => "template",
    }
}

/// One dialect per family, because a soup is only meaningful with a real
/// descriptor. Families with no dialect are named on stderr, never skipped
/// silently.
fn representatives() -> Vec<(&'static str, Dialect)> {
    let mut found: Vec<(&'static str, Dialect)> = Vec::new();
    for dialect in Dialect::ALL.iter().copied() {
        let name = family_name(dialect.family());
        if !found.iter().any(|(seen, _)| *seen == name) {
            found.push((name, dialect));
        }
    }
    for expected in ["curly", "hash", "dash", "css", "lisp", "markup", "template"] {
        if !found.iter().any(|(seen, _)| *seen == expected) {
            eprintln!(
                "!!! properties: no dialect declares the {expected} family; its soups are not generated"
            );
        }
    }
    assert!(
        !found.is_empty(),
        "no dialect in Dialect::ALL is usable for property tests"
    );
    found
}

fn soup(dialect: Dialect, rng: &mut Rng) -> Soup {
    let syntax = dialect.syntax();
    let lines: Vec<&'static str> = syntax.line.iter().map(|line| line.marker).collect();
    let blocks: Vec<(&'static str, &'static str)> = syntax
        .block
        .iter()
        .map(|block| (block.open, block.close))
        .collect();
    let quotes: Vec<(&'static str, &'static str)> = syntax
        .quotes
        .iter()
        .map(|quote| {
            let close = if quote.close.is_empty() {
                quote.open
            } else {
                quote.close
            };
            (quote.open, close)
        })
        .collect();

    let statements: Vec<&'static str> =
        CODE.iter()
            .copied()
            .filter(|statement| {
                // A statement may not carry this family's comment marker: in Lisp
                // the trailing `;` of `x = 1;` starts a comment, so a generator
                // that called it code would report a loss the scanner never made.
                !lines.iter().any(|marker| statement.contains(*marker))
                    && !syntax.block.iter().any(|block| {
                        statement.contains(block.open) || statement.contains(block.close)
                    })
            })
            .collect();
    assert!(
        !statements.is_empty(),
        "no code statement is marker-free for {dialect:?}"
    );

    let mut source = String::new();
    let mut code = String::new();
    let mut needs_break = false;
    let count = 1 + rng.below(8);
    for _ in 0..count {
        if !source.is_empty() {
            if needs_break {
                source.push_str(terminator(rng));
            } else if rng.chance(3) {
                source.push_str(terminator(rng));
                if rng.chance(3) {
                    source.push_str(terminator(rng));
                }
            } else {
                source.push(' ');
            }
        }
        needs_break = false;

        match rng.below(6) {
            0 | 1 => {
                let statement = statements[rng.below(statements.len())];
                source.push_str(statement);
                code.push_str(statement);
                code.push('\n');
            }
            2 | 3 if !lines.is_empty() => {
                source.push_str(lines[rng.below(lines.len())]);
                source.push(' ');
                source.push_str(prose(rng, &[]));
                // A line comment owns the rest of the line, so the next
                // fragment has to start a new one.
                needs_break = true;
            }
            4 if !blocks.is_empty() => {
                let (open, close) = blocks[rng.below(blocks.len())];
                source.push_str(open);
                source.push(' ');
                source.push_str(prose(rng, &[open, close]));
                source.push(' ');
                source.push_str(close);
            }
            _ if !quotes.is_empty() => {
                let (open, close) = quotes[rng.below(quotes.len())];
                let body = quote_body(
                    dialect.family(),
                    open,
                    rng,
                    &[open, close, "\\", "\"", "'", "`"],
                );
                source.push_str(open);
                source.push_str(body);
                source.push_str(close);
            }
            _ => {
                let statement = statements[rng.below(statements.len())];
                source.push_str(statement);
                code.push_str(statement);
                code.push('\n');
            }
        }
    }
    Soup { source, code }
}

/// A body that keeps the quote fragment plausible for its family.
///
/// In the curly family `'` is a char literal (or a Rust lifetime), not a
/// string: a multi-character body would let a `//` marker open a comment, so a
/// tick always gets the one-character body every curly language accepts. A
/// triple-quoted body also rejects the smart quotes that fold onto its own
/// delimiter at either edge.
fn quote_body(family: Family, open: &str, rng: &mut Rng, forbidden: &[&str]) -> &'static str {
    if family == Family::Curly && open == "'" {
        return "x";
    }
    let mut forbidden = forbidden.to_vec();
    if open.len() >= 3 {
        forbidden.extend_from_slice(match open.as_bytes()[0] {
            b'"' => &["\u{201c}", "\u{201d}"],
            b'\'' => &["\u{2018}", "\u{2019}"],
            _ => &[],
        });
    }
    prose(rng, &forbidden)
}

fn prose(rng: &mut Rng, forbidden: &[&str]) -> &'static str {
    let pool: Vec<&'static str> = PROSE
        .iter()
        .copied()
        .filter(|text| !forbidden.iter().any(|bad| text.contains(bad)))
        .collect();
    if pool.is_empty() {
        return "note";
    }
    pool[rng.below(pool.len())]
}

fn terminator(rng: &mut Rng) -> &'static str {
    match rng.below(3) {
        0 => "\r\n",
        1 => "\r",
        _ => "\n",
    }
}

fn seed(test: &str, family: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in test.bytes().chain(family.bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn purged_soup(dialect: Dialect, family: &str, case: usize, soup: &Soup) -> String {
    match purge::purge(soup.source.as_bytes(), dialect) {
        Outcome::Changed(edited, _) => String::from_utf8(edited.content).expect("utf-8"),
        Outcome::Untouched(_) => soup.source.clone(),
        Outcome::Skipped(reason) => panic!(
            "!!! properties: {family} soup {case} for {dialect:?} was skipped ({reason}); the generator only emits balanced sources\n--- source ---\n{:?}",
            soup.source
        ),
    }
}

#[test]
fn idempotent() {
    for (family, dialect) in representatives() {
        let mut rng = Rng::seeded(seed("idempotent", family));
        for case in 0..CASES_PER_FAMILY {
            let soup = soup(dialect, &mut rng);
            let once = purged_soup(dialect, family, case, &soup);
            let twice = purged_soup(
                dialect,
                family,
                case,
                &Soup {
                    source: once.clone(),
                    code: String::new(),
                },
            );
            assert_eq!(
                twice, once,
                "!!! properties: {family} soup {case} for {dialect:?} is not idempotent\n--- source ---\n{:?}\n--- once ---\n{once:?}",
                soup.source
            );
        }
    }
}

#[test]
fn no_non_whitespace_loss() {
    for (family, dialect) in representatives() {
        let mut rng = Rng::seeded(seed("no-non-whitespace-loss", family));
        for case in 0..CASES_PER_FAMILY {
            let soup = soup(dialect, &mut rng);
            let content = purged_soup(dialect, family, case, &soup);
            let code = non_whitespace(soup.code.as_bytes());
            let kept = non_whitespace(content.as_bytes());
            assert!(
                is_subsequence(&code, &kept),
                "!!! properties: {family} soup {case} for {dialect:?} lost non-whitespace code bytes\n--- source ---\n{:?}\n--- purged ---\n{content:?}\n--- code ---\n{:?}",
                soup.source,
                soup.code
            );
        }
    }
}

#[test]
fn relex_clean() {
    for (family, dialect) in representatives() {
        let mut rng = Rng::seeded(seed("relex-clean", family));
        for case in 0..CASES_PER_FAMILY {
            let soup = soup(dialect, &mut rng);
            let content = purged_soup(dialect, family, case, &soup);
            match purge::purge(content.as_bytes(), dialect) {
                Outcome::Untouched(_) => {}
                Outcome::Changed(again, _) => panic!(
                    "!!! properties: {family} soup {case} for {dialect:?} is still dirty after purging\n--- source ---\n{:?}\n--- once ---\n{content:?}\n--- twice ---\n{:?}",
                    soup.source,
                    String::from_utf8_lossy(&again.content)
                ),
                Outcome::Skipped(reason) => panic!(
                    "!!! properties: {family} soup {case} for {dialect:?} does not read back after purging ({reason})\n--- source ---\n{:?}\n--- once ---\n{content:?}",
                    soup.source
                ),
            }
        }
    }
}

#[test]
fn ranges_in_bounds() {
    for (family, dialect) in representatives() {
        let mut rng = Rng::seeded(seed("ranges-in-bounds", family));
        for case in 0..CASES_PER_FAMILY {
            let soup = soup(dialect, &mut rng);
            let Outcome::Changed(edited, _) = purge::purge(soup.source.as_bytes(), dialect) else {
                continue;
            };
            let mut previous_end = 0usize;
            for (index, change) in edited.changes.iter().enumerate() {
                let start = change.range.start;
                let end = change.range.end;
                assert!(
                    start <= end,
                    "!!! properties: {family} soup {case} for {dialect:?} change {index} is backwards: {start}..{end}\n--- source ---\n{:?}",
                    soup.source
                );
                assert!(
                    end <= soup.source.len(),
                    "!!! properties: {family} soup {case} for {dialect:?} change {index} ends at {end}, past the end of the {} byte source\n--- source ---\n{:?}",
                    soup.source.len(),
                    soup.source
                );
                assert!(
                    soup.source.is_char_boundary(start) && soup.source.is_char_boundary(end),
                    "!!! properties: {family} soup {case} for {dialect:?} change {index} {start}..{end} is not on character boundaries\n--- source ---\n{:?}",
                    soup.source
                );
                assert!(
                    start >= previous_end,
                    "!!! properties: {family} soup {case} for {dialect:?} change {index} {start}..{end} overlaps or is out of order (previous ended at {previous_end})\n--- source ---\n{:?}",
                    soup.source
                );
                previous_end = end;
            }
        }
    }
}

fn non_whitespace(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect()
}

fn is_subsequence(needle: &[u8], haystack: &[u8]) -> bool {
    let mut at = 0usize;
    for byte in haystack {
        if at < needle.len() && needle[at] == *byte {
            at += 1;
        }
    }
    at == needle.len()
}
