#![forbid(unsafe_code)]

use dotfmt_core::repair::Repair;
use dotfmt_yaml::repair;

#[test]
fn tab_indentation_becomes_spaces_level_for_level() {
    let repaired = repair("a:\n\tb: 1\n\tc:\n\t\t- d\n\n\t# note\n\te: 2\n");
    assert_eq!(
        repaired.text,
        "a:\n  b: 1\n  c:\n    - d\n\n  # note\n  e: 2\n"
    );
    assert_eq!(repaired.repairs.of(Repair::Indent), 5);
    assert_eq!(repaired.repairs.fixes()[0].line, 2);
}

#[test]
fn tabs_beside_space_indentation_are_left_for_a_person() {
    for input in ["a:\n  b: 1\n\tc: 2\n", "a:\n\t  b: 1\n"] {
        let repaired = repair(input);
        assert_eq!(repaired.text, input);
        assert!(repaired.repairs.is_empty(), "{input:?}");
    }
}

#[test]
fn other_syntax_errors_and_valid_input_are_left_alone() {
    for input in ["a: [1, 2\n", "a: b: c\n", "a:\n  b: \"x\tz\"\n"] {
        let repaired = repair(input);
        assert_eq!(repaired.text, input);
        assert!(repaired.repairs.is_empty(), "{input:?}");
    }
}
