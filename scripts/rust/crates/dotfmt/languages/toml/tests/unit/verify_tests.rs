use dotfmt_core::diagnostic::DiagnosticKind;

use crate::config::Config;
use crate::{compare, model, parse, verify};

fn fingerprint(input: &str, config: &Config) -> verify::Fingerprint {
    let document = parse(input).expect("valid TOML");
    let model = model::build(&document).expect("model");
    verify::Fingerprint::of(&model, config)
}

fn default(input: &str) -> verify::Fingerprint {
    fingerprint(input, &Config::default())
}

#[test]
fn layout_and_map_order_do_not_change_the_fingerprint() {
    let base = default("a = 1\nb = 2\n");
    assert!(base.matches(&default("a = 1\nb = 2\n")));
    assert!(base.matches(&default("b = 2\na = 1\n")));
    assert!(base.matches(&default("a=1 # note\nb   =  2\n")));
    assert!(!base.matches(&default("[x]\na.b = 1\n")));
}

#[test]
fn values_shape_and_array_order_are_part_of_the_fingerprint() {
    let base = default("a = 1\nb = [1, 2]\n");
    assert!(base.matches(&default("a = 1\nb = [1, 2]\n")));
    assert!(!base.matches(&default("a = 2\nb = [1, 2]\n")));
    assert!(!base.matches(&default("a = 1\nb = [2, 1]\n")));
    assert!(!base.matches(&default("a = 1\nb = [1, 2, 3]\n")));
    assert!(!base.matches(&default("a = [1]\nb = [1, 2]\n")));
    assert!(!base.matches(&default("a = \"1\"\nb = [1, 2]\n")));
}

#[test]
fn table_structure_is_part_of_the_fingerprint() {
    let base = default("[table]\nkey = 1\n");
    assert!(base.matches(&default("[table]\nkey = 1\n")));
    assert!(base.matches(&default("table.key = 1\n")));
    assert!(!base.matches(&default("[other]\nkey = 1\n")));
    assert!(!base.matches(&default("key = 1\n")));
    let array = default("[[row]]\nn = 1\n[[row]]\nn = 2\n");
    assert!(array.matches(&default("[[row]]\nn = 1\n[[row]]\nn = 2\n")));
    assert!(!array.matches(&default("[[row]]\nn = 2\n[[row]]\nn = 1\n")));
    assert!(!array.matches(&default("[[row]]\nn = 1\n")));
}

#[test]
fn inline_tables_compare_as_maps() {
    assert!(default("a = { x = 1, y = 2 }\n").matches(&default("a = { y = 2, x = 1 }\n")));
    assert!(!default("a = { x = 1 }\n").matches(&default("a = { x = 2 }\n")));
}

#[test]
fn floats_compare_by_bit_pattern() {
    assert!(default("a = nan\n").matches(&default("a = nan\n")));
    assert!(!default("a = nan\n").matches(&default("a = -nan\n")));
    assert!(!default("a = 0.0\n").matches(&default("a = -0.0\n")));
    assert!(!default("a = 1\n").matches(&default("a = 1.0\n")));
}

#[test]
fn literals_are_compared_as_written() {
    assert!(default("a = 1\n").matches(&default("a = 1\n")));
    assert!(!default("a = 1\n").matches(&default("a = +1\n")));
    assert!(!default("a = 1.0\n").matches(&default("a = 1.00\n")));
    assert!(!default("a = \"x\"\n").matches(&default("a = 'x'\n")));
}

#[test]
fn comments_are_compared_in_order() {
    let base = default("# one\na = 1 # two\n");
    assert!(base.comments_match(&default("# one\na = 1 # two\n")));
    assert!(!base.comments_match(&default("# two\na = 1 # one\n")));
    assert!(!base.comments_match(&default("# one\na = 1\n")));
    let reordered = Config {
        reorder_keys: true,
        ..Config::default()
    };
    let expected = fingerprint("# one\na = 1 # two\n", &reordered);
    assert!(expected.comments_match(&fingerprint("# two\na = 1 # one\n", &reordered)));
}

#[test]
fn mismatches_are_reported_as_internal_failures() {
    let error = compare(&default("a = 1\n"), &default("a = 2\n")).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Internal);
    assert_eq!(error.message, "formatting would change values");
    let error = compare(&default("a = 1\n"), &default("a = 1 # note\n")).unwrap_err();
    assert_eq!(error.kind, DiagnosticKind::Internal);
    assert_eq!(error.message, "formatting would change comments");
    assert!(compare(&default("a = 1\n"), &default("a = 1\n")).is_ok());
}
