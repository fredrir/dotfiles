use super::*;

#[test]
fn deferred_tui_starts_for_planned_link_work_before_apply() {
    assert!(starts_tui(
        &Event::Started {
            profile: "macos".to_string(),
            dry_run: false,
            peer: Some("archie".to_string()),
        },
        false,
    ));
    assert!(!starts_tui(
        &Event::Started {
            profile: "macos".to_string(),
            dry_run: true,
            peer: Some("archie".to_string()),
        },
        false,
    ));
    assert!(starts_tui(
        &Event::PhaseStarted {
            phase: Phase::Tooling,
            total: None,
        },
        false,
    ));
    assert!(starts_tui(
        &Event::PhaseStarted {
            phase: Phase::Links,
            total: Some(1),
        },
        false,
    ));
    assert!(!starts_tui(
        &Event::PhaseStarted {
            phase: Phase::Links,
            total: Some(0),
        },
        false,
    ));
    assert!(!starts_tui(
        &Event::PhaseStarted {
            phase: Phase::Secrets,
            total: Some(1),
        },
        false,
    ));
    assert!(starts_tui(
        &Event::Item {
            action: crate::event::Action::Merge,
            path: std::path::PathBuf::from("settings.json"),
            detail: String::new(),
            changed: true,
        },
        false,
    ));
}
