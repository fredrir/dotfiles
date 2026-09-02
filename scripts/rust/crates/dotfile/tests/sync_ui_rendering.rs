use std::path::PathBuf;
use std::time::Duration;

use dotfile_cli::decision::{Choice, Prompt, Request};
use dotfile_cli::event::{Action, Event, Phase, Summary};
use dotfile_cli::ui::plain;
use dotfile_cli::ui::tui::{UiModel, render_buffer};
use dotfile_cli::ui::{UiPolicy, completion_line};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

#[test]
fn sync_ui_policy_honors_terminal_and_accessibility_signals() {
    assert_eq!(
        UiPolicy::from_signals(true, false, Some("xterm-256color"), None, false, false),
        UiPolicy {
            interactive: false,
            color: false,
            motion: false,
        }
    );
    assert!(!UiPolicy::from_signals(true, true, Some("dumb"), None, false, false).interactive);
    assert!(
        !UiPolicy::from_signals(true, true, Some("xterm"), Some("true"), false, false).interactive
    );
    assert!(!UiPolicy::from_signals(false, true, Some("xterm"), None, false, false).interactive);
    assert_eq!(
        UiPolicy::from_signals(true, true, Some("xterm"), None, true, false),
        UiPolicy {
            interactive: true,
            color: false,
            motion: false,
        }
    );
    assert_eq!(
        UiPolicy::from_signals(true, true, Some("xterm"), None, false, true),
        UiPolicy {
            interactive: true,
            color: true,
            motion: false,
        }
    );
}

#[test]
fn sync_ui_compact_view_uses_only_reported_progress() {
    let mut model = UiModel::new(false);
    model.apply(&Event::Started {
        profile: "macos".to_string(),
        dry_run: false,
        peer: None,
    });
    model.apply(&Event::PhaseStarted {
        phase: Phase::Links,
        total: Some(10),
    });
    model.apply(&Event::Progress {
        phase: Phase::Links,
        completed: 4,
        total: Some(10),
        label: "/Users/test/.gitconfig".to_string(),
    });
    model.apply(&Event::Item {
        action: Action::Link,
        path: PathBuf::from("/Users/test/.gitconfig"),
        detail: String::new(),
        changed: true,
    });
    let rendered = render(&model, 72, 4);
    assert!(rendered.contains("DOTFILE  /  SYNC  macos"));
    assert!(rendered.contains("links  4/10"));
    assert!(rendered.contains("4 / 10  |  40%"));
    assert!(!rendered.contains(".gitconfig"));
}

#[test]
fn sync_ui_verbose_view_keeps_unique_activity_and_current_label() {
    let mut model = UiModel::new(true);
    model.apply(&Event::Started {
        profile: "macos".to_string(),
        dry_run: false,
        peer: None,
    });
    model.apply(&Event::Progress {
        phase: Phase::Merge,
        completed: 2,
        total: Some(3),
        label: "settings.json".to_string(),
    });
    let unchanged_check = model.apply(&Event::Item {
        action: Action::Check,
        path: PathBuf::from("/tmp/unchanged.json"),
        detail: String::new(),
        changed: false,
    });
    let item = Event::Item {
        action: Action::Merge,
        path: PathBuf::from("/tmp/settings.json"),
        detail: "formatting".to_string(),
        changed: true,
    };
    let first = model.apply(&item);
    let duplicate = model.apply(&item);
    assert_eq!(model.item_count(), 1);
    assert!(unchanged_check.output.is_empty());
    assert_eq!(first.output, ["merge /tmp/settings.json (formatting)"]);
    assert!(duplicate.output.is_empty());
    let rendered = render(&model, 72, 11);
    assert!(rendered.contains("settings.json"));
    assert!(rendered.contains("merge /tmp/settings.json (formatting)"));
}

#[test]
fn sync_ui_push_plan_finishes_with_exact_summary_and_timeline() {
    let mut model = UiModel::new(false);
    model.apply(&Event::Started {
        profile: "macos".to_string(),
        dry_run: true,
        peer: Some("archie".to_string()),
    });
    model.apply(&Event::PhaseStarted {
        phase: Phase::Push,
        total: Some(2),
    });
    model.apply(&Event::PhaseStarted {
        phase: Phase::Remote,
        total: Some(1),
    });
    model.apply(&Event::Finished(Summary {
        profile: "macos".to_string(),
        peer: Some("archie".to_string()),
        remote_changed: Some(2),
        checked: 184,
        changed: 4,
        links: 2,
        merges: 1,
        secrets: 0,
        generated: 1,
        dry_run: true,
        elapsed: Duration::from_millis(63),
    }));
    let rendered = render(&model, 80, 5);
    assert!(rendered.contains("PUSH  macos  →  archie"));
    assert!(rendered.contains("● local ━━━━━ ● origin ━━━━━ ● peer"));
    assert!(rendered.contains("◇ PLAN READY  4 changes pending | 63 ms"));
    assert!(rendered.contains("2 links  |  1 merge  |  1 generated"));
}

