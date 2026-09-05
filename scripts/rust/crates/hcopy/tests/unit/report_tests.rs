use super::*;
use std::path::PathBuf;

fn plan() -> Plan {
    Plan {
        direction: Direction::Push,
        host: "archie".into(),
        local: PathBuf::from("/Users/fredrir/projects/my-app"),
        local_display: "~/projects/my-app".into(),
        remote: "/home/fredrir/projects/my-app".into(),
        remote_display: "~/projects/my-app".into(),
        route: Some(Route::Cable),
        dry_run: false,
        checksum: false,
        all: false,
    }
}

#[test]
fn the_header_reads_in_the_direction_of_the_copy() {
    let style = Style::plain();
    assert_eq!(
        header(
            &style,
            Direction::Push,
            "macie",
            "archie",
            Some(Route::Cable)
        ),
        "hpush   macie → archie   cable"
    );
    assert_eq!(
        header(
            &style,
            Direction::Pull,
            "macie",
            "archie",
            Some(Route::Cable)
        ),
        "hpull   archie → macie   cable"
    );
}

#[test]
fn an_unknown_route_is_left_out_rather_than_guessed() {
    let line = header(&Style::plain(), Direction::Push, "macie", "archie", None);
    assert_eq!(line, "hpush   macie → archie");
}

#[test]
fn both_endpoints_name_the_machine_they_are_on() {
    let lines = endpoints(&Style::plain(), &plan(), "macie");
    assert_eq!(lines[0], "  from  macie:~/projects/my-app");
    assert_eq!(lines[1], "  to    archie:~/projects/my-app");
}

#[test]
fn a_pull_swaps_the_endpoints_without_moving_the_labels() {
    let mut plan = plan();
    plan.direction = Direction::Pull;
    let lines = endpoints(&Style::plain(), &plan, "macie");
    assert_eq!(lines[0], "  from  archie:~/projects/my-app");
    assert_eq!(lines[1], "  to    macie:~/projects/my-app");
}

#[test]
fn a_copy_that_moved_nothing_says_so_in_one_line() {
    let summary = summary(&Style::plain(), &plan(), &Outcome::default());
    assert_eq!(summary, "  already in sync");
}

#[test]
fn a_finished_copy_reports_what_it_moved_and_how_fast() {
    let outcome = Outcome {
        files: 47,
        created: 3,
        bytes: 12_900_000,
        elapsed: Duration::from_millis(800),
        lines: Vec::new(),
    };
    let summary = summary(&Style::plain(), &plan(), &outcome);
    assert!(summary.starts_with("  47 files  12.3 MiB  (3 new)  in 0.8 s"));
    assert!(summary.contains("MiB/s"));
}

#[test]
fn a_dry_run_never_claims_a_duration_or_a_rate() {
    let mut plan = plan();
    plan.dry_run = true;
    let outcome = Outcome {
        files: 47,
        created: 0,
        bytes: 12_900_000,
        elapsed: Duration::from_millis(800),
        lines: Vec::new(),
    };
    let summary = summary(&Style::plain(), &plan, &outcome);
    assert_eq!(summary, "  47 files  12.3 MiB  to transfer (dry run)");
}

#[test]
fn a_duration_keeps_the_digits_that_change() {
    assert_eq!(seconds(Duration::from_millis(800)), "0.8 s");
    assert_eq!(seconds(Duration::from_secs(42)), "42 s");
    assert_eq!(seconds(Duration::from_secs(125)), "2m 05s");
}

#[test]
fn a_rate_needs_both_bytes_and_time_to_mean_anything() {
    assert_eq!(rate(0, Duration::from_secs(1)), None);
    assert_eq!(rate(100, Duration::ZERO), None);
    assert_eq!(
        rate(104_857_600, Duration::from_secs(1)),
        Some("100.0 MiB/s".to_string())
    );
}
