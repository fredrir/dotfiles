use super::*;
use crate::bios::export;

#[test]
fn summary_names_duplicates_and_unified_shows_lines() {
    let before = export::parse("A [1]\nB [x]\nB [y]\n");
    let after = export::parse("A [2]\nB [x]\nB [z]\n");
    let rows = summary(&export::changed(&before, &after), &before);
    assert_eq!(rows[0], vec!["A", "1", "2"]);
    assert_eq!(rows[1], vec!["B#2", "y", "z"]);
    let text = unified("A [1]\n", "A [2]\n", ("a", "b"));
    assert!(text.contains("-A [1]"));
    assert!(text.contains("+A [2]"));
}
