use super::*;

#[test]
fn reports_keep_unicode_details_within_the_available_cells() {
    let report = Report::new("Files", "東京 project ready").detail("場所", "/tmp/長いパス/abc");
    assert!(report.plain().contains("場所   /tmp/長いパス/abc"));
    for width in 5..60 {
        for row in report.rows(width) {
            let text = match row {
                Row::Text(text) => text,
                Row::Detail(label, value) => format!("{label}   {value}"),
                Row::Hint => continue,
            };
            assert!(
                text_width(&text) <= width.saturating_sub(4).max(1),
                "{width}: {text:?}"
            );
        }
    }
}

#[test]
fn report_rows_escape_terminal_controls_without_changing_copy_content() {
    let report = Report::new("Failure", "value\x1b[2J").detail("name", "x\x1b[31m");
    assert!(report.plain().contains("\x1b[2J"));
    for row in report.rows(80) {
        match row {
            Row::Text(text) => assert!(!text.contains('\x1b')),
            Row::Detail(label, value) => {
                assert!(!label.contains('\x1b') && !value.contains('\x1b'))
            }
            Row::Hint => {}
        }
    }
}
