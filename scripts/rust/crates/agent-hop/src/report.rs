use hostkit::Route;
use workstation::Style;

use crate::cli::Agent;

pub struct View<'a> {
    pub this: &'a str,
    pub peer: &'a str,
    pub route: Option<Route>,
    pub agent: Agent,
    pub session_id: &'a str,
    pub source_workspace: &'a str,
    pub destination_workspace: &'a str,
    pub source_transcript: &'a str,
    pub destination_transcript: &'a str,
}

pub fn header(style: &Style, view: &View<'_>) -> String {
    let this = safe(view.this);
    let peer = safe(view.peer);
    let mut text = format!(
        "{}   {} {} {}",
        style.bold("agent-hop"),
        style.bold(&this),
        style.dim("→"),
        style.bold(&peer)
    );
    if let Some(route) = view.route {
        text.push_str(&format!("   {}", style.teal(route.name())));
    }
    text
}

pub fn details(style: &Style, view: &View<'_>) -> Vec<String> {
    let session_id = safe(view.session_id);
    vec![
        row(style, "agent  ", &style.bold(view.agent.name())),
        row(style, "session", &session_id),
        endpoints(
            style,
            "work   ",
            view.this,
            view.source_workspace,
            view.peer,
            view.destination_workspace,
        ),
        endpoints(
            style,
            "session",
            view.this,
            view.source_transcript,
            view.peer,
            view.destination_transcript,
        ),
        row(style, "fork   ", "new session ID on destination"),
        row(style, "files  ", "worktree is not synchronized"),
    ]
}

pub fn copied(style: &Style, bytes: u64, attachments: usize) -> String {
    let extra = match attachments {
        0 => String::new(),
        1 => " and 1 attachment".to_string(),
        count => format!(" and {count} attachments"),
    };
    format!(
        "  {}  transcript {}{}",
        style.green("✓"),
        size(bytes),
        extra
    )
}

pub fn reused(style: &Style) -> String {
    format!(
        "  {}",
        style.dim("identical transcript already on destination")
    )
}

pub fn attachments(style: &Style, count: usize) -> String {
    let noun = if count == 1 {
        "attachment"
    } else {
        "attachments"
    };
    format!("  {}  synced {count} {noun}", style.green("✓"))
}

pub fn dry_run(style: &Style) -> String {
    format!("  {}", style.dim("dry run — nothing copied or started"))
}

pub fn copied_without_connect(style: &Style, peer: &str) -> String {
    let peer = safe(peer);
    format!(
        "  {}",
        style.dim(&format!(
            "ready — rerun without --no-connect to open on {peer}"
        ))
    )
}

pub fn launching(style: &Style, agent: Agent, peer: &str) -> String {
    let peer = safe(peer);
    format!(
        "  {}  opening a new {} session on {}",
        style.green("▸"),
        style.bold(agent.name()),
        style.bold(&peer)
    )
}

fn row(style: &Style, label: &str, value: &str) -> String {
    format!("  {}  {value}", style.dim(label))
}

fn endpoints(
    style: &Style,
    label: &str,
    this: &str,
    source: &str,
    peer: &str,
    destination: &str,
) -> String {
    let this = safe(this);
    let source = safe(source);
    let peer = safe(peer);
    let destination = safe(destination);
    row(
        style,
        label,
        &format!(
            "{} {} {}",
            style.teal(&format!("{this}:{source}")),
            style.dim("→"),
            style.teal(&format!("{peer}:{destination}"))
        ),
    )
}

fn safe(value: &str) -> String {
    crate::preview::sanitize(value)
}

fn size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    match value {
        _ if value >= KIB * KIB * KIB => format!("{:.2} GiB", value / (KIB * KIB * KIB)),
        _ if value >= KIB * KIB => format!("{:.1} MiB", value / (KIB * KIB)),
        _ if value >= KIB => format!("{:.0} KiB", value / KIB),
        _ => format!("{bytes} B"),
    }
}

#[cfg(test)]
#[path = "../tests/unit/report_tests.rs"]
mod tests;
