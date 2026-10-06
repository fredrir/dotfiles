#![forbid(unsafe_code)]

use mdfmt::{config::Config, format};
use testkit::tree_pairs;

#[test]
fn shipped_defaults_match_built_in_formatting() {
    let shipped = include_str!("../../../../../../shared/tools/mdfmt.dotfile");
    let root = tree_pairs(&[("mdfmt.dotfile", shipped)]);
    let config = Config::read(&root.path().join("mdfmt.dotfile")).unwrap();
    let built_in = Config::default();
    assert_eq!(config.width, built_in.width);
    assert_eq!(config.table_style, built_in.table_style);
    assert_eq!(config.heading_blank_lines, built_in.heading_blank_lines);
    assert_eq!(
        format("* item", &config).unwrap(),
        format("* item", &built_in).unwrap()
    );
    assert_eq!(config.final_newline, built_in.final_newline);
}

#[test]
fn configured_list_markers_are_used_for_nested_lists_and_tasks() {
    for marker in ["*", "+", "-"] {
        let config = format!("mdfmt {{\nlist_marker = {marker}\n}}");
        let root = tree_pairs(&[("mdfmt.dotfile", &config)]);
        let config = Config::read(&root.path().join("mdfmt.dotfile")).unwrap();
        let text = format("- one\n  - two\n- [ ] task", &config).unwrap();
        assert!(text.starts_with(&format!("{marker} one")), "{text}");
        assert!(text.contains(&format!("{marker} [ ] task")), "{text}");
        assert_eq!(format(&text, &config).unwrap(), text);
    }
}
