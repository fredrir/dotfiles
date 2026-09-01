use std::time::Duration;

use hostkit::Route;
use workstation::Style;

use crate::cli::Direction;
use crate::transfer::{Outcome, Plan};

pub fn header(
    style: &Style,
    direction: Direction,
    this: &str,
    peer: &str,
    route: Option<Route>,
) -> String {
    let (from, to) = match direction {
        Direction::Push => (this, peer),
        Direction::Pull => (peer, this),
    };
    let mut line = format!(
        "{}   {} {} {}",
        style.bold(direction.program()),
        style.bold(from),
        style.dim("→"),
        style.bold(to),
    );
    if let Some(route) = route {
        line.push_str(&format!("   {}", style.teal(route.name())));
    }
    line
}

pub fn endpoints(style: &Style, plan: &Plan, this: &str) -> Vec<String> {
    let local = format!("{this}:{}", plan.local_display);
    let remote = format!("{}:{}", plan.host, plan.remote_display);
    let (from, to) = match plan.direction {
        Direction::Push => (local, remote),
        Direction::Pull => (remote, local),
    };
    vec![
        format!("  {}  {}", style.dim("from"), style.teal(&from)),
        format!("  {}  {}", style.dim("to  "), style.teal(&to)),
    ]
}

pub fn progress(style: &Style, outcome: &Outcome) -> String {
    format!(
        "  {} {} {}",
        style.dim("▸"),
        style.bold(&files(outcome.files)),
        style.dim(&size(outcome.bytes)),
    )
}

pub fn summary(style: &Style, plan: &Plan, outcome: &Outcome) -> String {
    if outcome.quiet() {
        let already = match plan.dry_run {
            true => "already in sync",
            false => "already in sync",
        };
        return format!("  {}", style.dim(already));
    }

    let mut line = format!(
        "  {}  {}",
        style.bold(&files(outcome.files)),
        style.bold(&size(outcome.bytes)),
    );
    if outcome.created > 0 {
        line.push_str(&format!(
            "  {}",
            style.dim(&format!("({} new)", outcome.created))
        ));
    }
    if plan.dry_run {
        line.push_str(&format!("  {}", style.dim("to transfer (dry run)")));
        return line;
    }
    line.push_str(&format!(
        "  {}",
        style.dim(&format!("in {}", seconds(outcome.elapsed)))
    ));
    if let Some(rate) = rate(outcome.bytes, outcome.elapsed) {
        line.push_str(&format!("  {}  {}", style.dim("·"), style.dim(&rate)));
    }
    line
}

fn files(count: usize) -> String {
    match count {
        1 => "1 file".to_string(),
        other => format!("{other} files"),
    }
}

pub fn size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    match value {
        _ if value >= KIB * KIB * KIB => format!("{:.2} GiB", value / (KIB * KIB * KIB)),
        _ if value >= KIB * KIB => format!("{:.1} MiB", value / (KIB * KIB)),
        _ if value >= KIB => format!("{:.0} KiB", value / KIB),
        _ => format!("{bytes} B"),
    }
}

fn seconds(elapsed: Duration) -> String {
    let value = elapsed.as_secs_f64();
    match value {
        _ if value >= 60.0 => format!("{}m {:02}s", (value / 60.0) as u64, (value % 60.0) as u64),
        _ if value >= 10.0 => format!("{value:.0} s"),
        _ => format!("{value:.1} s"),
    }
}

fn rate(bytes: u64, elapsed: Duration) -> Option<String> {
    let seconds = elapsed.as_secs_f64();
    if bytes == 0 || seconds <= 0.0 {
        return None;
    }
    Some(format!("{}/s", size((bytes as f64 / seconds) as u64)))
}

#[cfg(test)]
mod tests {
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
    fn one_file_is_not_one_files() {
        assert_eq!(files(1), "1 file");
        assert_eq!(files(0), "0 files");
        assert_eq!(files(2), "2 files");
    }

    #[test]
    fn a_size_carries_the_unit_it_is_worth_reading_in() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(512), "512 B");
        assert_eq!(size(2048), "2 KiB");
        assert_eq!(size(12_900_000), "12.3 MiB");
        assert_eq!(size(3_221_225_472), "3.00 GiB");
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
}
