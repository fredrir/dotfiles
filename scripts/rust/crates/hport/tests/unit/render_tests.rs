use super::*;

fn entry(port: u16, alias: Status, mirror: Status) -> Entry {
    Entry {
        port,
        process: "bun".into(),
        alias,
        mirror,
    }
}

fn state(services: Vec<Entry>) -> State {
    State {
        pid: 1,
        peer: "archie".into(),
        route: Some("cable".into()),
        connected: true,
        error: None,
        services,
    }
}

#[test]
fn active_forwards_print_as_urls() {
    let cells = row("archie", &entry(5173, Status::Active, Status::Active));
    assert_eq!(cells[2], ("http://archie:5173".into(), Tone::Good));
    assert_eq!(cells[3], ("http://localhost:5173".into(), Tone::Good));
}

#[test]
fn a_busy_localhost_names_its_holder() {
    let cells = row(
        "archie",
        &entry(5173, Status::Active, Status::Busy(Some("node".into()))),
    );
    assert_eq!(cells[3].0, "busy (node)");
    let cells = row("archie", &entry(5173, Status::Pending, Status::Busy(None)));
    assert_eq!(cells[2].0, "pending");
    assert_eq!(cells[3].0, "busy");
}

#[test]
fn a_failure_is_flagged() {
    let cells = row(
        "archie",
        &entry(5173, Status::Failed("in use".into()), Status::Pending),
    );
    assert_eq!(cells[2], ("failed".into(), Tone::Bad));
}

#[test]
fn the_table_has_a_header_and_one_row_per_port() {
    let text = status(
        &Style::plain(),
        Host::Macie,
        &state(vec![
            entry(5173, Status::Active, Status::Active),
            entry(8080, Status::Active, Status::Busy(Some("java".into()))),
        ]),
    );
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "archie → macie  cable");
    assert!(lines[1].starts_with("PORT  PROCESS  ARCHIE"), "{text}");
    assert!(lines[2].contains("http://archie:5173") && lines[2].ends_with("http://localhost:5173"));
    assert!(lines[3].ends_with("busy (java)"), "{text}");
}

#[test]
fn no_ports_says_so() {
    let text = status(&Style::plain(), Host::Macie, &state(Vec::new()));
    assert_eq!(text, "archie → macie  cable\nno ports on archie");
}

#[test]
fn a_disconnected_daemon_shows_its_error() {
    let text = status(
        &Style::plain(),
        Host::Macie,
        &State {
            connected: false,
            route: None,
            error: Some("archie: connection lost".into()),
            ..state(Vec::new())
        },
    );
    assert_eq!(text, "archie → macie  archie: connection lost");
}

#[test]
fn a_connected_daemon_without_the_alias_says_how_to_fix_it() {
    let text = status(
        &Style::plain(),
        Host::Macie,
        &State {
            error: Some("127.0.0.2 is missing; run hport setup".into()),
            ..state(Vec::new())
        },
    );
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "archie → macie  cable");
    assert_eq!(lines[1], "127.0.0.2 is missing; run hport setup");
}
