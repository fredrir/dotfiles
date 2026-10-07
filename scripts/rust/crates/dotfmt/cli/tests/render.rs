#![forbid(unsafe_code)]

use dotfmt::render::{self, Mode, Row};

fn row(name: &'static str, files: usize) -> Row {
    Row {
        name,
        files,
        missing: Vec::new(),
        findings: false,
        failed: false,
        ran: 1,
        note: None,
        output: String::new(),
        blamed: Vec::new(),
    }
}

#[test]
fn a_row_whose_every_tool_is_missing_does_not_claim_success() {
    let ran = Row {
        missing: vec!["yamlfmt", "yamllint"],
        ran: 0,
        ..row("yaml", 2)
    };
    let lines = render::report(&[ran], Mode::Check, &workstation::Style::plain());
    assert_eq!(
        lines[0],
        "  yaml  2 files  yamlfmt not installed, yamllint not installed"
    );
}

#[test]
fn a_row_that_ran_says_so_and_still_names_the_tool_it_was_missing() {
    let ran = Row {
        missing: vec!["goimports"],
        ..row("go", 1)
    };
    let lines = render::report(&[ran], Mode::Write, &workstation::Style::plain());
    assert_eq!(
        lines[0],
        "  go  1 file   formatted  goimports not installed"
    );
}

#[test]
fn a_missing_tool_is_counted_in_the_tally_apart_from_the_findings() {
    let done = vec![
        Row {
            findings: true,
            ran: 2,
            ..row("python", 3)
        },
        Row {
            missing: vec!["sqlfluff"],
            ran: 0,
            ..row("sql", 1)
        },
    ];
    assert_eq!(
        render::tally(&done, Mode::Check),
        "4 files checked, 1 with findings, 1 not installed"
    );
}

// ------------------------------------------------------------- the summary

fn summary(done: &[Row], mode: Mode) -> Vec<String> {
    render::summary(done, mode, &workstation::Style::plain())
}

#[test]
fn a_run_with_nothing_to_report_is_one_line() {
    let done = vec![row("python", 120), row("lua", 48)];
    assert_eq!(summary(&done, Mode::Write), ["168 / 168 files formatted"]);
}

#[test]
fn a_check_run_says_clean_rather_than_formatted() {
    assert_eq!(
        summary(&[row("toml", 42)], Mode::Check),
        ["42 / 42 files clean"]
    );
}

#[test]
fn a_failed_provider_gets_a_line_and_its_files_leave_the_count() {
    let done = vec![
        Row {
            failed: true,
            ..row("web", 40)
        },
        Row {
            failed: true,
            ..row("yaml", 5)
        },
        row("python", 123),
    ];
    assert_eq!(
        summary(&done, Mode::Write),
        [
            "web   40 files  failed",
            "yaml   5 files  failed",
            "123 / 168 files formatted",
        ]
    );
}

#[test]
fn drift_names_the_provider_in_the_same_shape() {
    let done = vec![
        Row {
            findings: true,
            ..row("python", 3)
        },
        row("lua", 7),
    ];
    assert_eq!(
        summary(&done, Mode::Check),
        ["python  3 files  findings", "7 / 10 files clean"]
    );
}

#[test]
fn the_files_named_under_a_provider_are_capped() {
    let blamed: Vec<String> = (0..9).map(|nth| format!("f{nth}.py")).collect();
    let done = vec![Row {
        failed: true,
        blamed,
        ..row("python", 9)
    }];
    let lines = summary(&done, Mode::Write);
    assert_eq!(lines[1], "  f0.py");
    assert_eq!(lines[5], "  f4.py");
    assert_eq!(lines[6], "  … and 4 more");
    assert_eq!(lines[7], "0 / 9 files formatted");
}

#[test]
fn a_failure_that_names_no_file_shows_what_the_tool_said() {
    let done = vec![Row {
        failed: true,
        output: "\nconfmt --owns: unexpected argument\n".to_string(),
        ..row("confmt", 0)
    }];
    assert_eq!(
        summary(&done, Mode::Write),
        [
            "confmt  0 files  failed",
            "  confmt --owns: unexpected argument",
            "0 / 0 files formatted",
        ]
    );
}

#[test]
fn a_row_whose_every_tool_is_missing_is_left_out_of_the_count() {
    let done = vec![
        row("python", 10),
        Row {
            missing: vec!["sqlfluff"],
            ran: 0,
            ..row("sql", 4)
        },
    ];
    assert_eq!(summary(&done, Mode::Write), ["10 / 10 files formatted"]);
}

#[test]
fn when_nothing_ran_at_all_the_missing_tools_are_the_report() {
    let done = vec![
        Row {
            missing: vec!["yamlfmt", "yamllint"],
            ran: 0,
            ..row("yaml", 2)
        },
        Row {
            missing: vec!["yamlfmt"],
            ran: 0,
            ..row("sql", 1)
        },
    ];
    assert_eq!(
        summary(&done, Mode::Write),
        ["yamlfmt, yamllint not installed"]
    );
}

#[test]
fn heading_and_configuration_results_keep_their_visual_shape() {
    let style = workstation::Style::plain();
    assert_eq!(
        render::heading("dotfmt", std::path::Path::new("project"), "sync", &style),
        ["", "  dotfmt  project  sync", ""]
    );
    let copied = render::Placement {
        name: "ruff.toml",
        exists: false,
    };
    let replaced = render::Placement {
        name: ".editorconfig",
        exists: true,
    };
    assert_eq!(
        render::placed(&[&copied, &replaced], &style),
        ["  copied ruff.toml", "  replaced .editorconfig"]
    );
    assert_eq!(render::provenance(&render::Source::Repo, &style), None);
    assert_eq!(
        render::provenance(&render::Source::Embedded, &style).as_deref(),
        Some("  from the copies built into this binary; no checkout was found")
    );
}
