use super::*;

#[test]
fn columns_are_padded_to_the_widest_cell() {
    let text = render(
        &["metric", "value"],
        &[
            vec!["tctl".into(), "75.0".into()],
            vec!["radiator".into(), "1048".into()],
        ],
    );
    assert_eq!(text, "metric    value\ntctl      75.0\nradiator  1048\n");
}

#[test]
fn styled_right_aligns_numeric_columns() {
    let columns = [Column::new("k"), Column::right("v")];
    let rows = vec![
        vec![Cell::text("a"), Cell::text("1")],
        vec![Cell::text("b"), Cell::text("20")],
    ];
    let text = styled(&Style::plain(), &columns, &rows);
    assert_eq!(text, "k   v\na   1\nb  20\n");
}

#[test]
fn styled_paints_headers_and_cells() {
    let style = Style::for_stdout_with_color(true);
    let columns = [Column::new("name")];
    let rows = vec![vec![Cell::paint(Role::Accent, "x")]];
    let text = styled(&style, &columns, &rows);
    let lines = text.lines().collect::<Vec<_>>();
    assert!(lines[0].contains("\u{1b}["), "{text}");
    assert!(lines[1].contains("\u{1b}["), "{text}");
    assert!(text.contains('x'), "{text}");
    assert!(text.contains("name"), "{text}");
}
