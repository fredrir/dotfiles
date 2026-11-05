#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use doc_purge_core::lang::{Dialect, LANGUAGES};
use doc_purge_core::syntax::Engine;

#[test]
fn dialect_all_set_equals_languages() {
    let mut unreachable: Vec<String> = Vec::new();
    for dialect in Dialect::ALL.iter().copied() {
        if !LANGUAGES.iter().any(|language| language.dialect == dialect) {
            unreachable.push(format!("{} ({dialect:?})", dialect.syntax().name));
        }
    }
    assert!(
        unreachable.is_empty(),
        "{} dialect(s) are in Dialect::ALL but no LANGUAGES entry names them: {}",
        unreachable.len(),
        unreachable.join(", ")
    );

    let mut unknown: Vec<String> = Vec::new();
    for language in LANGUAGES {
        if !Dialect::ALL.contains(&language.dialect) {
            unknown.push(format!(
                "{} ({:?}, extensions {:?})",
                language.name, language.dialect, language.extensions
            ));
        }
    }
    assert!(
        unknown.is_empty(),
        "{} LANGUAGES entry(s) name a dialect that Dialect::ALL does not contain: {}",
        unknown.len(),
        unknown.join(", ")
    );
}

#[test]
fn every_dialect_has_line_or_block_markers() {
    let mut markerless: Vec<String> = Vec::new();
    for dialect in Dialect::ALL.iter().copied() {
        let syntax = dialect.syntax();
        if syntax.line.is_empty() && syntax.block.is_empty() {
            markerless.push(format!("{} ({dialect:?})", syntax.name));
        }
    }
    assert!(
        markerless.is_empty(),
        "{} dialect(s) have neither a line nor a block comment marker: {}",
        markerless.len(),
        markerless.join(", ")
    );
}

#[test]
fn syntax_names_are_unique() {
    let mut names: BTreeMap<&'static str, Vec<Dialect>> = BTreeMap::new();
    for dialect in Dialect::ALL.iter().copied() {
        names
            .entry(dialect.syntax().name)
            .or_default()
            .push(dialect);
    }
    let clashes: Vec<String> = names
        .iter()
        .filter(|(_, dialects)| dialects.len() > 1)
        .map(|(name, dialects)| format!("{name:?} is used by {dialects:?}"))
        .collect();
    assert!(
        clashes.is_empty(),
        "{} syntax().name value(s) are not unique: {}",
        clashes.len(),
        clashes.join("; ")
    );
}

#[test]
fn no_generic_engine_without_markers() {
    let mut problems: Vec<String> = Vec::new();
    for dialect in Dialect::ALL.iter().copied() {
        let syntax = dialect.syntax();
        if matches!(syntax.engine, Engine::Generic)
            && syntax.line.is_empty()
            && syntax.block.is_empty()
        {
            problems.push(format!(
                "{} ({dialect:?}) runs on Engine::Generic with no line or block markers",
                syntax.name
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} generic-engine dialect(s) have nothing to lex: {}",
        problems.len(),
        problems.join(", ")
    );
}
