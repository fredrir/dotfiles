#![forbid(unsafe_code)]

//! The canonical behaviour matrix from SPEC §7.
//!
//! Each dialect gets one row per canonical cell. The rows are built from the
//! dialect's descriptor ([`Dialect::syntax`]) at runtime, because the
//! descriptor is what the matrix audits: a dialect that cannot express a cell
//! says so on stderr instead of passing silently.

#[path = "support/mod.rs"]
mod support;

use doc_purge_core::lang::Dialect;

pub use support::{CELLS, Case, Expect};

/// The nine canonical cells a single dialect can express.
pub fn cells(dialect: Dialect) -> Vec<Case> {
    support::cells(dialect)
}

/// Every row the harness runs: one [`cells`] set per dialect in `LANGUAGES`.
pub fn cases() -> &'static [Case] {
    support::cases()
}

#[test]
fn every_cell_is_known() {
    for case in cases() {
        assert!(
            CELLS.contains(&case.cell),
            "{} ({:?}) has a row for the unknown cell {:?}",
            case.dialect.syntax().name,
            case.dialect,
            case.cell
        );
    }
}

#[test]
fn line_alone() {
    run_cell("line_alone");
}

#[test]
fn line_inline() {
    run_cell("line_inline");
}

#[test]
fn line_in_string() {
    run_cell("line_in_string");
}

#[test]
fn block_alone() {
    run_cell("block_alone");
}

#[test]
fn block_between_tokens() {
    run_cell("block_between_tokens");
}

#[test]
fn directive_kept() {
    run_cell("directive_kept");
}

#[test]
fn load_bearing() {
    run_cell("load_bearing");
}

#[test]
fn unterminated() {
    run_cell("unterminated");
}

#[test]
fn idempotent() {
    let mut checked = 0usize;
    for case in cases().iter().filter(|case| case.cell == "idempotent") {
        // A pending dialect has no purging pass to run twice: its placeholder
        // row asserts the skip, which is the whole claim the cell can make while
        // the scanner is missing, and `check` owns that assertion. Counting it
        // out keeps `checked` meaning "rows that were really purged twice".
        if matches!(case.expect, Expect::Skipped(_)) {
            check(case);
            continue;
        }
        let once = support::purged(&case.source, case.dialect);
        if let Expect::Purged(want) = case.expect {
            assert_eq!(
                once,
                want,
                "{} / idempotent: source {:#?}",
                label(case),
                case.source
            );
        }
        let twice = support::purged(&once, case.dialect);
        assert_eq!(
            twice,
            once,
            "{} / idempotent: purging twice differs from purging once\n--- once ---\n{once:?}",
            label(case)
        );
        assert_invariants_everywhere(case);
        checked += 1;
    }
    assert!(checked > 0, "matrix has no idempotent row for any dialect");
}

fn run_cell(cell: &'static str) {
    assert!(CELLS.contains(&cell), "unknown cell {cell}");
    let mut checked = 0usize;
    for case in cases().iter().filter(|case| case.cell == cell) {
        check(case);
        checked += 1;
    }
    assert!(checked > 0, "matrix has no {cell} row for any dialect");
}

fn check(case: &Case) {
    match case.expect {
        Expect::Purged(want) => {
            let got = support::purged(&case.source, case.dialect);
            assert_eq!(
                got,
                want,
                "{} / {}: source {:#?}",
                label(case),
                case.cell,
                case.source
            );
        }
        Expect::Survives => {
            let got = support::purged(&case.source, case.dialect);
            assert_eq!(
                got,
                case.source,
                "{} / {}: the file should be left alone, source {:#?}",
                label(case),
                case.cell,
                case.source
            );
        }
        Expect::Skipped(prefix) => match support::left_alone(&case.source, case.dialect) {
            Some(reason) => assert!(
                reason.starts_with(prefix),
                "{} / {}: skipped as {reason:?}, expected a reason starting with {prefix:?}; source {:#?}",
                label(case),
                case.cell,
                case.source
            ),
            None => panic!(
                "{} / {}: expected a skip whose reason starts with {prefix:?}, but the file was purged; source {:#?}",
                label(case),
                case.cell,
                case.source
            ),
        },
    }
    assert_invariants_everywhere(case);
}

/// The invariants from SPEC §7 have to hold for every terminator the tool
/// accepts, not only for `\n`, so each cell is replayed with CRLF, with bare
/// CR and behind a byte order mark.
fn assert_invariants_everywhere(case: &Case) {
    if matches!(case.expect, Expect::Skipped(_)) {
        return;
    }
    support::assert_invariants(case.dialect, &case.source);
    if case.source.contains('\n') {
        for terminator in ["\r\n", "\r"] {
            support::assert_invariants(case.dialect, &case.source.replace('\n', terminator));
        }
    }
    support::assert_invariants(case.dialect, &format!("\u{feff}{}", case.source));
}

fn label(case: &Case) -> String {
    format!("{} ({:?})", case.dialect.syntax().name, case.dialect)
}