fn completion_summary() -> Summary {
    Summary {
        profile: "macos".to_string(),
        peer: None,
        remote_changed: None,
        checked: 184,
        changed: 0,
        links: 1,
        merges: 1,
        secrets: 0,
        generated: 0,
        dry_run: false,
        elapsed: Duration::from_millis(63),
    }
}

#[test]
fn sync_ui_persisted_local_completion_is_minimal() {
    let mut summary = completion_summary();
    assert_eq!(completion_line(&summary), "✓ Synced");
    summary.changed = 1;
    assert_eq!(completion_line(&summary), "✓ Synced 1 change");
    summary.changed = 4;
    assert_eq!(completion_line(&summary), "✓ Synced 4 changes");
}

#[test]
fn sync_ui_persisted_dry_run_and_push_completion_stays_compact() {
    let mut dry_run = completion_summary();
    dry_run.dry_run = true;
    assert_eq!(completion_line(&dry_run), "○ Plan ready");
    dry_run.changed = 1;
    assert_eq!(completion_line(&dry_run), "○ Plan ready 1 change");
    dry_run.changed = 3;
    dry_run.peer = Some("archie".to_string());
    assert_eq!(completion_line(&dry_run), "○ Plan ready 3 changes → archie");

    let mut push = completion_summary();
    push.peer = Some("archie".to_string());
    push.remote_changed = Some(3);
    assert_eq!(completion_line(&push), "✓ Synced → archie 3 changes");
    push.changed = 2;
    assert_eq!(
        completion_line(&push),
        "✓ Synced 2 local changes → archie 3 changes"
    );
    push.remote_changed = None;
    assert_eq!(completion_line(&push), "✓ Synced 2 changes → archie");
}

#[test]
fn sync_ui_warning_hint_and_failure_remain_actionable() {
    let mut model = UiModel::new(false);
    model.apply(&Event::Started {
        profile: "macos".to_string(),
        dry_run: false,
        peer: None,
    });
    let warning = model.apply(&Event::Warning {
        message: "settings need a decision".to_string(),
        hint: Some("rerun with --resolve repo or live".to_string()),
    });
    assert_eq!(
        warning.output,
        [
            "warning: settings need a decision",
            "  hint: rerun with --resolve repo or live",
        ]
    );
    model.apply(&Event::Failed {
        phase: Phase::Merge,
        message: "settings need a decision".to_string(),
        hint: Some("rerun with --resolve repo or live".to_string()),
    });
    let rendered = render(&model, 72, 4);
    assert!(!model.active());
    assert!(rendered.contains("× FAILED  settings need a decision"));
    assert!(rendered.contains("! settings need a decision"));
}

#[test]
fn sync_ui_verbose_text_cannot_inject_terminal_controls_or_lines() {
    let mut model = UiModel::new(true);
    let started = model.apply(&Event::Started {
        profile: "macos\nforged\u{1b}[31m".to_string(),
        dry_run: false,
        peer: None,
    });
    let item = model.apply(&Event::Item {
        action: Action::Merge,
        path: PathBuf::from("/tmp/settings\nforged.json"),
        detail: "updated\r\u{7}alert".to_string(),
        changed: true,
    });
    let warning = model.apply(&Event::Warning {
        message: "warning\nforged\u{1b}[2J".to_string(),
        hint: Some("hint\tforged".to_string()),
    });
    let output = started
        .output
        .into_iter()
        .chain(item.output)
        .chain(warning.output)
        .collect::<Vec<_>>()
        .join("|");
    assert!(!output.chars().any(char::is_control));
    assert!(!output.contains('\u{1b}'));
    assert!(output.contains("macos forged"));
    assert!(output.contains("[31m"));
    assert!(output.contains("updated"));
    assert!(output.contains("alert"));
    assert!(output.contains("warning forged"));
    assert!(output.contains("[2J"));
}

