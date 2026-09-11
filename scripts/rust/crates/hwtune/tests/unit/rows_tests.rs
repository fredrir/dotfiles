use super::*;

#[test]
fn render_aligns_labels_and_indents_details() {
    let rows = vec![
        Row::ok("boost", "5455 MHz"),
        Row::bad("Tcl", "spec says 30").with_details(vec!["line 14: 32".into()]),
    ];
    let text = render(&rows, &Style::plain());
    assert!(text.contains("ok    boost  5455 MHz"));
    assert!(text.contains("bad   Tcl    spec says 30"));
    assert!(text.contains("line 14: 32"));
}

#[test]
fn counts_group_by_kind() {
    let rows = vec![
        Row::ok("a", ""),
        Row::bad("b", ""),
        Row::warn("c", ""),
        Row::note("d", ""),
    ];
    assert_eq!(counts(&rows), (1, 1, 1));
}

#[test]
fn marks_use_fixed_width() {
    let style = Style::plain();
    for kind in [Kind::Ok, Kind::Bad, Kind::Warn, Kind::Note] {
        assert_eq!(mark(kind, &style).len(), 4);
    }
}
