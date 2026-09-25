#![forbid(unsafe_code)]

use dotfile_cli::doctor::report::{Report, Row, Status, render};
use workstation::Style;

fn plain(rows: &[Row], show_all: bool) -> String {
    render(
        &Report {
            profile: "arch-linux/kde",
            rows,
            show_all,
        },
        &Style::plain(),
    )
}

#[test]
fn healthy_report_says_nothing_missing() {
    let rows = [
        Row::missing("tools", 4, Vec::new()),
        Row::new(Status::Ok, "links", "63 linked", 0),
        Row::new(Status::Note, "optional", "2 not installed", 0),
    ];
    let output = plain(&rows, false);
    assert_eq!(output, "nothing missing\n\n");
}