#[test]
fn sync_ui_merge_decision_starts_safe_and_requires_selected_response() {
    let mut model = UiModel::new(false);
    model.show_decision(Request {
        id: 41,
        prompt: Prompt::Merge {
            path: PathBuf::from("/tmp/settings.json"),
            key: "editor.fontFamily".to_string(),
            repo: "Berkeley Mono\nRegular".to_string(),
            live: "JetBrains Mono".to_string(),
        },
    });
    assert_eq!(model.selected_choice(), Some(Choice::Skip));
    let rendered = render(&model, 78, 4);
    assert!(rendered.contains("  MERGE"));
    assert!(rendered.contains("editor.fontFamily"));
    assert!(rendered.contains("Berkeley Mono↵Regular"));
    assert!(rendered.contains("JetBrains Mono"));
    assert!(rendered.contains("repo"));
    assert!(rendered.contains("live"));
    assert!(rendered.contains("ignore"));
    assert!(rendered.contains("skip"));
    assert!(rendered.contains("abort"));
    model.select_previous();
    assert_eq!(model.selected_choice(), Some(Choice::Ignore));
    let (request, choice) = model.decision_response().unwrap();
    assert_eq!(request.id, 41);
    assert_eq!(choice, Choice::Ignore);
    assert_eq!(model.cancel_response().unwrap().1, Choice::Abort);
}

#[test]
fn sync_ui_merge_target_preselects_default_and_cycles_named_targets() {
    let mut model = UiModel::new(false);
    model.show_decision(Request {
        id: 43,
        prompt: Prompt::MergeTarget {
            path: PathBuf::from("/tmp/settings.json"),
            key: "editor.fontFamily".to_string(),
            targets: vec![
                "shared".to_string(),
                "macos".to_string(),
                "work/client-with-a-long-overlay-name".to_string(),
            ],
            default: 1,
        },
    });
    assert_eq!(model.selected_choice(), Some(Choice::Target(1)));
    let rendered = render(&model, 78, 4);
    assert!(rendered.contains("  TARGET"));
    assert!(rendered.contains("macos"));
    assert!(rendered.contains("3 destinations"));
    assert!(rendered.contains("2/3"));
    model.select_next();
    assert_eq!(model.selected_choice(), Some(Choice::Target(2)));
    assert_eq!(model.decision_response().unwrap().1, Choice::Target(2));
    assert_eq!(model.cancel_response().unwrap().1, Choice::Cancel);
}

#[test]
fn sync_ui_remote_decision_shows_host_count_and_safe_cancel() {
    let mut model = UiModel::new(false);
    model.show_decision(Request {
        id: 42,
        prompt: Prompt::RemoteChanges {
            host: "archie".to_string(),
            changes: vec![
                "shared/zsh/.zshrc".to_string(),
                "macos/git/.gitconfig".to_string(),
                "PACKAGES.md".to_string(),
            ],
        },
    });
    assert_eq!(model.selected_choice(), Some(Choice::Cancel));
    assert_eq!(model.cancel_response().unwrap().1, Choice::Cancel);
    let rendered = render(&model, 78, 5);
    assert!(rendered.contains("3 incoming changes"));
    assert!(rendered.contains("+2 more"));
    assert!(rendered.contains("discard"));
    assert!(rendered.contains("cancel"));
}

#[test]
fn sync_ui_plain_mode_answers_safe_defaults_without_stdin() {
    dotfile_cli::cancel::reset();
    let (event_sender, event_receiver) = crossbeam_channel::bounded(8);
    let (client, server) = dotfile_cli::decision::channel();
    let worker = std::thread::spawn(move || {
        event_sender
            .send(Event::Started {
                profile: "macos".to_string(),
                dry_run: false,
                peer: None,
            })
            .unwrap();
        let merge = client.choose(Prompt::Merge {
            path: PathBuf::from("/tmp/settings.json"),
            key: "editor.fontFamily".to_string(),
            repo: "repo".to_string(),
            live: "live".to_string(),
        })?;
        let remote = client.choose(Prompt::RemoteChanges {
            host: "archie".to_string(),
            changes: vec!["PACKAGES.md".to_string()],
        })?;
        let target = client.choose(Prompt::MergeTarget {
            path: PathBuf::from("/tmp/settings.json"),
            key: "editor.fontFamily".to_string(),
            targets: vec!["shared".to_string(), "macos".to_string()],
            default: 1,
        })?;
        let summary = Summary {
            profile: "macos".to_string(),
            peer: None,
            remote_changed: None,
            checked: usize::from(merge == Choice::Skip)
                + usize::from(remote == Choice::Cancel)
                + usize::from(target == Choice::Cancel),
            changed: 0,
            links: 0,
            merges: 0,
            secrets: 0,
            generated: 0,
            dry_run: false,
            elapsed: Duration::ZERO,
        };
        event_sender.send(Event::Finished(summary.clone())).unwrap();
        Ok(summary)
    });
    let summary = plain::run(event_receiver, server, worker, false).unwrap();
    assert_eq!(summary.checked, 3);
}

fn render(model: &UiModel, width: u16, height: u16) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    render_buffer(model, area, &mut buffer, 0, false);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
