#![forbid(unsafe_code)]

#[path = "support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::path::Path;

use doc_purge_core::lang::{Dialect, LANGUAGES, for_path};

#[test]
fn every_dialect_has_a_row() {
    let mut missing: Vec<String> = Vec::new();
    for dialect in Dialect::ALL.iter().copied() {
        if support::cells(dialect).is_empty() {
            missing.push(format!("{} ({dialect:?})", dialect.syntax().name));
        }
    }
    assert!(
        missing.is_empty(),
        "{} dialect(s) in Dialect::ALL have no matrix row at all: {}",
        missing.len(),
        missing.join(", ")
    );
}

#[test]
fn every_extension_resolves() {
    let mut claimants: BTreeMap<&'static str, Vec<Dialect>> = BTreeMap::new();
    for language in LANGUAGES {
        for extension in language.extensions {
            claimants
                .entry(extension)
                .or_default()
                .push(language.dialect);
        }
    }

    let mut problems: Vec<String> = Vec::new();
    for language in LANGUAGES {
        for extension in language.extensions {
            if extension.contains('.') {
                eprintln!(
                    "!!! conformance: {extension:?} is a suffix, not an extension; for_path cannot resolve it for {:?}",
                    language.dialect
                );
                continue;
            }
            let path = Path::new("sample").with_extension(extension);
            match for_path(&path).map(|found| found.dialect) {
                Some(found) if found == language.dialect => {}
                Some(found) => {
                    let shared = claimants
                        .get(extension)
                        .is_some_and(|dialects| dialects.len() > 1 && dialects.contains(&found));
                    if shared {
                        eprintln!(
                            "!!! conformance: .{extension} is claimed by {found:?} and {:?}; for_path returns the first claimant and detect() sniffs the rest",
                            language.dialect
                        );
                    } else {
                        problems.push(format!(
                            ".{extension} resolves to {found:?}, but LANGUAGES lists it for {:?}",
                            language.dialect
                        ));
                    }
                }
                None => problems.push(format!(
                    ".{extension} resolves to nothing, but LANGUAGES lists it for {:?}",
                    language.dialect
                )),
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} extension(s) do not resolve to their dialect: {}",
        problems.len(),
        problems.join("; ")
    );
}

#[test]
fn no_dialect_is_untested() {
    let cases = support::cases();
    let mut problems: Vec<String> = Vec::new();
    for dialect in Dialect::ALL.iter().copied() {
        let syntax = dialect.syntax();
        let name = syntax.name;
        if !cases.iter().any(|case| case.dialect == dialect) {
            problems.push(format!(
                "{name} ({dialect:?}) is in Dialect::ALL but has no row in CASES is it missing from LANGUAGES?"
            ));
            continue;
        }
        let rows = support::cells(dialect);
        let has = |cell: &str| rows.iter().any(|case| case.cell == cell);
        let line = !syntax.line.is_empty();
        let block = syntax.block.first();
        let inline_block = block.is_some_and(|block| support::inline_marker(block.at));
        let wanted = [
            (line, "line_alone"),
            (line, "line_inline"),
            (line, "load_bearing"),
            (block.is_some(), "block_alone"),
            (block.is_some(), "unterminated"),
            (inline_block, "block_between_tokens"),
            (!syntax.quotes.is_empty(), "line_in_string"),
            (
                (line || inline_block) && !syntax.directives.is_empty(),
                "directive_kept",
            ),
            (true, "idempotent"),
        ];
        for (demanded, cell) in wanted {
            if demanded && !has(cell) {
                problems.push(format!(
                    "{name} ({dialect:?}) declares the descriptor fields for {cell} but the matrix has no {cell} row"
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} dialect(s) are untested: {}",
        problems.len(),
        problems.join("; ")
    );
}
