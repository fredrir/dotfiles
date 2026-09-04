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
