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
mod tests {
    use super::*;

    fn view() -> View<'static> {
        View {
            this: "macie",
            peer: "archie",
            route: Some(Route::Cable),
            agent: Agent::Codex,
            session_id: "01999999-1111-7222-8333-444444444444",
            source_workspace: "~/projects/app",
            destination_workspace: "~/projects/app",
            source_transcript: "~/.codex/sessions/2026/09/02/source.jsonl",
            destination_transcript: "~/.codex/sessions/2026/09/02/source.jsonl",
        }
    }

    #[test]
    fn the_plain_header_names_both_hosts_and_the_route() {
        assert_eq!(
            header(&Style::plain(), &view()),
            "agent-hop   macie → archie   cable"
        );
    }

    #[test]
    fn details_keep_the_compatibility_fields_visible() {
        let lines = details(&Style::plain(), &view());
        assert_eq!(lines[0], "  agent    codex");
        assert!(lines[1].contains("01999999"));
        assert_eq!(
            lines[2],
            "  work     macie:~/projects/app → archie:~/projects/app"
        );
        assert!(lines[3].contains("macie:~/.codex/sessions"));
        assert!(lines[4].contains("new session ID"));
        assert!(lines[5].contains("not synchronized"));
    }

    #[test]
    fn colored_output_uses_the_shared_theme_escapes() {
        let style = Style::for_stdout_with_color(true);
        assert!(header(&style, &view()).contains("\x1b["));
        assert!(copied(&style, 1024, 1).contains("\x1b["));
    }

    #[test]
    fn status_lines_distinguish_each_terminal_outcome() {
        let style = Style::plain();
        assert_eq!(copied(&style, 0, 0), "  ✓  transcript 0 B");
        assert!(copied(&style, 2048, 2).contains("2 KiB and 2 attachments"));
        assert!(reused(&style).contains("identical"));
        assert!(attachments(&style, 1).contains("1 attachment"));
        assert!(attachments(&style, 2).contains("2 attachments"));
        assert!(dry_run(&style).contains("nothing copied"));
        assert!(copied_without_connect(&style, "archie").contains("archie"));
        assert!(launching(&style, Agent::Claude, "archie").contains("claude"));
    }

    #[test]
    fn dynamic_paths_cannot_inject_terminal_controls() {
        let mut unsafe_view = view();
        unsafe_view.source_workspace = "~/bad\x1b]8;;https://example.test\x1b\\link\nnext";
        let lines = details(&Style::plain(), &unsafe_view);
        assert!(!lines[2].contains('\x1b'));
        assert!(!lines[2].contains('\n'));
        assert!(!lines[2].contains("https://example.test"));
    }
}
