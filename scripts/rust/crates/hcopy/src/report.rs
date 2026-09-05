use std::time::Duration;

use hostkit::Route;
use workstation::Style;
use workstation::text::counted;
use workstation::units::bytes;

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
        style.bold(&counted(outcome.files, "file", "files")),
        style.dim(&bytes(outcome.bytes)),
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
        style.bold(&counted(outcome.files, "file", "files")),
        style.bold(&bytes(outcome.bytes)),
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
        line.push_str(&format!("  {}  {}", style.dim("|"), style.dim(&rate)));
    }
    line
}

fn seconds(elapsed: Duration) -> String {
    let value = elapsed.as_secs_f64();
    match value {
        _ if value >= 60.0 => format!("{}m {:02}s", (value / 60.0) as u64, (value % 60.0) as u64),
        _ if value >= 10.0 => format!("{value:.0} s"),
        _ => format!("{value:.1} s"),
    }
}

fn rate(total: u64, elapsed: Duration) -> Option<String> {
    let seconds = elapsed.as_secs_f64();
    if total == 0 || seconds <= 0.0 {
        return None;
    }
    Some(format!("{}/s", bytes((total as f64 / seconds) as u64)))
}

#[cfg(test)]
#[path = "../tests/unit/report_tests.rs"]
mod tests;
